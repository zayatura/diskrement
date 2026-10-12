//! The pipeline (`Pipeline.scala`): returns reports, performs no path
//! resolution, and — for scan — **writes no catalog rows**.

pub mod planning;
pub mod reports;
pub mod staged;

use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use diskrement_core::{
    diff, manifest, replication, Capacity, DiscArg, DiscContents, DiscKey, DiscState, DiscStatus,
    DiscType, PlannedDisc, RelPath, ScannedFile, SessionManifest,
};

use crate::burn::verifier::{DiscExpectation, Verifier};
use crate::burn::{json, Burner, DiscIdentity, DiscImage, DiscMetadata};
use crate::catalog::model::{
    CatalogUpdate, DiscWrite, FileFingerprint, FileHistoryEntry, FileWrite, LostReport, Project,
    ProjectId, ProjectInit, Session, SessionState, SessionWrite,
};
use crate::catalog::Catalog;
use crate::error::{PipelineError, UnrecordedDisc};
use crate::fs::fingerprint_cache::FingerprintCache;
use crate::fs::hasher;
use crate::fs::scanner::{self, parallelism_from_env, ScanOutput};
use crate::fs::snapshot::{self, SnapshotHandle};
use crate::progress::ProgressSink;

use reports::{
    BurnReport, DiscBurn, PlanReport, RemainingEstimate, ScanReport, SetPlan, StatusReport,
    VerifyReport,
};

/// The paths a pipeline needs, resolved by the CLI (`ProjectPaths` +
/// `CachePath` in the Scala wiring). `work_dir` for `scan`/`status` comes from
/// the project row, not the CLI flag.
pub struct PipelinePaths {
    pub work_dir: PathBuf,
    pub scan_cache: PathBuf,
}

pub struct Pipeline<'a> {
    catalog: &'a mut Catalog,
    paths: PipelinePaths,
    progress: &'a dyn ProgressSink,
}

impl<'a> Pipeline<'a> {
    pub fn new(
        catalog: &'a mut Catalog,
        paths: PipelinePaths,
        progress: &'a dyn ProgressSink,
    ) -> Self {
        Pipeline {
            catalog,
            paths,
            progress,
        }
    }

    fn failed(e: impl std::error::Error + 'static) -> PipelineError {
        PipelineError::failed(&e)
    }

    fn corrupt_scan(e: impl std::error::Error + 'static) -> PipelineError {
        PipelineError::Failed(format!(
            "corrupt scan.json: {}",
            crate::error::with_causes(&e)
        ))
    }

    fn project(&self, name: &str) -> Result<Project, PipelineError> {
        self.catalog
            .find_project(name)
            .map_err(Self::failed)?
            .ok_or_else(|| PipelineError::ProjectNotFound(name.to_string()))
    }

    /// `init`: same-filesystem check, CoW probe, then the one project row. No
    /// snapshot, no scan, no other rows (`PipelineLive.init`).
    pub fn init(&mut self, pi: &ProjectInit) -> Result<ProjectId, PipelineError> {
        let src = PathBuf::from(&pi.source_root);
        let work = PathBuf::from(&pi.work_dir);
        // `getFileStore` equality via st_dev; a missing source surfaces here
        // as Failed (exit 5), exactly like the Scala getFileStore throw.
        let (sdev, wdev) = same_filesystem(&src, &work).map_err(Self::failed)?;
        if sdev != wdev {
            return Err(PipelineError::NotSameFilesystem { src, work });
        }
        snapshot::check_cow_capable(&work).map_err(|e| PipelineError::NotCowCapable {
            work_dir: work,
            detail: e.to_string(),
        })?;
        self.catalog.create_project(pi).map_err(Self::failed)
    }

    /// `scan`: reuse a staged scan unless `--force`/`--verify`; otherwise
    /// snapshot → hash → stage → cache, and diff against the catalog view
    /// (`PipelineLive.scan`). Added/modified/unchanged come from that view;
    /// the deleted count comes from the catalog history, like the plan's
    /// tombstones. `hashed` is `None` on the reuse path.
    pub fn scan(
        &mut self,
        name: &str,
        verify: bool,
        force: bool,
    ) -> Result<ScanReport, PipelineError> {
        let p = self.project(name)?;
        if !force && !verify {
            if let Some(files) =
                staged::read_scan(&self.paths.work_dir).map_err(Self::corrupt_scan)?
            {
                return self.count(&p, &files, None);
            }
        }
        let (files, hashed) = self.fresh_scan(&p, verify)?;
        self.count(&p, &files, Some(hashed))
    }

    fn fresh_scan(
        &mut self,
        p: &Project,
        verify: bool,
    ) -> Result<(Vec<ScannedFile>, usize), PipelineError> {
        let src = PathBuf::from(&p.source_root);
        // Snapshot failures (missing source, non-CoW, I/O) are Failed here —
        // exit 5 — because only `init` produces the typed 3s (`PipelineLive`
        // maps SnapshotError to Failed in freshScan).
        let handle = snapshot::create(&src, &self.paths.work_dir).map_err(Self::failed)?;
        let prior: BTreeMap<RelPath, FileFingerprint> =
            self.catalog.fingerprints(p.id).map_err(Self::failed)?;
        // The cache never fails a scan; a read failure falls back to empty.
        let cached = FingerprintCache::new(self.paths.scan_cache.clone())
            .read_all()
            .unwrap_or_default();
        let mut merged = prior;
        // The cache wins on conflict: it is fresher — it re-fingerprinted
        // touched-but-identical files so later scans fast-path them.
        for (k, v) in cached {
            merged.insert(k, v);
        }
        let par = parallelism_from_env();
        for w in &par.warnings {
            eprintln!("{w}");
        }
        let out: ScanOutput = scanner::scan(
            &handle.root,
            &merged,
            verify,
            par.value,
            self.progress,
            &hasher::hash,
        )
        .map_err(|e| match e {
            scanner::ScanError::NonUtf8Names(names) => PipelineError::NonUtf8FileNames(names),
            other => Self::failed(other),
        })?;
        staged::write_scan(&self.paths.work_dir, &out.files).map_err(Self::failed)?;
        // Cache write is best-effort; a failure already warned with the path.
        let mut entries = BTreeMap::new();
        for f in &out.files {
            entries.insert(
                f.rel_path.clone(),
                FileFingerprint {
                    size_bytes: f.size_bytes,
                    mtime: f.mtime,
                    content_hash: f.content_hash.clone(),
                },
            );
        }
        let _ = FingerprintCache::new(self.paths.scan_cache.clone()).put_all(&entries);
        // The snapshot is NOT released: burn re-derives `<work>/snapshot`;
        // only `reset` removes it.
        let ScanOutput { files, hashed } = out;
        Ok((files, hashed))
    }

    fn count(
        &self,
        p: &Project,
        files: &[ScannedFile],
        hashed: Option<usize>,
    ) -> Result<ScanReport, PipelineError> {
        let view = self.catalog.read_view(p.id).map_err(Self::failed)?;
        let cs = diff::diff(&view, files);
        // Deletions come from the history, like `plan`'s tombstones, so the
        // scan never reports a different count from the plan that follows it.
        let recorded = self.catalog.fingerprints(p.id).map_err(Self::failed)?;
        Ok(ScanReport {
            added: cs.added.len(),
            modified: cs.modified.len(),
            deleted: diff::tombstones(recorded.keys(), files).len(),
            unchanged: cs.unchanged.len(),
            hashed,
            scanned_files: files.len(),
        })
    }

    /// `plan`: the staged scan plus the catalog view decide the disc each copy
    /// set writes this cycle (`PipelineLive.plan`). Stages `staged/plan.json` and
    /// writes **no catalog rows** — no `PLANNED` state is ever persisted; the
    /// staged plan in the work dir is the recovery unit.
    pub fn plan(
        &self,
        name: &str,
        disc_type_override: Option<DiscType>,
        finalize: bool,
    ) -> Result<PlanReport, PipelineError> {
        let p = self.project(name)?;
        let scanned = staged::read_scan(&self.paths.work_dir)
            .map_err(Self::corrupt_scan)?
            .ok_or_else(|| PipelineError::NoStagedScan(name.to_string()))?;
        let view = self.catalog.read_view(p.id).map_err(Self::failed)?;
        let states = self.catalog.read_disc_states(p.id).map_err(Self::failed)?;
        let recorded = self.catalog.fingerprints(p.id).map_err(Self::failed)?;
        let tombstones = diff::tombstones(recorded.keys(), &scanned);
        let disc_type = disc_type_override.unwrap_or(p.default_disc_type);
        let cap = planning::effective_capacity(
            Capacity::from_disc_type(disc_type, p.margin_percent),
            planning::disc_bytes_from_env(),
        );
        let next = self.next_sessions(p.id)?;
        let report = planning::plan_from(
            &view,
            &scanned,
            tombstones,
            &states,
            disc_type,
            cap,
            finalize,
            p.copies,
            |disc| next.get(&disc).copied().unwrap_or(1),
        );
        staged::write_plan(&self.paths.work_dir, &report).map_err(Self::failed)?;
        Ok(report)
    }

    /// `burn`: write the staged plan's discs in set order, committing each one
    /// as soon as it is written.
    ///
    /// Per disc the ordering is the design (`PipelineLive.burn`): the disc tree
    /// is staged **before** anything reaches the live catalog, and the snapshot
    /// embedded on the disc is the same update applied to a work-dir copy, so
    /// the disc knows about its own session — and about every disc this cycle
    /// already committed. After each commit the staged plan is rewritten to the
    /// discs still to write, so it stays the recovery unit and no `PLANNED` row
    /// is ever persisted. A failure after at least one commit is a
    /// `PartialBurn`; re-running `burn` resumes at the first uncommitted disc.
    ///
    /// `now` is supplied by the caller rather than read from the clock: it
    /// keeps the pipeline deterministic.
    pub fn burn(
        &mut self,
        name: &str,
        now: DateTime<Utc>,
        burner: &Burner<'_>,
    ) -> Result<BurnReport, PipelineError> {
        let p = self.project(name)?;
        let scanned = staged::read_scan(&self.paths.work_dir)
            .map_err(Self::failed)?
            .ok_or_else(|| PipelineError::NoStagedScan(name.to_string()))?;
        if staged::is_legacy_plan(&self.paths.work_dir) {
            return Err(PipelineError::StalePlan(
                "it was staged by an older version of diskrement".to_string(),
            ));
        }
        let mut report = staged::read_plan(&self.paths.work_dir)
            .map_err(Self::failed)?
            .ok_or_else(|| PipelineError::NoStagedPlan(name.to_string()))?;
        if !report.oversized.is_empty() {
            return Err(PipelineError::OversizedFiles(
                report
                    .oversized
                    .iter()
                    .map(|f| f.rel_path.clone())
                    .collect(),
            ));
        }
        // A plan with no disc is a finished cycle with nothing to write: the
        // loop below does not run, and the tail ends the cycle as usual. Its
        // deletions wait for the next burned disc (issue #9).
        let deferred_deletions = report.sets.is_empty().then_some(report.tombstones.len());

        let mut done: Vec<DiscKey> = Vec::new();
        let mut written: Vec<DiscBurn> = Vec::new();
        let mut remaining = RemainingEstimate {
            more_discs: 0,
            more_files: 0,
            more_bytes: 0,
        };
        // Once a disc has committed, every failure must say so: the operator
        // has to know which discs the catalog now holds.
        let partial = |done: &[DiscKey],
                       unrecorded: Option<UnrecordedDisc>,
                       pending: &[SetPlan],
                       cause: PipelineError| {
            if done.is_empty() {
                return cause;
            }
            PipelineError::PartialBurn {
                committed: done.iter().map(|k| k.name(p.copies)).collect(),
                unrecorded,
                pending: pending
                    .iter()
                    .map(|s| s.disc.key().name(p.copies))
                    .collect(),
                cause: Box::new(cause),
            }
        };
        while let Some(sp) = report.sets.first().cloned() {
            match self.burn_disc(&p, &scanned, &report, &sp, now, burner) {
                Ok(burned) => {
                    done.push(sp.disc.key());
                    written.extend(burned);
                    remaining = RemainingEstimate {
                        more_discs: remaining.more_discs.max(sp.remaining.more_discs),
                        more_files: remaining.more_files.max(sp.remaining.more_files),
                        more_bytes: remaining.more_bytes.max(sp.remaining.more_bytes),
                    };
                    report.sets.remove(0);
                    if !report.sets.is_empty() {
                        // The plan on disk still lists the disc just
                        // committed; a re-run skips it by session ordinal.
                        staged::write_plan(&self.paths.work_dir, &report)
                            .map_err(|e| partial(&done, None, &report.sets, Self::failed(e)))?;
                    }
                }
                Err(DiscFailure::Unwritten(e)) => {
                    return Err(partial(&done, None, &report.sets, e));
                }
                Err(DiscFailure::Unrecorded {
                    cause,
                    surely_written,
                }) => {
                    let name = sp.disc.key().name(p.copies);
                    let disc = if surely_written {
                        UnrecordedDisc::written(name)
                    } else {
                        UnrecordedDisc::maybe_written(name)
                    };
                    return Err(partial(&done, Some(disc), &report.sets[1..], cause));
                }
            }
        }

        snapshot::release(&SnapshotHandle {
            root: self.paths.work_dir.join("snapshot"),
        });
        staged::clear(&self.paths.work_dir)
            .map_err(|e| partial(&done, None, &[], Self::failed(e)))?;
        let view = self
            .catalog
            .read_view(p.id)
            .map_err(|e| partial(&done, None, &[], Self::failed(e)))?;
        let current = self
            .catalog
            .fingerprints(p.id)
            .map_err(|e| partial(&done, None, &[], Self::failed(e)))?;
        Ok(BurnReport {
            discs: written,
            remaining,
            coverage: replication::coverage(&view, &sizes(&current), p.copies),
            deferred_deletions,
        })
    }

    /// Write one planned disc and commit its session. `Ok(None)` means the
    /// catalog already holds the session this plan expected: an earlier run
    /// committed the disc and stopped before rewriting the staged plan.
    fn burn_disc(
        &mut self,
        p: &Project,
        scanned: &[ScannedFile],
        report: &PlanReport,
        sp: &SetPlan,
        now: DateTime<Utc>,
        burner: &Burner<'_>,
    ) -> Result<Option<DiscBurn>, DiscFailure> {
        let disc = &sp.disc;
        let key = disc.key();
        let disc_name = key.name(p.copies);

        // The catalog cannot catch this: deadness is derived from `disc_event`,
        // so the disc row itself still reads OPEN. Without this guard the
        // burner would commit a session onto a disc that no longer physically
        // exists.
        let states = self
            .catalog
            .read_disc_states(p.id)
            .map_err(DiscFailure::unwritten)?;
        if states
            .iter()
            .any(|s| s.key() == key && s.status == DiscStatus::Dead)
        {
            return Err(DiscFailure::Unwritten(PipelineError::StalePlan(format!(
                "disc {disc_name} was marked dead after this plan was staged"
            ))));
        }

        let sess_ord = self
            .next_session_ordinal(p.id, key)
            .map_err(DiscFailure::Unwritten)?;
        if sess_ord == sp.session_ordinal + 1 {
            // Committed by an earlier run that stopped before its cleanup or
            // the plan rewrite; finish the cleanup it owed.
            if !burner.retains_staging() {
                discard_staging(&self.staging_dir(&disc_name));
            }
            return Ok(None);
        }
        if sess_ord != sp.session_ordinal {
            return Err(DiscFailure::Unwritten(PipelineError::StalePlan(format!(
                "disc {disc_name} would get session {sess_ord}, but this plan was staged for session {}",
                sp.session_ordinal
            ))));
        }

        // `plan_from` no longer emits a disc with nothing to write (issue #9),
        // but a plan staged by the build before it may still carry one: its
        // old set-A fallback: either an open disc with nothing added, or, with
        // `--finalize`, a fresh disc sealed empty. A seal of an existing disc
        // (`!is_new && will_finalize`) is the one valid empty disc.
        if disc.files.is_empty() && (disc.is_new || !disc.will_finalize) {
            return Err(DiscFailure::Unwritten(PipelineError::StalePlan(format!(
                "disc {disc_name} has nothing to write; it was planned by an older version of diskrement"
            ))));
        }

        if let Some(detail) = planning::stale_detail(disc, &report.tombstones, scanned) {
            return Err(DiscFailure::Unwritten(PipelineError::StalePlan(detail)));
        }

        let snap_root = self.paths.work_dir.join("snapshot");
        if !snap_root.exists() {
            return Err(DiscFailure::Unwritten(PipelineError::SnapshotMissing(
                p.name.clone(),
            )));
        }

        // Classified against the catalog as it stands now — after any disc
        // this cycle already committed — so a twin's files become copies of
        // the versions its sibling just recorded.
        let view = self
            .catalog
            .read_view(p.id)
            .map_err(DiscFailure::unwritten)?;
        let contents = replication::classify(&view, &disc.files);
        // A tombstone is written by the first disc to commit; later discs
        // find the path already deleted. "Still current" is the catalog's
        // history, not `read_view`: a version whose last live copy died after
        // `plan` is gone from the view but not deleted, and the manifest
        // lists its tombstone, so the catalog must record it too.
        // `fingerprints` is exactly "latest change is not DELETED", unfiltered
        // by dead discs.
        let not_deleted = self
            .catalog
            .fingerprints(p.id)
            .map_err(DiscFailure::unwritten)?;
        let tombstones: Vec<RelPath> = report
            .tombstones
            .iter()
            .filter(|t| not_deleted.contains_key(*t))
            .cloned()
            .collect();
        let manifest = manifest::build(
            &p.name,
            sess_ord,
            disc,
            &contents,
            report.tombstones.clone(),
        );
        let label = format!("{} disc {disc_name}", p.name);
        let update = CatalogUpdate {
            project_id: p.id,
            finalize_disc_ordinals: Vec::new(),
            discs: vec![disc_write(
                &label,
                disc,
                report.disc_type,
                sess_ord,
                now,
                &contents,
                tombstones,
                &manifest,
            )],
        };

        let tmp_catalog = self.paths.work_dir.join("burn-catalog.sqlite");
        if let Some(parent) = tmp_catalog.parent() {
            std::fs::create_dir_all(parent).map_err(DiscFailure::unwritten)?;
        }
        let _ = std::fs::remove_file(&tmp_catalog);
        self.catalog
            .snapshot_to(&tmp_catalog)
            .map_err(DiscFailure::unwritten)?;
        // Materialize the post-burn state into the snapshot this disc embeds.
        // Applies to the copy only; the live catalog is committed after the
        // disc is staged.
        Catalog::apply_update_to(&tmp_catalog, &update).map_err(DiscFailure::unwritten)?;

        let pointers = self
            .catalog
            .archive_pointers(p.id)
            .map_err(DiscFailure::unwritten)?;
        let image = DiscImage {
            staging_dir: self.staging_dir(&disc_name),
            snapshot_root: snap_root,
            files: disc.files.clone(),
            metadata: DiscMetadata {
                identity: DiscIdentity {
                    project_name: p.name.clone(),
                    disc_ordinal: disc.ordinal,
                    copy_set: disc.copy_set,
                    copies: p.copies,
                    session_ordinal: sess_ord,
                    created_at: now,
                },
                manifest,
                catalog_snapshot: tmp_catalog.clone(),
                pointers,
            },
            close_disc: disc.will_finalize,
        };
        let burned = burner.burn(&image).map_err(burner_failure)?;

        // The crash-safe commit point: the disc exists, so the catalog may now
        // learn about it. The committed state equals the embedded snapshot.
        self.catalog
            .apply_update(&update)
            .map_err(|e| DiscFailure::Unrecorded {
                cause: Self::failed(e),
                surely_written: true,
            })?;
        let _ = std::fs::remove_file(&tmp_catalog);
        // Only after the commit: until then the tree is part of what a retry
        // or an Unrecorded disc leaves behind.
        if !burner.retains_staging() {
            discard_staging(&burned.staging_dir);
        }

        Ok(Some(DiscBurn {
            disc: key,
            label,
            finalized: disc.will_finalize,
            staging_dir: burned.staging_dir,
            image_bytes: burned.image_bytes,
        }))
    }

