//! The pipeline (`Pipeline.scala`): returns reports, performs no path
//! resolution, and — for scan — **writes no catalog rows**. P3 lands `init`,
//! `scan` and `status`; later phases append the rest of the nine methods.

pub mod planning;
pub mod reports;
pub mod staged;

use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use diskrement_core::{
    diff, manifest, Capacity, ChangeSet, DiscStatus, DiscType, RelPath, ScannedFile,
    SessionManifest,
};

use crate::burn::{json, Burner, DiscIdentity, DiscImage, DiscMetadata};
use crate::catalog::model::{
    CatalogUpdate, DiscWrite, FileFingerprint, FileWrite, Project, ProjectId, ProjectInit,
    SessionState, SessionWrite,
};
use crate::catalog::Catalog;
use crate::error::PipelineError;
use crate::fs::fingerprint_cache::FingerprintCache;
use crate::fs::hasher;
use crate::fs::scanner::{self, parallelism_from_env, ScanOutput};
use crate::fs::snapshot::{self, SnapshotHandle};
use crate::progress::ProgressSink;

use reports::{BurnReport, DiscBurn, PlanReport, ScanReport, StatusReport};

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

    fn failed(e: impl std::fmt::Display) -> PipelineError {
        PipelineError::Failed(e.to_string())
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
    /// (`PipelineLive.scan`). `hashed` is `None` on the reuse path.
    pub fn scan(
        &mut self,
        name: &str,
        verify: bool,
        force: bool,
    ) -> Result<ScanReport, PipelineError> {
        let p = self.project(name)?;
        if !force && !verify {
            if let Some(files) = staged::read_scan(&self.paths.work_dir)
                .map_err(|e| Self::failed(format!("corrupt scan.json: {e}")))?
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
        .map_err(Self::failed)?;
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
        // only `reset` (P6) removes it.
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
        Ok(ScanReport {
            added: cs.added.len(),
            modified: cs.modified.len(),
            deleted: cs.deleted.len(),
            unchanged: cs.unchanged.len(),
            hashed,
            scanned_files: files.len(),
        })
    }

    /// `plan`: the staged scan plus the catalog view decide the one disc this
    /// cycle writes (`PipelineLive.plan`). Stages `staged/plan.json` and
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
            .map_err(|e| Self::failed(format!("corrupt scan.json: {e}")))?
            .ok_or_else(|| PipelineError::NoStagedScan(name.to_string()))?;
        let view = self.catalog.read_view(p.id).map_err(Self::failed)?;
        let states = self.catalog.read_disc_states(p.id).map_err(Self::failed)?;
        let disc_type = disc_type_override.unwrap_or(p.default_disc_type);
        let cap = planning::effective_capacity(
            Capacity::from_disc_type(disc_type, p.margin_percent),
            planning::disc_bytes_from_env(),
        );
        let report = planning::plan_from(&view, &scanned, &states, disc_type, cap, finalize);
        staged::write_plan(&self.paths.work_dir, &report).map_err(Self::failed)?;
        Ok(report)
    }

    /// `burn`: write the staged plan's one disc, then commit its one session.
    ///
    /// The ordering is the design (`PipelineLive.burn`). The disc tree is
    /// staged **before** anything reaches the live catalog, and the snapshot
    /// embedded on the disc is the same update applied to a work-dir copy, so
    /// the disc knows about its own session. A crash before the commit leaves
    /// the catalog untouched and the staged plan intact — the staged plan is
    /// the recovery unit, and no `PLANNED` row is ever persisted.
    ///
    /// `now` is supplied by the caller rather than read from the clock, as
    /// `ProjectInit::created_at` already is: it keeps the pipeline
    /// deterministic and lets the corpus replay compare committed timestamps
    /// exactly.
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
        let report = staged::read_plan(&self.paths.work_dir)
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

        // The catalog cannot catch this: deadness is derived from `disc_event`,
        // so the disc row itself still reads OPEN. Without this guard the
        // burner would commit a session onto a disc that no longer physically
        // exists.
        let states = self.catalog.read_disc_states(p.id).map_err(Self::failed)?;
        if states
            .iter()
            .any(|s| s.ordinal == report.disc.ordinal && s.status == DiscStatus::Dead)
        {
            return Err(PipelineError::StalePlan(format!(
                "disc {} was marked dead after this plan was staged",
                report.disc.ordinal
            )));
        }

        let snap_root = self.paths.work_dir.join("snapshot");
        if !snap_root.exists() {
            return Err(PipelineError::SnapshotMissing(name.to_string()));
        }

        let view = self.catalog.read_view(p.id).map_err(Self::failed)?;
        let change_set = diff::diff(&view, &scanned);
        let sess_ord = self.next_session_ordinal(p.id, report.disc.ordinal)?;
        let manifest = manifest::build(
            name,
            sess_ord,
            &report.disc,
            &change_set,
            report.tombstones.clone(),
        );
        let label = format!("{name} disc {}", report.disc.ordinal);
        let update = CatalogUpdate {
            project_id: p.id,
            finalize_disc_ordinals: Vec::new(),
            discs: vec![disc_write(
                &label,
                &report,
                sess_ord,
                now,
                &change_set,
                &manifest,
            )],
        };

        let tmp_catalog = self.paths.work_dir.join("burn-catalog.sqlite");
        if let Some(parent) = tmp_catalog.parent() {
            std::fs::create_dir_all(parent).map_err(Self::failed)?;
        }
        let _ = std::fs::remove_file(&tmp_catalog);
        self.catalog
            .snapshot_to(&tmp_catalog)
            .map_err(Self::failed)?;
        // Materialize the post-burn state into the snapshot this disc embeds.
        // Applies to the copy only; the live catalog is committed after the
        // disc is staged.
        Catalog::apply_update_to(&tmp_catalog, &update).map_err(Self::failed)?;

        let pointers = self.catalog.archive_pointers(p.id).map_err(Self::failed)?;
        let image = DiscImage {
            staging_dir: self
                .paths
                .work_dir
                .join("staging")
                .join(format!("disc-{}", report.disc.ordinal)),
            snapshot_root: snap_root.clone(),
            files: report.disc.files.clone(),
            metadata: DiscMetadata {
                identity: DiscIdentity {
                    project_name: name.to_string(),
                    disc_ordinal: report.disc.ordinal,
                    session_ordinal: sess_ord,
                    created_at: now,
                },
                manifest,
                catalog_snapshot: tmp_catalog.clone(),
                pointers,
            },
            close_disc: report.disc.will_finalize,
        };
        let burned = burner.burn(&image)?;

        // The crash-safe commit point: the disc exists, so the catalog may now
        // learn about it. The committed state equals the embedded snapshot.
        self.catalog.apply_update(&update).map_err(Self::failed)?;
        snapshot::release(&SnapshotHandle { root: snap_root });
        staged::clear(&self.paths.work_dir).map_err(Self::failed)?;
        let _ = std::fs::remove_file(&tmp_catalog);

        Ok(BurnReport {
            disc: DiscBurn {
                ordinal: report.disc.ordinal,
                label,
                finalized: report.disc.will_finalize,
                staging_dir: burned.staging_dir,
                image_bytes: burned.image_bytes,
            },
            remaining: report.remaining,
        })
    }

    /// One past the highest session ordinal already committed for this disc
    /// (`PipelineLive.existingSessions`). Both BURNED and VERIFIED count.
    fn next_session_ordinal(
        &self,
        project: ProjectId,
        disc_ordinal: u32,
    ) -> Result<u32, PipelineError> {
        let mut highest = 0u32;
        for state in [SessionState::Burned, SessionState::Verified] {
            for s in self
                .catalog
                .sessions_by_state(project, state)
                .map_err(Self::failed)?
            {
                if s.disc_ordinal == disc_ordinal {
                    highest = highest.max(s.ordinal);
                }
            }
        }
        Ok(highest + 1)
    }

    /// `status`: catalog reads only, never mutates (`PipelineLive.status`).
    pub fn status(&self, name: &str) -> Result<StatusReport, PipelineError> {
        let p = self.project(name)?;
        let discs = self.catalog.read_disc_states(p.id).map_err(Self::failed)?;
        let view = self.catalog.read_view(p.id).map_err(Self::failed)?;
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
        Ok(StatusReport {
            open_disc_ordinal: discs
                .iter()
                .find(|d| d.status == DiscStatus::Open)
                .map(|d| d.ordinal),
            total_current_files: view.current.len(),
            discs,
            last_burn_at,
        })
    }
}

