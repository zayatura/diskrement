# Rust Rewrite P4 — planner wiring, the staged plan, and the `plan` command Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land P4 — the plan half of the staged store (`staged/plan.json`),
the pure planning function that wires `Diff` + `Planner` into a one-disc-per-cycle
`PlanReport`, the `plan` command including `--finalize`, the oversize surface,
and the staged-state error contract (`NoStagedScan`, `NoStagedPlan`,
`OversizedFiles`, `StalePlan`) — ending with a Rust `plan` whose staged JSON is
semantically identical to the corpus's and whose stdout matches the render
oracle byte for byte.

**Architecture:** Spec §5.5. `Pipeline::plan` is the thin edge — resolve the
project, read the staged scan, read the catalog view and disc states, resolve
capacity, write `staged/plan.json`. Everything that decides anything lives in a
new pure module `pipeline::planning`: `resolve_disc_bytes`,
`effective_capacity`, and `plan_from`, none of which touch the filesystem or the
catalog. That split is what lets the fixture oracle re-plan the harvested
scenario directly from the corpus, with no env var and no temp catalog games.

**Tech Stack:** Rust 2021; `serde`, `serde_json`, `rusqlite` (bundled),
`chrono`, `clap` 4 derive, `tempfile`, `assert_cmd`, `predicates` — all already
workspace dependencies. **No new dependencies.**

**Spec:** `docs/superpowers/specs/2026-09-05-rust-rewrite-design.md` — §3.1
(staged JSON is a frozen format), §3.2 (CLI surface and exit codes), §4 (the
corpus and the render oracle), §5.5 (pipeline/errors subsystems), §8.1 (the
differential task every phase from P3 carries), §9 (the P4 row). The Scala
sources are the behavioral oracle: **read the file named in each task before
porting it.**

---

## Global Constraints

- **Branch:** `feat/rewrite-p4`, forked from `feat/rust-rewrite`. Never commit
  directly to `feat/rust-rewrite`. Merge back at the end with an explicit
  `Merge feat/rewrite-p4: <summary>` commit and delete the phase branch.

```bash
git switch feat/rust-rewrite && git switch -c feat/rewrite-p4
```

- **Do not touch** `core/`, `shell/`, `cli/`, `build.sbt`, `project/`,
  `fixtures/`, `scripts/harvest-fixtures.sh`. The Scala tree must stay
  buildable until P10, and the corpus is a frozen, spec-level artifact.
- **After every task:**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

  All clean, with no drop in test count. Baseline at plan start:
  **220 passed, 3 ignored** (the three `#[ignore]`d differential entry points).
- **Commit scopes:** `feat(pipeline):`, `test(pipeline):`, `feat(cli):`,
  `feat(difftest):`.
- **User-visible strings and exit codes are contract** (§3.2). Copy them
  verbatim from the Scala source named in each task; the only sanctioned
  divergence is clap's own usage/argument-error wording.
- **Time formatting only through `diskrement_shell::catalog::time`**
  (`format`/`parse`). Never `chrono`'s serde or `Display` for stored values.
- **`plan` writes no catalog rows.** Like `scan`, it reads the view and the
  disc states and stages JSON. The catalog is append-only and is written only
  by `burn` (P5).
- **`plan` does not consume the staged scan.** `scan.json` survives; `burn`
  clears staged state after a successful write. Re-running `plan` re-plans from
  the same staged scan and overwrites `plan.json`.
- **`DISKREMENT_DISC_BYTES` may only shrink usable capacity** and stays
  undocumented in `--help` (CLAUDE.md). A value ≥ the margin-adjusted capacity
  is ignored, so it can never plan past the safety margin.
- **`StalePlan` advice is always "re-run plan", never "run reset"** — `reset`
  destroys the staging that the staged-disc verifier needs. The port pins the
  negative assertion, exactly as `ExitMappingSpec` does.
- **Appending `plan` to `runner::RUST_IMPLEMENTED` happens in the same commit
  that lands it in the binary** (Task 10). Otherwise the Rust and interleaved
  differential modes keep skipping every scenario that uses it, and a
  fully-skipped run still exits green.

## Scala oracle files (read before porting each area)

| area | Scala source | Scala spec |
|---|---|---|
| staged plan JSON | `shell/src/main/scala/diskrement/shell/pipeline/StagedStore.scala` | `StagedStoreSpec.scala` |
| plan orchestration | `.../pipeline/PipelineLive.scala` (`def plan`) | `PipelineSpillSpec.scala`, `PipelineCapacityOverrideSpec.scala` |
| report types | `.../pipeline/Pipeline.scala` (`PlanReport`, `RemainingEstimate`) | — |
| errors + exit codes | `.../pipeline/PipelineError.scala` | `cli/src/test/scala/diskrement/cli/ExitMappingSpec.scala` |
| planner | `core/src/main/scala/diskrement/core/Planner.scala` (**already ported in P1** — read it only to check the wiring above it) | `PlannerTest.scala`, `PlannerEdgeTest.scala` |
| args | `cli/src/main/scala/diskrement/cli/Commands.scala` (the `plan` block) | `CommandsSpec.scala` |
| render | `cli/src/main/scala/diskrement/cli/Render.scala` (`def plan`) | `RenderSpec.scala` |
| dispatch | `cli/src/main/scala/diskrement/cli/Runner.scala` (the `Plan` case) | `CliSmokeSpec.scala` |

## Corpus oracles used by this phase

| artifact | what it pins |
|---|---|
| `fixtures/work/01-after-plan1/staged/{scan,plan}.json` | cycle 1: a spill across two discs, first disc sealed, `remaining = {1,1,800}` |
| `fixtures/work/02-after-plan3/staged/{scan,plan}.json` | cycle 2: a modify + delete, and the **empty-open-disc skip** (disc 2 is OPEN with 200 free, the 800-byte file cannot top it off, so the cycle plans a new disc 3) |
| `fixtures/catalog/02-after-scan1.sqlite` | the catalog state cycle 1 planned against (no discs) |
| `fixtures/catalog/06-after-scan3.sqlite` | the catalog state cycle 2 planned against (disc 1 FINALIZED, disc 2 OPEN with `b.bin`) |
| `fixtures/run-manifest.txt` steps 03, 10, 18 | the exact stdout of `plan`, `plan` with a deletion, and `plan --finalize` |

The harvest ran with `DISKREMENT_DISC_BYTES=1000` and the simulated burner
(`fixtures/README.md`).

## File Structure

| file | responsibility |
|---|---|
| `crates/diskrement-shell/src/pipeline/reports.rs` (modify) | add `RemainingEstimate` and `PlanReport` (data only) |
| `crates/diskrement-shell/src/pipeline/staged.rs` (modify) | share one `ScannedFileJson`; add `plan.json` write/read and `clear` |
| `crates/diskrement-shell/src/pipeline/planning.rs` (create) | the pure half of `plan`: capacity resolution and `plan_from` |
| `crates/diskrement-shell/src/pipeline/mod.rs` (modify) | declare `planning`; add `Pipeline::plan` |
| `crates/diskrement-shell/src/error.rs` (modify) | the staged-state error variants and their exit codes |
| `crates/diskrement-shell/tests/fixture_plan.rs` (create) | re-plan the harvested cycles and diff against the Scala-staged plans |
| `crates/diskrement-cli/src/render.rs` (modify) | `plan_report`, verbatim from `Render.plan` |
| `crates/diskrement-cli/src/args.rs` (modify) | the `plan` subcommand |
| `crates/diskrement-cli/src/main.rs` (modify) | dispatch `plan` |
| `crates/diskrement-cli/tests/binary.rs` (modify) | end-to-end `plan` scenarios through the spawned binary |
| `crates/diskrement-difftest/src/corpus.rs` (modify) | four `plan` scenarios |
| `crates/diskrement-difftest/src/runner.rs` (modify) | one line: `plan` joins `RUST_IMPLEMENTED` |

---

### Task 1: The plan report types and the staged plan codec

**Files:**
- Modify: `crates/diskrement-shell/src/pipeline/reports.rs`
- Modify: `crates/diskrement-shell/src/pipeline/staged.rs`
- Test: both files' `#[cfg(test)] mod tests`

**Interfaces:**
- Produces: `PlanReport`, `RemainingEstimate` in
  `diskrement_shell::pipeline::reports`; `staged::write_plan`,
  `staged::read_plan`, `staged::plan_path`. Task 4 builds a `PlanReport`,
  Task 5 stages it, Task 6 compares one against the corpus.

Read `shell/src/main/scala/diskrement/shell/pipeline/StagedStore.scala` first —
`PlannedDiscJson`, `RemainingJson` and `PlanReportJson` are the frozen field
spellings, and `Pipeline.scala`'s `PlanReport` / `RemainingEstimate` are the
shapes.

- [ ] **Step 1: Write the failing tests**

Append to `crates/diskrement-shell/src/pipeline/staged.rs`'s `mod tests`:

```rust
    fn plan_report() -> PlanReport {
        PlanReport {
            disc: PlannedDisc {
                ordinal: 1,
                is_new: true,
                will_finalize: false,
                files: vec![sf("a.txt"), sf("sub/b.txt")],
                prior_used_bytes: 0,
                usable_bytes: 1_000,
            },
            disc_type: DiscType::BdRDl,
            tombstones: vec![RelPath("c".into())],
            remaining: RemainingEstimate {
                more_discs: 1,
                more_files: 2,
                more_bytes: 30,
            },
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
        assert_eq!(v["disc"]["ordinal"], 1);
        assert_eq!(v["disc"]["isNew"], true);
        assert_eq!(v["disc"]["willFinalize"], false);
        assert_eq!(v["disc"]["priorUsedBytes"], 0);
        assert_eq!(v["disc"]["usableBytes"], 1000);
        assert_eq!(v["disc"]["files"][0]["relPath"], "a.txt");
        assert_eq!(v["discType"], "BD_R_DL");
        assert_eq!(v["tombstones"][0], "c");
        assert_eq!(v["remaining"]["moreDiscs"], 1);
        assert_eq!(v["remaining"]["moreFiles"], 2);
        assert_eq!(v["remaining"]["moreBytes"], 30);
        assert_eq!(v["oversized"][0]["relPath"], "huge.bin");
    }
```