    /// One past the highest session ordinal committed on each disc that has
    /// any (`PipelineLive.existingSessions`). Both BURNED and VERIFIED count.
    fn next_sessions(&self, project: ProjectId) -> Result<BTreeMap<DiscKey, u32>, PipelineError> {
        let mut next: BTreeMap<DiscKey, u32> = BTreeMap::new();
        for state in [SessionState::Burned, SessionState::Verified] {
            for s in self
                .catalog
                .sessions_by_state(project, state)
                .map_err(Self::failed)?
            {
                let n = next.entry(s.disc).or_insert(1);
                *n = (*n).max(s.ordinal + 1);
            }
        }
        Ok(next)
    }

    /// `work_dir/staging/disc-<name>`, one tree per disc (`DiscKey::name`).
    fn staging_dir(&self, disc_name: &str) -> PathBuf {
        self.paths
            .work_dir
            .join("staging")
            .join(format!("disc-{disc_name}"))
    }

    /// The session ordinal `disc` gets next: 1 for a disc with none yet.
    fn next_session_ordinal(
        &self,
        project: ProjectId,
        disc: DiscKey,
    ) -> Result<u32, PipelineError> {
        Ok(self
            .next_sessions(project)?
            .get(&disc)
            .copied()
            .unwrap_or(1))
    }

    /// `status`: catalog reads only, never mutates (`PipelineLive.status`).
    pub fn status(&self, name: &str) -> Result<StatusReport, PipelineError> {
        let p = self.project(name)?;
        let discs = self.catalog.read_disc_states(p.id).map_err(Self::failed)?;
        let view = self.catalog.read_view(p.id).map_err(Self::failed)?;
        let current = self.catalog.fingerprints(p.id).map_err(Self::failed)?;
        let burned = self
            .catalog
            .sessions_by_state(p.id, SessionState::Burned)
            .map_err(Self::failed)?;
        let verified = self
            .catalog
            .sessions_by_state(p.id, SessionState::Verified)
            .map_err(Self::failed)?;
        let last_burn_at = burned
            .iter()
            .chain(verified.iter())
            .filter_map(|s| s.burned_at)
            .max();
        // Each set's first open disc, as the single-copy report always showed
        // the first one it found.
        let mut open_discs: Vec<DiscKey> = Vec::new();
        for d in discs.iter().filter(|d| d.status == DiscStatus::Open) {
            if !open_discs.iter().any(|k| k.copy_set == d.copy_set) {
                open_discs.push(d.key());
            }
        }
        Ok(StatusReport {
            open_discs,
            coverage: replication::coverage(&view, &sizes(&current), p.copies),
            total_current_files: current.len(),
            discs,
            last_burn_at,
        })
    }

    /// `verify`: re-hash what each live disc should hold, and record a clean
    /// disc as VERIFIED (`PipelineLive.verify`).
    ///
    /// An explicitly named ordinal is an assertion of intent about *that*
    /// disc, so it is resolved before the dead-filter: silently reporting
    /// "nothing to verify" (exit 0) for a dead or nonexistent disc would read
    /// as reassurance. A sweep still skips dead discs quietly, which is the
    /// right default.
    ///
    /// `disc` is the operator's `DiscArg`, whose ordinal stays signed so
    /// `--disc -1` is answered here, with exit 2, rather than refused by the
    /// parser.
    pub fn verify(
        &mut self,
        name: &str,
        disc: Option<DiscArg>,
        verifier: &Verifier<'_>,
    ) -> Result<Vec<VerifyReport>, PipelineError> {
        let p = self.project(name)?;
        let states = self.catalog.read_disc_states(p.id).map_err(Self::failed)?;
        let sessions = self
            .catalog
            .sessions_by_state(p.id, SessionState::Burned)
            .map_err(Self::failed)?;

        let wanted = match disc {
            Some(arg) => {
                let key = resolve_disc(&p, arg, &states)?;
                if states
                    .iter()
                    .any(|s| s.key() == key && s.status == DiscStatus::Dead)
                {
                    return Err(PipelineError::DiscIsDead(key.name(p.copies)));
                }
                Some(key)
            }
            None => None,
        };

        let targets: Vec<DiscKey> = states
            .iter()
            .filter(|s| s.status != DiscStatus::Dead)
            .map(DiscState::key)
            .filter(|k| wanted.is_none_or(|w| w == *k))
            .collect();

        let mut reports = Vec::with_capacity(targets.len());
        for disc in targets {
            reports.push(self.verify_disc(&p, disc, &sessions, verifier)?);
        }
        Ok(reports)
    }

    /// One disc (`PipelineLive.verifyDisc`).
    ///
    /// The session marked VERIFIED is chosen from the BURNED list, so a disc
    /// whose session is already VERIFIED contributes nothing and the write is
    /// skipped — which is what makes a repeat `verify` a no-op rather than an
    /// illegal status transition. `max_by_key` rather than the Scala
    /// `lastOption`: session ordinals are unique per disc, so the highest is
    /// the latest regardless of row order.
    fn verify_disc(
        &mut self,
        p: &Project,
        disc: DiscKey,
        sessions: &[Session],
        verifier: &Verifier<'_>,
    ) -> Result<VerifyReport, PipelineError> {
        let latest_session = self
            .catalog
            .placements_for_disc(p.id, disc)
            .map_err(Self::failed)?;
        let recorded = self
            .catalog
            .disc_recorded_contents(p.id, disc)
            .map_err(Self::failed)?;
        let expectation = DiscExpectation {
            project_name: p.name.clone(),
            disc,
            disc_name: disc.name(p.copies),
            latest_session,
            visible: recorded.visible,
            superseded: recorded.superseded,
        };

        let outcome = verifier.verify(&expectation).map_err(PipelineError::Burn)?;
        if !outcome.skipped && outcome.mismatches.is_empty() {
            if let Some(session) = sessions
                .iter()
                .filter(|s| s.disc == disc)
                .max_by_key(|s| s.ordinal)
            {
                self.catalog
                    .mark_verified(p.id, disc, session.ordinal)
                    .map_err(Self::failed)?;
            }
        }

        Ok(VerifyReport {
            disc,
            files_checked: outcome.files_checked,
            mismatches: outcome.mismatches,
            skipped: outcome.skipped,
        })
    }

    /// `history`: every version of one path, oldest first
    /// (`PipelineLive.history`). Catalog reads only.
    pub fn history(
        &self,
        name: &str,
        rel_path: &RelPath,
    ) -> Result<Vec<FileHistoryEntry>, PipelineError> {
        let p = self.project(name)?;
        self.catalog
            .file_history(p.id, rel_path)
            .map_err(Self::failed)
    }

    /// `reset`: discard the cycle in progress (`PipelineLive.reset`).
    ///
    /// Four deletions, in order: the staged scan and plan, the temporary burn
    /// catalog, the staging trees, and the snapshot. The live catalog and the
    /// fingerprint sidecar are deliberately untouched — a reset discards a
    /// cycle, not history.
    ///
    /// Destroying `staging/` is why a `StalePlan` never advises `reset`: those
    /// trees are the staged verifier's only evidence.
    pub fn reset(&self, name: &str) -> Result<(), PipelineError> {
        self.project(name)?;
        staged::clear(&self.paths.work_dir).map_err(Self::failed)?;

        let burn_catalog = self.paths.work_dir.join("burn-catalog.sqlite");
        match std::fs::remove_file(&burn_catalog) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(Self::failed(e)),
        }

        let staging = self.paths.work_dir.join("staging");
        if staging.exists() {
            std::fs::remove_dir_all(&staging).map_err(Self::failed)?;
        }

        snapshot::release(&SnapshotHandle {
            root: self.paths.work_dir.join("snapshot"),
        });
        Ok(())
    }

    /// `disc mark-dead`: append the `MARKED_DEAD` event and report what it
    /// cost (`PipelineLive.markDead`).
    ///
    /// The two pre-checks are the whole reason this method exists rather than
    /// the CLI calling the catalog directly. `Catalog::mark_dead` refuses an
    /// unknown or already-dead ordinal too, but as `IllegalStatusTransition`,
    /// which travels as `Failed` and exits 5. The contract answer for both is
    /// exit 2, so they are caught here, against the *derived* disc states —
    /// `disc.status` never says `DEAD`.
    ///
    /// `disc` was validated by the parser as a positive ordinal with an
    /// optional set letter: mark-dead is irreversible, so a malformed argument
    /// never reaches the catalog.
    ///
    /// `at` comes from the caller, as `burn`'s `now` does: the pipeline reads
    /// no clock, which is what lets the corpus replay compare the committed
    /// timestamp exactly.
    pub fn mark_dead(
        &mut self,
        name: &str,
        disc: DiscArg,
        at: DateTime<Utc>,
    ) -> Result<LostReport, PipelineError> {
        let p = self.project(name)?;
        let states = self.catalog.read_disc_states(p.id).map_err(Self::failed)?;
        let key = resolve_disc(&p, disc, &states)?;
        if states
            .iter()
            .any(|s| s.key() == key && s.status == DiscStatus::Dead)
        {
            return Err(PipelineError::DiscAlreadyDead(key.name(p.copies)));
        }
        self.catalog.mark_dead(p.id, key, &at).map_err(Self::failed)
    }
}

/// Why one disc of a burn failed: before its media was touched, or after —
/// the disc exists (or may) but its commit did not land, so the catalog does
/// not know it.
///
/// There is deliberately no `From<PipelineError>`: every failure in
/// `burn_disc` picks its side explicitly, so a new `?` after the burner runs
/// cannot silently claim the disc was never written.
enum DiscFailure {
    Unwritten(PipelineError),
    Unrecorded {
        cause: PipelineError,
        surely_written: bool,
    },
}

impl DiscFailure {
    /// An unexpected failure before the burner touched any media.
    fn unwritten(e: impl std::error::Error + 'static) -> Self {
        DiscFailure::Unwritten(Pipeline::failed(e))
    }
}

/// Each current version's size, for `replication::coverage`.
fn sizes(current: &BTreeMap<RelPath, FileFingerprint>) -> BTreeMap<RelPath, u64> {
    current
        .iter()
        .map(|(path, f)| (path.clone(), f.size_bytes))
        .collect()
}

/// Remove a committed disc's staging tree, best-effort. The burned disc and
/// its commit are the record, so a failure here costs only space: it is
/// reported with the path and never fails the burn. Absent is fine.
fn discard_staging(dir: &Path) {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => eprintln!(
            "could not remove staging tree {} ({e}); it is safe to delete by hand",
            dir.display()
        ),
    }
}

/// Which side of the media write a burner failure falls on. Exhaustive on
/// purpose: a new `BurnError` variant must be classified here.
fn burner_failure(e: crate::burn::BurnError) -> DiscFailure {
    use crate::burn::BurnError as B;
    match e {
        // xorriso ran: a non-zero exit usually means the media was touched
        // (a BD-R is likely spent), and a failed post-burn check comes after
        // xorriso exited 0 with a session on the disc. An I/O failure while
        // xorriso ran is the same: the write may be on the media.
        B::BurnFailed { .. } | B::WriteInterrupted(_) | B::PostBurnCheckFailed(_) => {
            DiscFailure::Unrecorded {
                cause: PipelineError::Burn(e),
                surely_written: false,
            }
        }
        // Staging, tool and device checks, media inspection and the operator
        // prompt all run before anything is written.
        B::HashMismatch { .. }
        | B::InspectFailed { .. }
        | B::SourceFileMissing(_)
        | B::Io(_)
        | B::ToolMissing(_)
        | B::ToolTooOld(_)
        | B::DeviceNotFound(_)
        | B::DevicePermissionDenied(_)
        | B::NoMedia(_)
        | B::InsufficientCapacity { .. }
        | B::MountFailed(_)
        | B::Aborted => DiscFailure::Unwritten(PipelineError::Burn(e)),
    }
}

/// The disc `arg` names in this project, or `UnknownDisc` echoing the
/// argument. A bare ordinal means set A; a letter past the project's last set,
/// or an ordinal no disc has, is unknown.
fn resolve_disc(p: &Project, arg: DiscArg, states: &[DiscState]) -> Result<DiscKey, PipelineError> {
    arg.resolve(p.copies)
        .filter(|k| states.iter().any(|s| s.key() == *k))
        .ok_or_else(|| PipelineError::UnknownDisc(arg.to_string()))
}

/// `getFileStore` equality via `st_dev` (`PipelineLive.sameFilesystem`): both
/// paths must exist and carry the same device id.
fn same_filesystem(source: &Path, work: &Path) -> std::io::Result<(u64, u64)> {
    Ok((
        std::fs::metadata(source)?.dev(),
        std::fs::metadata(work)?.dev(),
    ))
}

