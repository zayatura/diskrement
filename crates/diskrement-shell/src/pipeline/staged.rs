//! The staged scan and plan (`StagedStore.scala`): `<work>/staged/scan.json`
//! is the hand-off from `scan` to `plan` and the recovery unit's first
//! artifact; `<work>/staged/plan.json` is the recovery unit itself — `plan`
//! stages it, `burn` consumes it. Field spellings follow the Scala zio-json
//! codec's camelCase. The plan is v2 since copy sets (one entry per set); a
//! v1 plan is detected by `is_legacy_plan` and answered with a stale-plan
//! error.

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
}

/// `<workDir>/staged/scan.json`.
pub fn scan_path(work_dir: &Path) -> PathBuf {
    work_dir.join("staged").join("scan.json")
}

/// `<workDir>/staged/plan.json`.
pub fn plan_path(work_dir: &Path) -> PathBuf {
    work_dir.join("staged").join("plan.json")
}

/// Compact JSON, like zio-json's `.toJson`.
fn write_json(file: &Path, json: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(file.parent().expect("staged has a parent"))?;
    let mut out = std::fs::File::create(file)?;
    out.write_all(json.as_bytes())
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

/// Discard both staged files and the `staged/` directory
/// (`StagedStoreLive.clear`). Absent files are not an error.
///
/// A non-empty `staged/` *is* an error, and deliberately so: the Scala
/// `Files.deleteIfExists(stagedDir)` threw `DirectoryNotEmptyException` in
/// that case, and something unexpected living in the recovery unit is worth
/// surfacing rather than stepping around.
pub fn clear(work_dir: &Path) -> std::io::Result<()> {
    remove_if_present(&scan_path(work_dir))?;
    remove_if_present(&plan_path(work_dir))?;
    match std::fs::remove_dir(work_dir.join("staged")) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn remove_if_present(file: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(file) {
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
        }
    }

    #[test]
    fn round_trips_a_staged_plan() {
        let dir = tempfile::TempDir::new().unwrap();
        let report = plan_report();
        write_plan(dir.path(), &report).unwrap();
        assert_eq!(read_plan(dir.path()).unwrap().unwrap(), report);
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
}