/// `getFileStore` equality via `st_dev` (`PipelineLive.sameFilesystem`): both
/// paths must exist and carry the same device id.
fn same_filesystem(source: &Path, work: &Path) -> std::io::Result<(u64, u64)> {
    Ok((
        std::fs::metadata(source)?.dev(),
        std::fs::metadata(work)?.dev(),
    ))
}

/// `PipelineLive.toDiscWrite`: split the disc's files into added vs modified
/// using the change set, and stamp every one with its `/data/`-rooted disc
/// path. `copy_index` is 0 until `copies > 1` is activated.
fn disc_write(
    label: &str,
    report: &PlanReport,
    sess_ord: u32,
    now: DateTime<Utc>,
    cs: &ChangeSet,
    manifest: &SessionManifest,
) -> DiscWrite {
    let added_paths: std::collections::HashSet<&RelPath> =
        cs.added.iter().map(|f| &f.rel_path).collect();
    let (added, modified): (Vec<&ScannedFile>, Vec<&ScannedFile>) = report
        .disc
        .files
        .iter()
        .partition(|f| added_paths.contains(&f.rel_path));
    let fw = |f: &ScannedFile| FileWrite {
        rel_path: f.rel_path.clone(),
        content_hash: f.content_hash.clone(),
        size_bytes: f.size_bytes,
        mtime: f.mtime,
        disc_path: format!("/data/{}", f.rel_path.0),
        copy_index: 0,
    };
    DiscWrite {
        ordinal: report.disc.ordinal,
        is_new: report.disc.is_new,
        disc_type: report.disc_type,
        usable_bytes: report.disc.usable_bytes,
        status: if report.disc.will_finalize {
            DiscStatus::Finalized
        } else {
            DiscStatus::Open
        },
        label: label.to_string(),
        session: SessionWrite {
            ordinal: sess_ord,
            burned_at: now,
            manifest_hash: json::manifest_hash(manifest),
            added: added.into_iter().map(fw).collect(),
            modified: modified.into_iter().map(fw).collect(),
            deleted: report.tombstones.clone(),
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
        assert_eq!(r.open_disc_ordinal, None);
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
        assert_eq!(report.disc.ordinal, 1);
        assert_eq!(report.disc.files.len(), 1);
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
            report.disc.usable_bytes,
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
            report.disc.usable_bytes,
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
            !e.pipeline
                .plan("p", None, false)
                .unwrap()
                .disc
                .will_finalize
        );
        assert!(e.pipeline.plan("p", None, true).unwrap().disc.will_finalize);
    }

    #[test]
    fn a_spilling_plan_seals_this_cycles_disc_and_reports_the_rest() {
        let mut e = setup();
        write_src(&e, "a.txt", &[b'a'; 200_000]);
        write_src(&e, "b.txt", &[b'b'; 200_000]);
        e.pipeline.init(&tiny_project_init(&e)).unwrap();
        e.pipeline.scan("p", false, false).unwrap();
        let report = e.pipeline.plan("p", None, false).unwrap();
        assert_eq!(report.disc.files.len(), 1);
        assert!(report.disc.will_finalize);
        assert_eq!(report.remaining.more_discs, 1);
        assert_eq!(report.remaining.more_files, 1);
        assert_eq!(report.remaining.more_bytes, 200_000);
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
        assert!(second.disc.will_finalize);
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
    use diskrement_core::{DiscType, RelPath, ScannedFile};

    use crate::burn::{simulated::SimulatedBurner, Burner};
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
            &[],
            DiscType::BdR,
            diskrement_core::Capacity::new(1_000_000),
            finalize,
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

    #[test]
    fn burning_stages_the_disc_and_commits_one_session() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], false);
        let report = run_burn(&mut env).unwrap();

        assert_eq!(report.disc.ordinal, 1);
        assert_eq!(report.disc.label, "p disc 1");
        assert!(!report.disc.finalized);
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
        assert_eq!(sessions[0].disc_ordinal, 1);
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
            &[],
            DiscType::BdR,
            diskrement_core::Capacity::new(1_000_000),
            false,
        );
        report.disc.ordinal = 1;
        report.disc.is_new = false;
        staged::write_plan(&env.work, &report).unwrap();

        env.catalog
            .mark_dead(ProjectId(1), 1, &now())
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

    #[test]
    fn a_finalizing_plan_marks_the_disc_finalized() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], true);
        let report = run_burn(&mut env).unwrap();
        assert!(report.disc.finalized);

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
        let view = env.catalog.read_view(ProjectId(1)).unwrap();
        let states = env.catalog.read_disc_states(ProjectId(1)).unwrap();
        let report = crate::pipeline::planning::plan_from(
            &view,
            &scanned,
            &states,
            DiscType::BdR,
            diskrement_core::Capacity::new(1_000_000),
            false,
        );
        staged::write_plan(&env.work, &report).unwrap();

        run_burn(&mut env).unwrap();
        let sessions = env
            .catalog
            .sessions_by_state(ProjectId(1), SessionState::Burned)
            .unwrap();
        let mut ordinals: Vec<u32> = sessions
            .iter()
            .filter(|s| s.disc_ordinal == 1)
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
}