/// The catalog write for one disc: its files split by commit-time
/// classification and stamped with their `/data/`-rooted disc path, plus the
/// tombstones still current in the catalog.
#[allow(clippy::too_many_arguments)]
fn disc_write(
    label: &str,
    disc: &PlannedDisc,
    disc_type: DiscType,
    sess_ord: u32,
    now: DateTime<Utc>,
    contents: &DiscContents,
    tombstones: Vec<RelPath>,
    manifest: &SessionManifest,
) -> DiscWrite {
    let fw = |f: &ScannedFile| FileWrite {
        rel_path: f.rel_path.clone(),
        content_hash: f.content_hash.clone(),
        size_bytes: f.size_bytes,
        mtime: f.mtime,
        disc_path: format!("/data/{}", f.rel_path.0),
    };
    DiscWrite {
        ordinal: disc.ordinal,
        copy_set: disc.copy_set,
        is_new: disc.is_new,
        disc_type,
        usable_bytes: disc.usable_bytes,
        status: if disc.will_finalize {
            DiscStatus::Finalized
        } else {
            DiscStatus::Open
        },
        label: label.to_string(),
        session: SessionWrite {
            ordinal: sess_ord,
            burned_at: now,
            manifest_hash: json::manifest_hash(manifest),
            added: contents.added.iter().map(fw).collect(),
            modified: contents.modified.iter().map(fw).collect(),
            copied: contents.copied.iter().map(fw).collect(),
            deleted: tombstones,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::progress::NoopSink;
    use diskrement_core::DiscType;
    use std::path::PathBuf;

    /// Real catalog, real filesystems. The catalog leaks into a `'static`
    /// pipeline for test ergonomics; its TempDir lives in `_dirs`.
    struct Env {
        _dirs: Vec<tempfile::TempDir>,
        src: PathBuf,
        work: PathBuf,
        pipeline: Pipeline<'static>,
    }

    fn setup() -> Env {
        let cat_dir = tempfile::TempDir::new().unwrap();
        let src_dir = tempfile::TempDir::new().unwrap();
        let work_dir = tempfile::TempDir::new().unwrap();
        let (src, work) = (src_dir.path().to_path_buf(), work_dir.path().to_path_buf());
        let catalog = Box::leak(Box::new(
            Catalog::open(&cat_dir.path().join("catalog.sqlite")).unwrap(),
        ));
        let pipeline = Pipeline::new(
            catalog,
            PipelinePaths {
                work_dir: work.clone(),
                scan_cache: work.join("cat.p.scancache.sqlite"),
            },
            &NoopSink,
        );
        Env {
            _dirs: vec![cat_dir, src_dir, work_dir],
            src,
            work,
            pipeline,
        }
    }

    fn project_init(env: &Env) -> ProjectInit {
        ProjectInit {
            name: "p".into(),
            source_root: env.src.display().to_string(),
            work_dir: env.work.display().to_string(),
            default_disc_type: DiscType::BdR,
            margin_percent: 2.0,
            copies: 1,
            created_at: crate::catalog::testutil::ts(),
        }
    }

    fn write_src(env: &Env, rel: &str, bytes: &[u8]) {
        let p = env.src.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    #[test]
    fn init_creates_a_project_and_status_reports_zero_discs() {
        let mut e = setup();
        let id = e.pipeline.init(&project_init(&e)).unwrap();
        assert_eq!(id.0, 1);
        let r = e.pipeline.status("p").unwrap();
        assert!(r.discs.is_empty());
        assert_eq!(r.total_current_files, 0);
        assert!(r.open_discs.is_empty());
        assert_eq!(r.last_burn_at, None);
    }

    #[test]
    fn init_twice_fails_with_the_duplicate_project_detail() {
        let mut e = setup();
        e.pipeline.init(&project_init(&e)).unwrap();
        let err = e.pipeline.init(&project_init(&e)).unwrap_err();
        match err {
            PipelineError::Failed(detail) => {
                assert!(detail.contains("project already exists: p"), "{detail}")
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn status_on_an_unknown_project_fails_projectnotfound() {
        let e = setup();
        let err = e.pipeline.status("nosuch").unwrap_err();
        assert!(matches!(err, PipelineError::ProjectNotFound(_)));
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn scan_on_a_fresh_project_reports_all_added_and_stages_them() {
        let mut e = setup();
        write_src(&e, "a.txt", b"aaaaa");
        write_src(&e, "sub/b.txt", b"b");
        e.pipeline.init(&project_init(&e)).unwrap();
        let r = e.pipeline.scan("p", false, false).unwrap();
        assert_eq!((r.added, r.modified, r.deleted, r.unchanged), (2, 0, 0, 0));
        assert_eq!(r.hashed, Some(2));
        assert_eq!(r.scanned_files, 2);
        assert_eq!(staged::read_scan(&e.work).unwrap().unwrap().len(), 2);
    }

    /// An unexpected failure still says what failed: the rendered detail
    /// carries the whole cause chain and the file, not just the outermost
    /// "snapshot I/O error".
    #[test]
    fn an_unexpected_scan_failure_names_the_file_and_the_os_error() {
        use std::os::unix::fs::PermissionsExt as _;
        let mut e = setup();
        write_src(&e, "secret.bin", b"s");
        std::fs::set_permissions(
            e.src.join("secret.bin"),
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();
        e.pipeline.init(&project_init(&e)).unwrap();
        if std::fs::File::open(e.src.join("secret.bin")).is_ok() {
            eprintln!("SKIP: running as root, a mode-000 file is still readable");
            return;
        }

        let err = e.pipeline.scan("p", false, false).unwrap_err();
        let msg = err.to_string();
        assert_eq!(err.exit_code(), 5);
        assert!(msg.contains("snapshot I/O error"), "{msg}");
        assert!(msg.contains("secret.bin"), "{msg}");
        assert!(msg.contains("Permission denied"), "{msg}");
    }

    #[test]
    fn a_second_scan_without_burn_reuses_the_staged_scan() {
        // The reuse rule: the second scan reports hashed = None and still
        // says added (nothing burned yet, so the catalog view is empty).
        let mut e = setup();
        write_src(&e, "a.txt", b"a");
        write_src(&e, "b.txt", b"bb");
        e.pipeline.init(&project_init(&e)).unwrap();
        assert_eq!(e.pipeline.scan("p", false, false).unwrap().hashed, Some(2));
        let second = e.pipeline.scan("p", false, false).unwrap();
        assert_eq!(second.hashed, None);
        assert_eq!(second.added, 2);
    }

    #[test]
    fn a_forced_rescan_hashes_nothing_once_the_cache_is_primed() {
        let mut e = setup();
        for (n, bytes) in [
            ("a.txt", &b"a"[..]),
            ("b.txt", &b"bb"[..]),
            ("c.txt", &b"ccc"[..]),
        ] {
            write_src(&e, n, bytes);
        }
        e.pipeline.init(&project_init(&e)).unwrap();
        assert_eq!(e.pipeline.scan("p", false, false).unwrap().hashed, Some(3));
        let second = e.pipeline.scan("p", false, true).unwrap(); // force: fresh scan
        assert_eq!(second.hashed, Some(0));
        assert_eq!(second.added, 3); // nothing burned yet
        assert_eq!(second.unchanged, 0);
    }

    #[test]
    fn a_forced_rescan_rehashes_only_a_modified_file() {
        let mut e = setup();
        write_src(&e, "a.txt", b"a");
        write_src(&e, "b.txt", b"bb");
        e.pipeline.init(&project_init(&e)).unwrap();
        e.pipeline.scan("p", false, false).unwrap();
        write_src(&e, "b.txt", b"bbb");
        assert_eq!(e.pipeline.scan("p", false, true).unwrap().hashed, Some(1));
    }

    #[test]
    fn verify_forces_a_full_rehash_even_with_a_primed_cache() {
        let mut e = setup();
        write_src(&e, "a.txt", b"a");
        write_src(&e, "b.txt", b"bb");
        e.pipeline.init(&project_init(&e)).unwrap();
        e.pipeline.scan("p", false, false).unwrap();
        assert_eq!(e.pipeline.scan("p", true, true).unwrap().hashed, Some(2));
    }

    #[test]
    fn verify_with_a_staged_scan_and_no_force_still_rehashes() {
        let mut e = setup();
        write_src(&e, "a.txt", b"a");
        write_src(&e, "b.txt", b"bb");
        e.pipeline.init(&project_init(&e)).unwrap();
        e.pipeline.scan("p", false, false).unwrap();
        assert_eq!(e.pipeline.scan("p", true, false).unwrap().hashed, Some(2));
    }

    /// A pipeline whose cache path is a directory: every cache op then fails
    /// non-corruptly (open error), exercising the never-fatal rule end to
    /// end. The cache TempDir is leaked so it outlives the leaked pipeline.
    fn with_poisoned_cache(e: &Env) -> Pipeline<'static> {
        let catalog = Box::leak(Box::new(
            Catalog::open(&e._dirs[0].path().join("catalog.sqlite")).unwrap(),
        ));
        let cache_dir = Box::leak(Box::new(tempfile::TempDir::new().unwrap()));
        std::fs::create_dir_all(cache_dir.path().join("cache")).unwrap();
        Pipeline::new(
            catalog,
            PipelinePaths {
                work_dir: e.work.clone(),
                scan_cache: cache_dir.path().join("cache"), // a directory
            },
            &NoopSink,
        )
    }

    #[test]
    fn a_failing_cache_read_still_succeeds_and_hashes_everything() {
        let e = setup();
        let mut pl = with_poisoned_cache(&e);
        write_src(&e, "a.txt", b"a");
        write_src(&e, "b.txt", b"bb");
        write_src(&e, "c.txt", b"ccc");
        pl.init(&project_init(&e)).unwrap();
        let r = pl.scan("p", false, false).unwrap();
        assert_eq!(r.hashed, Some(3));
        assert_eq!(r.added, 3);
    }

    #[test]
    fn a_failing_cache_write_still_succeeds_and_nothing_was_cached() {
        let e = setup();
        let mut pl = with_poisoned_cache(&e);
        write_src(&e, "a.txt", b"a");
        write_src(&e, "b.txt", b"bb");
        write_src(&e, "c.txt", b"ccc");
        pl.init(&project_init(&e)).unwrap();
        assert_eq!(pl.scan("p", false, false).unwrap().hashed, Some(3));
        // nothing cached, so the forced scan re-hashes everything
        assert_eq!(pl.scan("p", false, true).unwrap().hashed, Some(3));
    }

    /// A project whose margin leaves 250_000 usable bytes, so large-ish files
    /// spill and go oversized without touching `DISKREMENT_DISC_BYTES` — which
    /// is process-global and unusable from parallel tests.
    ///
    /// Deviation from the plan's 99.9999998% → 50 bytes (the Scala
    /// PipelineSpillSpec idiom): `Capacity::from_disc_type` quantises the
    /// margin to thousandths of a percent, so 99.9999998% rounds to a full
    /// 100% margin and 0 usable bytes — every file would be oversized. One
    /// thousandth of a percent of a BD-R is 250_000 bytes, the smallest
    /// non-zero usable capacity a margin can produce here; the files below
    /// are sized to that.
    fn tiny_project_init(env: &Env) -> ProjectInit {
        ProjectInit {
            margin_percent: 99.999,
            ..project_init(env)
        }
    }

    #[test]
    fn plan_without_a_staged_scan_is_exit_2_with_the_run_scan_advice() {
        let mut e = setup();
        e.pipeline.init(&project_init(&e)).unwrap();
        let err = e.pipeline.plan("p", None, false).unwrap_err();
        assert_eq!(err.to_string(), "no staged scan for p (run scan)");
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn plan_on_an_unknown_project_fails_projectnotfound() {
        let e = setup();
        let err = e.pipeline.plan("nosuch", None, false).unwrap_err();
        assert!(matches!(err, PipelineError::ProjectNotFound(_)));
    }

    #[test]
    fn plan_stages_the_report_it_returns() {
        let mut e = setup();
        write_src(&e, "a.txt", b"aaaaa");
        e.pipeline.init(&project_init(&e)).unwrap();
        e.pipeline.scan("p", false, false).unwrap();
        let report = e.pipeline.plan("p", None, false).unwrap();
        assert_eq!(report.sets[0].disc.ordinal, 1);
        assert_eq!(report.sets[0].disc.files.len(), 1);
        assert_eq!(report.disc_type, DiscType::BdR);
        let staged_report = staged::read_plan(&e.work).unwrap().unwrap();
        assert_eq!(staged_report, report);
    }

    #[test]
    fn plan_uses_the_projects_default_disc_type_and_margin() {
        let mut e = setup();
        write_src(&e, "a.txt", b"a");
        e.pipeline.init(&project_init(&e)).unwrap();
        e.pipeline.scan("p", false, false).unwrap();
        let report = e.pipeline.plan("p", None, false).unwrap();
        assert_eq!(
            report.sets[0].disc.usable_bytes,
            diskrement_core::Capacity::from_disc_type(DiscType::BdR, 2.0).usable_bytes
        );
    }

    #[test]
    fn an_explicit_disc_type_overrides_the_projects_default() {
        let mut e = setup();
        write_src(&e, "a.txt", b"a");
        e.pipeline.init(&project_init(&e)).unwrap();
        e.pipeline.scan("p", false, false).unwrap();
        let report = e.pipeline.plan("p", Some(DiscType::Dvd), false).unwrap();
        assert_eq!(report.disc_type, DiscType::Dvd);
        assert_eq!(
            report.sets[0].disc.usable_bytes,
            diskrement_core::Capacity::from_disc_type(DiscType::Dvd, 2.0).usable_bytes
        );
    }

    #[test]
    fn finalize_seals_a_disc_that_would_have_been_left_open() {
        let mut e = setup();
        write_src(&e, "a.txt", b"a");
        e.pipeline.init(&project_init(&e)).unwrap();
        e.pipeline.scan("p", false, false).unwrap();
        assert!(
            !e.pipeline.plan("p", None, false).unwrap().sets[0]
                .disc
                .will_finalize
        );
        assert!(
            e.pipeline.plan("p", None, true).unwrap().sets[0]
                .disc
                .will_finalize
        );
    }

    #[test]
    fn a_spilling_plan_seals_this_cycles_disc_and_reports_the_rest() {
        let mut e = setup();
        write_src(&e, "a.txt", &[b'a'; 200_000]);
        write_src(&e, "b.txt", &[b'b'; 200_000]);
        e.pipeline.init(&tiny_project_init(&e)).unwrap();
        e.pipeline.scan("p", false, false).unwrap();
        let report = e.pipeline.plan("p", None, false).unwrap();
        assert_eq!(report.sets[0].disc.files.len(), 1);
        assert!(report.sets[0].disc.will_finalize);
        assert_eq!(report.sets[0].remaining.more_discs, 1);
        assert_eq!(report.sets[0].remaining.more_files, 1);
        assert_eq!(report.sets[0].remaining.more_bytes, 200_000);
    }

    #[test]
    fn a_file_larger_than_any_disc_is_surfaced_as_oversized() {
        let mut e = setup();
        write_src(&e, "big.txt", &[b'x'; 300_000]);
        e.pipeline.init(&tiny_project_init(&e)).unwrap();
        e.pipeline.scan("p", false, false).unwrap();
        let report = e.pipeline.plan("p", None, false).unwrap();
        assert_eq!(report.oversized.len(), 1);
        assert_eq!(report.oversized[0].rel_path.as_str(), "big.txt");
    }

    #[test]
    fn re_planning_overwrites_the_staged_plan_and_leaves_the_staged_scan() {
        // `plan` does not consume the staged scan; `burn` clears staged state.
        let mut e = setup();
        write_src(&e, "a.txt", b"a");
        e.pipeline.init(&project_init(&e)).unwrap();
        e.pipeline.scan("p", false, false).unwrap();
        e.pipeline.plan("p", None, false).unwrap();
        let second = e.pipeline.plan("p", None, true).unwrap();
        assert!(second.sets[0].disc.will_finalize);
        assert_eq!(staged::read_plan(&e.work).unwrap().unwrap(), second);
        assert_eq!(staged::read_scan(&e.work).unwrap().unwrap().len(), 1);
    }

    /// Row counts of every table `burn` writes. Spelled out here rather than
    /// reached for through `crate::testutil` so the test does not depend on
    /// the `testutil` feature being enabled for the lib's own unit tests.
    fn burn_written_rows(pipeline: &Pipeline<'_>) -> Vec<String> {
        let conn = pipeline.catalog.raw_connection_for_tests();
        [
            "disc",
            "session",
            "file_version",
            "placement",
            "change_record",
        ]
        .iter()
        .map(|table| {
            let n: i64 = conn
                .query_row(&format!("select count(*) from {table}"), [], |r| r.get(0))
                .expect("countable");
            format!("{table}={n}")
        })
        .collect()
    }

    #[test]
    fn plan_writes_no_catalog_rows() {
        // The catalog is append-only and only `burn` appends. A plan that
        // wrote rows would break the "no PLANNED rows are persisted"
        // invariant: the staged plan in the work dir is the recovery unit.
        let mut e = setup();
        write_src(&e, "a.txt", b"a");
        e.pipeline.init(&project_init(&e)).unwrap();
        e.pipeline.scan("p", false, false).unwrap();
        let before = burn_written_rows(&e.pipeline);
        e.pipeline.plan("p", None, false).unwrap();
        assert_eq!(burn_written_rows(&e.pipeline), before);
        assert!(before.iter().all(|line| line.ends_with("=0")), "{before:?}");
    }
}

#[cfg(test)]
mod burn_tests {
    use super::*;
    use chrono::{TimeZone as _, Utc};
    use diskrement_core::{Coverage, DiscKey, DiscType, RelPath, ScannedFile, SetCoverage};

    use crate::burn::testkit::{ScriptedExec, ScriptedOperator, BLANK};
    use crate::burn::xorriso::burner::XorrisoBurner;
    use crate::burn::{simulated::SimulatedBurner, BurnDevice, BurnError, BurnOperator, Burner};
    use crate::catalog::model::ProjectInit;
    use crate::progress::NoopSink;

    struct Env {
        _dir: tempfile::TempDir,
        catalog: Catalog,
        work: PathBuf,
        scan_cache: PathBuf,
        source: PathBuf,
    }

    fn now() -> chrono::DateTime<Utc> {
        Utc.timestamp_opt(1_757_100_066, 623_959_846).unwrap()
    }

    /// A project whose snapshot already holds `files`, with `scan.json` and
    /// `plan.json` staged — the state `burn` expects to find.
    fn staged_env(files: &[(&str, &[u8])], finalize: bool) -> Env {
        staged_env_with(files, finalize, 1)
    }

    /// `staged_env` for a project keeping `copies` copies.
    fn staged_env_with(files: &[(&str, &[u8])], finalize: bool, copies: u32) -> Env {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let work = root.join("work");
        let source = root.join("source");
        let snapshot = work.join("snapshot");
        std::fs::create_dir_all(&snapshot).unwrap();
        std::fs::create_dir_all(&source).unwrap();

        let mut scanned: Vec<ScannedFile> = Vec::new();
        for (rel, body) in files {
            let abs = snapshot.join(rel);
            std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
            std::fs::write(&abs, body).unwrap();
            scanned.push(ScannedFile {
                rel_path: RelPath((*rel).to_string()),
                content_hash: crate::fs::hasher::hash(&abs).unwrap(),
                size_bytes: body.len() as u64,
                mtime: Utc.timestamp_opt(1_757_000_000, 0).unwrap(),
            });
        }

        let mut catalog = Catalog::open(&root.join("catalog.sqlite")).unwrap();
        catalog
            .create_project(&ProjectInit {
                name: "p".to_string(),
                source_root: source.display().to_string(),
                work_dir: work.display().to_string(),
                default_disc_type: DiscType::BdR,
                margin_percent: 2.0,
                copies,
                created_at: now(),
            })
            .unwrap();

        staged::write_scan(&work, &scanned).unwrap();
        let report = crate::pipeline::planning::plan_from(
            &diskrement_core::CatalogView::empty(),
            &scanned,
            Vec::new(),
            &[],
            DiscType::BdR,
            diskrement_core::Capacity::new(1_000_000),
            finalize,
            copies,
            |_| 1,
        );
        staged::write_plan(&work, &report).unwrap();

        Env {
            _dir: dir,
            catalog,
            work: work.clone(),
            scan_cache: root.join("scancache.sqlite"),
            source,
        }
    }

    fn run_burn(env: &mut Env) -> Result<BurnReport, PipelineError> {
        let sink = NoopSink;
        let burner = Burner::Simulated(SimulatedBurner::new(&sink, 2));
        let mut pipeline = Pipeline::new(
            &mut env.catalog,
            PipelinePaths {
                work_dir: env.work.clone(),
                scan_cache: env.scan_cache.clone(),
            },
            &sink,
        );
        pipeline.burn("p", now(), &burner)
    }

    /// `project.json` as the scripted drive reads it back after each burn.
    /// Only project, disc and session are compared, not `createdAt`.
    const ID_1: &str = r#"{"projectName":"p","discOrdinal":1,"sessionOrdinal":1,"createdAt":"2026-08-30T10:00:00Z"}"#;
    const ID_1A: &str = r#"{"projectName":"p","discOrdinal":1,"copySet":0,"copies":2,"sessionOrdinal":1,"createdAt":"2026-08-30T10:00:00Z"}"#;
    const ID_1B: &str = r#"{"projectName":"p","discOrdinal":1,"copySet":1,"copies":2,"sessionOrdinal":1,"createdAt":"2026-08-30T10:00:00Z"}"#;

    /// `run_burn` with the real backend, xorriso scripted by `script`.
    fn run_real_burn(
        env: &mut Env,
        script: &ScriptedExec,
        operator: &dyn BurnOperator,
    ) -> Result<BurnReport, PipelineError> {
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
        run_real_burn_with(env, &exec, operator)
    }

    /// `run_real_burn` with any `Exec`, for scripts that fail mid-run.
    fn run_real_burn_with(
        env: &mut Env,
        exec: &dyn crate::burn::cmd::Exec,
        operator: &dyn BurnOperator,
    ) -> Result<BurnReport, PipelineError> {
        let sink = NoopSink;
        let burner = Burner::Real(XorrisoBurner::new(
            BurnDevice::RealDevice("/dev/sr0".to_string()),
            exec,
            operator,
            &sink,
            2,
        ));
        let mut pipeline = Pipeline::new(
            &mut env.catalog,
            PipelinePaths {
                work_dir: env.work.clone(),
                scan_cache: env.scan_cache.clone(),
            },
            &sink,
        );
        pipeline.burn("p", now(), &burner)
    }

    /// Confirms every insert, recording whether `probe` exists at each prompt.
    struct ProbeOperator {
        probe: PathBuf,
        seen: std::cell::RefCell<Vec<bool>>,
    }

    impl BurnOperator for ProbeOperator {
        fn confirm_insert(&self, _message: &str) -> bool {
            self.seen.borrow_mut().push(self.probe.exists());
            true
        }

        fn progress(&self, _line: &str) {}
    }

    fn burned_sessions(env: &Env) -> usize {
        env.catalog
            .sessions_by_state(ProjectId(1), SessionState::Burned)
            .unwrap()
            .len()
    }

    #[test]
    fn a_real_burn_removes_the_staging_tree_once_the_disc_commits() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], false);
        let script = ScriptedExec::new().toc(&[BLANK]).identities(&[Some(ID_1)]);
        let op = ScriptedOperator::new(&[true]);

        let report = run_real_burn(&mut env, &script, &op).unwrap();

        assert_eq!(report.discs.len(), 1);
        assert!(script.ran("-commit"), "the disc was written");
        assert_eq!(burned_sessions(&env), 1);
        assert!(
            !env.work.join("staging/disc-1").exists(),
            "the committed disc's tree is gone"
        );
    }

    /// Twins are staged from the snapshot, never from each other: 1A's tree
    /// is already gone when the operator is asked for 1B, and 1B still stages
    /// (the stager re-hashes every file) and commits.
    #[test]
    fn a_two_copy_real_burn_removes_each_tree_before_the_next_disc() {
        let mut env = staged_env_with(&[("a.bin", b"aaaa")], false, 2);
        let script = ScriptedExec::new()
            .toc(&[BLANK, BLANK])
            .identities(&[Some(ID_1A), Some(ID_1B)]);
        let op = ProbeOperator {
            probe: env.work.join("staging/disc-1A"),
            seen: Default::default(),
        };

        let report = run_real_burn(&mut env, &script, &op).unwrap();

        assert_eq!(
            report.discs.iter().map(|d| d.disc).collect::<Vec<_>>(),
            vec![DiscKey::new(1, 0), DiscKey::new(1, 1)]
        );
        assert_eq!(burned_sessions(&env), 2);
        assert_eq!(
            *op.seen.borrow(),
            vec![false, false],
            "1A's tree was removed before 1B's prompt"
        );
        assert!(!env.work.join("staging/disc-1A").exists());
        assert!(!env.work.join("staging/disc-1B").exists());
    }

    /// Commit after success: a disc whose commit failed keeps its tree; the
    /// twin committed before it has already lost its own.
    #[test]
    fn a_real_burn_keeps_the_tree_of_a_disc_whose_commit_failed() {
        let mut env = staged_env_with(&[("a.bin", b"aaaa")], false, 2);
        env.catalog
            .raw_connection_for_tests()
            .execute_batch(
                "create temp trigger refuse_1b before insert on disc
                   when new.copy_set = 1
                 begin select raise(abort, 'refused for the test'); end;",
            )
            .unwrap();
        let script = ScriptedExec::new()
            .toc(&[BLANK, BLANK])
            .identities(&[Some(ID_1A), Some(ID_1B)]);
        let op = ScriptedOperator::new(&[true, true]);

        let err = run_real_burn(&mut env, &script, &op).unwrap_err();

        assert_eq!(
            partial_parts(&err),
            (
                vec!["1A".to_string()],
                Some(UnrecordedDisc::written("1B")),
                vec![]
            )
        );
        assert!(!env.work.join("staging/disc-1A").exists());
        assert!(env.work.join("staging/disc-1B/data/a.bin").is_file());
    }

    /// An I/O failure while xorriso writes 1B, after 1A committed, reaches the
    /// operator as "may have been written", never as "not written" (#11).
    #[test]
    fn an_io_failure_mid_write_reports_the_disc_as_maybe_written() {
        let mut env = staged_env_with(&[("a.bin", b"aaaa")], false, 2);
        let script = ScriptedExec::new()
            .toc(&[BLANK, BLANK])
            .identities(&[Some(ID_1A)]);
        let commits = std::cell::Cell::new(0);
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| {
            if c.iter().any(|a| a == "-commit") {
                commits.set(commits.get() + 1);
                if commits.get() == 2 {
                    return Err(BurnError::Io(std::io::Error::other("broken pipe")));
                }
            }
            script.respond(c, l)
        };

        let err =
            run_real_burn_with(&mut env, &exec, &ScriptedOperator::new(&[true, true])).unwrap_err();

        assert_eq!(
            partial_parts(&err),
            (
                vec!["1A".to_string()],
                Some(UnrecordedDisc::maybe_written("1B")),
                vec![]
            )
        );
        assert_eq!(err.exit_code(), 6);
    }

    /// An unclassified `-toc` failure while checking 1B's media comes before
    /// any write, so a partial burn lists 1B as not written (#17).
    #[test]
    fn an_inspection_failure_reports_the_disc_as_not_written() {
        let mut env = staged_env_with(&[("a.bin", b"aaaa")], false, 2);
        let script = ScriptedExec::new().toc(&[BLANK]).identities(&[Some(ID_1A)]);
        let tocs = std::cell::Cell::new(0);
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| {
            if c.iter().any(|a| a == "-toc") {
                tocs.set(tocs.get() + 1);
                if tocs.get() == 2 {
                    return Ok(crate::burn::cmd::CmdOutput {
                        exit_code: 5,
                        lines: vec!["xorriso : FAILURE : odd drive".to_string()],
                    });
                }
            }
            script.respond(c, l)
        };

        let err =
            run_real_burn_with(&mut env, &exec, &ScriptedOperator::new(&[true, true])).unwrap_err();

        assert_eq!(
            partial_parts(&err),
            (vec!["1A".to_string()], None, vec!["1B".to_string()])
        );
        assert!(err.to_string().contains("odd drive"), "{err}");
        assert_eq!(err.exit_code(), 6);
    }

    /// Pins existing behaviour: a declined prompt stages and commits nothing.
    #[test]
    fn a_declined_real_burn_stages_nothing_and_commits_nothing() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], false);
        let script = ScriptedExec::new().toc(&[BLANK]);
        let op = ScriptedOperator::new(&[false]);

        let err = run_real_burn(&mut env, &script, &op).unwrap_err();

        assert!(
            matches!(err, PipelineError::Burn(BurnError::Aborted)),
            "{err}"
        );
        assert_eq!(err.exit_code(), 0);
        assert!(!env.work.join("staging").exists());
        assert_eq!(burned_sessions(&env), 0);
    }

    #[test]
    fn resuming_a_real_burn_removes_a_committed_discs_leftover_tree() {
        let mut env = staged_env_with(&[("a.bin", b"aaaa")], false, 2);
        // A directory where the rewrite's temp file goes makes it fail, so
        // the staged plan still lists 1A after 1A commits.
        let tmp = env.work.join("staged/plan.json.tmp");
        std::fs::create_dir_all(&tmp).unwrap();
        let script = ScriptedExec::new().toc(&[BLANK]).identities(&[Some(ID_1A)]);
        let err = run_real_burn(&mut env, &script, &ScriptedOperator::new(&[true])).unwrap_err();
        assert_eq!(
            partial_parts(&err),
            (vec!["1A".to_string()], None, vec!["1B".to_string()])
        );

        // What a crash between 1A's commit and its cleanup leaves behind.
        let leftover = env.work.join("staging/disc-1A/data");
        std::fs::create_dir_all(&leftover).unwrap();
        std::fs::write(leftover.join("a.bin"), b"aaaa").unwrap();
        std::fs::remove_dir(&tmp).unwrap();

        let script = ScriptedExec::new().toc(&[BLANK]).identities(&[Some(ID_1B)]);
        let report = run_real_burn(&mut env, &script, &ScriptedOperator::new(&[true])).unwrap();

        assert_eq!(
            report.discs.iter().map(|d| d.disc).collect::<Vec<_>>(),
            vec![DiscKey::new(1, 1)],
            "1A is not burned twice"
        );
        assert!(
            !env.work.join("staging/disc-1A").exists(),
            "the resume path cleans up 1A"
        );
        assert!(!env.work.join("staging/disc-1B").exists());
    }

    #[test]
    fn discarding_a_staging_tree_removes_all_of_it() {
        let dir = tempfile::TempDir::new().unwrap();
        let tree = dir.path().join("staging/disc-1");
        std::fs::create_dir_all(tree.join("data/n")).unwrap();
        std::fs::write(tree.join("data/n/a.bin"), b"a").unwrap();

        discard_staging(&tree);

        assert!(!tree.exists());
        assert!(dir.path().join("staging").is_dir(), "only the disc's tree");
    }

    #[test]
    fn discarding_a_missing_staging_tree_is_quiet() {
        let dir = tempfile::TempDir::new().unwrap();
        discard_staging(&dir.path().join("staging/disc-9"));
        assert!(dir.path().is_dir());
    }

    /// A tree that cannot be removed is left in place, and the call still
    /// returns: the disc is already committed, so this costs space, not data.
    #[test]
    fn a_staging_tree_that_cannot_be_removed_is_left_without_failing() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::TempDir::new().unwrap();
        let tree = dir.path().join("staging/disc-1");
        let locked = tree.join("data/locked");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::write(locked.join("a.bin"), b"a").unwrap();
        std::fs::write(locked.join("probe"), b"p").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        if std::fs::remove_file(locked.join("probe")).is_ok() {
            eprintln!("SKIP: running as root, a read-only directory is still writable");
            return;
        }

        discard_staging(&tree);

        assert!(locked.join("a.bin").exists(), "the tree is left in place");
        // Let the TempDir clean up after itself.
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn burning_stages_the_disc_and_commits_one_session() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], false);
        let report = run_burn(&mut env).unwrap();

        assert_eq!(report.discs[0].disc.ordinal, 1);
        assert_eq!(report.discs[0].label, "p disc 1");
        assert!(!report.discs[0].finalized);
        assert!(env.work.join("staging/disc-1/data/a.bin").is_file());
        assert!(env
            .work
            .join("staging/disc-1/.diskrement/catalog.sqlite")
            .is_file());

        let sessions = env
            .catalog
            .sessions_by_state(ProjectId(1), SessionState::Burned)
            .unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].disc.ordinal, 1);
        assert_eq!(sessions[0].ordinal, 1);
    }

    /// The staged plan is consumed, the snapshot released, and the temporary
    /// burn catalog removed — the whole work-dir cleanup in one place.
    #[test]
    fn a_successful_burn_clears_the_staged_state() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], false);
        run_burn(&mut env).unwrap();

        assert!(staged::read_scan(&env.work).unwrap().is_none());
        assert!(staged::read_plan(&env.work).unwrap().is_none());
        assert!(!env.work.join("snapshot").exists(), "snapshot released");
        assert!(!env.work.join("burn-catalog.sqlite").exists());
        assert!(env.work.join("staging/disc-1").is_dir(), "staging survives");
    }

    /// The embedded snapshot is post-burn: it already knows about the session
    /// being written, so a single disc describes the whole archive.
    #[test]
    fn the_embedded_catalog_already_contains_this_burns_session() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], false);
        run_burn(&mut env).unwrap();

        let embedded = rusqlite::Connection::open_with_flags(
            env.work.join("staging/disc-1/.diskrement/catalog.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let sessions: i64 = embedded
            .query_row("select count(*) from session", [], |r| r.get(0))
            .unwrap();
        assert_eq!(sessions, 1, "the disc knows about its own session");
    }

    #[test]
    fn burning_without_a_staged_scan_is_a_state_error() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], false);
        staged::clear(&env.work).unwrap();
        let err = run_burn(&mut env).unwrap_err();
        assert!(matches!(err, PipelineError::NoStagedScan(_)), "{err}");
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn burning_without_a_staged_plan_is_a_state_error() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], false);
        std::fs::remove_file(staged::plan_path(&env.work)).unwrap();
        let err = run_burn(&mut env).unwrap_err();
        assert!(matches!(err, PipelineError::NoStagedPlan(_)), "{err}");
    }

    #[test]
    fn burning_without_a_snapshot_is_a_state_error() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], false);
        std::fs::remove_dir_all(env.work.join("snapshot")).unwrap();
        let err = run_burn(&mut env).unwrap_err();
        assert!(matches!(err, PipelineError::SnapshotMissing(_)), "{err}");
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn burning_an_unknown_project_is_a_lookup_error() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], false);
        let sink = NoopSink;
        let burner = Burner::Simulated(SimulatedBurner::new(&sink, 1));
        let mut pipeline = Pipeline::new(
            &mut env.catalog,
            PipelinePaths {
                work_dir: env.work.clone(),
                scan_cache: env.scan_cache.clone(),
            },
            &sink,
        );
        let err = pipeline.burn("nosuch", now(), &burner).unwrap_err();
        assert!(matches!(err, PipelineError::ProjectNotFound(_)), "{err}");
    }

    /// A failed burn commits nothing: the catalog is untouched and the staged
    /// plan survives, so the operator can retry.
    #[test]
    fn a_failed_burn_leaves_the_catalog_and_the_staged_plan_alone() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], false);
        // Delete the source file after planning: the stager's existence check
        // fails, standing in for any mid-burn failure.
        std::fs::remove_file(env.work.join("snapshot/a.bin")).unwrap();

        let err = run_burn(&mut env).unwrap_err();
        assert!(matches!(err, PipelineError::Burn(_)), "{err}");
        assert_eq!(err.exit_code(), 6);

        assert!(
            staged::read_plan(&env.work).unwrap().is_some(),
            "plan survives"
        );
        let sessions = env
            .catalog
            .sessions_by_state(ProjectId(1), SessionState::Burned)
            .unwrap();
        assert!(sessions.is_empty(), "nothing committed");
    }

    /// `plan` records oversized files and still succeeds; `burn` is the half
    /// that refuses them.
    #[test]
    fn oversized_files_are_refused_at_burn_time() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], false);
        let mut report = staged::read_plan(&env.work).unwrap().unwrap();
        report.oversized.push(ScannedFile {
            rel_path: RelPath("huge.bin".to_string()),
            content_hash: diskrement_core::ContentHash("00".repeat(32)),
            size_bytes: 1 << 40,
            mtime: now(),
        });
        staged::write_plan(&env.work, &report).unwrap();

        let err = run_burn(&mut env).unwrap_err();
        assert!(matches!(err, PipelineError::OversizedFiles(_)), "{err}");
        assert_eq!(err.exit_code(), 3);
    }

    /// A skipped catch-up copy is not an oversized file: `burn` writes the
    /// rest of the plan (#12).
    #[test]
    fn an_unplaceable_catch_up_copy_does_not_stop_the_burn() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], false);
        let mut report = staged::read_plan(&env.work).unwrap().unwrap();
        report.unplaceable_catch_up.push(ScannedFile {
            rel_path: RelPath("film.mkv".to_string()),
            content_hash: diskrement_core::ContentHash("00".repeat(32)),
            size_bytes: 1 << 40,
            mtime: now(),
        });
        staged::write_plan(&env.work, &report).unwrap();

        let burned = run_burn(&mut env).expect("the burn goes ahead");
        assert_eq!(burned.discs.len(), 1);
    }

    /// A disc marked dead after the plan was staged. The catalog cannot catch
    /// this — deadness is derived from `disc_event`, so the disc row still
    /// reads OPEN — and without the guard the burn would write a session onto
    /// a disc that no longer exists.
    #[test]
    fn a_disc_marked_dead_after_planning_makes_the_plan_stale() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], false);
        run_burn(&mut env).unwrap();

        // Re-stage a plan for the same (now burned) disc, then kill the disc.
        let scanned = vec![ScannedFile {
            rel_path: RelPath("a.bin".to_string()),
            content_hash: crate::fs::hasher::hash(&env.work.join("staging/disc-1/data/a.bin"))
                .unwrap(),
            size_bytes: 4,
            mtime: now(),
        }];
        std::fs::create_dir_all(env.work.join("snapshot")).unwrap();
        std::fs::write(env.work.join("snapshot/a.bin"), b"aaaa").unwrap();
        staged::write_scan(&env.work, &scanned).unwrap();
        let mut report = crate::pipeline::planning::plan_from(
            &diskrement_core::CatalogView::empty(),
            &scanned,
            Vec::new(),
            &[],
            DiscType::BdR,
            diskrement_core::Capacity::new(1_000_000),
            false,
            1,
            |_| 1,
        );
        report.sets[0].disc.ordinal = 1;
        report.sets[0].disc.is_new = false;
        staged::write_plan(&env.work, &report).unwrap();

        env.catalog
            .mark_dead(ProjectId(1), DiscKey::new(1, 0), &now())
            .expect("marking disc 1 dead");

        let err = run_burn(&mut env).unwrap_err();
        match &err {
            PipelineError::StalePlan(detail) => {
                assert!(detail.contains("disc 1"), "{detail}");
                assert!(detail.contains("marked dead"), "{detail}");
            }
            other => panic!("expected StalePlan, got {other}"),
        }
        // The advice is always "re-run plan": `reset` would delete the staging
        // the staged-disc verifier needs.
        assert!(err.to_string().contains("re-run plan"), "{err}");
        assert!(!err.to_string().contains("reset"), "{err}");
    }

    /// `scan --force` after `plan` re-stages the scan but leaves the plan: a
    /// planned file whose content changed in between must not reach the
    /// stager, where it used to fail as an exit-6 "content hash mismatch".
    #[test]
    fn a_planned_file_that_changed_in_the_staged_scan_makes_the_plan_stale() {
        let mut env = staged_env(&[("a.bin", b"aaaa"), ("b.bin", b"bbbb")], false);
        let mut scanned = staged::read_scan(&env.work).unwrap().unwrap();
        std::fs::write(env.work.join("snapshot/a.bin"), b"AAAA-changed").unwrap();
        scanned[0].content_hash =
            crate::fs::hasher::hash(&env.work.join("snapshot/a.bin")).unwrap();
        scanned[0].size_bytes = 12;
        staged::write_scan(&env.work, &scanned).unwrap();

        let err = run_burn(&mut env).unwrap_err();
        match &err {
            PipelineError::StalePlan(detail) => assert!(detail.contains("a.bin"), "{detail}"),
            other => panic!("expected StalePlan, got {other}"),
        }
        assert_eq!(err.exit_code(), 2);
        assert!(!err.to_string().contains("reset"), "{err}");
        assert!(
            env.catalog
                .sessions_by_state(ProjectId(1), SessionState::Burned)
                .unwrap()
                .is_empty(),
            "nothing committed"
        );
        assert!(
            !env.work.join("staging/disc-1").exists(),
            "refused before staging"
        );
    }

    /// A path the plan tombstones that is back in the staged scan: burning
    /// would record a deletion of a file that exists.
    #[test]
    fn a_tombstoned_path_present_in_the_staged_scan_makes_the_plan_stale() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], false);
        let mut report = staged::read_plan(&env.work).unwrap().unwrap();
        report.tombstones.push(RelPath("a.bin".to_string()));
        staged::write_plan(&env.work, &report).unwrap();

        let err = run_burn(&mut env).unwrap_err();
        match &err {
            PipelineError::StalePlan(detail) => assert!(detail.contains("a.bin"), "{detail}"),
            other => panic!("expected StalePlan, got {other}"),
        }
    }

    #[test]
    fn a_finalizing_plan_marks_the_disc_finalized() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], true);
        let report = run_burn(&mut env).unwrap();
        assert!(report.discs[0].finalized);

        let states = env.catalog.read_disc_states(ProjectId(1)).unwrap();
        assert_eq!(states.len(), 1);
        assert_eq!(states[0].status, diskrement_core::DiscStatus::Finalized);
    }

    /// Session ordinals count per disc, from the highest already committed.
    #[test]
    fn a_second_session_on_the_same_disc_gets_ordinal_two() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], false);
        run_burn(&mut env).unwrap();

        let scanned = vec![ScannedFile {
            rel_path: RelPath("b.bin".to_string()),
            content_hash: {
                std::fs::create_dir_all(env.work.join("snapshot")).unwrap();
                std::fs::write(env.work.join("snapshot/b.bin"), b"bbbb").unwrap();
                crate::fs::hasher::hash(&env.work.join("snapshot/b.bin")).unwrap()
            },
            size_bytes: 4,
            mtime: now(),
        }];
        staged::write_scan(&env.work, &scanned).unwrap();
        // The pipeline's own `plan`, so the staged plan records session 2.
        let sink = NoopSink;
        Pipeline::new(
            &mut env.catalog,
            PipelinePaths {
                work_dir: env.work.clone(),
                scan_cache: env.scan_cache.clone(),
            },
            &sink,
        )
        .plan("p", None, false)
        .unwrap();

        run_burn(&mut env).unwrap();
        let sessions = env
            .catalog
            .sessions_by_state(ProjectId(1), SessionState::Burned)
            .unwrap();
        let mut ordinals: Vec<u32> = sessions
            .iter()
            .filter(|s| s.disc.ordinal == 1)
            .map(|s| s.ordinal)
            .collect();
        ordinals.sort_unstable();
        assert_eq!(ordinals, vec![1, 2]);
    }

    /// The `source` field exists only so the project row is honest; nothing in
    /// `burn` reads it. Asserting it keeps the fixture from rotting silently.
    #[test]
    fn burn_reads_the_snapshot_not_the_source_root() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], false);
        std::fs::remove_dir_all(&env.source).unwrap();
        run_burn(&mut env).expect("burn works from the snapshot alone");
    }

    #[test]
    fn a_two_copy_burn_writes_twins_and_the_second_holds_copies() {
        let mut env = staged_env_with(&[("a.bin", b"aaaa"), ("b.bin", b"bbbb")], false, 2);
        let report = run_burn(&mut env).unwrap();
        assert_eq!(
            report
                .discs
                .iter()
                .map(|d| d.label.as_str())
                .collect::<Vec<_>>(),
            vec!["p disc 1A", "p disc 1B"]
        );
        assert!(env.work.join("staging/disc-1A/data/a.bin").is_file());
        assert!(env.work.join("staging/disc-1B/data/a.bin").is_file());
        let count = |t: &str| -> i64 {
            env.catalog
                .raw_connection_for_tests()
                .query_row(&format!("select count(*) from {t}"), [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(
            count("file_version"),
            2,
            "1B recorded copies, not new versions"
        );
        assert_eq!(count("placement"), 4);
        assert_eq!(
            report.coverage,
            Coverage {
                sets: vec![SetCoverage::default(); 2],
                no_live_copy: SetCoverage::default(),
            }
        );
        assert!(
            staged::read_plan(&env.work).unwrap().is_none(),
            "staged state cleared"
        );
    }

    /// A regular file where 1B's staging directory goes makes the stager fail
    /// on 1B only.
    fn block_disc(env: &Env, name: &str) -> std::path::PathBuf {
        let blocker = env.work.join(format!("staging/disc-{name}"));
        std::fs::create_dir_all(blocker.parent().unwrap()).unwrap();
        std::fs::write(&blocker, b"not a directory").unwrap();
        blocker
    }

    #[test]
    fn a_failure_on_the_second_set_keeps_the_first_and_resumes_at_the_second() {
        let mut env = staged_env_with(&[("a.bin", b"aaaa")], false, 2);
        let blocker = block_disc(&env, "1B");

        let err = run_burn(&mut env).unwrap_err();
        assert!(matches!(err, PipelineError::PartialBurn { .. }), "{err}");
        assert_eq!(err.exit_code(), 6);
        assert!(
            err.to_string()
                .ends_with("1A committed; 1B not written (re-run burn)"),
            "{err}"
        );

        let staged = staged::read_plan(&env.work)
            .unwrap()
            .expect("plan survives");
        assert_eq!(staged.sets.len(), 1);
        assert_eq!(staged.sets[0].disc.copy_set, 1);
        assert!(
            env.work.join("snapshot").exists(),
            "the snapshot survives too"
        );

        std::fs::remove_file(&blocker).unwrap();
        let report = run_burn(&mut env).unwrap();
        assert_eq!(
            report
                .discs
                .iter()
                .map(|d| d.label.as_str())
                .collect::<Vec<_>>(),
            vec!["p disc 1B"]
        );
        let sessions = env
            .catalog
            .sessions_by_state(ProjectId(1), SessionState::Burned)
            .unwrap();
        assert_eq!(
            sessions.iter().map(|s| s.disc).collect::<Vec<_>>(),
            vec![DiscKey::new(1, 0), DiscKey::new(1, 1)]
        );
    }

    #[test]
    fn a_disc_committed_before_the_plan_was_rewritten_is_skipped() {
        let mut env = staged_env_with(&[("a.bin", b"aaaa")], false, 2);
        let original = std::fs::read(staged::plan_path(&env.work)).unwrap();
        let blocker = block_disc(&env, "1B");
        run_burn(&mut env).unwrap_err();

        // Simulate the crash window: 1A is committed, but the plan on disk
        // still lists both sets.
        std::fs::write(staged::plan_path(&env.work), original).unwrap();
        std::fs::remove_file(&blocker).unwrap();

        let report = run_burn(&mut env).unwrap();
        assert_eq!(report.discs.len(), 1, "1A skipped, 1B written");
        assert_eq!(report.discs[0].disc, DiscKey::new(1, 1));
        let on_1a = env
            .catalog
            .sessions_by_state(ProjectId(1), SessionState::Burned)
            .unwrap()
            .into_iter()
            .filter(|s| s.disc == DiscKey::new(1, 0))
            .count();
        assert_eq!(on_1a, 1, "1A was not burned twice");
    }

    /// Stage a fresh cycle over `files`: the snapshot holds them, `scan.json`
    /// lists them, and the pipeline's own `plan` stages the plan.
    fn restage(env: &mut Env, files: &[(&str, &[u8])]) -> PlanReport {
        let snapshot = env.work.join("snapshot");
        let _ = std::fs::remove_dir_all(&snapshot);
        std::fs::create_dir_all(&snapshot).unwrap();
        let mut scanned = Vec::new();
        for (rel, body) in files {
            let abs = snapshot.join(rel);
            std::fs::write(&abs, body).unwrap();
            scanned.push(ScannedFile {
                rel_path: RelPath((*rel).to_string()),
                content_hash: crate::fs::hasher::hash(&abs).unwrap(),
                size_bytes: body.len() as u64,
                mtime: now(),
            });
        }
        staged::write_scan(&env.work, &scanned).unwrap();
        let sink = NoopSink;
        Pipeline::new(
            &mut env.catalog,
            PipelinePaths {
                work_dir: env.work.clone(),
                scan_cache: env.scan_cache.clone(),
            },
            &sink,
        )
        .plan("p", None, false)
        .unwrap()
    }

    /// `(copy set, session ordinal)` of every DELETED change record of `path`.
    fn deletions_of(env: &Env, path: &str) -> Vec<(u32, u32)> {
        let conn = env.catalog.raw_connection_for_tests();
        let mut stmt = conn
            .prepare(
                "select d.copy_set, s.ordinal from change_record cr
                   join file_version fv on fv.id = cr.file_version_id
                   join session s on s.id = cr.session_id
                   join disc d on d.id = s.disc_id
                  where cr.kind = 'DELETED' and fv.rel_path = ?1
                  order by cr.id",
            )
            .unwrap();
        stmt.query_map([path], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    fn manifest_deleted(env: &Env, disc: &str) -> Vec<String> {
        let raw = std::fs::read_to_string(
            env.work
                .join(format!("staging/disc-{disc}/.diskrement/manifest.json")),
        )
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        v["deleted"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p.as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn a_two_copy_deletion_is_recorded_once_on_set_a_and_listed_by_both_discs() {
        let mut env = staged_env_with(&[("a.bin", b"aaaa"), ("gone.bin", b"gggg")], false, 2);
        run_burn(&mut env).unwrap();

        let plan = restage(&mut env, &[("a.bin", b"aaaa"), ("b.bin", b"bbbb")]);
        assert_eq!(plan.sets.len(), 2, "both sets take b.bin");
        assert_eq!(plan.tombstones, vec![RelPath("gone.bin".into())]);
        run_burn(&mut env).unwrap();

        assert_eq!(
            deletions_of(&env, "gone.bin"),
            vec![(0, 2)],
            "one DELETED record, on 1A's second session"
        );
        assert_eq!(manifest_deleted(&env, "1A"), vec!["gone.bin"]);
        assert_eq!(manifest_deleted(&env, "1B"), vec!["gone.bin"]);
    }

    /// Everything `burn` could change in place: the status of every disc and
    /// the state of every session.
    fn status_rows(env: &Env) -> Vec<String> {
        let conn = env.catalog.raw_connection_for_tests();
        let mut out = Vec::new();
        for q in [
            "select id, status from disc order by id",
            "select id, state from session order by id",
        ] {
            let mut stmt = conn.prepare(q).unwrap();
            let rows = stmt
                .query_map([], |r| {
                    Ok(format!(
                        "{}={}",
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?
                    ))
                })
                .unwrap();
            out.extend(rows.map(Result::unwrap));
        }
        out
    }

    fn row_counts(env: &Env) -> Vec<(String, i64)> {
        let conn = env.catalog.raw_connection_for_tests();
        [
            "disc",
            "session",
            "file_version",
            "placement",
            "change_record",
            "disc_event",
            "lost_version",
        ]
        .iter()
        .map(|t| {
            let n: i64 = conn
                .query_row(&format!("select count(*) from {t}"), [], |r| r.get(0))
                .unwrap();
            (t.to_string(), n)
        })
        .collect()
    }

    /// (a) Issue #9: a deletion-only cycle at copies=2 writes no disc, in
    /// either set, and touches no catalog table; it still ends the cycle.
    /// Both twins are sealed first, so the old set-A fallback would have
    /// opened disc 2A: the bug itself.
    #[test]
    fn a_deletion_only_cycle_burns_nothing_and_ends_the_cycle() {
        let mut env = staged_env_with(&[("a.bin", b"aaaa"), ("gone.bin", b"gggg")], true, 2);
        run_burn(&mut env).unwrap();
        let before = (row_counts(&env), status_rows(&env));

        let plan = restage(&mut env, &[("a.bin", b"aaaa")]);
        assert!(plan.sets.is_empty());
        assert_eq!(plan.tombstones, vec![RelPath("gone.bin".into())]);
        let report = run_burn(&mut env).unwrap();

        assert!(report.discs.is_empty());
        assert_eq!(report.deferred_deletions, Some(1));
        assert_eq!(
            (row_counts(&env), status_rows(&env)),
            before,
            "nothing reached the catalog"
        );
        assert!(deletions_of(&env, "gone.bin").is_empty());
        assert!(!env.work.join("staging/disc-2A").exists());
        assert!(!env.work.join("staging/disc-2B").exists());
        assert!(!env.work.join("snapshot").exists(), "snapshot released");
        assert!(staged::read_plan(&env.work).unwrap().is_none());
        assert!(staged::read_scan(&env.work).unwrap().is_none());
    }

    /// (b) The deferred deletion rides the next disc that is burned anyway:
    /// recorded once, on 1A, listed by both twins' manifests.
    #[test]
    fn a_deferred_deletion_is_recorded_by_the_next_burned_disc() {
        let mut env = staged_env_with(&[("a.bin", b"aaaa"), ("gone.bin", b"gggg")], false, 2);
        run_burn(&mut env).unwrap();
        restage(&mut env, &[("a.bin", b"aaaa")]);
        run_burn(&mut env).unwrap();

        let plan = restage(&mut env, &[("a.bin", b"aaaa"), ("b.bin", b"bbbb")]);
        assert_eq!(plan.sets.len(), 2);
        assert_eq!(plan.tombstones, vec![RelPath("gone.bin".into())]);
        let report = run_burn(&mut env).unwrap();

        assert_eq!(report.deferred_deletions, None);
        assert_eq!(deletions_of(&env, "gone.bin"), vec![(0, 2)]);
        assert_eq!(manifest_deleted(&env, "1A"), vec!["gone.bin"]);
        assert_eq!(manifest_deleted(&env, "1B"), vec!["gone.bin"]);
    }

    /// (c) A deletion deferred while its only disc dies drops out of the live
    /// view, but is still a tombstone, so the scan counts it and the next
    /// burned disc records it.
    #[test]
    fn a_deletion_deferred_past_its_discs_death_is_still_recorded() {
        let mut env = staged_env(&[("a.bin", b"aaaa"), ("gone.bin", b"gggg")], false);
        run_burn(&mut env).unwrap();
        restage(&mut env, &[("a.bin", b"aaaa")]);
        run_burn(&mut env).unwrap();
        env.catalog
            .mark_dead(ProjectId(1), DiscKey::new(1, 0), &now())
            .unwrap();

        let plan = restage(&mut env, &[("a.bin", b"aaaa"), ("b.bin", b"bbbb")]);
        assert_eq!(plan.tombstones, vec![RelPath("gone.bin".into())]);
        let sink = NoopSink;
        let scan = Pipeline::new(
            &mut env.catalog,
            PipelinePaths {
                work_dir: env.work.clone(),
                scan_cache: env.scan_cache.clone(),
            },
            &sink,
        )
        .scan("p", false, false)
        .unwrap();
        assert_eq!(scan.deleted, 1, "the scan agrees with the plan");

        run_burn(&mut env).unwrap();
        assert_eq!(deletions_of(&env, "gone.bin"), vec![(0, 1)], "on disc 2");
    }

    /// A plan staged by the build before #9 can hold its set-A fallback: a
    /// planned disc with nothing to write (two shapes, this is the open one). Burning it must be refused with
    /// "re-run plan" advice, writing nothing.
    #[test]
    fn a_pre_upgrade_plan_with_an_empty_fallback_disc_is_stale() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], false);
        let mut plan = staged::read_plan(&env.work).unwrap().unwrap();
        plan.sets[0].disc.files.clear();
        staged::write_plan(&env.work, &plan).unwrap();
        let before = (row_counts(&env), status_rows(&env));

        let err = run_burn(&mut env).unwrap_err();

        match &err {
            PipelineError::StalePlan(detail) => {
                assert!(detail.contains("nothing to write"), "{detail}");
                assert!(!detail.contains("reset"), "{detail}");
            }
            other => panic!("expected StalePlan, got {other}"),
        }
        assert_eq!(err.exit_code(), 2);
        assert_eq!((row_counts(&env), status_rows(&env)), before);
    }

    /// The other fallback shape: with `--finalize` and no open disc, the old
    /// planner fell back to a fresh set-A disc sealed empty.
    #[test]
    fn a_pre_upgrade_plan_with_an_empty_fresh_seal_disc_is_stale() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], true);
        let mut plan = staged::read_plan(&env.work).unwrap().unwrap();
        let disc = &mut plan.sets[0].disc;
        disc.files.clear();
        assert!(disc.is_new && disc.will_finalize);
        staged::write_plan(&env.work, &plan).unwrap();
        let before = (row_counts(&env), status_rows(&env));

        let err = run_burn(&mut env).unwrap_err();

        match &err {
            PipelineError::StalePlan(detail) => {
                assert!(detail.contains("nothing to write"), "{detail}");
            }
            other => panic!("expected StalePlan, got {other}"),
        }
        assert_eq!((row_counts(&env), status_rows(&env)), before);
    }

    /// Burning a zero-set plan with the real backend never touches the
    /// drive: no xorriso command, no insert prompt.
    #[test]
    fn a_real_burn_of_a_plan_with_no_disc_runs_no_command_and_asks_nothing() {
        let mut env = staged_env_with(&[("a.bin", b"aaaa"), ("gone.bin", b"gggg")], true, 2);
        run_burn(&mut env).unwrap();
        restage(&mut env, &[("a.bin", b"aaaa")]);
        let script = ScriptedExec::new();
        let op = ScriptedOperator::new(&[]);

        let report = run_real_burn(&mut env, &script, &op).unwrap();

        assert_eq!(report.deferred_deletions, Some(1));
        assert!(report.discs.is_empty());
        assert!(!script.ran(""), "no command was executed");
        assert!(op.log().is_empty(), "{:?}", op.log());
    }

    /// A tombstoned path whose current version lost its last live copy
    /// between `plan` and `burn` is still current in the catalog: the
    /// deletion must be recorded, as the disc's manifest says it is.
    #[test]
    fn a_tombstone_whose_version_lost_its_last_copy_is_still_recorded() {
        let mut env = staged_env(&[("a.bin", b"aaaa"), ("gone.bin", b"gggg")], true);
        run_burn(&mut env).unwrap();

        let plan = restage(&mut env, &[("a.bin", b"aaaa"), ("b.bin", b"bbbb")]);
        assert_eq!(plan.sets[0].disc.key(), DiscKey::new(2, 0));
        assert_eq!(plan.tombstones, vec![RelPath("gone.bin".into())]);
        env.catalog
            .mark_dead(ProjectId(1), DiscKey::new(1, 0), &now())
            .unwrap();

        run_burn(&mut env).unwrap();
        assert_eq!(deletions_of(&env, "gone.bin"), vec![(0, 1)]);
        assert_eq!(manifest_deleted(&env, "2"), vec!["gone.bin"]);
    }

    fn partial_parts(err: &PipelineError) -> (Vec<String>, Option<UnrecordedDisc>, Vec<String>) {
        match err {
            PipelineError::PartialBurn {
                committed,
                unrecorded,
                pending,
                ..
            } => (committed.clone(), unrecorded.clone(), pending.clone()),
            other => panic!("expected PartialBurn, got {other}"),
        }
    }

    #[test]
    fn a_plan_rewrite_failure_after_a_commit_is_a_partial_burn_that_resumes() {
        let mut env = staged_env_with(&[("a.bin", b"aaaa")], false, 2);
        // A directory where the rewrite's temp file goes makes it fail.
        let tmp = env.work.join("staged/plan.json.tmp");
        std::fs::create_dir_all(&tmp).unwrap();

        let err = run_burn(&mut env).unwrap_err();
        assert_eq!(
            partial_parts(&err),
            (vec!["1A".to_string()], None, vec!["1B".to_string()])
        );
        assert_eq!(err.exit_code(), 5);
        assert!(err.to_string().contains("(re-run burn)"), "{err}");

        std::fs::remove_dir(&tmp).unwrap();
        let report = run_burn(&mut env).unwrap();
        assert_eq!(
            report.discs.iter().map(|d| d.disc).collect::<Vec<_>>(),
            vec![DiscKey::new(1, 1)],
            "1A is not burned twice"
        );
    }

    #[test]
    fn a_cleanup_failure_after_every_disc_committed_names_them_all() {
        let mut env = staged_env_with(&[("a.bin", b"aaaa")], false, 2);
        // Something foreign in `staged/` makes `clear` refuse.
        std::fs::write(env.work.join("staged/foreign"), b"x").unwrap();

        let err = run_burn(&mut env).unwrap_err();
        assert_eq!(
            partial_parts(&err),
            (vec!["1A".to_string(), "1B".to_string()], None, vec![])
        );
        assert!(!err.to_string().contains("not written"), "{err}");
    }

    #[test]
    fn a_commit_failure_after_the_disc_was_written_says_so() {
        let mut env = staged_env_with(&[("a.bin", b"aaaa")], false, 2);
        // A temp trigger lives on the live connection only, so the embedded
        // snapshot copy still builds and the burner writes 1B; the live
        // commit of 1B then fails.
        env.catalog
            .raw_connection_for_tests()
            .execute_batch(
                "create temp trigger refuse_1b before insert on disc
                   when new.copy_set = 1
                 begin select raise(abort, 'refused for the test'); end;",
            )
            .unwrap();

        let err = run_burn(&mut env).unwrap_err();
        assert_eq!(
            partial_parts(&err),
            (
                vec!["1A".to_string()],
                Some(UnrecordedDisc::written("1B")),
                vec![]
            )
        );
        assert!(env.work.join("staging/disc-1B/data/a.bin").is_file());
        let text = err.to_string();
        assert!(text.contains("1B written but not recorded"), "{text}");
        assert!(!text.contains("not written"), "{text}");
    }

    #[test]
    fn a_plan_staged_by_an_older_build_is_stale_not_corrupt() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], false);
        std::fs::write(
            staged::plan_path(&env.work),
            r#"{"disc":{"ordinal":1,"isNew":true,"willFinalize":false,"files":[],
                "priorUsedBytes":0,"usableBytes":100},"discType":"BD_R","tombstones":[],
                "remaining":{"moreDiscs":0,"moreFiles":0,"moreBytes":0},"oversized":[]}"#,
        )
        .unwrap();
        let err = run_burn(&mut env).unwrap_err();
        assert!(matches!(err, PipelineError::StalePlan(_)), "{err}");
        assert!(!err.to_string().contains("reset"), "{err}");
    }

    /// The real burner is the only one that can fail after touching media, so
    /// the classification is pinned on the error variants directly.
    #[test]
    fn a_burner_failure_after_xorriso_ran_may_have_written_the_disc() {
        use crate::burn::BurnError as B;
        let maybe = |e: B| {
            matches!(
                burner_failure(e),
                DiscFailure::Unrecorded {
                    surely_written: false,
                    ..
                }
            )
        };
        assert!(maybe(B::BurnFailed {
            exit_code: 5,
            output_tail: vec![],
        }));
        assert!(maybe(B::PostBurnCheckFailed("unreadable".into())));
        assert!(maybe(B::WriteInterrupted(std::io::Error::other("pipe"))));

        let unwritten = |e: B| matches!(burner_failure(e), DiscFailure::Unwritten(_));
        assert!(unwritten(B::Aborted));
        assert!(unwritten(B::InspectFailed {
            exit_code: 5,
            output_tail: vec![],
        }));
        assert!(unwritten(B::ToolMissing("x".into())));
        assert!(unwritten(B::ToolTooOld("1.4".into())));
        assert!(unwritten(B::DeviceNotFound("/dev/sr9".into())));
        assert!(unwritten(B::DevicePermissionDenied("/dev/sr0".into())));
        assert!(unwritten(B::NoMedia("/dev/sr0".into())));
        assert!(unwritten(B::InsufficientCapacity {
            needed_bytes: 2,
            free_bytes: 1,
        }));
        assert!(unwritten(B::MountFailed("m".into())));
        assert!(unwritten(B::SourceFileMissing(RelPath("a".into()))));
        assert!(unwritten(B::Io(std::io::Error::other("staging"))));
        assert!(unwritten(B::HashMismatch {
            rel_path: RelPath("a".into()),
            expected: diskrement_core::ContentHash("x".into()),
            actual: diskrement_core::ContentHash("y".into()),
        }));
    }
}