The test module needs `PlanReport`, `RemainingEstimate`, `PlannedDisc` and
`DiscType` in scope. Add no imports here: Step 4 adds all four to the module's
own `use` lines, and the existing `use super::*;` in `mod tests` picks them up.
Until then the module does not compile — which is exactly what Step 2 checks.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p diskrement-shell staged`
Expected: FAIL — `cannot find function write_plan in this scope` (and
`unresolved import ...reports::PlanReport`).

- [ ] **Step 3: Add the report types**

Append to `crates/diskrement-shell/src/pipeline/reports.rs`:

```rust
use diskrement_core::{DiscType, PlannedDisc, RelPath, ScannedFile};

/// How much outstanding change did NOT fit on the disc planned this cycle
/// (`Pipeline.scala`'s `RemainingEstimate`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemainingEstimate {
    pub more_discs: usize,
    pub more_files: usize,
    pub more_bytes: u64,
}

/// The single disc this cycle will write, plus what spills to later cycles
/// (`Pipeline.scala`'s `PlanReport`).
///
/// `tombstones` are the whole scan's deletions rather than this disc's: on a
/// cross-cycle spill they attach to the session actually written this cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanReport {
    pub disc: PlannedDisc,
    pub disc_type: DiscType,
    pub tombstones: Vec<RelPath>,
    pub remaining: RemainingEstimate,
    pub oversized: Vec<ScannedFile>,
}
```

The existing `use diskrement_core::DiscState;` line stays; merge the two
`use diskrement_core::...` lines into one if `cargo fmt` prefers it.

- [ ] **Step 4: Share one scanned-file codec and add the plan codec**

In `crates/diskrement-shell/src/pipeline/staged.rs`, replace the
`ScanJsonEntry` struct and the bodies of `write_scan`/`read_scan` with the
shared form below. Keep the module doc comment; extend it with the plan half.

```rust
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use diskrement_core::{ContentHash, DiscType, PlannedDisc, RelPath, ScannedFile};
use serde::{Deserialize, Serialize};

use crate::catalog::time;
use crate::pipeline::reports::{PlanReport, RemainingEstimate};

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
struct RemainingJson {
    #[serde(rename = "moreDiscs")]
    more_discs: usize,
    #[serde(rename = "moreFiles")]
    more_files: usize,
    #[serde(rename = "moreBytes")]
    more_bytes: u64,
}

/// Field order follows the Scala case class, so a hand-diff of two plan.json
/// files lines up. Order is not an oracle — the corpus rules compare parsed
/// values (spec §4) — but matching it costs nothing.
#[derive(Serialize, Deserialize)]
struct PlanReportJson {
    disc: PlannedDiscJson,
    #[serde(rename = "discType")]
    disc_type: String,
    tombstones: Vec<String>,
    remaining: RemainingJson,
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
        disc: PlannedDiscJson {
            ordinal: report.disc.ordinal,
            is_new: report.disc.is_new,
            will_finalize: report.disc.will_finalize,
            files: report.disc.files.iter().map(ScannedFileJson::of).collect(),
            prior_used_bytes: report.disc.prior_used_bytes,
            usable_bytes: report.disc.usable_bytes,
        },
        disc_type: report.disc_type.as_str().to_string(),
        tombstones: report
            .tombstones
            .iter()
            .map(|p| p.as_str().to_string())
            .collect(),
        remaining: RemainingJson {
            more_discs: report.remaining.more_discs,
            more_files: report.remaining.more_files,
            more_bytes: report.remaining.more_bytes,
        },
        oversized: report.oversized.iter().map(ScannedFileJson::of).collect(),
    };
    write_json(
        &plan_path(work_dir),
        &serde_json::to_string(&json).expect("serializable"),
    )
}

/// `Ok(None)` when nothing is staged; a decode failure — including an enum
/// spelling this build does not know — is a corrupt-file defect.
pub fn read_plan(work_dir: &Path) -> Result<Option<PlanReport>, serde_json::Error> {
    let Some(raw) = read_opt(&plan_path(work_dir))? else {
        return Ok(None);
    };
    let j: PlanReportJson = serde_json::from_str(&raw)?;
    let mut files = Vec::with_capacity(j.disc.files.len());
    for f in j.disc.files {
        files.push(f.into_scanned()?);
    }
    let mut oversized = Vec::with_capacity(j.oversized.len());
    for f in j.oversized {
        oversized.push(f.into_scanned()?);
    }
    Ok(Some(PlanReport {
        disc: PlannedDisc {
            ordinal: j.disc.ordinal,
            is_new: j.disc.is_new,
            will_finalize: j.disc.will_finalize,
            files,
            prior_used_bytes: j.disc.prior_used_bytes,
            usable_bytes: j.disc.usable_bytes,
        },
        disc_type: DiscType::from_str(&j.disc_type).map_err(decode_error)?,
        tombstones: j.tombstones.into_iter().map(RelPath).collect(),
        remaining: RemainingEstimate {
            more_discs: j.remaining.more_discs,
            more_files: j.remaining.more_files,
            more_bytes: j.remaining.more_bytes,
        },
        oversized,
    }))
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p diskrement-shell staged`
Expected: PASS — the four pre-existing scan tests plus the five new plan tests.

- [ ] **Step 6: Run the verification bar**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
```

Expected: all clean; 225 passed.

- [ ] **Step 7: Commit**

```bash
git add crates/diskrement-shell/src/pipeline/reports.rs \
        crates/diskrement-shell/src/pipeline/staged.rs
git commit -m "feat(pipeline): the staged plan report and its frozen JSON codec"
```

---

### Task 2: `staged::clear`

**Files:**
- Modify: `crates/diskrement-shell/src/pipeline/staged.rs`
- Test: same file's `mod tests`

**Interfaces:**
- Produces: `staged::clear(work_dir) -> std::io::Result<()>`. Nothing in P4
  calls it; `burn` (P5) and `reset` (P6) do. It lands here because it is part
  of the staged store, and because the round-trip tests that prove it are
  cheapest next to the codec they undo.

Read `StagedStore.scala`'s `def clear`: it deletes `scan.json`, `plan.json`,
then the `staged/` directory itself, each with `deleteIfExists`.

- [ ] **Step 1: Write the failing test**

Append to `mod tests` in `crates/diskrement-shell/src/pipeline/staged.rs`:

```rust
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
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p diskrement-shell staged::tests::clear`
Expected: FAIL with `cannot find function clear in this scope`.

- [ ] **Step 3: Implement `clear`**

Append to `crates/diskrement-shell/src/pipeline/staged.rs` (above `mod tests`):

```rust
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p diskrement-shell staged`
Expected: PASS — 12 tests in the module.

- [ ] **Step 5: Run the verification bar**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
```

Expected: all clean; 228 passed.

- [ ] **Step 6: Commit**

```bash
git add crates/diskrement-shell/src/pipeline/staged.rs
git commit -m "feat(pipeline): clear the staged scan and plan"
```

---

### Task 3: The staged-state errors and their exit codes

**Files:**
- Modify: `crates/diskrement-shell/src/error.rs`
- Test: same file's `mod tests`

**Interfaces:**
- Produces: `PipelineError::{NoStagedScan, NoStagedPlan, OversizedFiles,
  StalePlan}` and their `exit_code()` mapping. Task 5 constructs
  `NoStagedScan`; the other three are constructed by `burn` in P5 and are
  pinned here because the exit ladder and the `StalePlan` wording are the
  contract this phase owns (spec §9, P4 row).

Read `PipelineError.scala` for the messages and
`cli/src/test/scala/diskrement/cli/ExitMappingSpec.scala` for the codes.

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `crates/diskrement-shell/src/error.rs`:

```rust
    #[test]
    fn staged_state_errors_are_exit_2_and_oversize_is_exit_3() {
        assert_eq!(PipelineError::NoStagedScan("p".into()).exit_code(), 2);
        assert_eq!(PipelineError::NoStagedPlan("p".into()).exit_code(), 2);
        assert_eq!(PipelineError::StalePlan("d".into()).exit_code(), 2);
        assert_eq!(
            PipelineError::OversizedFiles(vec![RelPath("huge.bin".into())]).exit_code(),
            3
        );
    }

    #[test]
    fn the_staged_state_messages_match_the_scala_render_verbatim() {
        assert_eq!(
            PipelineError::NoStagedScan("photos".into()).to_string(),
            "no staged scan for photos (run scan)"
        );
        assert_eq!(
            PipelineError::NoStagedPlan("photos".into()).to_string(),
            "no staged plan for photos (run plan)"
        );
        assert_eq!(
            PipelineError::OversizedFiles(vec![
                RelPath("huge.bin".into()),
                RelPath("also/huge.bin".into())
            ])
            .to_string(),
            "oversized files cannot be placed: huge.bin, also/huge.bin"
        );
    }

    /// The one assertion `ExitMappingSpec` makes negatively: `reset` deletes
    /// `workDir/staging`, which is the staged-disc verifier's evidence, so the
    /// advertised recovery from a stale plan is `plan` and only `plan`.
    #[test]
    fn stale_plan_advises_re_running_plan_and_never_mentions_reset() {
        let msg = PipelineError::StalePlan(
            "disc 3 was marked dead after this plan was staged".into(),
        )
        .to_string();
        assert_eq!(
            msg,
            "staged plan is stale: disc 3 was marked dead after this plan was \
             staged (re-run plan)"
        );
        assert!(msg.contains("re-run plan"), "{msg}");
        assert!(!msg.contains("reset"), "{msg}");
    }
```

and add to that module's imports:

```rust
    use diskrement_core::RelPath;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p diskrement-shell error`
Expected: FAIL — `no variant named NoStagedScan found for enum PipelineError`.

- [ ] **Step 3: Add the variants**

In `crates/diskrement-shell/src/error.rs`, add to `enum PipelineError`, after
`ProjectNotFound`:

```rust
    #[error("no staged scan for {0} (run scan)")]
    NoStagedScan(String),
    #[error("no staged plan for {0} (run plan)")]
    NoStagedPlan(String),
    /// `plan` alone is the whole recovery: the staged scan and the snapshot
    /// both survive, and `plan` re-reads the view and the disc states. Never
    /// advise `reset` — it deletes `workDir/staging`, the staged-disc
    /// verifier's evidence.
    #[error("staged plan is stale: {0} (re-run plan)")]
    StalePlan(String),
    /// Surfaced by `plan` in its report and refused by `burn` (P5): the
    /// two-stage oversize check.
    #[error(
        "oversized files cannot be placed: {}",
        .0.iter().map(diskrement_core::RelPath::as_str).collect::<Vec<_>>().join(", ")
    )]
    OversizedFiles(Vec<diskrement_core::RelPath>),
