//! The pipeline (`Pipeline.scala`): returns reports, performs no path
//! resolution, and — for scan — **writes no catalog rows**. P3 lands `init`,
//! `scan` and `status`; later phases append the rest of the nine methods.

pub mod planning;
pub mod reports;
pub mod staged;

use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use diskrement_core::{diff, Capacity, DiscStatus, DiscType, RelPath, ScannedFile};

use crate::catalog::model::{FileFingerprint, Project, ProjectId, ProjectInit, SessionState};
use crate::catalog::Catalog;
use crate::error::PipelineError;
use crate::fs::fingerprint_cache::FingerprintCache;
use crate::fs::hasher;
use crate::fs::scanner::{self, parallelism_from_env, ScanOutput};
use crate::fs::snapshot;
use crate::progress::ProgressSink;

use reports::{PlanReport, ScanReport, StatusReport};

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
