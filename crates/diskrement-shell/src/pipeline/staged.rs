//! The staged scan and plan (`StagedStore.scala`): `<work>/staged/scan.json`
//! is written by `plan` beside the plan and read back by `burn` to detect
//! drift; `<work>/staged/plan.json` is the recovery unit itself — `plan`
//! stages it, `burn` consumes it. Field spellings follow the Scala zio-json
//! codec's camelCase. The plan is v2 since copy sets (one entry per set); a
//! v1 plan is detected by `is_legacy_plan` and answered with a stale-plan
//! error.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use diskrement_core::{ContentHash, DiscType, PlannedDisc, RelPath, ScannedFile};
use serde::{Deserialize, Serialize};

use crate::catalog::time;
use crate::pipeline::reports::{PlanReport, RemainingEstimate, SetPlan};

/// A decode failure that is not an I/O failure: a corrupt staged file is disk
/// corruption, and the Scala surfaced it as a defect rather than a typed error.
fn decode_error(e: impl std::fmt::Display) -> serde_json::Error {
    <serde_json::Error as serde::de::Error>::custom(e.to_string())
}

#[derive(Serialize, Deserialize)]
struct ScannedFileJson {
    #[serde(rename = "relPath")]
    rel_path: String,
    #[serde(rename = "contentHash")]
    content_hash: String,
    #[serde(rename = "sizeBytes")]
    size_bytes: u64,
    mtime: String,
}

impl ScannedFileJson {
    // Named `of`, not `from`: an inherent `from` trips clippy's
    // `should_implement_trait`, and `-D warnings` makes that fatal.
    fn of(f: &ScannedFile) -> Self {
        ScannedFileJson {
            rel_path: f.rel_path.as_str().to_string(),
            content_hash: f.content_hash.as_str().to_string(),
            size_bytes: f.size_bytes,
            mtime: time::format(&f.mtime),
        }
    }

    fn into_scanned(self) -> Result<ScannedFile, serde_json::Error> {
        Ok(ScannedFile {
            rel_path: RelPath(self.rel_path),
            content_hash: ContentHash(self.content_hash),
            size_bytes: self.size_bytes,
            mtime: time::parse(&self.mtime).map_err(decode_error)?,
        })
    }
}

#[derive(Serialize, Deserialize)]
struct PlannedDiscJson {
    ordinal: u32,
    #[serde(rename = "copySet")]
    copy_set: u32,
    #[serde(rename = "isNew")]
    is_new: bool,
    #[serde(rename = "willFinalize")]
    will_finalize: bool,
    files: Vec<ScannedFileJson>,
    #[serde(rename = "priorUsedBytes")]
    prior_used_bytes: u64,
    #[serde(rename = "usableBytes")]
    usable_bytes: u64,
}

#[derive(Serialize, Deserialize)]
struct SetPlanJson {
    disc: PlannedDiscJson,
    #[serde(rename = "catchUp")]
    catch_up: usize,
    #[serde(rename = "sessionOrdinal")]
    session_ordinal: u32,
    remaining: RemainingJson,
}

#[derive(Serialize, Deserialize)]
struct RemainingJson {
    #[serde(rename = "moreDiscs")]
    more_discs: usize,
    #[serde(rename = "moreFiles")]
    more_files: usize,
    #[serde(rename = "moreBytes")]
    more_bytes: u64,
}

/// The v2 staged plan: one entry per copy set this cycle writes. A v1 plan
/// had a single top-level `disc` instead of `sets`; see `is_legacy_plan`.
#[derive(Serialize, Deserialize)]
struct PlanReportJson {
    copies: u32,
    sets: Vec<SetPlanJson>,
    #[serde(rename = "discType")]
    disc_type: String,
    tombstones: Vec<String>,
    oversized: Vec<ScannedFileJson>,
    /// Absent from a plan staged before issue #12; such a plan lists those
    /// files under `oversized` and is refused until re-planned.
    #[serde(rename = "unplaceableCatchUp", default)]
    unplaceable_catch_up: Vec<ScannedFileJson>,
}