```

and extend `exit_code`:

```rust
    pub fn exit_code(&self) -> i32 {
        match self {
            PipelineError::ProjectNotFound(_)
            | PipelineError::NoStagedScan(_)
            | PipelineError::NoStagedPlan(_)
            | PipelineError::StalePlan(_) => 2,
            PipelineError::NotSameFilesystem { .. }
            | PipelineError::NotCowCapable { .. }
            | PipelineError::OversizedFiles(_) => 3,
            PipelineError::Failed(_) => 5,
        }
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p diskrement-shell error`
Expected: PASS.

- [ ] **Step 5: Run the verification bar**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
```

Expected: all clean; 231 passed.

- [ ] **Step 6: Commit**

```bash
git add crates/diskrement-shell/src/error.rs
git commit -m "feat(pipeline): the staged-state errors and their exit codes"
```

---

### Task 4: The pure planning function

**Files:**
- Create: `crates/diskrement-shell/src/pipeline/planning.rs`
- Modify: `crates/diskrement-shell/src/pipeline/mod.rs` (add `pub mod planning;`)
- Test: `crates/diskrement-shell/src/pipeline/planning.rs`'s `mod tests`

**Interfaces:**
- Produces: `planning::resolve_disc_bytes`, `planning::disc_bytes_from_env`,
  `planning::effective_capacity`, `planning::plan_from`. Task 5 calls all four;
  Task 6 calls `effective_capacity` and `plan_from` against the corpus.

Read `PipelineLive.scala`'s `def plan` before writing this: everything from
`capBytes` down to `report` is what moves here, unchanged in behavior.

- [ ] **Step 1: Write the failing tests**

Create `crates/diskrement-shell/src/pipeline/planning.rs` containing only the
module doc and the tests, so the first run fails for the right reason:

```rust
//! The pure half of `plan` (`PipelineLive.plan`): everything between reading
//! the staged scan and writing `staged/plan.json`.
//!
//! No filesystem, no catalog, no clock. `Pipeline::plan` is the thin edge
//! around this — which is what lets the fixture oracle re-plan the harvested
//! scenario straight out of `fixtures/`, with no environment variable and no
//! temporary catalog.

#[cfg(test)]
mod tests {
    use super::*;
    // `CatalogView::from_scanned` is core's own helper for "the view that
    // results from recording exactly these files", which is what a
    // no-change or partial-change scenario needs here.
    use diskrement_core::{
        Capacity, CatalogView, ContentHash, DiscState, DiscStatus, DiscType, RelPath, ScannedFile,
    };

    fn ts() -> chrono::DateTime<chrono::Utc> {
        crate::catalog::time::parse("2026-06-01T00:00:00Z").unwrap()
    }

    fn file(rel: &str, size: u64) -> ScannedFile {
        ScannedFile {
            rel_path: RelPath(rel.into()),
            // The hash only has to be stable and distinct per (path, size).
            content_hash: ContentHash(format!("{rel}-{size}")),
            size_bytes: size,
            mtime: ts(),
        }
    }

    #[test]
    fn an_unset_override_leaves_the_margin_adjusted_capacity_alone() {
        let normal = Capacity::from_disc_type(DiscType::BdR, 2.0);
        assert_eq!(effective_capacity(normal, None), normal);
    }

    #[test]
    fn the_override_only_shrinks() {
        let normal = Capacity::from_disc_type(DiscType::BdR, 2.0);
        // Below the margin-adjusted capacity: honoured.
        assert_eq!(
            effective_capacity(normal, Some(1_000)),
            Capacity::new(1_000)
        );
        // At or above it: ignored, so the override can never plan past the
        // safety margin (CLAUDE.md).
        assert_eq!(effective_capacity(normal, Some(normal.usable_bytes)), normal);
        assert_eq!(effective_capacity(normal, Some(999_999_999_999)), normal);
    }

    #[test]
    fn only_a_positive_integer_is_a_usable_override() {
        // `toLongOption.filter(_ > 0)`: everything else is unset, silently.
        assert_eq!(resolve_disc_bytes(Some("1000".into())), Some(1000));
        assert_eq!(resolve_disc_bytes(None), None);
        assert_eq!(resolve_disc_bytes(Some("".into())), None);
        assert_eq!(resolve_disc_bytes(Some("0".into())), None);
        assert_eq!(resolve_disc_bytes(Some("-5".into())), None);
        assert_eq!(resolve_disc_bytes(Some("lots".into())), None);
        assert_eq!(resolve_disc_bytes(Some(" 1000 ".into())), None);
    }

    #[test]
    fn a_single_disc_plan_reports_nothing_remaining() {
        let files = vec![file("a.bin", 300), file("b.bin", 300)];
        let report = plan_from(
            &CatalogView::empty(),
            &files,
            &[],
            DiscType::BdR,
            Capacity::new(1_000),
            false,
        );
        assert_eq!(report.disc.ordinal, 1);
        assert!(report.disc.is_new);
        assert!(!report.disc.will_finalize);
        assert_eq!(report.disc.files.len(), 2);
        assert_eq!(
            report.remaining,
            RemainingEstimate {
                more_discs: 0,
                more_files: 0,
                more_bytes: 0
            }
        );
        assert!(report.oversized.is_empty());
        assert_eq!(report.disc_type, DiscType::BdR);
    }

    #[test]
    fn a_spill_seals_this_cycles_disc_and_reports_the_remainder() {
        let files = vec![file("a.bin", 800), file("b.bin", 800)];
        let report = plan_from(
            &CatalogView::empty(),
            &files,
            &[],
            DiscType::BdR,
            Capacity::new(1_000),
            false,
        );
        assert_eq!(report.disc.files.len(), 1);
        assert!(report.disc.will_finalize, "a spilled disc is sealed");
        assert_eq!(
            report.remaining,
            RemainingEstimate {
                more_discs: 1,
                more_files: 1,
                more_bytes: 800
            }
        );
    }

    #[test]
    fn finalize_seals_a_disc_that_would_have_been_left_open() {
        let files = vec![file("a.bin", 300)];
        let open = plan_from(
            &CatalogView::empty(),
            &files,
            &[],
            DiscType::BdR,
            Capacity::new(1_000),
            false,
        );
        let sealed = plan_from(
            &CatalogView::empty(),
            &files,
            &[],
            DiscType::BdR,
            Capacity::new(1_000),
            true,
        );
        assert!(!open.disc.will_finalize);
        assert!(sealed.disc.will_finalize);
    }

    #[test]
    fn finalize_of_an_unchanged_tree_seals_the_open_disc_with_no_files() {
        // Corpus step 18: `plan p --finalize` after a scan that found nothing.
        let files = vec![file("a.bin", 300)];
        let states = [DiscState {
            ordinal: 4,
            status: DiscStatus::Open,
            usable_bytes: 1_000,
            used_bytes: 300,
        }];
        let report = plan_from(
            &CatalogView::from_scanned(&files),
            &files,
            &states,
            DiscType::BdR,
            Capacity::new(1_000),
            true,
        );
        assert_eq!(report.disc.ordinal, 4);
        assert!(report.disc.files.is_empty());
        assert!(!report.disc.is_new);
        assert!(report.disc.will_finalize);
        assert_eq!(report.remaining.more_discs, 0);
    }

    #[test]
    fn a_file_that_cannot_top_off_the_open_disc_opens_a_fresh_one_this_cycle() {
        // The empty-open-disc skip: the planner keeps the open disc first, but
        // a cycle must not burn an empty finalizing placeholder. Corpus
        // fixture `work/02-after-plan3` is this case.
        let existing = file("a.bin", 800);
        let mut files = vec![existing.clone(), file("b.bin", 800)];
        files.sort_by(|x, y| x.rel_path.cmp(&y.rel_path));
        let states = [DiscState {
            ordinal: 2,
            status: DiscStatus::Open,
            usable_bytes: 1_000,
            used_bytes: 800,
        }];
        let report = plan_from(
            &CatalogView::from_scanned(&[existing]),
            &files,
            &states,
            DiscType::BdR,
            Capacity::new(1_000),
            false,
        );
        assert_eq!(report.disc.ordinal, 3, "a fresh disc, not the open one");
        assert!(report.disc.is_new);
        assert_eq!(report.disc.files.len(), 1);
        assert_eq!(report.disc.files[0].rel_path.as_str(), "b.bin");
        assert_eq!(report.remaining.more_discs, 0);
    }

    #[test]
    fn oversized_files_are_reported_and_the_rest_is_still_planned() {
        let files = vec![file("big.bin", 5_000), file("small.bin", 100)];
        let report = plan_from(
            &CatalogView::empty(),
            &files,
            &[],
            DiscType::BdR,
            Capacity::new(1_000),
            false,
        );
        assert_eq!(report.oversized.len(), 1);
        assert_eq!(report.oversized[0].rel_path.as_str(), "big.bin");
        assert_eq!(report.disc.files.len(), 1);
        assert_eq!(report.disc.files[0].rel_path.as_str(), "small.bin");
    }

    #[test]
    fn deletions_become_tombstones_on_this_cycles_disc() {
        let gone = file("gone.bin", 100);
        let report = plan_from(
            &CatalogView::from_scanned(&[gone]),
            &[file("a.bin", 100)],
            &[],
            DiscType::BdR,
            Capacity::new(1_000),
            false,
        );
        assert_eq!(report.tombstones.len(), 1);
        assert_eq!(report.tombstones[0].as_str(), "gone.bin");
    }

    #[test]
    fn a_no_op_scan_plans_an_empty_disc_rather_than_panicking() {
        // `position` finds nothing; the Scala fell back to index 0 and so does
        // this. `Planner::plan` always returns at least one disc, so indexing
        // is safe — this test is what keeps that assumption honest.
        let files = vec![file("a.bin", 300)];
        let report = plan_from(
            &CatalogView::from_scanned(&files),
            &files,
            &[],
            DiscType::BdR,
            Capacity::new(1_000),
            false,
        );
        assert!(report.disc.files.is_empty());
        assert!(report.tombstones.is_empty());
        assert_eq!(report.remaining.more_discs, 0);
    }
}
```

Add to `crates/diskrement-shell/src/pipeline/mod.rs`, next to the existing
`pub mod reports;` / `pub mod staged;`:

```rust
pub mod planning;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p diskrement-shell planning`
Expected: FAIL — `cannot find function effective_capacity in this scope` (and
the same for `resolve_disc_bytes`, `plan_from`, `RemainingEstimate`).

- [ ] **Step 3: Implement the module**

Insert above the `#[cfg(test)]` block in
`crates/diskrement-shell/src/pipeline/planning.rs`:

```rust
use diskrement_core::{diff, planner, Capacity, CatalogView, DiscState, DiscType, ScannedFile};

use crate::pipeline::reports::{PlanReport, RemainingEstimate};

/// `DISKREMENT_DISC_BYTES`, the test-only capacity override.
///
/// Deliberately undocumented in `--help` (CLAUDE.md). It makes multi-disc
/// scenarios cheap and deterministic, and it may only ever **shrink** usable
/// capacity, so it cannot plan a disc past the safety margin in production.
pub fn disc_bytes_from_env() -> Option<u64> {
    resolve_disc_bytes(std::env::var("DISKREMENT_DISC_BYTES").ok())
}

/// The pure half: `_.flatMap(_.toLongOption).filter(_ > 0L)`.
///
/// No trimming, matching `toLongOption` — `" 1000 "` is not a number, and a
/// value that is not a positive integer is silently treated as unset, with no
/// warning. That is deliberately unlike `DISKREMENT_HASH_PARALLELISM`, which
/// warns: this variable is a test hook rather than a documented knob.
pub fn resolve_disc_bytes(raw: Option<String>) -> Option<u64> {
    raw.and_then(|s| s.parse::<u64>().ok()).filter(|n| *n > 0)
}

/// Apply the shrink-only override to the margin-adjusted capacity.
pub fn effective_capacity(normal: Capacity, override_bytes: Option<u64>) -> Capacity {
    match override_bytes {
        Some(n) if n < normal.usable_bytes => Capacity::new(n),
        _ => normal,
    }
}

/// Diff, plan, and reduce the full multi-disc plan to the **one disc this
/// cycle writes** (`PipelineLive.plan`).
///
/// One disc per cycle: burn the first disc that actually carries new files.
/// The planner keeps the existing open disc first by contract; when a new file
/// cannot top it off, that open disc is an empty finalizing placeholder — skip
/// it so this cycle writes the new disc instead of wasting a cycle on an empty
/// session. The skipped disc stays open. When no disc has files (a no-op scan,
/// or an explicit `--finalize` of an empty open disc), keep the head as-is.
///
/// `tombstones` are the whole scan's deletions, not one disc's: on a
/// cross-cycle spill they attach to the session actually written this cycle.
pub fn plan_from(
    view: &CatalogView,
    scanned: &[ScannedFile],
    states: &[DiscState],
    disc_type: DiscType,
    cap: Capacity,
    finalize: bool,
) -> PlanReport {
    let changes = diff::diff(view, scanned);
    let full = planner::plan(&changes, states, cap);
    // `Planner::plan` always returns at least one disc, so both the index and
    // the `idx + 1` slice below are in range.
    let idx = full
        .discs
        .iter()
        .position(|d| !d.files.is_empty())
        .unwrap_or(0);
    let mut disc = full.discs[idx].clone();
    // The planner already seals this disc iff the plan spilled; `--finalize`
    // forces a seal even when everything fit.
    disc.will_finalize = disc.will_finalize || finalize;
    let tail = &full.discs[idx + 1..];
    let remaining = RemainingEstimate {
        more_discs: tail.len(),
        more_files: tail.iter().map(|d| d.files.len()).sum(),
        more_bytes: tail
            .iter()
            .flat_map(|d| d.files.iter())
            .map(|f| f.size_bytes)
            .sum(),
    };
    PlanReport {
        disc,
        disc_type,
        tombstones: full.tombstones,
        remaining,
        oversized: full.oversized,
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p diskrement-shell planning`
Expected: PASS — 11 tests.

If `a_file_that_cannot_top_off_the_open_disc_opens_a_fresh_one_this_cycle`
fails with ordinal 2 instead of 3, the bug is in the head-index selection, not
in the planner: `position` must skip the *empty* leading disc.

- [ ] **Step 5: Run the verification bar**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
```

Expected: all clean; 242 passed.

- [ ] **Step 6: Commit**

```bash
git add crates/diskrement-shell/src/pipeline/planning.rs \
        crates/diskrement-shell/src/pipeline/mod.rs
git commit -m "feat(pipeline): the pure planning function and the capacity override"
```

---

### Task 5: `Pipeline::plan`

**Files:**
- Modify: `crates/diskrement-shell/src/pipeline/mod.rs`
- Test: same file's `mod tests`

**Interfaces:**
- Produces: `Pipeline::plan(&self, name, disc_type_override: Option<DiscType>,
  finalize: bool) -> Result<PlanReport, PipelineError>`. Task 8 calls it from
  the binary.

Read `PipelineLive.scala`'s `def plan` again alongside this: the edge is the
project lookup, the staged-scan read, the two catalog reads, and the stage
write. Everything else went to `planning` in Task 4.

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `crates/diskrement-shell/src/pipeline/mod.rs`:

```rust
    /// A project whose margin leaves ~50 usable bytes, so tiny files spill and
    /// go oversized without touching `DISKREMENT_DISC_BYTES` — which is
    /// process-global and unusable from parallel tests.
    /// BD_R nominal 25_000_000_000 at margin 99.9999998% → 50 bytes.
    fn tiny_project_init(env: &Env) -> ProjectInit {
        ProjectInit {
            margin_percent: 99.9999998,
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
        assert!(!e.pipeline.plan("p", None, false).unwrap().disc.will_finalize);
        assert!(e.pipeline.plan("p", None, true).unwrap().disc.will_finalize);
    }

    #[test]
    fn a_spilling_plan_seals_this_cycles_disc_and_reports_the_rest() {
        let mut e = setup();
        write_src(&e, "a.txt", &[b'a'; 40]);
        write_src(&e, "b.txt", &[b'b'; 40]);
        e.pipeline.init(&tiny_project_init(&e)).unwrap();
        e.pipeline.scan("p", false, false).unwrap();
        let report = e.pipeline.plan("p", None, false).unwrap();
        assert_eq!(report.disc.files.len(), 1);
        assert!(report.disc.will_finalize);
        assert_eq!(report.remaining.more_discs, 1);
        assert_eq!(report.remaining.more_files, 1);
        assert_eq!(report.remaining.more_bytes, 40);
    }

    #[test]
    fn a_file_larger_than_any_disc_is_surfaced_as_oversized() {
        let mut e = setup();
        write_src(&e, "big.txt", &[b'x'; 100]);
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
        assert!(
            before.iter().all(|line| line.ends_with("=0")),
            "{before:?}"
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p diskrement-shell pipeline::tests`
Expected: FAIL — `no method named plan found for struct Pipeline`.

- [ ] **Step 3: Implement `Pipeline::plan`**

In `crates/diskrement-shell/src/pipeline/mod.rs`, extend the imports:

```rust
use diskrement_core::{diff, Capacity, DiscStatus, DiscType, RelPath, ScannedFile};
```

(add `Capacity` and `DiscType` to the existing `diskrement_core` import), and:

```rust
use reports::{PlanReport, ScanReport, StatusReport};
```

Then add the method to `impl<'a> Pipeline<'a>`, after `count` and before
`status`:

```rust
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p diskrement-shell pipeline`
Expected: PASS — the 11 P3 pipeline tests plus 10 new ones.

`burn_written_rows` reaches `pipeline.catalog`, a private field: that is legal
because `mod tests` is a child of the module that declares it. It returns an
owned `Vec<String>`, so nothing is borrowed across the `plan` call.

- [ ] **Step 5: Run the verification bar**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
```

Expected: all clean; 252 passed.

- [ ] **Step 6: Commit**

```bash
git add crates/diskrement-shell/src/pipeline/mod.rs
git commit -m "feat(pipeline): plan, with --finalize, oversize and the disc-bytes override"
```

---

### Task 6: The fixture plan oracle

**Files:**
- Create: `crates/diskrement-shell/tests/fixture_plan.rs`
- Oracle: `fixtures/work/01-after-plan1/`, `fixtures/work/02-after-plan3/`,
  `fixtures/catalog/02-after-scan1.sqlite`,
  `fixtures/catalog/06-after-scan3.sqlite`

**Interfaces:**
- Consumes: `planning::plan_from`, `planning::effective_capacity`,
  `staged::{read_scan, read_plan, write_plan}`, `Catalog::{read_view,
  read_disc_states, find_project}`, and `support::fixture_copy`.

This is the phase's exit-bar evidence: the staged plan the Rust planner
produces must be semantically identical to the one the Scala build staged at
the same moment. **Never open a fixture catalog with `Catalog::open`** — it
runs the migration runner and writes; `fixture_copy` exists for this.

- [ ] **Step 1: Write the failing test**

Create `crates/diskrement-shell/tests/fixture_plan.rs`:

```rust
//! Re-plan the harvested cycles from the corpus and diff the result against
//! the plan the Scala build staged at the same moment (spec §4, §9's P4 row).
//!
//! Every input comes out of `fixtures/`: the catalog state, the staged scan,
//! the project's disc type and margin, and the harvest's capacity override.
//! Nothing here is a value typed into this file and asserted against itself.

mod support;

use std::path::PathBuf;

use diskrement_shell::catalog::Catalog;
use diskrement_shell::pipeline::planning::{effective_capacity, plan_from};
use diskrement_shell::pipeline::reports::PlanReport;
use diskrement_shell::pipeline::staged;
use diskrement_core::Capacity;
use support::{fixture_copy, fixtures_dir};

/// `DISKREMENT_DISC_BYTES` for the harvest run (`fixtures/README.md`).
const HARVEST_DISC_BYTES: u64 = 1_000;

/// Re-plan one harvested moment. Returns (what Rust plans, what Scala staged).
fn replan(catalog_fixture: &str, work_fixture: &str) -> (PlanReport, PlanReport) {
    let (_tmp, catalog_path) = fixture_copy(catalog_fixture);
    let catalog = Catalog::open(&catalog_path).expect("the fixture catalog is already current");
    let project = catalog
        .find_project("p")
        .expect("readable")
        .expect("the harvest project is named p");
    let work: PathBuf = fixtures_dir().join(work_fixture);

    let scanned = staged::read_scan(&work)
        .expect("the fixture staged scan decodes")
        .expect("the fixture staged a scan");
    let view = catalog.read_view(project.id).expect("readable");
    let states = catalog.read_disc_states(project.id).expect("readable");
    let cap = effective_capacity(
        Capacity::from_disc_type(project.default_disc_type, project.margin_percent),
        Some(HARVEST_DISC_BYTES),
    );
    let ours = plan_from(
        &view,
        &scanned,
        &states,
        project.default_disc_type,
        cap,
        false,
    );
    let theirs = staged::read_plan(&work)
        .expect("the fixture staged plan decodes")
        .expect("the fixture staged a plan");
    // `_tmp` is a real binding, so the temp catalog outlives every read
    // above; drop order takes `catalog` down before it.
    (ours, theirs)
}

/// Cycle 1: two 800-byte files against a 1000-byte disc. The plan spills, so
/// this cycle's disc is sealed and one disc's worth remains.
#[test]
fn replanning_cycle_one_reproduces_the_scala_staged_plan() {
    let (ours, theirs) = replan("catalog/02-after-scan1.sqlite", "work/01-after-plan1");
    assert_eq!(ours, theirs);
    // Spelled out, so a failure says which property drifted rather than
    // printing two structs.
    assert_eq!(ours.disc.ordinal, 1);
    assert!(ours.disc.is_new);
    assert!(ours.disc.will_finalize, "the plan spilled");
    assert_eq!(ours.remaining.more_discs, 1);
    assert_eq!(ours.remaining.more_bytes, 800);
    assert!(ours.tombstones.is_empty());
}

/// Cycle 2: `a.bin` is superseded and `b.bin` deleted. Disc 2 is OPEN with
/// 200 bytes free, which the 800-byte new version cannot top off — so the
/// cycle plans a fresh disc 3 rather than burning an empty finalizing
/// placeholder, and the deletion rides along as a tombstone.
#[test]
fn replanning_cycle_two_reproduces_the_scala_staged_plan() {
    let (ours, theirs) = replan("catalog/06-after-scan3.sqlite", "work/02-after-plan3");
    assert_eq!(ours, theirs);
    assert_eq!(ours.disc.ordinal, 3, "the empty-open-disc skip");
    assert!(ours.disc.is_new);
    assert!(!ours.disc.will_finalize);
    assert_eq!(
        ours.tombstones.iter().map(|p| p.as_str()).collect::<Vec<_>>(),
        vec!["b.bin"]
    );
    assert_eq!(ours.remaining.more_discs, 0);
}

/// The exit-bar claim in the spec's own words: the staged JSON is semantically
/// identical to the fixture's. Field order is not an oracle (spec §4), so the
/// comparison is between parsed values.
#[test]
fn the_staged_plan_json_is_semantically_identical_to_the_fixtures() {
    for (catalog_fixture, work_fixture) in [
        ("catalog/02-after-scan1.sqlite", "work/01-after-plan1"),
        ("catalog/06-after-scan3.sqlite", "work/02-after-plan3"),
    ] {
        let (ours, _) = replan(catalog_fixture, work_fixture);
        let out = tempfile::TempDir::new().expect("a writable temp directory");
        staged::write_plan(out.path(), &ours).expect("the plan stages");

        let mine: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(out.path().join("staged/plan.json")).unwrap(),
        )
        .unwrap();
        let theirs: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(fixtures_dir().join(work_fixture).join("staged/plan.json"))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(mine, theirs, "{work_fixture}");
    }
}
```

No manifest change is needed: integration tests link the package's normal
dependencies as well as its dev-dependencies, which is how the existing
`fixture_replay.rs` already reaches `rusqlite`. `tempfile` is a dev-dependency
and `serde_json` a normal one; both are in scope here.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p diskrement-shell --test fixture_plan`
Expected: FAIL to compile until Tasks 4 and 5 are in (they are), then either
PASS or a real diff. A diff here is a **porting bug in the Rust side** — never
edit `fixtures/`.

- [ ] **Step 3: Run the test to verify it passes**

Run: `cargo test -p diskrement-shell --test fixture_plan -- --nocapture`
Expected: PASS — 3 tests.

- [ ] **Step 4: Confirm the corpus was not touched**

```bash
git status --porcelain -- fixtures
sha256sum -c fixtures/SHA256SUMS --quiet
```

Expected: the first prints nothing; the second reports no failures. A stray
`-wal`/`-shm` file beside a fixture means something opened it read-only
without `immutable=1`.

- [ ] **Step 5: Run the verification bar**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
```

Expected: all clean; 255 passed.

- [ ] **Step 6: Commit**

```bash
git add crates/diskrement-shell/tests/fixture_plan.rs
git commit -m "test(pipeline): re-plan the harvested cycles against the Scala staged plans"
```

---

### Task 7: `render::plan_report`

**Files:**
- Modify: `crates/diskrement-cli/src/render.rs`
- Test: same file's `mod tests`

**Interfaces:**
- Produces: `render::plan_report(&PlanReport) -> String`. Task 8 calls it.

Read `cli/src/main/scala/diskrement/cli/Render.scala`'s `def plan`. Copy the
strings character for character, em dash included. `fixtures/run-manifest.txt`
steps 03, 10 and 18 are the cross-implementation oracle for three of them.

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `crates/diskrement-cli/src/render.rs`:

```rust
    use diskrement_core::{ContentHash, PlannedDisc, RelPath, ScannedFile};
    use diskrement_shell::pipeline::reports::{PlanReport, RemainingEstimate};

    fn scanned(rel: &str, size: u64) -> ScannedFile {
        ScannedFile {
            rel_path: RelPath(rel.into()),
            content_hash: ContentHash("deadbeef".into()),
            size_bytes: size,
            mtime: ts(2026, 9, 5),
        }
    }

    fn plan_of(
        disc: PlannedDisc,
        tombstones: Vec<&str>,
        remaining: (usize, usize, u64),
        oversized: Vec<ScannedFile>,
    ) -> PlanReport {
        PlanReport {
            disc,
            disc_type: DiscType::BdR,
            tombstones: tombstones.into_iter().map(|t| RelPath(t.into())).collect(),
            remaining: RemainingEstimate {
                more_discs: remaining.0,
                more_files: remaining.1,
                more_bytes: remaining.2,
            },
            oversized,
        }
    }

    /// `fixtures/run-manifest.txt` step 03 — the cross-implementation render
    /// oracle, byte for byte.
    #[test]
    fn a_spilling_plan_renders_exactly_as_the_scala_build_printed_it() {
        let report = plan_of(
            PlannedDisc {
                ordinal: 1,
                is_new: true,
                will_finalize: true,
                files: vec![scanned("a.bin", 800)],
                prior_used_bytes: 0,
                usable_bytes: 1_000,
            },
            vec![],
            (1, 1, 800),
            vec![],
        );
        assert_eq!(
            plan_report(&report),
            "Plan: disc 1 (new disc, BD_R) — 1 files, 0.0 GB, will FINALIZE\n\
             ~1 more files (~1 more discs) remain after this one."
        );
    }

    /// `fixtures/run-manifest.txt` step 10 — a deletion rides along.
    #[test]
    fn a_plan_with_a_deletion_renders_exactly_as_the_scala_build_printed_it() {
        let report = plan_of(
            PlannedDisc {
                ordinal: 3,
                is_new: true,
                will_finalize: false,
                files: vec![scanned("a.bin", 800)],
                prior_used_bytes: 0,
                usable_bytes: 1_000,
            },
            vec!["b.bin"],
            (0, 0, 0),
            vec![],
        );
        assert_eq!(
            plan_report(&report),
            "Plan: disc 3 (new disc, BD_R) — 1 files, 0.0 GB, left OPEN\n\
             This disc completes the set.\n\
             Deletions: 1"
        );
    }

    /// `fixtures/run-manifest.txt` step 18 — `plan --finalize` on a scan that
    /// found nothing.
    #[test]
    fn sealing_an_empty_disc_renders_exactly_as_the_scala_build_printed_it() {
        let report = plan_of(
            PlannedDisc {
                ordinal: 4,
                is_new: false,
                will_finalize: true,
                files: vec![],
                prior_used_bytes: 800,
                usable_bytes: 1_000,
            },
            vec![],
            (0, 0, 0),
            vec![],
        );
        assert_eq!(
            plan_report(&report),
            "Plan: finalize disc 4 (seal; no new files)."
        );
    }

    #[test]
    fn a_plan_with_nothing_to_do_says_so() {
        let report = plan_of(
            PlannedDisc {
                ordinal: 1,
                is_new: false,
                will_finalize: false,
                files: vec![],
                prior_used_bytes: 0,
                usable_bytes: 25_000_000_000,
            },
            vec![],
            (0, 0, 0),
            vec![],
        );
        assert_eq!(plan_report(&report), "Nothing to burn: no changes to plan.");
    }

    #[test]
    fn appending_to_an_open_disc_names_the_disc_and_says_left_open() {
        let report = plan_of(
            PlannedDisc {
                ordinal: 2,
                is_new: false,
                will_finalize: false,
                files: vec![scanned("a.bin", 3_200_000_000)],
                prior_used_bytes: 10_000_000_000,
                usable_bytes: 25_000_000_000,
            },
            vec![],
            (0, 0, 0),
            vec![],
        );
        assert_eq!(
            plan_report(&report),
            "Plan: disc 2 (appending to open disc 2, BD_R) — 1 files, 13.2 GB, left OPEN\n\
             This disc completes the set."
        );
    }

    #[test]
    fn oversized_files_are_listed_under_the_summary() {
        let report = plan_of(
            PlannedDisc {
                ordinal: 1,
                is_new: true,
                will_finalize: false,
                files: vec![scanned("small.bin", 10)],
                prior_used_bytes: 0,
                usable_bytes: 25_000_000_000,
            },
            vec![],
            (0, 0, 0),
            vec![scanned("huge.bin", 99_000_000_000)],
        );
        assert_eq!(
            plan_report(&report),
            "Plan: disc 1 (new disc, BD_R) — 1 files, 0.0 GB, left OPEN\n\
             This disc completes the set.\n\
             Oversized (cannot be placed):\n\
             \x20 huge.bin  99.0 GB"
        );
    }

    /// An oversize-only plan still prints the summary block rather than
    /// "nothing to burn": the disc is empty but there is something to say.
    #[test]
    fn an_oversize_only_plan_is_not_nothing_to_burn() {
        let report = plan_of(
            PlannedDisc {
                ordinal: 1,
                is_new: true,
                will_finalize: false,
                files: vec![],
                prior_used_bytes: 0,
                usable_bytes: 1_000,
            },
            vec![],
            (0, 0, 0),
            vec![scanned("huge.bin", 5_000)],
        );
        assert!(plan_report(&report).starts_with("Plan: disc 1 (new disc, BD_R) — 0 files"));
    }
```

Note the `\x20` escape: the oversized lines start with two spaces, and a line
continuation would swallow leading whitespace.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p diskrement-cli render`
Expected: FAIL with `cannot find function plan_report in this scope`.

- [ ] **Step 3: Implement the renderer**

In `crates/diskrement-cli/src/render.rs`, extend the existing reports import to
`use diskrement_shell::pipeline::reports::{PlanReport, ScanReport, StatusReport};`
and add:

```rust
/// `Render.plan`. Three shapes: nothing to do, a bare seal, and the summary
/// block with its optional deletion and oversize lines.
// Consumed by `main` in the next task; the allow goes with it.
#[allow(dead_code)]
pub fn plan_report(p: &PlanReport) -> String {
    let d = &p.disc;
    if d.files.is_empty() && p.tombstones.is_empty() && p.oversized.is_empty() {
        return if d.will_finalize {
            format!("Plan: finalize disc {} (seal; no new files).", d.ordinal)
        } else {
            "Nothing to burn: no changes to plan.".to_string()
        };
    }
    let kind = if d.is_new {
        "new disc".to_string()
    } else {
        format!("appending to open disc {}", d.ordinal)
    };
    let fin = if d.will_finalize {
        "will FINALIZE"
    } else {
        "left OPEN"
    };
    let mut lines = vec![format!(
        "Plan: disc {} ({kind}, {}) — {} files, {}, {fin}",
        d.ordinal,
        p.disc_type,
        d.files.len(),
        gb(d.total_used_bytes())
    )];
    lines.push(if p.remaining.more_discs == 0 {
        "This disc completes the set.".to_string()
    } else {
        format!(
            "~{} more files (~{} more discs) remain after this one.",
            p.remaining.more_files, p.remaining.more_discs
        )
    });
    if !p.tombstones.is_empty() {
        lines.push(format!("Deletions: {}", p.tombstones.len()));
    }
    if !p.oversized.is_empty() {
        lines.push("Oversized (cannot be placed):".to_string());
        for f in &p.oversized {
            lines.push(format!("  {}  {}", f.rel_path.as_str(), gb(f.size_bytes)));
        }
    }
    lines.join("\n")
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p diskrement-cli render`
Expected: PASS — 7 new tests alongside the P3 ones.

- [ ] **Step 5: Run the verification bar**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
```

Expected: all clean; 262 passed.

- [ ] **Step 6: Commit**

```bash
git add crates/diskrement-cli/src/render.rs
git commit -m "feat(cli): render the plan report"
```

---

### Task 8: The `plan` command surface

**Files:**
- Modify: `crates/diskrement-cli/src/args.rs`
- Modify: `crates/diskrement-cli/src/main.rs`
- Modify: `crates/diskrement-cli/src/render.rs` (drop the `#[allow(dead_code)]`)
- Test: `crates/diskrement-cli/src/args.rs`'s `mod tests`

**Interfaces:**
- Produces: `args::Command::Plan { name, disc_type, finalize }` and its
  dispatch arm. Task 9 drives it through the spawned binary; Task 10 opens the
  differential gate on it.

Args and dispatch land together because adding a variant to `Command` makes
`main`'s two `match` expressions non-exhaustive: split across two commits, the
tree would not build in between.

Read `Commands.scala`'s `plan` block (`--disc-type` is optional here, with no
default, unlike `init`'s) and `Runner.scala`'s `Plan` case (the work dir comes
from the project row, not from a flag).

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `crates/diskrement-cli/src/args.rs`:

```rust
    #[test]
    fn plan_defaults_to_no_disc_type_override_and_no_finalize() {
        let cli = Cli::try_parse_from(["diskrement", "plan", "photos"]).unwrap();
        let Command::Plan {
            name,
            disc_type,
            finalize,
        } = cli.command
        else {
            panic!("not plan")
        };
        assert_eq!(name, "photos");
        assert_eq!(disc_type, None);
        assert!(!finalize);
    }

    #[test]
    fn plan_takes_a_disc_type_override_and_the_finalize_flag() {
        let cli = Cli::try_parse_from([
            "diskrement",
            "plan",
            "photos",
            "--disc-type",
            "DVD_DL",
            "--finalize",
        ])
        .unwrap();
        let Command::Plan {
            disc_type,
            finalize,
            ..
        } = cli.command
        else {
            panic!("not plan")
        };
        assert_eq!(disc_type, Some(DiscType::DvdDl));
        assert!(finalize);
    }

    #[test]
    fn plan_rejects_an_unknown_disc_type() {
        assert!(
            Cli::try_parse_from(["diskrement", "plan", "p", "--disc-type", "ZIP"]).is_err()
        );
    }

    #[test]
    fn plan_requires_a_project_name() {
        assert!(Cli::try_parse_from(["diskrement", "plan"]).is_err());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p diskrement-cli args`
Expected: FAIL — `no variant named Plan found for enum Command`.

- [ ] **Step 3: Add the subcommand**

In `crates/diskrement-cli/src/args.rs`, add to `enum Command`, after `Scan`:

```rust
    /// Plan the next disc from the staged scan.
    Plan {
        name: String,
        /// Override the project's default disc type for this plan only.
        #[arg(long)]
        disc_type: Option<DiscType>,
        /// Seal the disc even when everything fit.
        #[arg(long, default_value_t = false)]
        finalize: bool,
    },
```

- [ ] **Step 4: Dispatch it**

In `crates/diskrement-cli/src/main.rs`, add to the `project_name` match:

```rust
        Command::Plan { name, .. } => name,
```

and add the dispatch arm after `Command::Scan`:

```rust
        Command::Plan {
            name,
            disc_type,
            finalize,
        } => {
            // The work dir comes from the project row, not the CLI
            // (`Runner.withProject`). `plan` reports no progress.
            let mut catalog = open_catalog(&resolved.catalog)?;
            let p = find_or_fail(&catalog, &name)?;
            let pipeline = Pipeline::new(
                &mut catalog,
                PipelinePaths {
                    work_dir: PathBuf::from(&p.work_dir),
                    scan_cache: paths::scan_cache_path(&resolved.catalog, &name),
                },
                &NoopSink,
            );
            let report = pipeline.plan(&name, disc_type, finalize)?;
            say(&render::plan_report(&report));
            Ok(())
        }
```

Then delete the `#[allow(dead_code)]` above `plan_report` in
`crates/diskrement-cli/src/render.rs` — `main` consumes it now.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p diskrement-cli`
Expected: PASS.

- [ ] **Step 6: Try it by hand**

```bash
cargo build -p diskrement-cli
D=$(mktemp -d); mkdir -p "$D/src" "$D/cat"
head -c 800 /dev/zero > "$D/src/a.bin"
./target/debug/diskrement --catalog "$D/cat/c.sqlite" init p --source "$D/src" --work-dir "$D/work"
./target/debug/diskrement --catalog "$D/cat/c.sqlite" scan p
./target/debug/diskrement --catalog "$D/cat/c.sqlite" plan p
./target/debug/diskrement --catalog "$D/cat/c.sqlite" plan p --finalize
cat "$D/work/staged/plan.json"; echo
```

Expected: `Plan: disc 1 (new disc, BD_R) — 1 files, 0.0 GB, left OPEN` then
`This disc completes the set.`; the second invocation says `will FINALIZE`;
`plan.json` carries `"willFinalize":true` and `"discType":"BD_R"`. `$D` must
be on a CoW filesystem or `init` refuses it — set `TMPDIR` accordingly.

- [ ] **Step 7: Run the verification bar**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
```

Expected: all clean; 266 passed.

- [ ] **Step 8: Commit**

```bash
git add crates/diskrement-cli/src/args.rs crates/diskrement-cli/src/main.rs \
        crates/diskrement-cli/src/render.rs
git commit -m "feat(cli): wire plan into the diskrement binary"
```

---

### Task 9: End-to-end `plan` through the spawned binary

**Files:**
- Modify: `crates/diskrement-cli/tests/binary.rs`

**Interfaces:**
- Consumes: the binary from Task 8. This is the only place
  `DISKREMENT_DISC_BYTES` is exercised end to end — `assert_cmd` sets it per
  child, which unit tests cannot do safely because `std::env` is
  process-global and the test harness is threaded.

- [ ] **Step 1: Write the failing tests**

Append to `crates/diskrement-cli/tests/binary.rs`:

```rust
mod p4 {
    use super::*;

    fn cmd() -> Command {
        Command::cargo_bin("diskrement").expect("binary builds")
    }

    /// A scratch project with `sizes` written into the source tree, already
    /// init'd and scanned. Returns the temp dir, the catalog path and the work
    /// dir.
    fn scanned_project(
        sizes: &[(&str, usize)],
    ) -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let src = root.join("src");
        std::fs::create_dir_all(&src).unwrap();
        for (name, size) in sizes {
            std::fs::write(src.join(name), vec![b'x'; *size]).unwrap();
        }
        let catalog = root.join("catalog/cat.sqlite");
        std::fs::create_dir_all(catalog.parent().unwrap()).unwrap();
        let work = root.join("work");
        cmd()
            .arg("--catalog")
            .arg(&catalog)
            .args(["init", "p", "--source"])
            .arg(&src)
            .arg("--work-dir")
            .arg(&work)
            .assert()
            .success();
        cmd()
            .arg("--catalog")
            .arg(&catalog)
            .args(["scan", "p"])
            .assert()
            .success();
        (dir, catalog, work)
    }

    #[test]
    fn plan_stages_a_plan_and_prints_the_summary() {
        let (_dir, catalog, work) = scanned_project(&[("a.bin", 800)]);
        cmd()
            .arg("--catalog")
            .arg(&catalog)
            .args(["plan", "p"])
            .assert()
            .success()
            .stdout(
                contains("Plan: disc 1 (new disc, BD_R) —")
                    .and(contains("left OPEN"))
                    .and(contains("This disc completes the set.")),
            );
        assert!(work.join("staged/plan.json").is_file());
        // The staged scan survives: `plan` does not consume it.
        assert!(work.join("staged/scan.json").is_file());
    }

    #[test]
    fn plan_finalize_seals_the_disc() {
        let (_dir, catalog, _work) = scanned_project(&[("a.bin", 800)]);
        cmd()
            .arg("--catalog")
            .arg(&catalog)
            .args(["plan", "p", "--finalize"])
            .assert()
            .success()
            .stdout(contains("will FINALIZE"));
    }

    #[test]
    fn plan_disc_type_overrides_the_projects_default() {
        let (_dir, catalog, _work) = scanned_project(&[("a.bin", 800)]);
        cmd()
            .arg("--catalog")
            .arg(&catalog)
            .args(["plan", "p", "--disc-type", "DVD"])
            .assert()
            .success()
            .stdout(contains("(new disc, DVD)"));
    }

    /// The shrink-only capacity override, end to end. Two 800-byte files
    /// against a 1000-byte disc: the plan spills and seals.
    #[test]
    fn the_disc_bytes_override_shrinks_capacity_and_spills() {
        let (_dir, catalog, _work) = scanned_project(&[("a.bin", 800), ("b.bin", 800)]);
        cmd()
            .arg("--catalog")
            .arg(&catalog)
            .args(["plan", "p"])
            .env("DISKREMENT_DISC_BYTES", "1000")
            .assert()
            .success()
            .stdout(
                contains("will FINALIZE")
                    .and(contains("~1 more files (~1 more discs) remain after this one.")),
            );
    }

    /// A value at or above the margin-adjusted capacity is ignored, so the
    /// override can never plan past the safety margin.
    #[test]
    fn the_disc_bytes_override_never_grows_capacity() {
        let (_dir, catalog, _work) = scanned_project(&[("a.bin", 800), ("b.bin", 800)]);
        cmd()
            .arg("--catalog")
            .arg(&catalog)
            .args(["plan", "p"])
            .env("DISKREMENT_DISC_BYTES", "999999999999")
            .assert()
            .success()
            .stdout(contains("This disc completes the set."));
    }

    #[test]
    fn an_oversized_file_is_listed_but_the_plan_still_succeeds() {
        let (_dir, catalog, _work) = scanned_project(&[("big.bin", 800), ("small.bin", 100)]);
        cmd()
            .arg("--catalog")
            .arg(&catalog)
            .args(["plan", "p"])
            .env("DISKREMENT_DISC_BYTES", "500")
            .assert()
            .success()
            .stdout(contains("Oversized (cannot be placed):").and(contains("big.bin")));
    }

    #[test]
    fn plan_without_a_staged_scan_exits_2_and_says_to_run_scan() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        let catalog = root.join("catalog/cat.sqlite");
        std::fs::create_dir_all(catalog.parent().unwrap()).unwrap();
        cmd()
            .arg("--catalog")
            .arg(&catalog)
            .args(["init", "p", "--source"])
            .arg(root.join("src"))
            .arg("--work-dir")
            .arg(root.join("work"))
            .assert()
            .success();
        cmd()
            .arg("--catalog")
            .arg(&catalog)
            .args(["plan", "p"])
            .assert()
            .code(2)
            .stderr(contains("no staged scan for p (run scan)"));
    }

    #[test]
    fn plan_on_an_unknown_project_exits_2() {
        let dir = tempfile::TempDir::new().unwrap();
        let catalog = dir.path().join("catalog/cat.sqlite");
        std::fs::create_dir_all(catalog.parent().unwrap()).unwrap();
        cmd()
            .arg("--catalog")
            .arg(&catalog)
            .args(["plan", "nosuch"])
            .assert()
            .code(2)
            .stderr(contains("project not found: nosuch"));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail, then pass**

Run: `cargo test -p diskrement-cli --test binary`
Expected: PASS once Task 8 is in. If `init` fails with
"work dir is not CoW-capable", the temp dir is not on btrfs/XFS/bcachefs —
export `TMPDIR` to one and re-run. That is an environment problem, not a bug;
say so rather than weakening the test.

- [ ] **Step 3: Run the verification bar**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
```

Expected: all clean; 274 passed.

- [ ] **Step 4: Commit**

```bash
git add crates/diskrement-cli/tests/binary.rs
git commit -m "test(cli): plan end to end through the spawned binary"
```

---

### Task 10: Open the differential gate on `plan`

**Files:**
- Modify: `crates/diskrement-difftest/src/corpus.rs`
- Modify: `crates/diskrement-difftest/src/runner.rs` (one line)

**Interfaces:**
- Consumes: the binary from Task 8 and the harness from P3.

Scenarios may use only commands the Rust binary implements, or the Rust and
interleaved modes skip them: after this task that is `init`, `scan`, `status`
and `plan`. `harvest-replay` keeps skipping until P5 lands `burn`.

**A scenario never hard-codes an expected exit code or line of output** — both
binaries run the same step and the assertion is that they agree.

- [ ] **Step 1: Add the scenarios**

In `crates/diskrement-difftest/src/corpus.rs`, extend `all()`:

```rust
pub fn all() -> Vec<Scenario> {
    vec![
        init_scan_status(),
        error_paths(),
        plan_basics(),
        plan_spill_and_seal(),
        plan_oversize(),
        plan_errors(),
        harvest_replay(),
    ]
}
```

and append the four scenarios before `harvest_replay`:

```rust
/// `plan` on a tree that fits one disc: the "completes the set" wording, the
/// `--finalize` seal, and the fact that re-planning is idempotent because it
/// does not consume the staged scan.
pub fn plan_basics() -> Scenario {
    Scenario {
        name: "plan-basics",
        disc_bytes: None,
        steps: vec![
            write("a.bin", 800),
            write("nested/b.bin", 120),
            cmd(
                "init p",
                &["init", "p", "--source", "{source}", "--work-dir", "{work}"],
            ),
            cmd("scan p", &["scan", "p"]),
            cmd("plan p", &["plan", "p"]),
            // Re-plan: same staged scan, same answer, plan.json overwritten.
            cmd("plan p again", &["plan", "p"]),
            cmd("plan p --finalize", &["plan", "p", "--finalize"]),
            // An explicit disc type changes the capacity the plan reports.
            cmd(
                "plan p --disc-type DVD",
                &["plan", "p", "--disc-type", "DVD"],
            ),
            cmd("status p", &["status", "p"]),
        ],
    }
}

/// The spill: two 800-byte files against a 1000-byte disc. This cycle's disc
/// is sealed and the remainder is reported — the shape the corpus's
/// `work/01-after-plan1` recorded.
pub fn plan_spill_and_seal() -> Scenario {
    Scenario {
        name: "plan-spill-and-seal",
        disc_bytes: Some(1000),
        steps: vec![
            write("a.bin", 800),
            write("b.bin", 800),
            cmd(
                "init p",
                &["init", "p", "--source", "{source}", "--work-dir", "{work}"],
            ),
            cmd("scan p", &["scan", "p"]),
            cmd("plan p", &["plan", "p"]),
            cmd("status p", &["status", "p"]),
        ],
    }
}

/// A file no disc can hold. `plan` must list it and still succeed; `burn`
/// refuses it, which is P5's half of the two-stage check.
pub fn plan_oversize() -> Scenario {
    Scenario {
        name: "plan-oversize",
        disc_bytes: Some(500),
        steps: vec![
            write("big.bin", 800),
            write("small.bin", 100),
            cmd(
                "init p",
                &["init", "p", "--source", "{source}", "--work-dir", "{work}"],
            ),
            cmd("scan p", &["scan", "p"]),
            cmd("plan p", &["plan", "p"]),
        ],
    }
}

/// `plan` before `scan`: a state error, not an argument error, so the message
/// and the exit code are both contract and both fully compared.
pub fn plan_errors() -> Scenario {
    Scenario {
        name: "plan-errors",
        disc_bytes: None,
        steps: vec![
            write("a.bin", 100),
            cmd(
                "init p",
                &["init", "p", "--source", "{source}", "--work-dir", "{work}"],
            ),
            cmd("plan before scan", &["plan", "p"]),
            // …and the same failure on an uninitialised project resolves to
            // the project lookup instead.
            cmd("plan on an unknown project", &["plan", "nosuch"]),
            // An unknown disc type fails in the argument parser, the one place
            // spec §3.2 lets clap and zio-cli word things differently.
            cmd_exit_only("plan with a bogus disc type", &["plan", "p", "--disc-type", "ZIP"]),
        ],
    }
}
```

`Expect::ExitOnly` exempts the *wording* only — the exit code is still
compared, because §3.2 sanctions a wording divergence and nothing more. If clap
and zio-cli disagree on the code for an invalid enumeration value, that is a
real finding: record it as a deviation and decide deliberately, rather than
loosening the step to make it pass.

Add the scenario-inventory tests to that file's `mod tests`:

```rust
    #[test]
    fn the_plan_scenarios_use_only_commands_p4_lands() {
        for s in [plan_basics(), plan_spill_and_seal(), plan_oversize(), plan_errors()] {
            for c in s.commands() {
                assert!(
                    ["init", "scan", "status", "plan"].contains(&c),
                    "scenario '{}' uses '{c}', which P4 does not land",
                    s.name
                );
            }
        }
    }

    #[test]
    fn the_spill_scenario_shrinks_the_disc_enough_to_spill() {
        // Two 800-byte files, a 1000-byte disc: without the override they
        // would share one disc and the scenario would test nothing.
        assert_eq!(plan_spill_and_seal().disc_bytes, Some(1000));
    }

    #[test]
    fn the_oversize_scenario_puts_one_file_beyond_any_disc() {
        // 800 bytes against a 500-byte disc, with a 100-byte file that still
        // fits: the plan must list the first and place the second.
        assert_eq!(plan_oversize().disc_bytes, Some(500));
    }
```

- [ ] **Step 2: Run the harness's own unit tests**

Run: `cargo test -p diskrement-difftest`
Expected: PASS — the new inventory tests included. `every_scenario_has_a_unique_name`
covers the four new names.

- [ ] **Step 3: Append `plan` to `RUST_IMPLEMENTED`**

In `crates/diskrement-difftest/src/runner.rs`:

```rust
pub const RUST_IMPLEMENTED: &[&str] = &["frobnicate", "init", "plan", "scan", "status"];
```

- [ ] **Step 4: Run the full differential suite, all three modes**

```bash
sbt cli/stage
export DISKREMENT_SCALA_BIN="$PWD/cli/target/universal/stage/bin/diskrement"
export TMPDIR=/path/on/btrfs-or-xfs
cargo build -p diskrement-cli
cargo test -p diskrement-difftest -- --ignored --nocapture
```

Expected:
- `the_harness_is_deterministic_against_itself`: **7 scenarios** reproduced
  identically.
- `rust_matches_scala`: `all-rust: 6 scenario(s) compared, 1 skipped` — the
  six that use only `init`/`scan`/`status`/`plan`, with `harvest-replay`
  skipped and its reason printed (it needs `burn`, `verify`, `history`,
  `disc`).
- `interleaved_matches_scala`: the same counts. This is the mode where Scala
  reads a `plan.json` that Rust wrote, and vice versa — the direction the
  frozen corpus structurally cannot test.

If a comparison fails, the diff is a **porting bug**: fix the Rust side to
match the Scala oracle. **Never widen a normalization rule** to make a diff
pass — each rule is a hole in the oracle (spec §11).

If the run prints `SKIP:` with no scenarios compared, the Scala launcher or
the CoW temp dir is missing. A green run that compared nothing is not a pass;
report the counts.

- [ ] **Step 5: Run the phase's full verification bar**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
git status --porcelain -- core shell cli build.sbt project fixtures   # must print nothing
```

Expected: all clean; 277 passed, 3 ignored.

- [ ] **Step 6: Commit**

```bash
git add crates/diskrement-difftest/src/corpus.rs crates/diskrement-difftest/src/runner.rs
git commit -m "feat(difftest): teach the harness that plan exists"
```

---

### Task 11: Land the phase

**Files:**
- Modify: this plan document (the execution-deviations section)

- [ ] **Step 1: Append every execution deviation**

Anything that differed from this plan — a signature that did not compile as
written, a test that was wrong, a behavior the Scala source settled differently
— goes under "Execution deviations" below. Deviations live in the plan doc, not
in commit messages (CLAUDE.md).

- [ ] **Step 2: Commit the plan update**

```bash
git add docs/superpowers/plans/2026-09-06-rust-rewrite-p4-planner-staging.md
git commit -m "docs(plan): record the P4 execution deviations"
```

- [ ] **Step 3: Merge the phase branch**

```bash
git switch feat/rust-rewrite
git merge --no-ff feat/rewrite-p4 -m "Merge feat/rewrite-p4: the staged plan, the pure planner wiring, and the plan command — --finalize, the shrink-only capacity override, oversize reporting and the staged-state error contract, proven against the corpus and against the Scala binary in all three harness modes"
git branch -d feat/rewrite-p4
git push origin feat/rust-rewrite
git push origin --delete feat/rewrite-p4 2>/dev/null || true
```

---

## Phase exit bar (spec §9, P4 row)

- [ ] **Staged JSON semantically identical to the fixture's** — Task 6's
  `the_staged_plan_json_is_semantically_identical_to_the_fixtures` compares
  parsed values against both harvested `plan.json` files, and the two
  re-planning tests compare the whole `PlanReport` structurally.
- [ ] Planner wiring ported: diff → plan → one-disc-per-cycle reduction,
  including the empty-open-disc skip (Tasks 4, 5, 6).
- [ ] Staged store complete: `plan.json` write/read plus `clear`, with the
  frozen field spellings pinned (Tasks 1, 2).
- [ ] `plan --finalize` seals a non-full disc, and seals an empty open disc
  after a no-op scan (Tasks 4, 7, 9).
- [ ] Oversize surfaced by `plan` and rendered (Tasks 4, 7, 9); the
  `OversizedFiles` error and its exit 3 pinned for P5's `burn` (Task 3).
- [ ] `StalePlan` message pinned, including the negative assertion that it
  never says "reset" (Task 3).
- [ ] `DISKREMENT_DISC_BYTES` may only shrink, and stays out of `--help`
  (Tasks 4, 9).
- [ ] Render matches `run-manifest.txt` steps 03, 10 and 18 byte for byte
  (Task 7).
- [ ] The differential suite passes in all three modes for every scenario
  reachable with `init`/`scan`/`status`/`plan`: 6 compared, 1 skipped loudly
  (Task 10).
- [ ] Routine suite, clippy `-D warnings`, fmt clean; the Scala tree and
  `fixtures/` untouched.
- [ ] Execution deviations appended below as they occur.

## Execution deviations

(appended during execution)

## Spec coverage

| spec §9 P4 requirement | task |
|---|---|
| planner wiring | 4, 5 |
| staged store (plan half + clear) | 1, 2 |
| `plan` including `--finalize` | 4, 5, 7, 8 |
| oversize | 3, 4, 7, 9 |
| `StalePlan` | 3 |
| staged JSON semantically identical to the fixture's | 6 |
| differential task for the phase's commands (§8.1) | 10 |
| CLI compatibility: flags, exit codes, render (§3.2) | 3, 7, 8, 9 |
| data compatibility: frozen JSON field and enum spellings (§3.1) | 1, 6 |

Deferred by design to later phases, each already named in the spec's phasing
table: `burn`, the stager, manifests, pointers and the embedded catalog
snapshot — including the `burn`-side producers of `NoStagedPlan`,
`OversizedFiles` and `StalePlan` (P5); `verify`, `history`, `reset`, the full
`ExitMappingSpec` port and the insta snapshots (P6); `disc mark-dead` and
derived `DEAD` (P7); the real xorriso backend (P8).