#[cfg(test)]
mod verify_tests {
    use super::*;
    use chrono::{TimeZone as _, Utc};
    use diskrement_core::{DiscType, RelPath, ScannedFile};

    use crate::burn::simulated::SimulatedBurner;
    use crate::burn::verifier::{StagingVerifier, Verifier};
    use crate::burn::Burner;
    use crate::catalog::model::ProjectInit;
    use crate::progress::NoopSink;

    struct Env {
        _dir: tempfile::TempDir,
        catalog: Catalog,
        work: PathBuf,
        scan_cache: PathBuf,
    }

    fn now() -> chrono::DateTime<Utc> {
        Utc.timestamp_opt(1_757_100_066, 623_959_846).unwrap()
    }

    /// A project with `files` staged and burned onto disc 1 — the state
    /// `verify` expects to find.
    fn burned_env(files: &[(&str, &[u8])], finalize: bool) -> Env {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let work = root.join("work");
        let source = root.join("source");
        let snapshot = work.join("snapshot");
        std::fs::create_dir_all(&snapshot).unwrap();
        std::fs::create_dir_all(&source).unwrap();

        let mut scanned: Vec<ScannedFile> = Vec::new();
        for (rel, body) in files {
            let abs = snapshot.join(rel);
            std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
            std::fs::write(&abs, body).unwrap();
            scanned.push(ScannedFile {
                rel_path: RelPath((*rel).to_string()),
                content_hash: crate::fs::hasher::hash(&abs).unwrap(),
                size_bytes: body.len() as u64,
                mtime: Utc.timestamp_opt(1_757_000_000, 0).unwrap(),
            });
        }

        let mut catalog = Catalog::open(&root.join("catalog.sqlite")).unwrap();
        catalog
            .create_project(&ProjectInit {
                name: "p".to_string(),
                source_root: source.display().to_string(),
                work_dir: work.display().to_string(),
                default_disc_type: DiscType::BdR,
                margin_percent: 2.0,
                copies: 1,
                created_at: now(),
            })
            .unwrap();

        staged::write_scan(&work, &scanned).unwrap();
        let report = crate::pipeline::planning::plan_from(
            &diskrement_core::CatalogView::empty(),
            &scanned,
            Vec::new(),
            &[],
            DiscType::BdR,
            diskrement_core::Capacity::new(1_000_000),
            finalize,
            1,
            |_| 1,
        );
        staged::write_plan(&work, &report).unwrap();

        let mut env = Env {
            _dir: dir,
            catalog,
            work: work.clone(),
            scan_cache: root.join("scancache.sqlite"),
        };
        {
            let sink = NoopSink;
            let burner = Burner::Simulated(SimulatedBurner::new(&sink, 2));
            let mut pipeline = Pipeline::new(
                &mut env.catalog,
                PipelinePaths {
                    work_dir: env.work.clone(),
                    scan_cache: env.scan_cache.clone(),
                },
                &sink,
            );
            pipeline.burn("p", now(), &burner).unwrap();
        }
        env
    }