/// `<workDir>/staged/scan.json`.
pub fn scan_path(work_dir: &Path) -> PathBuf {
    work_dir.join("staged").join("scan.json")
}

/// `<workDir>/staged/plan.json`.
pub fn plan_path(work_dir: &Path) -> PathBuf {
    work_dir.join("staged").join("plan.json")
}

/// Compact JSON, like zio-json's `.toJson`, written atomically.
///
/// `burn` rewrites `plan.json` right after each disc commits — exactly the
/// window a crash is most likely to hit — so the file must never be seen half
/// written: write `<file>.tmp`, flush it to disk, rename it over the target,
/// then flush the directory so the rename itself survives a power loss.
fn write_json(file: &Path, json: &str) -> std::io::Result<()> {
    let dir = file.parent().expect("staged has a parent");
    std::fs::create_dir_all(dir)?;
    let mut tmp_name = file.as_os_str().to_owned();
    tmp_name.push(".tmp");
    let tmp = PathBuf::from(tmp_name);
    {
        let mut out = std::fs::File::create(&tmp)?;
        out.write_all(json.as_bytes())?;
        out.sync_all()?;
    }
    std::fs::rename(&tmp, file)?;
    std::fs::File::open(dir)?.sync_all()
}

/// `Ok(None)` when the file does not exist.
fn read_opt(file: &Path) -> Result<Option<String>, serde_json::Error> {
    match std::fs::read_to_string(file) {
        Ok(raw) => Ok(Some(raw)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(serde_json::Error::io(e)),
    }
}

pub fn write_scan(work_dir: &Path, files: &[ScannedFile]) -> std::io::Result<()> {
    let entries: Vec<ScannedFileJson> = files.iter().map(ScannedFileJson::of).collect();
    write_json(
        &scan_path(work_dir),
        &serde_json::to_string(&entries).expect("serializable"),
    )
}

/// `Ok(None)` when nothing is staged; a decode failure is the caller's
/// defect (`corrupt scan.json: …` in the Scala `RuntimeException`).
pub fn read_scan(work_dir: &Path) -> Result<Option<Vec<ScannedFile>>, serde_json::Error> {
    let Some(raw) = read_opt(&scan_path(work_dir))? else {
        return Ok(None);
    };
    let entries: Vec<ScannedFileJson> = serde_json::from_str(&raw)?;
    let mut files = Vec::with_capacity(entries.len());
    for e in entries {
        files.push(e.into_scanned()?);
    }
    Ok(Some(files))
}

/// `<workDir>/pending-deletions.json`: the latest scan's deletions, which
/// wait for the next burned disc to be recorded (#9, #16).
///
/// It lives beside `staged/`, not in it: `burn` clears `staged/`, and a
/// deletion-only cycle burns nothing, so this file is all that still says
/// these paths were deleted rather than lost. It describes the source as last
/// scanned, so a file that comes back counts as deleted until the next plan.
pub fn pending_deletions_path(work_dir: &Path) -> PathBuf {
    work_dir.join("pending-deletions.json")
}

pub fn write_pending_deletions(work_dir: &Path, paths: &[RelPath]) -> std::io::Result<()> {
    let names: Vec<&str> = paths.iter().map(RelPath::as_str).collect();
    write_json(
        &pending_deletions_path(work_dir),
        &serde_json::to_string(&names).expect("serializable"),
    )
}

/// Empty when no scan has written the file yet.
pub fn read_pending_deletions(work_dir: &Path) -> Result<BTreeSet<RelPath>, serde_json::Error> {
    let Some(raw) = read_opt(&pending_deletions_path(work_dir))? else {
        return Ok(BTreeSet::new());
    };
    let names: Vec<String> = serde_json::from_str(&raw)?;
    Ok(names.into_iter().map(RelPath).collect())
}

pub fn write_plan(work_dir: &Path, report: &PlanReport) -> std::io::Result<()> {
    let json = PlanReportJson {
        copies: report.copies,
        sets: report
            .sets
            .iter()
            .map(|sp| SetPlanJson {
                disc: PlannedDiscJson {
                    ordinal: sp.disc.ordinal,
                    copy_set: sp.disc.copy_set,
                    is_new: sp.disc.is_new,
                    will_finalize: sp.disc.will_finalize,
                    files: sp.disc.files.iter().map(ScannedFileJson::of).collect(),
                    prior_used_bytes: sp.disc.prior_used_bytes,
                    usable_bytes: sp.disc.usable_bytes,
                },
                catch_up: sp.catch_up,
                session_ordinal: sp.session_ordinal,
                remaining: RemainingJson {
                    more_discs: sp.remaining.more_discs,
                    more_files: sp.remaining.more_files,
                    more_bytes: sp.remaining.more_bytes,
                },
            })
            .collect(),
        disc_type: report.disc_type.as_str().to_string(),
        tombstones: report
            .tombstones
            .iter()
            .map(|p| p.as_str().to_string())
            .collect(),
        oversized: report.oversized.iter().map(ScannedFileJson::of).collect(),
        unplaceable_catch_up: report
            .unplaceable_catch_up
            .iter()
            .map(ScannedFileJson::of)
            .collect(),
    };
    write_json(
        &plan_path(work_dir),
        &serde_json::to_string(&json).expect("serializable"),
    )
}

/// `Ok(None)` when nothing is staged; a decode failure — including an enum
/// spelling this build does not know — is a corrupt-file defect. Check
/// `is_legacy_plan` first: a v1 plan is not corrupt, just old.
pub fn read_plan(work_dir: &Path) -> Result<Option<PlanReport>, serde_json::Error> {
    let Some(raw) = read_opt(&plan_path(work_dir))? else {
        return Ok(None);
    };
    let j: PlanReportJson = serde_json::from_str(&raw)?;
    let files = |v: Vec<ScannedFileJson>| -> Result<Vec<ScannedFile>, serde_json::Error> {
        v.into_iter().map(ScannedFileJson::into_scanned).collect()
    };
    let mut sets = Vec::with_capacity(j.sets.len());
    for s in j.sets {
        sets.push(SetPlan {
            disc: PlannedDisc {
                ordinal: s.disc.ordinal,
                copy_set: s.disc.copy_set,
                is_new: s.disc.is_new,
                will_finalize: s.disc.will_finalize,
                files: files(s.disc.files)?,
                prior_used_bytes: s.disc.prior_used_bytes,
                usable_bytes: s.disc.usable_bytes,
            },
            catch_up: s.catch_up,
            session_ordinal: s.session_ordinal,
            remaining: RemainingEstimate {
                more_discs: s.remaining.more_discs,
                more_files: s.remaining.more_files,
                more_bytes: s.remaining.more_bytes,
            },
        });
    }
    Ok(Some(PlanReport {
        copies: j.copies,
        sets,
        disc_type: DiscType::from_str(&j.disc_type).map_err(decode_error)?,
        tombstones: j.tombstones.into_iter().map(RelPath).collect(),
        oversized: files(j.oversized)?,
        unplaceable_catch_up: files(j.unplaceable_catch_up)?,
    }))
}

/// True when the staged plan is a v1 plan — a single top-level `disc` — from a
/// build that predates copy sets. Anything unreadable is not "legacy";
/// `read_plan` reports it.
pub fn is_legacy_plan(work_dir: &Path) -> bool {
    std::fs::read_to_string(plan_path(work_dir))
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .is_some_and(|v| v.get("disc").is_some() && v.get("sets").is_none())
}

/// Discard the `staged/` directory: both staged files, any temp file an
/// interrupted `write_json` left, and anything else in it
/// (`StagedStoreLive.clear`). A missing directory is not an error.
///
/// `staged/` belongs to diskrement alone, so a stray file there (an editor's
/// swap file, a hand-made backup) goes with the rest. The Scala build refused
/// a non-empty `staged/`; since #4 every `plan` clears it first, and refusing
/// would block every cycle until the operator found the file.
pub fn clear(work_dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(work_dir.join("staged")) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diskrement_core::ContentHash;

    fn sf(rel: &str) -> ScannedFile {
        ScannedFile {
            rel_path: RelPath(rel.into()),
            content_hash: ContentHash("deadbeef".into()),
            size_bytes: 10,
            mtime: crate::catalog::time::parse("2026-06-01T00:00:00Z").unwrap(),
        }
    }

    #[test]
    fn round_trips_a_staged_scan() {
        let dir = tempfile::TempDir::new().unwrap();
        let files = vec![sf("a.txt"), sf("sub/b.txt")];
        write_scan(dir.path(), &files).unwrap();
        assert_eq!(read_scan(dir.path()).unwrap().unwrap(), files);
    }

    #[test]
    fn read_returns_none_when_nothing_is_staged() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(read_scan(dir.path()).unwrap().is_none());
    }

    #[test]
    fn a_corrupt_staged_scan_is_a_decode_error() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("staged")).unwrap();
        std::fs::write(dir.path().join("staged/scan.json"), "{not json").unwrap();
        let err = read_scan(dir.path()).unwrap_err();
        // The category (syntax/data/eof) depends on where the parse dies;
        // what matters is that it is a decode failure, not an I/O one.
        assert!(!err.is_io(), "{err}");
    }

    #[test]
    fn the_json_field_spellings_match_the_scala_codec() {
        let dir = tempfile::TempDir::new().unwrap();
        write_scan(dir.path(), &[sf("a.txt")]).unwrap();
        let raw = std::fs::read_to_string(dir.path().join("staged/scan.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let e = &v[0];
        assert_eq!(e["relPath"], "a.txt");
        assert_eq!(e["contentHash"], "deadbeef");
        assert_eq!(e["sizeBytes"], 10);
        assert_eq!(e["mtime"], "2026-06-01T00:00:00Z");
    }

    fn plan_report() -> PlanReport {
        PlanReport {
            copies: 1,
            sets: vec![SetPlan {
                disc: PlannedDisc {
                    ordinal: 1,
                    copy_set: 0,
                    is_new: true,
                    will_finalize: false,
                    files: vec![sf("a.txt"), sf("sub/b.txt")],
                    prior_used_bytes: 0,
                    usable_bytes: 1_000,
                },
                catch_up: 0,
                session_ordinal: 1,
                remaining: RemainingEstimate {
                    more_discs: 1,
                    more_files: 2,
                    more_bytes: 30,
                },
            }],
            disc_type: DiscType::BdRDl,
            tombstones: vec![RelPath("c".into())],
            oversized: vec![sf("huge.bin")],
            unplaceable_catch_up: vec![sf("big/copy.bin")],
        }
    }

    #[test]
    fn round_trips_a_staged_plan() {
        let dir = tempfile::TempDir::new().unwrap();
        let report = plan_report();
        write_plan(dir.path(), &report).unwrap();
        assert_eq!(read_plan(dir.path()).unwrap().unwrap(), report);
    }

    /// A plan staged before issue #12 has no `unplaceableCatchUp`; it reads
    /// as an empty list, and its catch-up files stay under `oversized`.
    #[test]
    fn a_plan_without_unplaceable_catch_up_reads_it_as_empty() {
        let dir = tempfile::TempDir::new().unwrap();
        write_plan(dir.path(), &plan_report()).unwrap();
        let path = dir.path().join("staged/plan.json");
        let mut v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        v.as_object_mut().unwrap().remove("unplaceableCatchUp");
        std::fs::write(&path, v.to_string()).unwrap();

        let read = read_plan(dir.path()).unwrap().unwrap();
        assert!(read.unplaceable_catch_up.is_empty());
        assert_eq!(read.oversized, plan_report().oversized);
    }

    #[test]
    fn pending_deletions_round_trip_and_survive_clearing_the_staged_files() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(read_pending_deletions(dir.path()).unwrap().is_empty());

        let paths = vec![RelPath("a/b.bin".into()), RelPath("c.bin".into())];
        write_pending_deletions(dir.path(), &paths).unwrap();
        write_scan(dir.path(), &[]).unwrap();
        clear(dir.path()).unwrap();

        assert_eq!(
            read_pending_deletions(dir.path()).unwrap(),
            paths.into_iter().collect::<BTreeSet<_>>()
        );
    }

    #[test]
    fn reading_a_plan_returns_none_when_nothing_is_staged() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(read_plan(dir.path()).unwrap().is_none());
    }

    #[test]
    fn a_corrupt_staged_plan_is_a_decode_error() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("staged")).unwrap();
        std::fs::write(dir.path().join("staged/plan.json"), "{not json").unwrap();
        assert!(!read_plan(dir.path()).unwrap_err().is_io());
    }

    #[test]
    fn an_unknown_disc_type_in_a_staged_plan_is_a_decode_error() {
        // The enum spellings are frozen (spec §3.1) and nothing else checks
        // them on this path: the catalog's `check` constraints never see
        // plan.json.
        let dir = tempfile::TempDir::new().unwrap();
        write_plan(dir.path(), &plan_report()).unwrap();
        let raw = std::fs::read_to_string(dir.path().join("staged/plan.json")).unwrap();
        std::fs::write(
            dir.path().join("staged/plan.json"),
            raw.replace("\"BD_R_DL\"", "\"ZIP\""),
        )
        .unwrap();
        let err = read_plan(dir.path()).unwrap_err();
        assert!(err.to_string().contains("unknown disc type: ZIP"), "{err}");
    }

    #[test]
    fn the_plan_json_field_spellings_match_the_scala_codec() {
        let dir = tempfile::TempDir::new().unwrap();
        write_plan(dir.path(), &plan_report()).unwrap();
        let raw = std::fs::read_to_string(dir.path().join("staged/plan.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["copies"], 1);
        let set = &v["sets"][0];
        assert_eq!(set["disc"]["ordinal"], 1);
        assert_eq!(set["disc"]["copySet"], 0);
        assert_eq!(set["disc"]["isNew"], true);
        assert_eq!(set["disc"]["willFinalize"], false);
        assert_eq!(set["disc"]["priorUsedBytes"], 0);
        assert_eq!(set["disc"]["usableBytes"], 1000);
        assert_eq!(set["disc"]["files"][0]["relPath"], "a.txt");
        assert_eq!(set["catchUp"], 0);
        assert_eq!(set["sessionOrdinal"], 1);
        assert_eq!(v["discType"], "BD_R_DL");
        assert_eq!(v["tombstones"][0], "c");
        assert_eq!(set["remaining"]["moreDiscs"], 1);
        assert_eq!(set["remaining"]["moreFiles"], 2);
        assert_eq!(set["remaining"]["moreBytes"], 30);
        assert_eq!(v["oversized"][0]["relPath"], "huge.bin");
        assert_eq!(v["unplaceableCatchUp"][0]["relPath"], "big/copy.bin");
    }

    #[test]
    fn clear_removes_both_staged_files_and_the_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        write_scan(dir.path(), &[sf("a.txt")]).unwrap();
        write_plan(dir.path(), &plan_report()).unwrap();
        clear(dir.path()).unwrap();
        assert!(read_scan(dir.path()).unwrap().is_none());
        assert!(read_plan(dir.path()).unwrap().is_none());
        assert!(!dir.path().join("staged").exists());
    }

    /// `staged/` belongs to diskrement alone, so anything else in it goes too.
    #[test]
    fn clear_removes_a_stray_file_with_the_rest() {
        let dir = tempfile::TempDir::new().unwrap();
        write_plan(dir.path(), &plan_report()).unwrap();
        std::fs::write(dir.path().join("staged/plan.json.bak"), b"x").unwrap();
        clear(dir.path()).unwrap();
        assert!(!dir.path().join("staged").exists());
    }

    #[test]
    fn clearing_nothing_is_not_an_error() {
        // `deleteIfExists` semantics: reset on a project that never staged
        // anything must succeed.
        let dir = tempfile::TempDir::new().unwrap();
        clear(dir.path()).unwrap();
    }

    #[test]
    fn clear_removes_the_plan_even_when_no_scan_was_staged() {
        let dir = tempfile::TempDir::new().unwrap();
        write_plan(dir.path(), &plan_report()).unwrap();
        clear(dir.path()).unwrap();
        assert!(read_plan(dir.path()).unwrap().is_none());
    }

    #[test]
    fn a_two_set_plan_round_trips() {
        let dir = tempfile::TempDir::new().unwrap();
        let disc = |set| PlannedDisc {
            ordinal: 1,
            copy_set: set,
            is_new: true,
            will_finalize: false,
            files: vec![],
            prior_used_bytes: 0,
            usable_bytes: 100,
        };
        let remaining = RemainingEstimate {
            more_discs: 0,
            more_files: 0,
            more_bytes: 0,
        };
        let report = PlanReport {
            copies: 2,
            sets: vec![
                SetPlan {
                    disc: disc(0),
                    catch_up: 0,
                    session_ordinal: 1,
                    remaining: remaining.clone(),
                },
                SetPlan {
                    disc: disc(1),
                    catch_up: 3,
                    session_ordinal: 1,
                    remaining,
                },
            ],
            disc_type: DiscType::BdR,
            tombstones: vec![RelPath("gone".into())],
            oversized: vec![],
            unplaceable_catch_up: vec![],
        };
        write_plan(dir.path(), &report).unwrap();
        assert_eq!(read_plan(dir.path()).unwrap(), Some(report));
        assert!(!is_legacy_plan(dir.path()));
    }

    #[test]
    fn a_v1_plan_is_recognised_as_legacy() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("staged")).unwrap();
        std::fs::write(
            plan_path(dir.path()),
            r#"{"disc":{"ordinal":1,"isNew":true,"willFinalize":false,"files":[],
                "priorUsedBytes":0,"usableBytes":100},"discType":"BD_R","tombstones":[],
                "remaining":{"moreDiscs":0,"moreFiles":0,"moreBytes":0},"oversized":[]}"#,
        )
        .unwrap();
        assert!(is_legacy_plan(dir.path()));
    }

    #[test]
    fn a_staged_write_replaces_the_file_whole_and_leaves_no_temp_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = plan_path(dir.path());
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "x".repeat(4096)).unwrap();
        use std::os::unix::fs::MetadataExt as _;
        let before = std::fs::metadata(&target).unwrap().ino();

        write_json(&target, "{}").unwrap();

        // A new inode: the file was renamed into place, not truncated and
        // rewritten, so a crash mid-write can never leave half a plan.
        assert_ne!(std::fs::metadata(&target).unwrap().ino(), before);

        assert_eq!(std::fs::read_to_string(&target).unwrap(), "{}");
        let names: Vec<String> = std::fs::read_dir(target.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            vec!["plan.json".to_string()],
            "no temp file left behind"
        );
    }

    #[test]
    fn clear_also_removes_a_temp_file_a_crash_left_behind() {
        let dir = tempfile::TempDir::new().unwrap();
        write_plan(dir.path(), &plan_report()).unwrap();
        std::fs::write(dir.path().join("staged/plan.json.tmp"), "{\"half").unwrap();
        std::fs::write(dir.path().join("staged/scan.json.tmp"), "[").unwrap();
        clear(dir.path()).unwrap();
        assert!(!dir.path().join("staged").exists());
    }
}