    fn run_verify(env: &mut Env, disc: Option<i64>) -> Result<Vec<VerifyReport>, PipelineError> {
        let verifier = Verifier::Staging(StagingVerifier::new(env.work.join("staging"), &NoopSink));
        let mut pipeline = Pipeline::new(
            &mut env.catalog,
            PipelinePaths {
                work_dir: env.work.clone(),
                scan_cache: env.scan_cache.clone(),
            },
            &NoopSink,
        );
        pipeline.verify(
            "p",
            disc.map(|o| DiscArg {
                ordinal: o,
                copy_set: None,
            }),
            &verifier,
        )
    }

    fn verified_discs(env: &Env) -> Vec<u32> {
        env.catalog
            .sessions_by_state(ProjectId(1), SessionState::Verified)
            .unwrap()
            .iter()
            .map(|s| s.disc.ordinal)
            .collect()
    }

    #[test]
    fn a_clean_disc_reports_no_mismatches_and_becomes_verified() {
        let mut env = burned_env(&[("a.txt", b"aaa")], false);
        let reports = run_verify(&mut env, None).unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].disc, DiscKey::new(1, 0));
        assert_eq!(reports[0].files_checked, 1);
        assert!(reports[0].mismatches.is_empty());
        assert!(!reports[0].skipped);
        assert_eq!(verified_discs(&env), vec![1]);
    }

    /// A second sweep finds no BURNED session for the disc and must be a
    /// no-op, not an illegal `BURNED -> VERIFIED` transition.
    #[test]
    fn verifying_an_already_verified_disc_again_is_a_clean_no_op() {
        let mut env = burned_env(&[("a.txt", b"aaa")], false);
        run_verify(&mut env, None).unwrap();
        let reports = run_verify(&mut env, None).unwrap();
        assert!(reports[0].mismatches.is_empty());
        assert_eq!(verified_discs(&env), vec![1]);
    }

    #[test]
    fn a_corrupted_staged_file_is_reported_as_a_mismatch_and_blocks_verified() {
        let mut env = burned_env(&[("a.txt", b"aaa")], false);
        std::fs::write(env.work.join("staging/disc-1/data/a.txt"), b"CORRUPT").unwrap();
        let reports = run_verify(&mut env, None).unwrap();
        assert_eq!(reports[0].mismatches, vec![RelPath("a.txt".to_string())]);
        assert_eq!(reports[0].files_checked, 1);
        assert!(
            verified_discs(&env).is_empty(),
            "a mismatch must not verify"
        );
    }

    /// `reset` removes the staging trees; the catalog session stays BURNED.
    /// A skipped disc is never marked verified.
    #[test]
    fn a_disc_with_no_staging_dir_is_skipped_and_stays_unverified() {
        let mut env = burned_env(&[("a.txt", b"aaa")], false);
        std::fs::remove_dir_all(env.work.join("staging")).unwrap();
        let reports = run_verify(&mut env, None).unwrap();
        assert_eq!(reports.len(), 1);
        assert!(reports[0].skipped);
        assert_eq!(reports[0].files_checked, 0);
        assert!(verified_discs(&env).is_empty());
    }

    #[test]
    fn naming_the_live_disc_checks_only_that_disc() {
        let mut env = burned_env(&[("a.txt", b"aaa")], false);
        let reports = run_verify(&mut env, Some(1)).unwrap();
        assert_eq!(
            reports.iter().map(|r| r.disc.ordinal).collect::<Vec<_>>(),
            vec![1]
        );
    }

    /// Silently answering "nothing to verify" for a disc the caller named
    /// would read as reassurance. Both the missing and the negative ordinal
    /// are lookup failures.
    #[test]
    fn naming_a_disc_that_does_not_exist_is_an_error() {
        let mut env = burned_env(&[("a.txt", b"aaa")], false);
        for ordinal in [99, -1] {
            let err = run_verify(&mut env, Some(ordinal)).unwrap_err();
            assert!(matches!(err, PipelineError::UnknownDisc(_)), "{err}");
            assert_eq!(err.exit_code(), 2);
            assert_eq!(err.to_string(), format!("no disc with ordinal {ordinal}"));
        }
    }

    #[test]
    fn verify_on_an_unknown_project_is_project_not_found() {
        let mut env = burned_env(&[("a.txt", b"aaa")], false);
        let verifier = Verifier::Staging(StagingVerifier::new(env.work.join("staging"), &NoopSink));
        let mut pipeline = Pipeline::new(
            &mut env.catalog,
            PipelinePaths {
                work_dir: env.work.clone(),
                scan_cache: env.scan_cache.clone(),
            },
            &NoopSink,
        );
        let err = pipeline.verify("nosuch", None, &verifier).unwrap_err();
        assert!(matches!(err, PipelineError::ProjectNotFound(_)), "{err}");
    }

    /// A seal session places no files, so its disc verifies clean with zero
    /// files checked — the `disc 4: 0 files OK` line in run-manifest step 21.
    #[test]
    fn a_sealed_disc_with_no_files_verifies_clean_with_zero_checked() {
        // A fresh project with nothing to place plans no disc (issue #9), so the
        // seal session is produced the way an operator does: burn a disc, then
        // `plan --finalize` an unchanged tree and burn the file-less seal.
        let mut env = burned_env(&[("a.txt", b"aaa")], false);
        // The burn removed the snapshot and the staged scan; put them back.
        std::fs::create_dir_all(env.work.join("snapshot")).unwrap();
        std::fs::write(env.work.join("snapshot/a.txt"), b"aaa").unwrap();
        let again = vec![ScannedFile {
            rel_path: RelPath("a.txt".to_string()),
            content_hash: crate::fs::hasher::hash(&env.work.join("snapshot/a.txt")).unwrap(),
            size_bytes: 3,
            mtime: Utc.timestamp_opt(1_757_000_000, 0).unwrap(),
        }];
        staged::write_scan(&env.work, &again).unwrap();
        {
            let sink = NoopSink;
            let burner = Burner::Simulated(SimulatedBurner::new(&sink, 2));
            let mut pipeline = Pipeline::new(
                &mut env.catalog,
                PipelinePaths {
                    work_dir: env.work.clone(),
                    scan_cache: env.scan_cache.clone(),
                },
                &sink,
            );
            let plan = pipeline.plan("p", None, true).unwrap();
            assert!(plan.sets[0].disc.files.is_empty());
            pipeline.burn("p", now(), &burner).unwrap();
        }
        let reports = run_verify(&mut env, None).unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].files_checked, 0);
        assert!(!reports[0].skipped);
        assert!(reports[0].mismatches.is_empty());
        assert_eq!(verified_discs(&env), vec![1]);
    }

    fn run_reset(env: &mut Env, name: &str) -> Result<(), PipelineError> {
        let pipeline = Pipeline::new(
            &mut env.catalog,
            PipelinePaths {
                work_dir: env.work.clone(),
                scan_cache: env.scan_cache.clone(),
            },
            &NoopSink,
        );
        pipeline.reset(name)
    }

    #[test]
    fn reset_clears_the_staged_state_the_staging_trees_and_the_snapshot() {
        let mut env = burned_env(&[("a.txt", b"aaa")], false);
        // A burn already consumed the staged plan and released the snapshot,
        // so put a cycle-in-progress back on disk for reset to find.
        std::fs::create_dir_all(env.work.join("snapshot")).unwrap();
        std::fs::write(env.work.join("snapshot/a.txt"), b"aaa").unwrap();
        std::fs::write(env.work.join("burn-catalog.sqlite"), b"stale").unwrap();
        staged::write_scan(&env.work, &[]).unwrap();
        assert!(env.work.join("staging/disc-1").is_dir());

        run_reset(&mut env, "p").unwrap();

        assert!(staged::read_scan(&env.work).unwrap().is_none());
        assert!(staged::read_plan(&env.work).unwrap().is_none());
        assert!(!env.work.join("staged").exists());
        assert!(!env.work.join("snapshot").exists());
        assert!(!env.work.join("burn-catalog.sqlite").exists());
        assert!(!env.work.join("staging").exists(), "staging is destroyed");
    }

    /// Reset is idempotent: nothing to delete is not an error.
    #[test]
    fn resetting_an_already_clean_work_dir_succeeds() {
        let mut env = burned_env(&[("a.txt", b"aaa")], false);
        run_reset(&mut env, "p").unwrap();
        run_reset(&mut env, "p").unwrap();
    }

    /// Reset discards a cycle, not history: the catalog is untouched.
    #[test]
    fn reset_leaves_the_catalog_alone() {
        let mut env = burned_env(&[("a.txt", b"aaa")], false);
        run_reset(&mut env, "p").unwrap();
        let sessions = env
            .catalog
            .sessions_by_state(ProjectId(1), SessionState::Burned)
            .unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].disc.ordinal, 1);
    }

    #[test]
    fn reset_on_an_unknown_project_is_project_not_found() {
        let mut env = burned_env(&[("a.txt", b"aaa")], false);
        let err = run_reset(&mut env, "nosuch").unwrap_err();
        assert!(matches!(err, PipelineError::ProjectNotFound(_)), "{err}");
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn history_reports_the_burned_version_of_a_path() {
        let mut env = burned_env(&[("a.txt", b"aaa")], false);
        let pipeline = Pipeline::new(
            &mut env.catalog,
            PipelinePaths {
                work_dir: env.work.clone(),
                scan_cache: env.scan_cache.clone(),
            },
            &NoopSink,
        );
        let entries = pipeline
            .history("p", &RelPath("a.txt".to_string()))
            .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].disc.ordinal, 1);
        assert_eq!(entries[0].session_ordinal, 1);
        assert_eq!(entries[0].size_bytes, 3);
        assert_eq!(
            entries[0].status,
            crate::catalog::model::VersionStatus::Current
        );
    }

    #[test]
    fn history_of_an_unknown_path_is_empty_not_an_error() {
        let mut env = burned_env(&[("a.txt", b"aaa")], false);
        let pipeline = Pipeline::new(
            &mut env.catalog,
            PipelinePaths {
                work_dir: env.work.clone(),
                scan_cache: env.scan_cache.clone(),
            },
            &NoopSink,
        );
        assert!(pipeline
            .history("p", &RelPath("nope.txt".to_string()))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn history_on_an_unknown_project_is_project_not_found() {
        let mut env = burned_env(&[("a.txt", b"aaa")], false);
        let pipeline = Pipeline::new(
            &mut env.catalog,
            PipelinePaths {
                work_dir: env.work.clone(),
                scan_cache: env.scan_cache.clone(),
            },
            &NoopSink,
        );
        let err = pipeline
            .history("nosuch", &RelPath("a.txt".to_string()))
            .unwrap_err();
        assert!(matches!(err, PipelineError::ProjectNotFound(_)), "{err}");
    }
}

#[cfg(test)]
mod mark_dead_tests {
    use super::*;
    use chrono::{TimeZone as _, Utc};
    use diskrement_core::{DiscType, RelPath, ScannedFile};

    use crate::burn::simulated::SimulatedBurner;
    use crate::burn::verifier::{StagingVerifier, Verifier};
    use crate::burn::Burner;
    use crate::catalog::model::{ProjectInit, VersionStatus};
    use crate::progress::NoopSink;

    struct Env {
        _dir: tempfile::TempDir,
        catalog: Catalog,
        work: PathBuf,
        scan_cache: PathBuf,
    }

    /// Distinct, deterministic instants: `burn` and `mark_dead` both take
    /// their time from the caller, so nothing here reads a clock.
    fn at(offset: i64) -> chrono::DateTime<Utc> {
        Utc.timestamp_opt(1_757_100_000 + offset, 0).unwrap()
    }

    /// A project with an empty catalog and nothing staged.
    ///
    /// `create_project` rather than `Pipeline::init`, and a hand-built
    /// snapshot rather than `Pipeline::scan`: `init` probes the work dir for
    /// reflink support and `scan` clones the source tree, neither of which
    /// mark-dead.s behaviour depends on. Every cycle below drives the real
    /// `plan` and `burn`, which is where mark-dead's consequences show up.
    fn env() -> Env {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let work = root.join("work");
        let source = root.join("source");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&work).unwrap();

        let mut catalog = Catalog::open(&root.join("catalog.sqlite")).unwrap();
        catalog
            .create_project(&ProjectInit {
                name: "p".to_string(),
                source_root: source.display().to_string(),
                work_dir: work.display().to_string(),
                default_disc_type: DiscType::BdR,
                margin_percent: 2.0,
                copies: 1,
                created_at: at(0),
            })
            .unwrap();

        Env {
            _dir: dir,
            catalog,
            work,
            scan_cache: root.join("scancache.sqlite"),
        }
    }

    /// Rebuild `work/snapshot` from `files` and stage them as the scan.
    ///
    /// A successful `burn` releases the snapshot and clears the staged state,
    /// so every cycle re-stages from scratch — exactly as a real `scan` would.
    fn stage(env: &Env, files: &[(&str, &[u8])]) {
        let snapshot = env.work.join("snapshot");
        let _ = std::fs::remove_dir_all(&snapshot);
        std::fs::create_dir_all(&snapshot).unwrap();

        let mut scanned: Vec<ScannedFile> = Vec::new();
        for (rel, body) in files {
            let abs = snapshot.join(rel);
            std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
            std::fs::write(&abs, body).unwrap();
            scanned.push(ScannedFile {
                rel_path: RelPath((*rel).to_string()),
                content_hash: crate::fs::hasher::hash(&abs).unwrap(),
                size_bytes: body.len() as u64,
                mtime: at(0),
            });
        }
        staged::write_scan(&env.work, &scanned).unwrap();
    }

    /// One full cycle: stage `files`, plan them, burn the planned disc.
    fn cycle(env: &mut Env, files: &[(&str, &[u8])], burned_at: chrono::DateTime<Utc>) {
        stage(env, files);
        let work = env.work.clone();
        let scan_cache = env.scan_cache.clone();
        let sink = NoopSink;
        let burner = Burner::Simulated(SimulatedBurner::new(&sink, 2));
        let mut pipeline = Pipeline::new(
            &mut env.catalog,
            PipelinePaths {
                work_dir: work,
                scan_cache,
            },
            &sink,
        );
        pipeline.plan("p", None, false).unwrap();
        pipeline.burn("p", burned_at, &burner).unwrap();
    }

    fn run_mark_dead(env: &mut Env, disc: &str) -> Result<LostReport, PipelineError> {
        let work = env.work.clone();
        let scan_cache = env.scan_cache.clone();
        let mut pipeline = Pipeline::new(
            &mut env.catalog,
            PipelinePaths {
                work_dir: work,
                scan_cache,
            },
            &NoopSink,
        );
        pipeline.mark_dead("p", disc.parse().unwrap(), at(500))
    }

    fn run_plan(env: &mut Env) -> Result<PlanReport, PipelineError> {
        let work = env.work.clone();
        let scan_cache = env.scan_cache.clone();
        let pipeline = Pipeline::new(
            &mut env.catalog,
            PipelinePaths {
                work_dir: work,
                scan_cache,
            },
            &NoopSink,
        );
        pipeline.plan("p", None, false)
    }

    fn run_verify(env: &mut Env, disc: Option<&str>) -> Result<Vec<VerifyReport>, PipelineError> {
        let work = env.work.clone();
        let scan_cache = env.scan_cache.clone();
        let verifier = Verifier::Staging(StagingVerifier::new(work.join("staging"), &NoopSink));
        let mut pipeline = Pipeline::new(
            &mut env.catalog,
            PipelinePaths {
                work_dir: work,
                scan_cache,
            },
            &NoopSink,
        );
        pipeline.verify("p", disc.map(|d| d.parse().unwrap()), &verifier)
    }

    /// The stored `disc.status` column for one ordinal, read straight out of
    /// SQLite. `read_disc_states` derives `DEAD` and would happily mask an
    /// illegal update-in-place, so the invariant tests read the row itself.
    fn stored_status(env: &Env, ordinal: u32) -> String {
        env.catalog
            .raw_connection_for_tests()
            .query_row(
                "select status from disc where project_id = 1 and ordinal = ?1",
                rusqlite::params![ordinal as i64],
                |row| row.get::<_, String>(0),
            )
            .expect("the disc row exists")
    }

    #[test]
    fn an_ordinal_no_disc_has_is_an_unknown_disc_error() {
        let mut e = env();
        cycle(&mut e, &[("a.txt", b"aaa")], at(10));
        let err = run_mark_dead(&mut e, "9").unwrap_err();
        assert!(
            matches!(err, PipelineError::UnknownDisc(ref s) if s == "9"),
            "{err:?}"
        );
        assert_eq!(err.to_string(), "no disc with ordinal 9");
        assert_eq!(err.exit_code(), 2);
    }

    /// The second mark-dead must be refused by the *pipeline's* pre-check, so
    /// it exits 2. Reaching the catalog would raise IllegalStatusTransition
    /// and exit 5.
    #[test]
    fn marking_the_same_disc_twice_is_exit_2_and_appends_no_second_event() {
        let mut e = env();
        cycle(&mut e, &[("a.txt", b"aaa")], at(10));
        run_mark_dead(&mut e, "1").expect("the first mark-dead succeeds");

        let err = run_mark_dead(&mut e, "1").unwrap_err();
        assert!(
            matches!(err, PipelineError::DiscAlreadyDead(ref s) if s == "1"),
            "{err:?}"
        );
        assert_eq!(err.exit_code(), 2);

        let events: i64 = e
            .catalog
            .raw_connection_for_tests()
            .query_row(
                "select count(*) from disc_event where kind = 'MARKED_DEAD'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(events, 1, "the refused call must append nothing");
    }

    #[test]
    fn mark_dead_on_an_unknown_project_is_a_lookup_failure() {
        let mut e = env();
        cycle(&mut e, &[("a.txt", b"aaa")], at(10));
        let work = e.work.clone();
        let scan_cache = e.scan_cache.clone();
        let mut pipeline = Pipeline::new(
            &mut e.catalog,
            PipelinePaths {
                work_dir: work,
                scan_cache,
            },
            &NoopSink,
        );
        let err = pipeline
            .mark_dead("nosuch", "1".parse().unwrap(), at(500))
            .unwrap_err();
        assert!(matches!(err, PipelineError::ProjectNotFound(_)), "{err:?}");
    }

    /// The invariant CLAUDE.md names: dead-ness is derived, so the row the
    /// event points at must be byte-identical afterwards. Asserting only the
    /// derived `DiscStatus::Dead` would pass against an update-in-place.
    #[test]
    fn the_disc_row_still_reads_open_after_the_event() {
        let mut e = env();
        cycle(&mut e, &[("a.txt", b"aaa")], at(10));
        assert_eq!(stored_status(&e, 1), "OPEN");

        run_mark_dead(&mut e, "1").expect("mark-dead succeeds");

        assert_eq!(
            stored_status(&e, 1),
            "OPEN",
            "disc.status must never say DEAD; the check constraint has no such member"
        );
        let derived = e.catalog.read_disc_states(ProjectId(1)).unwrap();
        assert_eq!(
            derived.iter().find(|s| s.ordinal == 1).map(|s| s.status),
            Some(DiscStatus::Dead),
            "and the derived read must still say DEAD"
        );
    }

    /// The whole coaster workflow in one test (`PipelineMarkDeadSpec`'s first
    /// case). Disc 1 takes two sessions, so it holds one superseded version
    /// (b@1) and two current ones (a@1, b@2) — one of each casualty class.
    #[test]
    fn a_dead_discs_live_files_re_plan_onto_a_fresh_disc() {
        let mut e = env();
        cycle(&mut e, &[("a.txt", b"aaa"), ("b.txt", b"bbb")], at(10));
        // A second session on the same open disc supersedes b.txt in place.
        cycle(&mut e, &[("a.txt", b"aaa"), ("b.txt", b"bbbb")], at(20));

        let report = run_mark_dead(&mut e, "1").expect("mark-dead succeeds");
        assert_eq!(report.disc, DiscKey::new(1, 0));
        assert_eq!(
            report
                .lost
                .iter()
                .map(|l| (l.rel_path.as_str(), l.session_ordinal))
                .collect::<Vec<_>>(),
            vec![("b.txt", 1)],
            "only the superseded version is permanently gone"
        );
        assert_eq!(
            report
                .reburn
                .iter()
                .map(|r| r.rel_path.as_str())
                .collect::<Vec<_>>(),
            vec!["a.txt", "b.txt"],
            "both current versions re-queue, ordered by path"
        );

        // The disc reads DEAD and is no longer the open disc.
        let status = {
            let work = e.work.clone();
            let scan_cache = e.scan_cache.clone();
            let pipeline = Pipeline::new(
                &mut e.catalog,
                PipelinePaths {
                    work_dir: work,
                    scan_cache,
                },
                &NoopSink,
            );
            pipeline.status("p").unwrap()
        };
        assert_eq!(
            status
                .discs
                .iter()
                .find(|d| d.ordinal == 1)
                .map(|d| d.status),
            Some(DiscStatus::Dead)
        );
        assert!(status.open_discs.is_empty());
        // Both are still current versions, now with no live copy (#6).
        assert_eq!(status.total_current_files, 2);
        assert_eq!(
            status.coverage.no_live_copy,
            diskrement_core::SetCoverage {
                missing_files: 2,
                missing_bytes: 7,
            }
        );

        // The recovery cycle plans both live files onto a brand-new disc 2.
        stage(&e, &[("a.txt", b"aaa"), ("b.txt", b"bbbb")]);
        let plan = run_plan(&mut e).expect("plan succeeds");
        assert!(plan.sets[0].disc.is_new);
        assert_eq!(plan.sets[0].disc.ordinal, 2);
        assert_eq!(
            plan.sets[0]
                .disc
                .files
                .iter()
                .map(|f| f.rel_path.as_str())
                .collect::<Vec<_>>(),
            vec!["a.txt", "b.txt"]
        );

        cycle(&mut e, &[("a.txt", b"aaa"), ("b.txt", b"bbbb")], at(30));

        // The sweep skips the dead disc silently and checks only disc 2.
        let reports = run_verify(&mut e, None).expect("verify succeeds");
        assert_eq!(
            reports.iter().map(|r| r.disc.ordinal).collect::<Vec<_>>(),
            vec![2]
        );
        assert!(reports
            .iter()
            .all(|r| !r.skipped && r.mismatches.is_empty()));

        // Naming the dead disc, though, is an assertion of intent and an error.
        let named = run_verify(&mut e, Some("1")).unwrap_err();
        assert!(
            matches!(named, PipelineError::DiscIsDead(ref s) if s == "1"),
            "{named:?}"
        );
    }

    /// LOST is per physical copy, not per content hash (CLAUDE.md). b@2 and
    /// b@3 share one hash: the copy on the dead disc reads LOST and the copy
    /// on the recovery disc reads CURRENT, and no test may collapse them.
    #[test]
    fn two_copies_of_one_hash_read_lost_and_current() {
        let mut e = env();
        cycle(&mut e, &[("b.txt", b"bbb")], at(10));
        cycle(&mut e, &[("b.txt", b"bbbb")], at(20));
        run_mark_dead(&mut e, "1").expect("mark-dead succeeds");
        cycle(&mut e, &[("b.txt", b"bbbb")], at(30));

        let history = e
            .catalog
            .file_history(ProjectId(1), &RelPath("b.txt".to_string()))
            .expect("history reads");

        assert_eq!(
            history
                .iter()
                .map(|h| (h.disc.ordinal, h.session_ordinal, h.status))
                .collect::<Vec<_>>(),
            vec![
                (1, 1, VersionStatus::Lost),
                (1, 2, VersionStatus::Lost),
                (2, 1, VersionStatus::Current),
            ]
        );
        assert_eq!(
            history[1].content_hash, history[2].content_hash,
            "the last two versions share a hash — LOST is about the copy, not the bytes"
        );
    }

    /// A file whose only copy dies between `plan` and `burn` is not on the
    /// disc this burn writes, so the burn report must not call it covered.
    #[test]
    fn a_burn_after_the_last_copy_died_reports_the_file_as_having_no_live_copy() {
        let mut e = env();
        let sink = NoopSink;
        let burner = Burner::Simulated(SimulatedBurner::new(&sink, 2));
        let paths = |e: &Env| PipelinePaths {
            work_dir: e.work.clone(),
            scan_cache: e.scan_cache.clone(),
        };

        // Disc 1: a.txt, sealed, so the next cycle opens disc 2.
        stage(&e, &[("a.txt", b"aaaa")]);
        {
            let p = paths(&e);
            let mut pipeline = Pipeline::new(&mut e.catalog, p, &sink);
            pipeline.plan("p", None, true).unwrap();
            pipeline.burn("p", at(1), &burner).unwrap();
        }

        // Cycle 2 plans b.txt onto disc 2; a.txt is unchanged and stays on
        // disc 1, which dies before the burn.
        stage(&e, &[("a.txt", b"aaaa"), ("b.txt", b"bbbbbb")]);
        {
            let p = paths(&e);
            let pipeline = Pipeline::new(&mut e.catalog, p, &sink);
            pipeline.plan("p", None, false).unwrap();
        }
        run_mark_dead(&mut e, "1").expect("mark-dead succeeds");

        let report = {
            let p = paths(&e);
            let mut pipeline = Pipeline::new(&mut e.catalog, p, &sink);
            pipeline.burn("p", at(600), &burner).unwrap()
        };
        assert_eq!(report.discs.len(), 1, "disc 2 is written");
        let gone = diskrement_core::SetCoverage {
            missing_files: 1,
            missing_bytes: 4,
        };
        assert_eq!(report.coverage.no_live_copy, gone);
        assert_eq!(report.coverage.sets, vec![gone]);
    }
}

#[cfg(test)]
mod copies_tests {
    use super::*;
    use crate::burn::simulated::SimulatedBurner;
    use crate::burn::verifier::StagingVerifier;
    use crate::progress::NoopSink;
    use chrono::TimeZone;
    use diskrement_core::{DiscArg, DiscKey, DiscStatus, DiscType};
    use proptest::prelude::*;

    struct Env {
        _dir: tempfile::TempDir,
        catalog: Catalog,
        work: PathBuf,
        source: PathBuf,
    }

    fn at(offset: i64) -> chrono::DateTime<Utc> {
        Utc.timestamp_opt(1_757_000_000 + offset, 0).unwrap()
    }

    fn paths(env: &Env) -> PipelinePaths {
        PipelinePaths {
            work_dir: env.work.clone(),
            scan_cache: env.work.join("scancache.sqlite"),
        }
    }

    /// A two-copy project with `a.bin` burned to 1A and 1B.
    fn burned_twice() -> Env {
        let dir = tempfile::TempDir::new().unwrap();
        let (source, work) = (dir.path().join("src"), dir.path().join("work"));
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(source.join("a.bin"), b"aaaa").unwrap();
        let mut env = Env {
            catalog: Catalog::open(&dir.path().join("cat.sqlite")).unwrap(),
            _dir: dir,
            work,
            source,
        };
        let pi = ProjectInit {
            name: "p".into(),
            source_root: env.source.display().to_string(),
            work_dir: env.work.display().to_string(),
            default_disc_type: DiscType::BdR,
            margin_percent: 2.0,
            copies: 2,
            created_at: at(0),
        };
        let p = paths(&env);
        let mut pipeline = Pipeline::new(&mut env.catalog, p, &NoopSink);
        pipeline.init(&pi).unwrap();
        pipeline.scan("p", false, false).unwrap();
        pipeline.plan("p", None, false).unwrap();
        let burner = Burner::Simulated(SimulatedBurner::new(&NoopSink, 2));
        pipeline.burn("p", at(1), &burner).unwrap();
        env
    }

    fn pipeline(env: &mut Env) -> Pipeline<'_> {
        let p = paths(env);
        Pipeline::new(&mut env.catalog, p, &NoopSink)
    }

    fn staging_verifier(env: &Env) -> Verifier<'static> {
        Verifier::Staging(StagingVerifier::new(env.work.join("staging"), &NoopSink))
    }

    #[test]
    fn status_reports_each_sets_open_disc_and_full_coverage() {
        let mut env = burned_twice();
        let s = pipeline(&mut env).status("p").unwrap();
        assert_eq!(s.open_discs, vec![DiscKey::new(1, 0), DiscKey::new(1, 1)]);
        assert!(s.coverage.sets.iter().all(|c| c.missing_files == 0));
    }

    #[test]
    fn marking_one_twin_dead_degrades_and_status_shows_the_gap() {
        let mut env = burned_twice();
        let report = pipeline(&mut env)
            .mark_dead("p", "1B".parse::<DiscArg>().unwrap(), at(2))
            .unwrap();
        assert_eq!(report.disc, DiscKey::new(1, 1));
        assert!(report.lost.is_empty() && report.reburn.is_empty());
        assert_eq!(report.degraded.len(), 1);
        let s = pipeline(&mut env).status("p").unwrap();
        assert_eq!(s.coverage.sets[1].missing_files, 1);
        assert_eq!(
            s.coverage.no_live_copy.missing_files, 0,
            "a.bin is still on 1A"
        );
    }

    /// Each version's copies share its number and sit together, 1A before 1B
    /// (#7). The current copy on the dead 1B still reads CURRENT, flagged as
    /// on a dead disc; the superseded one there reads LOST.
    #[test]
    fn a_two_copy_history_numbers_versions_not_copies() {
        let mut env = burned_twice();
        std::fs::write(env.source.join("a.bin"), b"AAAA").unwrap();
        pipeline(&mut env).scan("p", false, true).unwrap();
        pipeline(&mut env).plan("p", None, false).unwrap();
        let burner = Burner::Simulated(SimulatedBurner::new(&NoopSink, 2));
        pipeline(&mut env).burn("p", at(2), &burner).unwrap();
        pipeline(&mut env)
            .mark_dead("p", "1B".parse::<DiscArg>().unwrap(), at(3))
            .unwrap();

        let history = pipeline(&mut env)
            .history("p", &RelPath("a.bin".to_string()))
            .unwrap();
        assert_eq!(
            history
                .iter()
                .map(|h| (
                    h.version,
                    h.disc.name(2),
                    h.session_ordinal,
                    h.status,
                    h.on_dead_disc
                ))
                .collect::<Vec<_>>(),
            vec![
                (
                    1,
                    "1A".to_string(),
                    1,
                    crate::catalog::model::VersionStatus::Superseded,
                    false
                ),
                (
                    1,
                    "1B".to_string(),
                    1,
                    crate::catalog::model::VersionStatus::Lost,
                    true
                ),
                (
                    2,
                    "1A".to_string(),
                    2,
                    crate::catalog::model::VersionStatus::Current,
                    false
                ),
                (
                    2,
                    "1B".to_string(),
                    2,
                    crate::catalog::model::VersionStatus::Current,
                    true
                ),
            ]
        );
    }

    #[test]
    fn marking_both_twins_dead_leaves_no_set_complete_and_still_counts_the_file() {
        let mut env = burned_twice();
        pipeline(&mut env)
            .mark_dead("p", "1A".parse::<DiscArg>().unwrap(), at(2))
            .unwrap();
        pipeline(&mut env)
            .mark_dead("p", "1B".parse::<DiscArg>().unwrap(), at(3))
            .unwrap();
        let s = pipeline(&mut env).status("p").unwrap();
        // a.bin is 4 bytes ("aaaa").
        let gone = diskrement_core::SetCoverage {
            missing_files: 1,
            missing_bytes: 4,
        };
        assert_eq!(s.coverage.no_live_copy, gone);
        assert_eq!(s.coverage.sets, vec![gone, gone]);
        assert_eq!(s.total_current_files, 1, "a.bin is still a current version");
    }

    #[test]
    fn verify_names_a_disc_by_set_and_refuses_a_letter_past_the_last_set() {
        let mut env = burned_twice();
        let verifier = staging_verifier(&env);
        let one = pipeline(&mut env)
            .verify("p", Some("1B".parse().unwrap()), &verifier)
            .unwrap();
        assert_eq!(
            one.iter().map(|r| r.disc).collect::<Vec<_>>(),
            vec![DiscKey::new(1, 1)]
        );
        assert!(one[0].mismatches.is_empty() && !one[0].skipped);

        let err = pipeline(&mut env)
            .verify("p", Some("1C".parse().unwrap()), &verifier)
            .unwrap_err();
        assert!(
            matches!(err, PipelineError::UnknownDisc(ref s) if s == "1C"),
            "{err}"
        );
    }

    /// The sweep visits each twin once under its own key — before Task 13 it
    /// verified 1A twice and the second `mark_verified` failed — and marks
    /// both VERIFIED; a repeat sweep is a clean no-op.
    #[test]
    fn a_sweep_verifies_both_twins_once_each_and_a_repeat_is_a_no_op() {
        let mut env = burned_twice();
        let verifier = staging_verifier(&env);
        let reports = pipeline(&mut env).verify("p", None, &verifier).unwrap();
        assert_eq!(
            reports.iter().map(|r| r.disc).collect::<Vec<_>>(),
            vec![DiscKey::new(1, 0), DiscKey::new(1, 1)]
        );
        assert!(reports
            .iter()
            .all(|r| !r.skipped && r.mismatches.is_empty() && r.files_checked == 1));

        let verified = env
            .catalog
            .sessions_by_state(ProjectId(1), SessionState::Verified)
            .unwrap();
        let mut keys: Vec<DiscKey> = verified.iter().map(|s| s.disc).collect();
        keys.sort();
        assert_eq!(keys, vec![DiscKey::new(1, 0), DiscKey::new(1, 1)]);
        assert!(env
            .catalog
            .sessions_by_state(ProjectId(1), SessionState::Burned)
            .unwrap()
            .is_empty());

        let again = pipeline(&mut env).verify("p", None, &verifier).unwrap();
        assert_eq!(again.len(), 2);
        assert!(again.iter().all(|r| !r.skipped && r.mismatches.is_empty()));
    }

    /// What happens between two full cycles of the property below.
    #[derive(Debug, Clone)]
    enum Change {
        /// Create or rewrite `NAMES[i]` with this many kilobytes.
        Write(usize, u64),
        Delete(usize),
        /// Mark the i-th still-live disc (modulo their count) dead.
        KillDisc(usize),
    }

    const NAMES: [&str; 5] = ["a.bin", "b.bin", "c.bin", "d/e.bin", "f.bin"];

    fn change() -> impl Strategy<Value = Change> {
        prop_oneof![
            (0..NAMES.len(), 40..=200u64).prop_map(|(i, kb)| Change::Write(i, kb)),
            (0..NAMES.len()).prop_map(Change::Delete),
            (0..8usize).prop_map(Change::KillDisc),
        ]
    }

    /// scan → plan → burn until a plan has nothing to write. Every cycle
    /// writes at most one disc per set, so this converges; the bound only
    /// turns a regression into a failure instead of a hang.
    fn run_to_completion(env: &mut Env, clock: &mut i64) -> Result<(), TestCaseError> {
        for _ in 0..40 {
            pipeline(env).scan("p", false, true).unwrap();
            let report = pipeline(env).plan("p", None, false).unwrap();
            prop_assert!(report.oversized.is_empty() && report.unplaceable_catch_up.is_empty());
            if report.sets.is_empty() {
                return Ok(());
            }
            *clock += 1;
            let burner = Burner::Simulated(SimulatedBurner::new(&NoopSink, 2));
            pipeline(env).burn("p", at(*clock), &burner).unwrap();
        }
        Err(TestCaseError::fail("no fixpoint after 40 cycles"))
    }

    /// Spec §9 (#10): after a full cycle with no failures, every current
    /// version of a file still in the source has `copies` live copies, one
    /// per set, whatever was burned, rewritten, deleted or lost to dead discs
    /// along the way.
    ///
    /// `killed` is the test's own record of the discs it marked dead, so the
    /// physical check below does not trust the catalog's dead-disc logic,
    /// which also drives catch-up.
    fn assert_fully_replicated(
        env: &mut Env,
        copies: u32,
        killed: &std::collections::BTreeSet<DiscKey>,
    ) -> Result<(), TestCaseError> {
        let view = env.catalog.read_view(ProjectId(1)).unwrap();
        let all_sets: std::collections::BTreeSet<u32> = (0..copies).collect();
        // Walk the source rather than the view: `read_view` leaves out a
        // version with no live copy at all, and still lists a deleted file
        // whose deletion waits for the next burned disc (#9), which nothing
        // needs to catch up.
        for name in NAMES {
            if env.source.join(name).exists() {
                let entry = view.current.get(&RelPath(name.to_string()));
                prop_assert!(entry.is_some(), "{} has no live copy", name);
                prop_assert_eq!(
                    &entry.unwrap().live_sets,
                    &all_sets,
                    "{} is short of copies",
                    name
                );

                // Independently: the source's bytes sit on a disc the test
                // did not kill, in every set.
                let rel = RelPath(name.to_string());
                let hash = crate::fs::hasher::hash(&env.source.join(name)).unwrap();
                let held_in: std::collections::BTreeSet<u32> = env
                    .catalog
                    .file_history(ProjectId(1), &rel)
                    .unwrap()
                    .iter()
                    .filter(|h| h.content_hash == hash && !killed.contains(&h.disc))
                    .map(|h| h.disc.copy_set)
                    .collect();
                prop_assert_eq!(&held_in, &all_sets, "{}'s bytes are short of copies", name);
            }
        }
        // Coverage counts every catalog-current version, which includes a
        // deleted file whose deletion waits for the next burned disc (#9). If
        // its last copy died meanwhile, it reads "no live copy" until then:
        // the imprecision recorded with #6. Such pending deletions are the
        // only gap coverage may show.
        let status = pipeline(env).status("p").unwrap();
        let on_source = NAMES.iter().filter(|n| env.source.join(n).exists()).count();
        // The loop above proved every source file current, so this is exact.
        let pending = status.total_current_files - on_source;
        prop_assert!(status
            .coverage
            .sets
            .iter()
            .all(|c| c.missing_files <= pending));
        prop_assert!(status.coverage.no_live_copy.missing_files <= pending);
        Ok(())
    }

    proptest! {
        // Each case runs several real cycles against a catalog file and a
        // staged tree, so the case count stays small.
        #![proptest_config(ProptestConfig::with_cases(24))]

        #[test]
        fn after_a_full_cycle_every_current_version_has_copies_live_copies(
            copies in 1..=3u32,
            initial in prop::collection::vec((0..NAMES.len(), 40..=200u64), 2..6),
            rounds in prop::collection::vec(prop::collection::vec(change(), 1..4), 0..4),
        ) {
            let dir = tempfile::TempDir::new().unwrap();
            let (source, work) = (dir.path().join("src"), dir.path().join("work"));
            std::fs::create_dir_all(source.join("d")).unwrap();
            std::fs::create_dir_all(&work).unwrap();
            let mut env = Env {
                catalog: Catalog::open(&dir.path().join("cat.sqlite")).unwrap(),
                _dir: dir,
                work,
                source,
            };
            // Each write gets fresh bytes, so a rewrite is always a new version.
            let mut generation = 0u8;
            let mut write = |env: &Env, i: usize, kb: u64| {
                generation = generation.wrapping_add(1);
                std::fs::write(env.source.join(NAMES[i]), vec![generation; kb as usize * 1000])
                    .unwrap();
            };
            for (i, kb) in initial {
                write(&env, i, kb);
            }
            // A margin that leaves 250_000 usable bytes (see `tiny_project_init`),
            // so two or more files of 40–200 KB usually spill onto a second
            // disc per set.
            let init = ProjectInit {
                name: "p".into(),
                source_root: env.source.display().to_string(),
                work_dir: env.work.display().to_string(),
                default_disc_type: DiscType::BdR,
                margin_percent: 99.999,
                copies,
                created_at: at(0),
            };
            pipeline(&mut env).init(&init).unwrap();

            let mut clock = 0;
            let mut killed = std::collections::BTreeSet::new();
            run_to_completion(&mut env, &mut clock)?;
            assert_fully_replicated(&mut env, copies, &killed)?;

            for round in rounds {
                for c in round {
                    match c {
                        Change::Write(i, kb) => write(&env, i, kb),
                        Change::Delete(i) => {
                            let _ = std::fs::remove_file(env.source.join(NAMES[i]));
                        }
                        Change::KillDisc(i) => {
                            let live: Vec<DiscKey> = env
                                .catalog
                                .read_disc_states(ProjectId(1))
                                .unwrap()
                                .iter()
                                .filter(|d| d.status != DiscStatus::Dead)
                                .map(|d| d.key())
                                .collect();
                            if !live.is_empty() {
                                let victim = live[i % live.len()];
                                killed.insert(victim);
                                let arg = victim.name(copies);
                                clock += 1;
                                pipeline(&mut env)
                                    .mark_dead("p", arg.parse::<DiscArg>().unwrap(), at(clock))
                                    .unwrap();
                            }
                        }
                    }
                }
                run_to_completion(&mut env, &mut clock)?;
                assert_fully_replicated(&mut env, copies, &killed)?;
            }
        }
    }
}
