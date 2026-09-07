# Rust Rewrite P6 — `verify`, `history`, `reset`, and the full render surface Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land P6 — the staging disc verifier and the closed `Verifier` enum,
`Pipeline::verify` / `Pipeline::history` / `Pipeline::reset`, the `verify`,
`history` and `reset` commands with the result-driven exit-4 contract, and the
last three renderers — ending with a Rust CLI whose every success-path render is
pinned by an `insta` snapshot and matches `fixtures/run-manifest.txt` word for
word.

**Architecture:** Spec §5.2/§5.5. A new `burn/verifier.rs` holds
`DiscExpectation`, `VerifyOutcome`, `StagingVerifier` and the one-variant closed
`Verifier` enum whose second arm P8 adds — the mirror of `Burner`, and selected
by the same `--burner real|simulated` decision the CLI already makes once.
`Pipeline::verify` resolves an explicitly named ordinal *before* filtering (so a
named dead or nonexistent disc is an error, not a reassuring exit 0), sweeps the
live discs, and marks a session VERIFIED only when a real check found no
mismatch. `Pipeline::history` is a catalog passthrough; `Pipeline::reset`
discards the staged scan/plan, the temporary burn catalog, the staging trees and
the snapshot. The CLI gains the last three subcommands and a `dispatch` that
returns an exit code rather than unit, because a `verify` that finds corruption
is a *successful run with a bad outcome* (exit 4).

**Tech Stack:** Rust 2021; `serde`, `serde_json`, `rusqlite` (bundled), `sha2`,
`rayon`, `reflink-copy`, `walkdir`, `chrono`, `clap` 4 derive, `tempfile`,
`assert_cmd`, `predicates`, and — used for the first time in this phase —
`insta`, already declared as a `diskrement-cli` dev-dependency. **No new
dependencies.**

**Spec:** `docs/superpowers/specs/2026-09-05-rust-rewrite-design.md` — §3.1 (the
catalog schema is frozen; `DEAD` is derived), §3.2 (CLI surface and exit codes),
§4 (the corpus and the render oracle), §5.2 (`enum Verifier { Disc, Staging }`,
one backend decision made once in `cli`), §5.3 (`burn/verifier.rs` in the
layout), §5.5 (`thiserror` per module, one `exit_code()`), §8.1 (the
differential task every phase from P3 carries), §9 (the P6 row). The Scala
sources are the behavioral oracle: **read the file named in each task before
porting it.**

---

## Global Constraints

- **Branch:** `feat/rewrite-p6`, forked from `feat/rust-rewrite`. Never commit
  directly to `feat/rust-rewrite`. Merge back at the end with an explicit
  `Merge feat/rewrite-p6: <summary>` commit and delete the phase branch.

```bash
git switch feat/rust-rewrite && git switch -c feat/rewrite-p6
```

- **Do not touch** `core/`, `shell/`, `cli/`, `build.sbt`, `project/`,
  `fixtures/`, `scripts/harvest-fixtures.sh`. The Scala tree must stay
  buildable until P10, and the corpus is a frozen, spec-level artifact. Tests
  read `fixtures/` and **copy before opening** — never open a fixture database
  through `Catalog::open` (it runs the migration runner and writes) and never
  write inside `fixtures/`.
- **After every task:**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

  All clean, with no drop in test count. Baseline at plan start:
  **349 passed, 3 ignored** (the three `#[ignore]`d differential entry points).
- **Commit scopes:** `feat(verify):`, `test(verify):`, `feat(pipeline):`,
  `feat(cli):`, `test(cli):`, `feat(difftest):`, `docs(plan):`.
- **User-visible strings and exit codes are contract** (§3.2). Copy them
  verbatim from the Scala source named in each task; the only sanctioned
  divergence is clap's own usage/argument-error wording. Every expected string
  in this plan was captured by running the staged Scala launcher — they are
  transcripts, not guesses.
- **The catalog is append-only** — with the one sanctioned exception the schema
  itself encodes: `session.state` advances `BURNED → VERIFIED` in place, exactly
  as the Scala `markVerified` does. `verify` writes nothing else, and touches no
  `disc` row.
- **`DEAD` is derived, never stored.** `verify` reads dead-ness out of
  `read_disc_states`, which computes it from `disc_event`. Nothing in this phase
  may write a `disc.status` value other than `OPEN`/`FINALIZED`.
- **`reset` destroys the staging trees**, which are the staged verifier's only
  evidence. That is why `StalePlan`'s advice is "re-run plan" and never
  "run reset" — the negative assertion in `error.rs` already pins it; do not
  weaken it, and do not add `reset` to any recovery message this phase writes.
- **Time formatting only through `diskrement_shell::catalog::time`**
  (`format`/`parse`). Never `chrono`'s serde or `Display` for stored values.
- **`Pipeline` returns data and does no I/O path resolution**; the CLI resolves
  paths, builds the backend, and renders (CLAUDE.md invariant, enforced by the
  crate boundary).
- **Appending `verify`, `history` and `reset` to `runner::RUST_IMPLEMENTED`
  happens in the same commit that lands them in the binary** (Task 10).
  Otherwise the Rust and interleaved differential modes keep skipping every
  scenario that uses them, and a fully-skipped run still exits green.

---

## Scala oracle files (read before porting each area)

| Area | File |
|---|---|
| `DiscExpectation`, `VerifyOutcome`, the verifier port | `shell/src/main/scala/diskrement/shell/burn/DiscVerifier.scala` |
| The staging verifier | `shell/src/main/scala/diskrement/shell/pipeline/StagingDiscVerifier.scala` |
| Its spec (port it) | `shell/src/test/scala/diskrement/shell/pipeline/StagingDiscVerifierSpec.scala` |
| `verify`, `history`, `reset` orchestration | `shell/src/main/scala/diskrement/shell/pipeline/PipelineLive.scala` (`def verify`, `def verifyDisc`, `def history`, `def reset`) |
| `VerifyReport` | `shell/src/main/scala/diskrement/shell/pipeline/Pipeline.scala` |
| Pipeline error ADT | `shell/src/main/scala/diskrement/shell/pipeline/PipelineError.scala` |
| Pipeline verify spec (port it) | `shell/src/test/scala/diskrement/shell/pipeline/PipelineVerifySpec.scala` |
| Pipeline reset spec (port it) | `shell/src/test/scala/diskrement/shell/pipeline/PipelineScanResetSpec.scala` |
| `verify` / `history` / `reset` rendering, `verifyExit`, the exit ladder | `cli/src/main/scala/diskrement/cli/Render.scala` |
| The `Reset <name>.` line (it lives in the Runner, not Render) | `cli/src/main/scala/diskrement/cli/Runner.scala` (the `DiskrementCommand.Reset` case) |
| Command surface | `cli/src/main/scala/diskrement/cli/Commands.scala` (`verify`, `history`, `reset`) |
| Dispatch and the verify exit code | `cli/src/main/scala/diskrement/cli/Runner.scala` (`dispatch`) |
| Exit ladder spec (port it) | `cli/src/test/scala/diskrement/cli/ExitMappingSpec.scala` |
| Render spec (port it) | `cli/src/test/scala/diskrement/cli/RenderSpec.scala` |

## Corpus oracles used by this phase

| Fixture | Used by | What it pins |
|---|---|---|
| `fixtures/catalog/05-after-burn2.sqlite` + `fixtures/work/02-after-plan3/staging/` | Task 5 | a real two-disc `verify` sweep against a Scala-written catalog and Scala-staged trees; the `BURNED → VERIFIED` transition, and the *absence* of a transition for a session already VERIFIED |
| `fixtures/catalog/08-after-mark-dead.sqlite` | Task 5 | a sweep silently skips a dead disc; a *named* dead disc is `DiscIsDead` |
| `fixtures/run-manifest.txt` steps 05, 16, 21, 22 | Tasks 6, 7 | the byte-exact `verify` and `history` renders |
| `fixtures/run-manifest.txt` steps 01–04, 08, 10, 18, 20 | Task 7 | the already-landed renders, folded into the snapshot gallery |

Facts about those fixtures that the tasks below rely on, confirmed by query:

- `05-after-burn2.sqlite` holds disc 1 (`FINALIZED`, session 1 **`VERIFIED`** —
  run-manifest step 05 already verified it) and disc 2 (`OPEN`, session 1
  `BURNED`).
- `08-after-mark-dead.sqlite` holds discs 1, 2, 3; disc 3's `disc.status` column
  still reads `OPEN` and its dead-ness lives only in `disc_event`.
- `fixtures/work/02-after-plan3/staging/disc-1/data/a.bin` hashes to
  `8e19511b7991430a1950f27907234183151bb0dbf1fbf28cd5b765088feb9f70` and
  `.../disc-2/data/b.bin` to
  `3413ff9e166e40ad8798e36609f28433496ee3a0f8984f73d33b0a80ffef2a89` — the a@1
  and b@1 hashes the catalog records for those discs' latest sessions.

---

## File Structure

**Created:**

| Path | Responsibility |
|---|---|
| `crates/diskrement-shell/src/burn/verifier.rs` | `DiscExpectation`, `VerifyOutcome`, `StagingVerifier`, the closed `Verifier` enum |
| `crates/diskrement-shell/tests/fixture_verify.rs` | the corpus oracle for a `verify` sweep |
| `crates/diskrement-cli/src/snapshots/*.snap` | the eight hand-authored `insta` render galleries |

**Modified:**

| Path | Change |
|---|---|
| `.gitignore` | ignore `*.snap.new` |
| `crates/diskrement-shell/src/burn/mod.rs` | `pub mod verifier;` |
| `crates/diskrement-shell/src/error.rs` | `PipelineError::UnknownDisc`, `PipelineError::DiscIsDead`, their exit codes |
| `crates/diskrement-shell/src/pipeline/reports.rs` | `VerifyReport` |
| `crates/diskrement-shell/src/pipeline/mod.rs` | `Pipeline::verify`, `verify_disc`, `Pipeline::history`, `Pipeline::reset` |
| `crates/diskrement-cli/src/args.rs` | the `Verify`, `History` and `Reset` subcommands |
| `crates/diskrement-cli/src/render.rs` | `verify_report`, `history`, `reset_done`, `verify_exit`, the snapshot gallery |
| `crates/diskrement-cli/src/main.rs` | `dispatch` returns an exit code; the three new arms; verifier resolution |
| `crates/diskrement-cli/tests/binary.rs` | the `p6` end-to-end module |
| `crates/diskrement-difftest/src/runner.rs` | `RUST_IMPLEMENTED` += `"history"`, `"reset"`, `"verify"` |
| `crates/diskrement-difftest/src/corpus.rs` | the `verify-history-reset` scenario; two argument-error steps |

---

### Task 1: The verifier value types and the closed `Verifier` enum

**Files:**
- Create: `crates/diskrement-shell/src/burn/verifier.rs`
- Modify: `crates/diskrement-shell/src/burn/mod.rs`

Read `shell/src/main/scala/diskrement/shell/burn/DiscVerifier.scala` (the value
types and the port) and
`shell/src/main/scala/diskrement/shell/pipeline/StagingDiscVerifier.scala` (the
implementation this task ports) before writing anything.

Two notes on the shape:

- `DiscExpectation` carries all three views — `latest_session`, `visible` and
  `superseded` — even though the staging verifier reads only the first. P8's
  disc verifier reads the other two, and splitting the type across two phases
  would churn `Pipeline::verify_disc`'s only call.
- `Verifier` has **no lifetime parameter**, unlike `Burner<'a>`: the staging
  verifier borrows nothing (the Scala one reports no progress either). P8's
  `Disc` arm may add one; that is P8's churn to absorb.

- [ ] **Step 1: Write the failing test**

Create `crates/diskrement-shell/src/burn/verifier.rs` containing only the test
module for now, so the compile error names exactly what is missing:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn sha(body: &[u8]) -> ContentHash {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(body);
        ContentHash(format!("{:x}", h.finalize()))
    }

    fn expectation(pairs: &[(&str, &[u8])], ordinal: u32) -> DiscExpectation {
        DiscExpectation {
            project_name: "p".to_string(),
            disc_ordinal: ordinal,
            latest_session: pairs
                .iter()
                .map(|(rel, body)| (RelPath((*rel).to_string()), sha(body)))
                .collect(),
            visible: BTreeMap::new(),
            superseded: Vec::new(),
        }
    }

    /// The `StagingDiscVerifierSpec` port: a clean tree verifies, a corrupted
    /// file mismatches, and a disc with no staging dir is skipped rather than
    /// reported clean.
    #[test]
    fn a_clean_tree_verifies_a_corrupt_file_mismatches_and_a_missing_dir_skips() {
        let dir = tempfile::TempDir::new().unwrap();
        let staging = dir.path().join("staging");
        let data = staging.join("disc-1/data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("a.txt"), b"hello").unwrap();
        std::fs::write(data.join("bad.txt"), b"corrupted").unwrap();

        let v = StagingVerifier::new(staging);
        let exp = expectation(&[("a.txt", b"hello"), ("bad.txt", b"original")], 1);

        assert_eq!(
            v.verify(&exp).unwrap(),
            VerifyOutcome {
                files_checked: 2,
                mismatches: vec![RelPath("bad.txt".to_string())],
                skipped: false,
            }
        );

        let absent = DiscExpectation {
            disc_ordinal: 9,
            ..expectation(&[], 9)
        };
        assert_eq!(
            v.verify(&absent).unwrap(),
            VerifyOutcome {
                files_checked: 0,
                mismatches: Vec::new(),
                skipped: true,
            }
        );
    }

    /// A file present on the disc but absent from the expectation is a
    /// mismatch: `latestSession.get(rel).contains(h)` is false for a missing
    /// key, not merely for a wrong hash.
    #[test]
    fn a_staged_file_the_catalog_never_recorded_is_a_mismatch() {
        let dir = tempfile::TempDir::new().unwrap();
        let staging = dir.path().join("staging");
        let data = staging.join("disc-2/data");
        std::fs::create_dir_all(data.join("nested")).unwrap();
        std::fs::write(data.join("nested/extra.bin"), b"surprise").unwrap();

        let out = StagingVerifier::new(staging)
            .verify(&expectation(&[], 2))
            .unwrap();
        assert_eq!(out.files_checked, 1);
        assert_eq!(out.mismatches, vec![RelPath("nested/extra.bin".to_string())]);
        assert!(!out.skipped);
    }

    /// The staging tree holds only the disc's LATEST session, so an empty
    /// `data/` — what a `plan --finalize` seal session stages — is a clean
    /// zero-file check, not a skip. `fixtures/discs/disc-4/data/` is exactly
    /// this shape, and run-manifest step 21 prints `disc 4: 0 files OK`.
    #[test]
    fn an_empty_data_dir_is_a_clean_zero_file_check_not_a_skip() {
        let dir = tempfile::TempDir::new().unwrap();
        let staging = dir.path().join("staging");
        std::fs::create_dir_all(staging.join("disc-4/data")).unwrap();

        assert_eq!(
            StagingVerifier::new(staging).verify(&expectation(&[], 4)).unwrap(),
            VerifyOutcome {
                files_checked: 0,
                mismatches: Vec::new(),
                skipped: false,
            }
        );
    }

    /// Two mismatches come back in sorted order. The Scala verifier inherited
    /// `Files.walk`'s unspecified order; sorting is what makes the rendered
    /// mismatch list reproducible.
    #[test]
    fn mismatches_are_reported_in_sorted_order() {
        let dir = tempfile::TempDir::new().unwrap();
        let staging = dir.path().join("staging");
        let data = staging.join("disc-1/data");
        std::fs::create_dir_all(&data).unwrap();
        for name in ["z.bin", "a.bin", "m.bin"] {
            std::fs::write(data.join(name), b"wrong").unwrap();
        }
        let out = StagingVerifier::new(staging)
            .verify(&expectation(&[], 1))
            .unwrap();
        assert_eq!(
            out.mismatches,
            vec![
                RelPath("a.bin".to_string()),
                RelPath("m.bin".to_string()),
                RelPath("z.bin".to_string()),
            ]
        );
    }
}
```

Wire the module in `crates/diskrement-shell/src/burn/mod.rs`, in the existing
`pub mod` block (keep it alphabetical among the siblings):

```rust
pub mod verifier;
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p diskrement-shell verifier`
Expected: FAIL to compile — `cannot find type DiscExpectation`,
`VerifyOutcome`, `StagingVerifier` in this scope.

- [ ] **Step 3: Write the implementation**

Prepend to `crates/diskrement-shell/src/burn/verifier.rs`, above the test
module:

```rust
//! Read-back verification (`DiscVerifier.scala`,
//! `StagingDiscVerifier.scala`).
//!
//! `Verifier` is a closed enum for the same reason `Burner` is: there are
//! exactly two substrates, ever, and `--burner real|simulated` selects both
//! arms together — verifying a simulated burn by mounting a physical disc is
//! meaningless (spec §5.2). P6 lands `Staging`; P8 adds `Disc`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use diskrement_core::{ContentHash, RelPath};

use crate::burn::BurnError;
use crate::catalog::model::SupersededVersion;
use crate::fs::hasher;
use crate::fs::scanner::relativize;

/// What the catalog says one disc should hold.
///
/// `latest_session` is what the simulated backend's staging dir contains;
/// `visible` and `superseded` describe the physical disc — the merged mount
/// view, plus the versions later sessions of the same disc hid. Each verifier
/// consumes the fields that match its substrate, so P6's staging verifier reads
/// only `latest_session` and P8's disc verifier reads only the other two.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscExpectation {
    pub project_name: String,
    pub disc_ordinal: u32,
    pub latest_session: BTreeMap<RelPath, ContentHash>,
    pub visible: BTreeMap<RelPath, ContentHash>,
    pub superseded: Vec<SupersededVersion>,
}

/// The result of checking one disc.
///
/// `skipped` means nothing was checked — no staged data, or (P8) an operator
/// who declined the disc. A skipped disc is never marked verified, which is the
/// whole reason the flag exists rather than "zero files, zero mismatches".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyOutcome {
    pub files_checked: usize,
    pub mismatches: Vec<RelPath>,
    pub skipped: bool,
}

/// Verifies a disc against the work-dir staging tree (the simulated backend).
///
/// The staging dir holds only the disc's **latest** session, so
/// `latest_session` is the reference; an absent `data/` directory is a skip.
pub struct StagingVerifier {
    staging_root: PathBuf,
}

impl StagingVerifier {
    pub fn new(staging_root: PathBuf) -> Self {
        StagingVerifier { staging_root }
    }

    pub fn verify(&self, exp: &DiscExpectation) -> Result<VerifyOutcome, BurnError> {
        let data_dir = self
            .staging_root
            .join(format!("disc-{}", exp.disc_ordinal))
            .join("data");
        if !data_dir.exists() {
            return Ok(VerifyOutcome {
                files_checked: 0,
                mismatches: Vec::new(),
                skipped: true,
            });
        }

        // Sorted, unlike the Scala original: `Files.walk` order is
        // unspecified, so a multi-mismatch report was not reproducible run to
        // run. Sorting can only make the rendered list stable, and every
        // oracle in the corpus has at most one mismatch.
        let mut files = Vec::new();
        for entry in walkdir::WalkDir::new(&data_dir).sort_by_file_name() {
            let entry = entry.map_err(|e| BurnError::Io(std::io::Error::other(e)))?;
            if entry.file_type().is_file() {
                files.push(entry.into_path());
            }
        }

        let mut mismatches = Vec::new();
        for file in &files {
            let rel = relativize(&data_dir, file);
            let actual = hasher::hash(file).map_err(BurnError::Io)?;
            if exp.latest_session.get(&rel) != Some(&actual) {
                mismatches.push(rel);
            }
        }
        mismatches.sort();

        Ok(VerifyOutcome {
            files_checked: files.len(),
            mismatches,
            skipped: false,
        })
    }
}

/// The verification backends. One variant now; P8 adds `Disc`.
pub enum Verifier {
    Staging(StagingVerifier),
}

impl Verifier {
    pub fn verify(&self, exp: &DiscExpectation) -> Result<VerifyOutcome, BurnError> {
        match self {
            Verifier::Staging(v) => v.verify(exp),
        }
    }
}
```

Notes for the implementer:

- `relativize` is `pub(crate)` in `crates/diskrement-shell/src/fs/scanner.rs` —
  reuse it rather than re-deriving `strip_prefix(..).display().to_string()`.
- `mismatches.sort()` needs `RelPath: Ord`, which it already derives
  (`crates/diskrement-core/src/model.rs:8`) — no change to core is needed.
- `walkdir::WalkDir::sort_by_file_name()` makes the walk itself deterministic,
  so `files_checked` counting is stable too; the explicit `mismatches.sort()`
  stays because it is the property the test asserts and the render depends on.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p diskrement-shell verifier`
Expected: PASS, 4 tests.

- [ ] **Step 5: Verify the whole workspace**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

- [ ] **Step 6: Commit**

```bash
git add crates/diskrement-shell/src/burn/verifier.rs crates/diskrement-shell/src/burn/mod.rs
git commit -m "feat(verify): the staging disc verifier and the closed Verifier enum"
```

---

### Task 2: The two disc-state pipeline errors

**Files:**
- Modify: `crates/diskrement-shell/src/error.rs`

`PipelineError.scala` has three disc-state variants. P6 needs two of them:
`UnknownDisc` (a named ordinal that does not exist) and `DiscIsDead` (a named
ordinal that does). `DiscAlreadyDead` belongs to `disc mark-dead` and lands in
P7 — do not add it here.

The ordinal is `i64`, not `u32`, and that is deliberate. zio-cli's
`Options.integer("disc")` accepts a negative value and lets it flow through to
the domain: the staged Scala launcher answers `verify p --disc -1` with
`no disc with ordinal -1` and exit 2, **not** an argument error. Exit codes are
contract (§3.2) and only clap's *wording* may drift, so the Rust port must reach
the same exit 2 with the same message. Carrying `i64` through the pipeline is
what makes that happen without a conversion that could fail.

- [ ] **Step 1: Write the failing test**

Add to the existing `#[cfg(test)] mod tests` in
`crates/diskrement-shell/src/error.rs`:

```rust
    #[test]
    fn disc_state_errors_are_exit_2_with_their_messages() {
        assert_eq!(PipelineError::UnknownDisc(9).exit_code(), 2);
        assert_eq!(PipelineError::DiscIsDead(3).exit_code(), 2);
        assert_eq!(
            PipelineError::UnknownDisc(99).to_string(),
            "no disc with ordinal 99"
        );
        assert_eq!(
            PipelineError::DiscIsDead(3).to_string(),
            "disc 3 is marked dead"
        );
    }

    /// zio-cli let a negative `--disc` reach the domain, so the contract
    /// answer is exit 2 with the ordinal echoed back — not an argument error.
    /// Verified against the staged Scala launcher.
    #[test]
    fn a_negative_ordinal_is_a_lookup_failure_not_an_argument_error() {
        let e = PipelineError::UnknownDisc(-1);
        assert_eq!(e.to_string(), "no disc with ordinal -1");
        assert_eq!(e.exit_code(), 2);
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p diskrement-shell error::tests`
Expected: FAIL to compile — `no variant named UnknownDisc`.

- [ ] **Step 3: Write the implementation**

In `crates/diskrement-shell/src/error.rs`, add the two variants to
`PipelineError` immediately after `StalePlan` (keeping the state/lookup family
together):

```rust
    /// A `--disc` ordinal that names no disc in this project.
    ///
    /// The ordinal is `i64` because zio-cli let a negative value through to
    /// the domain, and the exit ladder — not clap's argument parser — is the
    /// contract for what happens next.
    #[error("no disc with ordinal {0}")]
    UnknownDisc(i64),
    /// A `--disc` ordinal that names a disc marked dead. A *sweep* skips dead
    /// discs quietly; naming one is an assertion of intent, and answering it
    /// with a reassuring exit 0 would be a lie.
    #[error("disc {0} is marked dead")]
    DiscIsDead(i64),
```

and extend the exit-2 arm of `exit_code`:

```rust
            PipelineError::ProjectNotFound(_)
            | PipelineError::NoStagedScan(_)
            | PipelineError::NoStagedPlan(_)
            | PipelineError::SnapshotMissing(_)
            | PipelineError::UnknownDisc(_)
            | PipelineError::DiscIsDead(_)
            | PipelineError::StalePlan(_) => 2,
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p diskrement-shell error::tests`
Expected: PASS.

- [ ] **Step 5: Verify the whole workspace**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

- [ ] **Step 6: Commit**

```bash
git add crates/diskrement-shell/src/error.rs
git commit -m "feat(pipeline): the unknown-disc and dead-disc error contract"
```

---

### Task 3: `VerifyReport` and `Pipeline::verify`

**Files:**
- Modify: `crates/diskrement-shell/src/pipeline/reports.rs`
- Modify: `crates/diskrement-shell/src/pipeline/mod.rs`

Read `PipelineLive.scala`'s `def verify` and `private def verifyDisc` before
porting. Four behaviors carry over exactly:

1. A named ordinal is resolved **before** the dead-filter, so `UnknownDisc` and
   `DiscIsDead` reach the caller instead of an empty report.
2. A sweep filters dead discs out silently.
3. `markVerified` runs only when the check was not skipped **and** found no
   mismatch.
4. The session it marks is chosen from `sessionsByState(BURNED)`. A disc whose
   session is already VERIFIED contributes nothing to that list, so a repeat
   `verify` is a clean no-op rather than an illegal transition. This is not
   incidental — `fixtures/catalog/05-after-burn2.sqlite` already contains a
   VERIFIED session, and Task 5 verifies against it.

- [ ] **Step 1: Write the failing test**

Add `VerifyReport` to `crates/diskrement-shell/src/pipeline/reports.rs`, at the
end of the file:

```rust
/// One disc's verification result (`Pipeline.scala`'s `VerifyReport`).
///
/// `skipped` is not the same as "zero files checked": a sealed disc whose last
/// session placed no files is a legitimate clean zero-file check, while a disc
/// with no staged data at all was never looked at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyReport {
    pub disc_ordinal: u32,
    pub files_checked: usize,
    pub mismatches: Vec<diskrement_core::RelPath>,
    pub skipped: bool,
}
```

Then add a new test module at the end of
`crates/diskrement-shell/src/pipeline/mod.rs`:

```rust
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
            &[],
            DiscType::BdR,
            diskrement_core::Capacity::new(1_000_000),
            finalize,
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
        let verifier = Verifier::Staging(StagingVerifier::new(env.work.join("staging")));
        let mut pipeline = Pipeline::new(
            &mut env.catalog,
            PipelinePaths {
                work_dir: env.work.clone(),
                scan_cache: env.scan_cache.clone(),
            },
            &NoopSink,
        );
        pipeline.verify("p", disc, &verifier)
    }

    fn verified_discs(env: &Env) -> Vec<u32> {
        env.catalog
            .sessions_by_state(ProjectId(1), SessionState::Verified)
            .unwrap()
            .iter()
            .map(|s| s.disc_ordinal)
            .collect()
    }

    #[test]
    fn a_clean_disc_reports_no_mismatches_and_becomes_verified() {
        let mut env = burned_env(&[("a.txt", b"aaa")], false);
        let reports = run_verify(&mut env, None).unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].disc_ordinal, 1);
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
        assert_eq!(
            reports[0].mismatches,
            vec![RelPath("a.txt".to_string())]
        );
        assert_eq!(reports[0].files_checked, 1);
        assert!(verified_discs(&env).is_empty(), "a mismatch must not verify");
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
            reports.iter().map(|r| r.disc_ordinal).collect::<Vec<_>>(),
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
        let verifier = Verifier::Staging(StagingVerifier::new(env.work.join("staging")));
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
        let mut env = burned_env(&[], true);
        let reports = run_verify(&mut env, None).unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].files_checked, 0);
        assert!(!reports[0].skipped);
        assert!(reports[0].mismatches.is_empty());
        assert_eq!(verified_discs(&env), vec![1]);
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p diskrement-shell verify_tests`
Expected: FAIL to compile — `no method named verify found for struct Pipeline`.

- [ ] **Step 3: Write the implementation**

Extend the imports at the top of
`crates/diskrement-shell/src/pipeline/mod.rs`:

```rust
use crate::burn::verifier::{DiscExpectation, Verifier};
```

add `Session` to the existing `crate::catalog::model` import list, and add
`VerifyReport` to the `reports::{...}` import line.

Then add these two methods to `impl<'a> Pipeline<'a>`, immediately after
`status`:

```rust
    /// `verify`: re-hash what each live disc should hold, and record a clean
    /// disc as VERIFIED (`PipelineLive.verify`).
    ///
    /// An explicitly named ordinal is an assertion of intent about *that*
    /// disc, so it is resolved before the dead-filter: silently reporting
    /// "nothing to verify" (exit 0) for a dead or nonexistent disc would read
    /// as reassurance. A sweep still skips dead discs quietly, which is the
    /// right default.
    ///
    /// `disc_ordinal` is `i64` rather than `u32` because zio-cli let a
    /// negative `--disc` reach the domain; the answer is a lookup failure with
    /// the ordinal echoed back, not an argument error.
    pub fn verify(
        &mut self,
        name: &str,
        disc_ordinal: Option<i64>,
        verifier: &Verifier,
    ) -> Result<Vec<VerifyReport>, PipelineError> {
        let p = self.project(name)?;
        let states = self.catalog.read_disc_states(p.id).map_err(Self::failed)?;
        let sessions = self
            .catalog
            .sessions_by_state(p.id, SessionState::Burned)
            .map_err(Self::failed)?;

        if let Some(o) = disc_ordinal {
            match states.iter().find(|s| i64::from(s.ordinal) == o) {
                None => return Err(PipelineError::UnknownDisc(o)),
                Some(s) if s.status == DiscStatus::Dead => {
                    return Err(PipelineError::DiscIsDead(o))
                }
                Some(_) => {}
            }
        }

        let targets: Vec<u32> = states
            .iter()
            .filter(|s| s.status != DiscStatus::Dead)
            .map(|s| s.ordinal)
            .filter(|o| disc_ordinal.is_none_or(|want| want == i64::from(*o)))
            .collect();

        let mut reports = Vec::with_capacity(targets.len());
        for ordinal in targets {
            reports.push(self.verify_disc(&p, ordinal, &sessions, verifier)?);
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
        ordinal: u32,
        sessions: &[Session],
        verifier: &Verifier,
    ) -> Result<VerifyReport, PipelineError> {
        let latest_session = self
            .catalog
            .placements_for_disc(p.id, ordinal)
            .map_err(Self::failed)?;
        let recorded = self
            .catalog
            .disc_recorded_contents(p.id, ordinal)
            .map_err(Self::failed)?;
        let expectation = DiscExpectation {
            project_name: p.name.clone(),
            disc_ordinal: ordinal,
            latest_session,
            visible: recorded.visible,
            superseded: recorded.superseded,
        };

        let outcome = verifier.verify(&expectation).map_err(PipelineError::Burn)?;
        if !outcome.skipped && outcome.mismatches.is_empty() {
            if let Some(session) = sessions
                .iter()
                .filter(|s| s.disc_ordinal == ordinal)
                .max_by_key(|s| s.ordinal)
            {
                self.catalog
                    .mark_verified(p.id, ordinal, session.ordinal)
                    .map_err(Self::failed)?;
            }
        }

        Ok(VerifyReport {
            disc_ordinal: ordinal,
            files_checked: outcome.files_checked,
            mismatches: outcome.mismatches,
            skipped: outcome.skipped,
        })
    }
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p diskrement-shell verify_tests`
Expected: PASS, 8 tests.

- [ ] **Step 5: Verify the whole workspace**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

- [ ] **Step 6: Commit**

```bash
git add crates/diskrement-shell/src/pipeline/reports.rs crates/diskrement-shell/src/pipeline/mod.rs
git commit -m "feat(pipeline): verify a burned disc against its recorded contents"
```

---

### Task 4: `Pipeline::history` and `Pipeline::reset`

**Files:**
- Modify: `crates/diskrement-shell/src/pipeline/mod.rs`

`history` is a catalog passthrough behind the project lookup. `reset` is four
deletions in a fixed order (`PipelineLive.reset`): the staged scan/plan, the
temporary `burn-catalog.sqlite`, the `staging/` trees, and the snapshot. It
deliberately does **not** touch the catalog or the fingerprint sidecar — a reset
discards a cycle in progress, not history.

- [ ] **Step 1: Write the failing test**

Add to the `verify_tests` module created in Task 3:

```rust
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
        assert_eq!(sessions[0].disc_ordinal, 1);
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
        assert_eq!(entries[0].disc_ordinal, 1);
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
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p diskrement-shell verify_tests`
Expected: FAIL to compile — `no method named reset` / `no method named history`.

- [ ] **Step 3: Write the implementation**

Add `FileHistoryEntry` to the `crate::catalog::model` import list in
`crates/diskrement-shell/src/pipeline/mod.rs`, then add these two methods to
`impl<'a> Pipeline<'a>` after `verify_disc`:

```rust
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
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p diskrement-shell verify_tests`
Expected: PASS, 15 tests in the module.

- [ ] **Step 5: Verify the whole workspace**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

- [ ] **Step 6: Commit**

```bash
git add crates/diskrement-shell/src/pipeline/mod.rs
git commit -m "feat(pipeline): file history and the work-dir reset"
```

---

### Task 5: The `verify` corpus oracle

**Files:**
- Create: `crates/diskrement-shell/tests/fixture_verify.rs`

This is the phase's cross-implementation proof for `verify`: a Rust sweep over a
catalog and staging trees the **Scala** build wrote, agreeing about which files
are on which disc and about which session becomes VERIFIED.

The pairing is exact. `fixtures/work/02-after-plan3/` is the work dir as it
stood after `plan` 3, so its `staging/` holds the trees left by burns 1 and 2 —
and `fixtures/catalog/05-after-burn2.sqlite` is the catalog at that same moment.
Disc 1's session is already VERIFIED there (run-manifest step 05 verified it),
which makes the same fixture pin the repeat-verify no-op for free.

- [ ] **Step 1: Write the failing test**

Create `crates/diskrement-shell/tests/fixture_verify.rs`:

```rust
//! `verify` against the harvested corpus (spec §4).
//!
//! The Rust sweep reads a Scala-written catalog and Scala-staged disc trees and
//! must agree about both halves: which files each disc holds, and which
//! sessions the check is allowed to advance to VERIFIED.

mod support;

use diskrement_shell::burn::verifier::{StagingVerifier, Verifier};
use diskrement_shell::catalog::model::{ProjectId, SessionState};
use diskrement_shell::catalog::Catalog;
use diskrement_shell::error::PipelineError;
use diskrement_shell::pipeline::{Pipeline, PipelinePaths};
use diskrement_shell::progress::NoopSink;

use support::{fixture_copy, fixture_work_copy};

struct Env {
    _catalog_dir: tempfile::TempDir,
    _work_dir: tempfile::TempDir,
    catalog: Catalog,
    work: std::path::PathBuf,
}

/// Copy one catalog snapshot and one work dir out of the corpus and open the
/// copy. Never `Catalog::open` a fixture in place: it runs the migration
/// runner and writes.
fn env(catalog_rel: &str, work_rel: &str) -> Env {
    let (catalog_dir, catalog_path) = fixture_copy(catalog_rel);
    let (work_dir, work) = fixture_work_copy(work_rel);
    let catalog = Catalog::open(&catalog_path).expect("the copied catalog opens");
    Env {
        _catalog_dir: catalog_dir,
        _work_dir: work_dir,
        catalog,
        work,
    }
}

fn sweep(env: &mut Env, disc: Option<i64>) -> Result<Vec<diskrement_shell::pipeline::reports::VerifyReport>, PipelineError> {
    let verifier = Verifier::Staging(StagingVerifier::new(env.work.join("staging")));
    let mut pipeline = Pipeline::new(
        &mut env.catalog,
        PipelinePaths {
            work_dir: env.work.clone(),
            scan_cache: env.work.join("unused.scancache.sqlite"),
        },
        &NoopSink,
    );
    pipeline.verify("p", disc, &verifier)
}

fn discs_in(env: &Env, state: SessionState) -> Vec<u32> {
    env.catalog
        .sessions_by_state(ProjectId(1), state)
        .expect("sessions")
        .iter()
        .map(|s| s.disc_ordinal)
        .collect()
}

/// The Scala build staged these trees and recorded these placements; the Rust
/// verifier must find them clean.
#[test]
fn a_sweep_over_the_corpus_staging_trees_is_clean_on_every_disc() {
    let mut env = env("catalog/05-after-burn2.sqlite", "work/02-after-plan3");
    let reports = sweep(&mut env, None).expect("verify succeeds");

    assert_eq!(
        reports
            .iter()
            .map(|r| (r.disc_ordinal, r.files_checked, r.skipped))
            .collect::<Vec<_>>(),
        vec![(1, 1, false), (2, 1, false)]
    );
    assert!(
        reports.iter().all(|r| r.mismatches.is_empty()),
        "{reports:?}"
    );
}

/// Disc 1's session was already VERIFIED when the corpus was harvested, so the
/// sweep must advance only disc 2 and leave disc 1 alone — the repeat-verify
/// no-op, pinned against real Scala-written state.
#[test]
fn the_sweep_advances_only_the_still_burned_session() {
    let mut env = env("catalog/05-after-burn2.sqlite", "work/02-after-plan3");
    assert_eq!(discs_in(&env, SessionState::Verified), vec![1]);
    assert_eq!(discs_in(&env, SessionState::Burned), vec![2]);

    sweep(&mut env, None).expect("verify succeeds");

    assert_eq!(discs_in(&env, SessionState::Verified), vec![1, 2]);
    assert!(discs_in(&env, SessionState::Burned).is_empty());
}

/// A corrupted byte in a staged file is caught, and the disc is not advanced.
#[test]
fn a_corrupted_corpus_payload_is_caught_and_blocks_the_transition() {
    let mut env = env("catalog/05-after-burn2.sqlite", "work/02-after-plan3");
    std::fs::write(env.work.join("staging/disc-2/data/b.bin"), b"CORRUPT")
        .expect("the work dir copy is writable");

    let reports = sweep(&mut env, None).expect("verify succeeds");
    let disc2 = reports
        .iter()
        .find(|r| r.disc_ordinal == 2)
        .expect("disc 2 is in the sweep");
    assert_eq!(
        disc2.mismatches,
        vec![diskrement_core::RelPath("b.bin".to_string())]
    );
    assert_eq!(discs_in(&env, SessionState::Burned), vec![2], "not advanced");
}

/// `DEAD` is derived from `disc_event`, and the corpus's disc 3 proves it: the
/// `disc.status` column still reads OPEN there. A sweep skips it silently.
#[test]
fn a_sweep_skips_a_dead_disc_without_mentioning_it() {
    let mut env = env("catalog/08-after-mark-dead.sqlite", "work/02-after-plan3");
    let reports = sweep(&mut env, None).expect("verify succeeds");
    assert_eq!(
        reports.iter().map(|r| r.disc_ordinal).collect::<Vec<_>>(),
        vec![1, 2],
        "disc 3 is dead and must not appear"
    );
}

/// Naming a dead disc is an assertion of intent about it, and gets an answer.
#[test]
fn naming_the_dead_disc_is_an_error_rather_than_a_silent_success() {
    let mut env = env("catalog/08-after-mark-dead.sqlite", "work/02-after-plan3");
    let err = sweep(&mut env, Some(3)).expect_err("a dead disc is refused");
    assert!(matches!(err, PipelineError::DiscIsDead(3)), "{err}");
    assert_eq!(err.to_string(), "disc 3 is marked dead");
    assert_eq!(err.exit_code(), 2);
}

#[test]
fn naming_an_ordinal_the_corpus_never_had_is_unknown_disc() {
    let mut env = env("catalog/05-after-burn2.sqlite", "work/02-after-plan3");
    let err = sweep(&mut env, Some(99)).expect_err("an unknown disc is refused");
    assert!(matches!(err, PipelineError::UnknownDisc(99)), "{err}");
    assert_eq!(err.exit_code(), 2);
}
```

- [ ] **Step 2: Run the test to verify it fails, then passes**

Run: `cargo test -p diskrement-shell --test fixture_verify`

If Tasks 1–4 landed correctly this compiles and passes on the first run. If it
does not, the failure is real: fix the implementation, never the fixture, and
never widen an assertion to make it green.

Expected: PASS, 6 tests.

- [ ] **Step 3: Confirm the corpus is untouched**

```bash
git status --porcelain fixtures/
```

Expected: empty. A `-wal` or `-shm` file appearing under `fixtures/` means a
fixture database was opened without `fixture_copy` — fix the test, not
`SHA256SUMS`.

- [ ] **Step 4: Verify the whole workspace**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

- [ ] **Step 5: Commit**

```bash
git add crates/diskrement-shell/tests/fixture_verify.rs
git commit -m "test(verify): sweep the corpus staging trees against the harvested catalogs"
```

---

### Task 6: `render::verify_report`, `render::history`, `render::reset_done`, `render::verify_exit`

**Files:**
- Modify: `crates/diskrement-cli/src/render.rs`

Read `Render.scala`'s `verify`, `history` and `verifyExit`, and `Runner.scala`'s
`DiskrementCommand.Reset` case (the `Reset <name>.` line lives there, not in
`Render`). Column spacing is contract and easy to lose in a reformat:

- history: `"  v"`, the index, **two** spaces, `disc N / session M`, **three**
  spaces, the GB column, **two** spaces, the hash, **two** spaces, the status.
- verify clean: `disc N: M files OK ✓` — note `files`, plural regardless of `M`.
- verify mismatch: `disc N: M checked, K MISMATCH:` then one `"  " + path` line
  per mismatch.

Every expected string below is a transcript of the staged Scala launcher or of
`fixtures/run-manifest.txt`.

- [ ] **Step 1: Write the failing test**

Add a new test module at the end of `crates/diskrement-cli/src/render.rs`:

```rust
#[cfg(test)]
mod p6_render_tests {
    use super::*;
    use diskrement_core::{ContentHash, RelPath};
    use diskrement_shell::catalog::model::{FileHistoryEntry, VersionStatus};
    use diskrement_shell::pipeline::reports::VerifyReport;

    fn report(ordinal: u32, checked: usize, mismatches: &[&str], skipped: bool) -> VerifyReport {
        VerifyReport {
            disc_ordinal: ordinal,
            files_checked: checked,
            mismatches: mismatches
                .iter()
                .map(|m| RelPath((*m).to_string()))
                .collect(),
            skipped,
        }
    }

    fn entry(
        hash: &str,
        size: u64,
        disc: u32,
        session: u32,
        status: VersionStatus,
    ) -> FileHistoryEntry {
        FileHistoryEntry {
            content_hash: ContentHash(hash.to_string()),
            size_bytes: size,
            disc_ordinal: disc,
            session_ordinal: session,
            status,
        }
    }

    #[test]
    fn an_empty_verify_says_there_is_nothing_to_check() {
        assert_eq!(verify_report(&[]), "Nothing to verify: no burned discs.");
    }

    /// `fixtures/run-manifest.txt` step 05, byte for byte.
    #[test]
    fn a_clean_disc_matches_the_render_oracle() {
        assert_eq!(
            verify_report(&[report(1, 1, &[], false)]),
            "disc 1: 1 files OK ✓"
        );
    }

    /// `fixtures/run-manifest.txt` step 21, byte for byte — including the
    /// sealed disc's zero-file line and the absence of the dead disc 3.
    #[test]
    fn a_three_disc_sweep_matches_the_render_oracle() {
        assert_eq!(
            verify_report(&[
                report(1, 1, &[], false),
                report(2, 1, &[], false),
                report(4, 0, &[], false),
            ]),
            "disc 1: 1 files OK ✓\n\
             disc 2: 1 files OK ✓\n\
             disc 4: 0 files OK ✓"
        );
    }

    #[test]
    fn a_skipped_disc_is_reported_as_not_checked() {
        assert_eq!(
            verify_report(&[report(3, 0, &[], true)]),
            "disc 3: skipped (not checked)"
        );
    }

    /// Transcribed from the staged Scala launcher on a corrupted staging tree.
    #[test]
    fn mismatches_are_listed_under_their_disc() {
        assert_eq!(
            verify_report(&[report(1, 2, &["a.bin"], false)]),
            "disc 1: 2 checked, 1 MISMATCH:\n  a.bin"
        );
        assert_eq!(
            verify_report(&[report(2, 5, &["bad/file.jpg", "other.raw"], false)]),
            "disc 2: 5 checked, 2 MISMATCH:\n  bad/file.jpg\n  other.raw"
        );
    }

    #[test]
    fn a_mixed_sweep_puts_one_disc_per_block_in_order() {
        assert_eq!(
            verify_report(&[
                report(1, 1, &[], false),
                report(2, 0, &[], true),
                report(3, 2, &["gone.bin"], false),
            ]),
            "disc 1: 1 files OK ✓\n\
             disc 2: skipped (not checked)\n\
             disc 3: 2 checked, 1 MISMATCH:\n  gone.bin"
        );
    }

    /// A verify that finds corruption is a *successful run with a bad
    /// outcome*: exit 4, not an error exit (`Render.verifyExit`).
    #[test]
    fn the_verify_exit_code_is_driven_by_the_result() {
        assert_eq!(verify_exit(&[]), 0);
        assert_eq!(verify_exit(&[report(1, 10, &[], false)]), 0);
        assert_eq!(verify_exit(&[report(1, 0, &[], true)]), 0);
        assert_eq!(
            verify_exit(&[report(1, 10, &[], false), report(2, 5, &["bad"], false)]),
            4
        );
    }

    #[test]
    fn an_empty_history_names_the_path() {
        assert_eq!(
            history(&RelPath("nope.bin".to_string()), &[]),
            "No history for nope.bin."
        );
    }

    /// `fixtures/run-manifest.txt` step 16, byte for byte. LOST is per
    /// physical copy: v2 and v3 share a content hash and read differently.
    #[test]
    fn the_three_version_history_matches_the_render_oracle() {
        let a1 = "8e19511b7991430a1950f27907234183151bb0dbf1fbf28cd5b765088feb9f70";
        let a2 = "e148cf0e8c66a7c18ea6be52a5215dd2a770d74b6c014d51d1e1e0463e882e77";
        assert_eq!(
            history(
                &RelPath("a.bin".to_string()),
                &[
                    entry(a1, 800, 1, 1, VersionStatus::Superseded),
                    entry(a2, 800, 3, 1, VersionStatus::Lost),
                    entry(a2, 800, 4, 1, VersionStatus::Current),
                ]
            ),
            format!(
                "a.bin\n\
                 \x20 v1  disc 1 / session 1   0.0 GB  {a1}  SUPERSEDED\n\
                 \x20 v2  disc 3 / session 1   0.0 GB  {a2}  LOST\n\
                 \x20 v3  disc 4 / session 1   0.0 GB  {a2}  CURRENT"
            )
        );
    }

    /// `fixtures/run-manifest.txt` step 22, byte for byte.
    #[test]
    fn a_deleted_paths_history_matches_the_render_oracle() {
        let b1 = "3413ff9e166e40ad8798e36609f28433496ee3a0f8984f73d33b0a80ffef2a89";
        assert_eq!(
            history(
                &RelPath("b.bin".to_string()),
                &[entry(b1, 800, 2, 1, VersionStatus::Deleted)]
            ),
            format!("b.bin\n\x20 v1  disc 2 / session 1   0.0 GB  {b1}  DELETED")
        );
    }

    #[test]
    fn the_history_gb_column_uses_the_same_one_decimal_format() {
        assert_eq!(
            history(
                &RelPath("big/movie.mkv".to_string()),
                &[entry("a1b2c3", 12_300_000_000, 7, 2, VersionStatus::Current)]
            ),
            "big/movie.mkv\n\x20 v1  disc 7 / session 2   12.3 GB  a1b2c3  CURRENT"
        );
    }

    #[test]
    fn reset_names_the_project_it_cleared() {
        assert_eq!(reset_done("p"), "Reset p.");
        assert_eq!(reset_done("photos"), "Reset photos.");
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p diskrement-cli p6_render_tests`
Expected: FAIL to compile — `cannot find function verify_report` / `history` /
`reset_done` / `verify_exit` in this scope.

- [ ] **Step 3: Write the implementation**

Add to `crates/diskrement-cli/src/render.rs`, after `status_report` and before
the first `#[cfg(test)]` module. The `#[allow(dead_code)]` attributes are the
convention this file already uses for a renderer that lands one task before its
dispatch arm; Task 8 removes them.

```rust
/// `Render.verify`. One block per disc, in sweep order; an empty sweep is a
/// sentence rather than a blank line.
#[allow(dead_code)]
pub fn verify_report(reports: &[VerifyReport]) -> String {
    if reports.is_empty() {
        return "Nothing to verify: no burned discs.".to_string();
    }
    reports
        .iter()
        .map(|v| {
            if v.skipped {
                format!("disc {}: skipped (not checked)", v.disc_ordinal)
            } else if v.mismatches.is_empty() {
                // "files" is not pluralised in the Scala original, and
                // run-manifest step 21 prints "disc 4: 0 files OK".
                format!("disc {}: {} files OK ✓", v.disc_ordinal, v.files_checked)
            } else {
                let listed = v
                    .mismatches
                    .iter()
                    .map(|p| format!("  {}", p.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n");
                format!(
                    "disc {}: {} checked, {} MISMATCH:\n{listed}",
                    v.disc_ordinal,
                    v.files_checked,
                    v.mismatches.len()
                )
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Result-driven exit (`Render.verifyExit`): a verify that finds corruption is
/// a **successful run with a bad outcome**, so it exits 4 rather than taking
/// the error ladder. A skipped disc found nothing and is not corruption.
#[allow(dead_code)]
pub fn verify_exit(reports: &[VerifyReport]) -> u8 {
    if reports.iter().any(|r| !r.mismatches.is_empty()) {
        4
    } else {
        0
    }
}

/// `Render.history`. The path, then one `v<n>` line per version, oldest first.
///
/// The column spacing is contract: two spaces after `v<n>`, **three** after the
/// session, two around the hash.
#[allow(dead_code)]
pub fn history(rel_path: &RelPath, entries: &[FileHistoryEntry]) -> String {
    if entries.is_empty() {
        return format!("No history for {}.", rel_path.as_str());
    }
    let mut lines = vec![rel_path.as_str().to_string()];
    for (i, e) in entries.iter().enumerate() {
        lines.push(format!(
            "  v{}  disc {} / session {}   {}  {}  {}",
            i + 1,
            e.disc_ordinal,
            e.session_ordinal,
            gb(e.size_bytes),
            e.content_hash,
            e.status
        ));
    }
    lines.join("\n")
}

/// The `reset` confirmation. It lives in `Runner.scala`'s dispatch rather than
/// in `Render.scala`; it is a renderer here so the snapshot gallery can cover
/// every line the CLI prints.
#[allow(dead_code)]
pub fn reset_done(name: &str) -> String {
    format!("Reset {name}.")
}
```

Extend the imports at the top of the file:

```rust
use diskrement_core::RelPath;
use diskrement_shell::catalog::model::{FileHistoryEntry, Project, ProjectId};
use diskrement_shell::pipeline::reports::{
    BurnReport, PlanReport, ScanReport, StatusReport, VerifyReport,
};
```

`ContentHash` and `VersionStatus` both implement `Display` (checked:
`crates/diskrement-core/src/model.rs` and
`crates/diskrement-shell/src/catalog/model.rs:188`), so `{}` on them is the
Scala `${e.contentHash.value}` / `${e.status}` interpolation exactly.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p diskrement-cli p6_render_tests`
Expected: PASS, 12 tests.

- [ ] **Step 5: Verify the whole workspace**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

- [ ] **Step 6: Commit**

```bash
git add crates/diskrement-cli/src/render.rs
git commit -m "feat(cli): render verify, history and reset"
```

---

### Task 7: The `insta` snapshot gallery over every renderer

**Files:**
- Modify: `.gitignore`
- Modify: `crates/diskrement-cli/src/render.rs`
- Create: `crates/diskrement-cli/src/snapshots/init.snap`
- Create: `crates/diskrement-cli/src/snapshots/scan.snap`
- Create: `crates/diskrement-cli/src/snapshots/plan.snap`
- Create: `crates/diskrement-cli/src/snapshots/status.snap`
- Create: `crates/diskrement-cli/src/snapshots/burn.snap`
- Create: `crates/diskrement-cli/src/snapshots/verify.snap`
- Create: `crates/diskrement-cli/src/snapshots/history.snap`
- Create: `crates/diskrement-cli/src/snapshots/reset.snap`

The P6 exit bar asks for insta snapshots covering **all** CLI output, not only
this phase's. One gallery per renderer, so an unrelated change shows up as a
diff on one file rather than on all eight.

What a snapshot is and is not: it pins Rust against Rust and catches accidental
drift in a refactor. It **cannot** catch a wording divergence from the Scala
build — the `assert_eq!` oracles in Task 6 and `fixtures/run-manifest.txt` do
that, and nothing here replaces them.

**Author every `.snap` by hand from the text below.** Do not run
`cargo insta review` or `--accept`, and do not set `INSTA_UPDATE`: a snapshot
the tool generated from the code under test asserts nothing. Insta compares
snapshot *content*; the `source`/`expression` header fields are informational.

- [ ] **Step 1: Ignore insta's pending files**

Append to `.gitignore`:

```
*.snap.new
```

- [ ] **Step 2: Write the failing test**

Add a new test module at the end of `crates/diskrement-cli/src/render.rs`:

```rust
/// Snapshot galleries covering every line the CLI prints.
///
/// These pin Rust against Rust: they catch drift introduced by a refactor, and
/// they cannot catch a divergence from the Scala build — `run-manifest.txt` and
/// the `assert_eq!` oracles above are what do that. Snapshots are authored by
/// hand for the same reason; one generated from the code under test would
/// assert nothing.
#[cfg(test)]
mod snapshots {
    use super::*;
    use chrono::TimeZone as _;
    use diskrement_core::{
        ContentHash, DiscState, DiscStatus, DiscType, PlannedDisc, RelPath, ScannedFile,
    };
    use diskrement_shell::catalog::model::{FileHistoryEntry, VersionStatus};
    use diskrement_shell::pipeline::reports::{
        BurnReport, DiscBurn, PlanReport, RemainingEstimate, ScanReport, StatusReport,
        VerifyReport,
    };

    /// Join labelled renders into one reviewable document.
    fn gallery(sections: &[(&str, String)]) -> String {
        sections
            .iter()
            .map(|(title, body)| format!("## {title}\n{body}"))
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// `prepend_module_to_snapshot => false` makes the file name exactly
    /// `<name>.snap`, so the committed paths are predictable and hand-writable.
    fn check(name: &str, doc: String) {
        insta::with_settings!({
            snapshot_path => "snapshots",
            prepend_module_to_snapshot => false,
        }, {
            insta::assert_snapshot!(name, doc);
        });
    }

    fn ts(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap()
    }

    fn scanned(rel: &str, size: u64) -> ScannedFile {
        ScannedFile {
            rel_path: RelPath(rel.to_string()),
            content_hash: ContentHash("deadbeef".to_string()),
            size_bytes: size,
            mtime: ts(2026, 9, 5, 14, 3, 0),
        }
    }

    fn scan_of(a: usize, m: usize, d: usize, u: usize, hashed: Option<usize>, total: usize) -> ScanReport {
        ScanReport {
            added: a,
            modified: m,
            deleted: d,
            unchanged: u,
            hashed,
            scanned_files: total,
        }
    }

    fn plan_of(
        disc: PlannedDisc,
        tombstones: &[&str],
        remaining: (usize, usize, u64),
        oversized: Vec<ScannedFile>,
    ) -> PlanReport {
        PlanReport {
            disc,
            disc_type: DiscType::BdR,
            tombstones: tombstones
                .iter()
                .map(|t| RelPath((*t).to_string()))
                .collect(),
            remaining: RemainingEstimate {
                more_discs: remaining.0,
                more_files: remaining.1,
                more_bytes: remaining.2,
            },
            oversized,
        }
    }

    fn disc(
        ordinal: u32,
        status: DiscStatus,
        used: u64,
        usable: u64,
    ) -> DiscState {
        DiscState {
            ordinal,
            status,
            usable_bytes: usable,
            used_bytes: used,
        }
    }

    fn project(name: &str, source: &str) -> Project {
        Project {
            id: ProjectId(1),
            name: name.to_string(),
            source_root: source.to_string(),
            work_dir: "/srv/work".to_string(),
            default_disc_type: DiscType::BdR,
            margin_percent: 2.0,
            copies: 1,
            created_at: ts(2026, 1, 1, 0, 0, 0),
        }
    }

    fn burn_of(ordinal: u32, label: &str, finalized: bool, more_discs: usize) -> BurnReport {
        BurnReport {
            disc: DiscBurn {
                ordinal,
                label: label.to_string(),
                finalized,
                staging_dir: "/srv/work/staging/disc-1".into(),
                image_bytes: 4096,
            },
            remaining: RemainingEstimate {
                more_discs,
                more_files: 1,
                more_bytes: 800,
            },
        }
    }

    fn verify_of(ordinal: u32, checked: usize, mismatches: &[&str], skipped: bool) -> VerifyReport {
        VerifyReport {
            disc_ordinal: ordinal,
            files_checked: checked,
            mismatches: mismatches
                .iter()
                .map(|m| RelPath((*m).to_string()))
                .collect(),
            skipped,
        }
    }

    fn version(
        hash: &str,
        size: u64,
        d: u32,
        s: u32,
        status: VersionStatus,
    ) -> FileHistoryEntry {
        FileHistoryEntry {
            content_hash: ContentHash(hash.to_string()),
            size_bytes: size,
            disc_ordinal: d,
            session_ordinal: s,
            status,
        }
    }

    #[test]
    fn init_gallery() {
        check(
            "init",
            gallery(&[
                ("a fresh project", init_done("photos", ProjectId(7))),
                ("a multi-digit id", init_done("archive", ProjectId(42))),
            ]),
        );
    }

    #[test]
    fn scan_gallery() {
        check(
            "scan",
            gallery(&[
                ("a fresh scan", scan_report(&scan_of(2, 1, 0, 3, Some(3), 6))),
                (
                    "a reused staged scan (no Hashed line)",
                    scan_report(&scan_of(2, 1, 0, 3, None, 6)),
                ),
                ("nothing changed", scan_report(&scan_of(0, 0, 0, 6, Some(0), 6))),
            ]),
        );
    }

    #[test]
    fn plan_gallery() {
        check(
            "plan",
            gallery(&[
                (
                    "nothing to do",
                    plan_report(&plan_of(
                        PlannedDisc {
                            ordinal: 1,
                            is_new: false,
                            will_finalize: false,
                            files: vec![],
                            prior_used_bytes: 0,
                            usable_bytes: 25_000_000_000,
                        },
                        &[],
                        (0, 0, 0),
                        vec![],
                    )),
                ),
                (
                    "a bare seal (run-manifest step 18)",
                    plan_report(&plan_of(
                        PlannedDisc {
                            ordinal: 4,
                            is_new: false,
                            will_finalize: true,
                            files: vec![],
                            prior_used_bytes: 800,
                            usable_bytes: 1_000,
                        },
                        &[],
                        (0, 0, 0),
                        vec![],
                    )),
                ),
                (
                    "a spilling plan (run-manifest step 03)",
                    plan_report(&plan_of(
                        PlannedDisc {
                            ordinal: 1,
                            is_new: true,
                            will_finalize: true,
                            files: vec![scanned("a.bin", 800)],
                            prior_used_bytes: 0,
                            usable_bytes: 1_000,
                        },
                        &[],
                        (1, 1, 800),
                        vec![],
                    )),
                ),
                (
                    "appending to an open disc",
                    plan_report(&plan_of(
                        PlannedDisc {
                            ordinal: 2,
                            is_new: false,
                            will_finalize: false,
                            files: vec![scanned("a.bin", 3_200_000_000)],
                            prior_used_bytes: 10_000_000_000,
                            usable_bytes: 25_000_000_000,
                        },
                        &[],
                        (0, 0, 0),
                        vec![],
                    )),
                ),
                (
                    "a deletion rides along (run-manifest step 10)",
                    plan_report(&plan_of(
                        PlannedDisc {
                            ordinal: 3,
                            is_new: true,
                            will_finalize: false,
                            files: vec![scanned("a.bin", 800)],
                            prior_used_bytes: 0,
                            usable_bytes: 1_000,
                        },
                        &["b.bin"],
                        (0, 0, 0),
                        vec![],
                    )),
                ),
                (
                    "oversized files",
                    plan_report(&plan_of(
                        PlannedDisc {
                            ordinal: 1,
                            is_new: true,
                            will_finalize: false,
                            files: vec![scanned("small.bin", 10)],
                            prior_used_bytes: 0,
                            usable_bytes: 25_000_000_000,
                        },
                        &[],
                        (0, 0, 0),
                        vec![scanned("huge.bin", 99_000_000_000)],
                    )),
                ),
            ]),
        );
    }

    #[test]
    fn status_gallery() {
        let two = StatusReport {
            discs: vec![
                disc(1, DiscStatus::Finalized, 24_800_000_000, 25_000_000_000),
                disc(2, DiscStatus::Open, 3_200_000_000, 25_000_000_000),
            ],
            open_disc_ordinal: Some(2),
            total_current_files: 556,
            last_burn_at: Some(ts(2026, 6, 21, 14, 3, 0)),
        };
        let fresh = StatusReport {
            discs: vec![],
            open_disc_ordinal: None,
            total_current_files: 0,
            last_burn_at: None,
        };
        let with_dead = StatusReport {
            discs: vec![
                disc(1, DiscStatus::Finalized, 0, 0),
                disc(2, DiscStatus::Open, 0, 0),
                disc(3, DiscStatus::Dead, 0, 0),
                disc(4, DiscStatus::Finalized, 0, 0),
            ],
            open_disc_ordinal: Some(2),
            total_current_files: 1,
            last_burn_at: Some(ts(2026, 9, 5, 20, 1, 3)),
        };
        check(
            "status",
            gallery(&[
                (
                    "two discs, one open",
                    status_report(&project("photos", "/srv/photos"), &two),
                ),
                (
                    "nothing burned yet",
                    status_report(&project("photos", "/srv/photos"), &fresh),
                ),
                (
                    "a dead disc in the table (run-manifest step 20 shape)",
                    status_report(&project("p", "/srv/source"), &with_dead),
                ),
            ]),
        );
    }

    #[test]
    fn burn_gallery() {
        check(
            "burn",
            gallery(&[
                (
                    "a finalized disc with more to come (run-manifest step 04)",
                    burn_report(&burn_of(1, "p disc 1", true, 1)),
                ),
                (
                    "an open disc, set complete (run-manifest step 08)",
                    burn_report(&burn_of(2, "p disc 2", false, 0)),
                ),
            ]),
        );
    }

    #[test]
    fn verify_gallery() {
        check(
            "verify",
            gallery(&[
                ("nothing burned", verify_report(&[])),
                (
                    "a clean sweep (run-manifest step 21)",
                    verify_report(&[
                        verify_of(1, 1, &[], false),
                        verify_of(2, 1, &[], false),
                        verify_of(4, 0, &[], false),
                    ]),
                ),
                ("a skipped disc", verify_report(&[verify_of(3, 0, &[], true)])),
                (
                    "mismatches",
                    verify_report(&[verify_of(2, 5, &["bad/file.jpg", "other.raw"], false)]),
                ),
                (
                    "a mixed sweep",
                    verify_report(&[
                        verify_of(1, 1, &[], false),
                        verify_of(2, 0, &[], true),
                        verify_of(3, 2, &["gone.bin"], false),
                    ]),
                ),
            ]),
        );
    }

    #[test]
    fn history_gallery() {
        let a1 = "8e19511b7991430a1950f27907234183151bb0dbf1fbf28cd5b765088feb9f70";
        let a2 = "e148cf0e8c66a7c18ea6be52a5215dd2a770d74b6c014d51d1e1e0463e882e77";
        let b1 = "3413ff9e166e40ad8798e36609f28433496ee3a0f8984f73d33b0a80ffef2a89";
        check(
            "history",
            gallery(&[
                (
                    "no history",
                    history(&RelPath("nope.bin".to_string()), &[]),
                ),
                (
                    "three versions, one lost (run-manifest step 16)",
                    history(
                        &RelPath("a.bin".to_string()),
                        &[
                            version(a1, 800, 1, 1, VersionStatus::Superseded),
                            version(a2, 800, 3, 1, VersionStatus::Lost),
                            version(a2, 800, 4, 1, VersionStatus::Current),
                        ],
                    ),
                ),
                (
                    "a deleted path (run-manifest step 22)",
                    history(
                        &RelPath("b.bin".to_string()),
                        &[version(b1, 800, 2, 1, VersionStatus::Deleted)],
                    ),
                ),
                (
                    "the GB column",
                    history(
                        &RelPath("big/movie.mkv".to_string()),
                        &[version("a1b2c3", 12_300_000_000, 7, 2, VersionStatus::Current)],
                    ),
                ),
            ]),
        );
    }

    #[test]
    fn reset_gallery() {
        check(
            "reset",
            gallery(&[
                ("a one-word project", reset_done("p")),
                ("a longer name", reset_done("photos")),
            ]),
        );
    }
}
```

- [ ] **Step 3: Author the eight snapshot files by hand**

Create `crates/diskrement-cli/src/snapshots/init.snap`:

```
---
source: crates/diskrement-cli/src/render.rs
expression: doc
---
## a fresh project
Created project photos (id 7).

## a multi-digit id
Created project archive (id 42).
```

Create `crates/diskrement-cli/src/snapshots/scan.snap`:

```
---
source: crates/diskrement-cli/src/render.rs
expression: doc
---
## a fresh scan
Scanned: 2 added, 1 modified, 0 deleted, 3 unchanged.
Hashed 3 of 6 files (3 fingerprints reused).

## a reused staged scan (no Hashed line)
Scanned: 2 added, 1 modified, 0 deleted, 3 unchanged.

## nothing changed
Scanned: 0 added, 0 modified, 0 deleted, 6 unchanged.
Hashed 0 of 6 files (6 fingerprints reused).
```

Create `crates/diskrement-cli/src/snapshots/plan.snap`:

```
---
source: crates/diskrement-cli/src/render.rs
expression: doc
---
## nothing to do
Nothing to burn: no changes to plan.

## a bare seal (run-manifest step 18)
Plan: finalize disc 4 (seal; no new files).

## a spilling plan (run-manifest step 03)
Plan: disc 1 (new disc, BD_R) — 1 files, 0.0 GB, will FINALIZE
~1 more files (~1 more discs) remain after this one.

## appending to an open disc
Plan: disc 2 (appending to open disc 2, BD_R) — 1 files, 13.2 GB, left OPEN
This disc completes the set.

## a deletion rides along (run-manifest step 10)
Plan: disc 3 (new disc, BD_R) — 1 files, 0.0 GB, left OPEN
This disc completes the set.
Deletions: 1

## oversized files
Plan: disc 1 (new disc, BD_R) — 1 files, 0.0 GB, left OPEN
This disc completes the set.
Oversized (cannot be placed):
  huge.bin  99.0 GB
```

Create `crates/diskrement-cli/src/snapshots/status.snap`:

```
---
source: crates/diskrement-cli/src/render.rs
expression: doc
---
## two discs, one open
Project photos  (source /srv/photos, BD_R, margin 2.0%)
Discs:
  #1   FINALIZED  24.8 GB / 25.0 GB
  #2   OPEN       3.2 GB / 25.0 GB  <- open
Current files: 556      Last burn: 2026-06-21T14:03:00Z

## nothing burned yet
Project photos  (source /srv/photos, BD_R, margin 2.0%)
Discs:
Current files: 0      Last burn: never

## a dead disc in the table (run-manifest step 20 shape)
Project p  (source /srv/source, BD_R, margin 2.0%)
Discs:
  #1   FINALIZED  0.0 GB / 0.0 GB
  #2   OPEN       0.0 GB / 0.0 GB  <- open
  #3   DEAD       0.0 GB / 0.0 GB
  #4   FINALIZED  0.0 GB / 0.0 GB
Current files: 1      Last burn: 2026-09-05T20:01:03Z
```

Create `crates/diskrement-cli/src/snapshots/burn.snap`:

```
---
source: crates/diskrement-cli/src/render.rs
expression: doc
---
## a finalized disc with more to come (run-manifest step 04)
✔ Wrote disc 1  —  label: "p disc 1"
Status: FINALIZED — set this disc aside, it is sealed.
~1 more discs of changes remain. Run: scan → plan → burn

## an open disc, set complete (run-manifest step 08)
✔ Wrote disc 2  —  label: "p disc 2"
Status: left OPEN — may receive more sessions later.
No more changes — the set is up to date.
```

Create `crates/diskrement-cli/src/snapshots/verify.snap`:

```
---
source: crates/diskrement-cli/src/render.rs
expression: doc
---
## nothing burned
Nothing to verify: no burned discs.

## a clean sweep (run-manifest step 21)
disc 1: 1 files OK ✓
disc 2: 1 files OK ✓
disc 4: 0 files OK ✓

## a skipped disc
disc 3: skipped (not checked)

## mismatches
disc 2: 5 checked, 2 MISMATCH:
  bad/file.jpg
  other.raw

## a mixed sweep
disc 1: 1 files OK ✓
disc 2: skipped (not checked)
disc 3: 2 checked, 1 MISMATCH:
  gone.bin
```

Create `crates/diskrement-cli/src/snapshots/history.snap`:

```
---
source: crates/diskrement-cli/src/render.rs
expression: doc
---
## no history
No history for nope.bin.

## three versions, one lost (run-manifest step 16)
a.bin
  v1  disc 1 / session 1   0.0 GB  8e19511b7991430a1950f27907234183151bb0dbf1fbf28cd5b765088feb9f70  SUPERSEDED
  v2  disc 3 / session 1   0.0 GB  e148cf0e8c66a7c18ea6be52a5215dd2a770d74b6c014d51d1e1e0463e882e77  LOST
  v3  disc 4 / session 1   0.0 GB  e148cf0e8c66a7c18ea6be52a5215dd2a770d74b6c014d51d1e1e0463e882e77  CURRENT

## a deleted path (run-manifest step 22)
b.bin
  v1  disc 2 / session 1   0.0 GB  3413ff9e166e40ad8798e36609f28433496ee3a0f8984f73d33b0a80ffef2a89  DELETED

## the GB column
big/movie.mkv
  v1  disc 7 / session 2   12.3 GB  a1b2c3  CURRENT
```

Create `crates/diskrement-cli/src/snapshots/reset.snap`:

```
---
source: crates/diskrement-cli/src/render.rs
expression: doc
---
## a one-word project
Reset p.

## a longer name
Reset photos.
```

- [ ] **Step 4: Run the snapshot tests**

Run: `cargo test -p diskrement-cli snapshots`
Expected: PASS, 8 tests.

If a test fails, insta writes a `.snap.new` beside the `.snap` and prints the
diff. **A content difference is a real failure.** Read the diff and decide which
side is wrong:

- the renderer disagrees with `run-manifest.txt` → fix the renderer;
- the hand-authored snapshot has a typo in a label or a spacing run → fix the
  `.snap`.

Never copy the `.snap.new` over the `.snap` to make the test pass — that
launders the code's current behavior into the assertion. Delete any leftover
`.snap.new` before committing:

```bash
find crates -name '*.snap.new' -delete
```

- [ ] **Step 5: Prove the snapshots actually assert something**

A gallery that silently passes on anything is worse than none. Break one line on
purpose and confirm the suite goes red:

```bash
sed -i 's/^Reset p\.$/Reset q./' crates/diskrement-cli/src/snapshots/reset.snap
cargo test -p diskrement-cli snapshots   # expect: FAIL on reset
git checkout crates/diskrement-cli/src/snapshots/reset.snap
find crates -name '*.snap.new' -delete
cargo test -p diskrement-cli snapshots   # expect: PASS
```

- [ ] **Step 6: Verify the whole workspace**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

- [ ] **Step 7: Commit**

```bash
git add .gitignore crates/diskrement-cli/src/render.rs crates/diskrement-cli/src/snapshots
git commit -m "test(cli): snapshot every renderer's output"
```

---

### Task 8: The `verify`, `history` and `reset` commands

**Files:**
- Modify: `crates/diskrement-cli/src/args.rs`
- Modify: `crates/diskrement-cli/src/main.rs`

Read `Commands.scala` (the three command definitions) and `Runner.scala`'s
`dispatch` before wiring.

Three things to get right:

1. **`dispatch` must return an exit code.** `verify` exits 4 when it finds
   corruption, and that is a *successful run*, not an error — so it cannot come
   from `PipelineError::exit_code`. Change the signature to
   `Result<u8, PipelineError>`; every existing arm ends `Ok(0)`.
2. **`--disc` is `i64` with `allow_negative_numbers`.** zio-cli accepted
   `--disc -1` and answered `no disc with ordinal -1` with exit 2; clap's
   default would reject `-1` as an unexpected argument (exit 1), which is an
   exit-code divergence and therefore not sanctioned by §3.2.
3. **The backend decision covers both arms** (spec §5.2). `--burner simulated`
   selects the staging verifier; `--burner real` (the default) must refuse
   until P8 lands the disc verifier, exactly as `burn` already refuses. Falling
   back to the staging verifier on a real request would report a physical disc
   as verified without ever reading it.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` in `crates/diskrement-cli/src/args.rs`:

```rust
    #[test]
    fn verify_takes_an_optional_disc_ordinal() {
        let sweep = Cli::try_parse_from(["diskrement", "verify", "p"]).unwrap();
        let Command::Verify { name, disc } = sweep.command else {
            panic!("not verify")
        };
        assert_eq!(name, "p");
        assert_eq!(disc, None);

        let one = Cli::try_parse_from(["diskrement", "verify", "p", "--disc", "3"]).unwrap();
        let Command::Verify { disc, .. } = one.command else {
            panic!("not verify")
        };
        assert_eq!(disc, Some(3));
    }

    /// zio-cli let a negative ordinal through to the domain, which answers it
    /// with exit 2. Rejecting it in the parser would be exit 1 — an exit-code
    /// divergence, which §3.2 does not sanction.
    #[test]
    fn verify_accepts_a_negative_disc_ordinal_and_leaves_it_to_the_domain() {
        let cli = Cli::try_parse_from(["diskrement", "verify", "p", "--disc", "-1"]).unwrap();
        let Command::Verify { disc, .. } = cli.command else {
            panic!("not verify")
        };
        assert_eq!(disc, Some(-1));
    }

    #[test]
    fn verify_rejects_a_non_numeric_disc_ordinal() {
        assert!(Cli::try_parse_from(["diskrement", "verify", "p", "--disc", "abc"]).is_err());
    }

    #[test]
    fn history_takes_a_project_and_a_relative_path() {
        let cli = Cli::try_parse_from(["diskrement", "history", "p", "nested/a.bin"]).unwrap();
        let Command::History { name, rel_path } = cli.command else {
            panic!("not history")
        };
        assert_eq!(name, "p");
        assert_eq!(rel_path, "nested/a.bin");
    }

    #[test]
    fn history_requires_both_arguments() {
        assert!(Cli::try_parse_from(["diskrement", "history", "p"]).is_err());
        assert!(Cli::try_parse_from(["diskrement", "history"]).is_err());
    }

    #[test]
    fn reset_takes_only_a_name() {
        let cli = Cli::try_parse_from(["diskrement", "reset", "p"]).unwrap();
        let Command::Reset { name } = cli.command else {
            panic!("not reset")
        };
        assert_eq!(name, "p");
        assert!(Cli::try_parse_from(["diskrement", "reset"]).is_err());
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p diskrement-cli args`
Expected: FAIL to compile — `no variant named Verify` on `Command`.

- [ ] **Step 3: Add the subcommands**

In `crates/diskrement-cli/src/args.rs`, append to `enum Command`:

```rust
    /// Discard staged scan/plan state.
    Reset { name: String },
    /// Re-hash burned disc(s) against the catalog.
    ///
    /// `allow_negative_numbers` and the `i64` type are deliberate: zio-cli let
    /// `--disc -1` reach the domain, which answers with exit 2. Rejecting it
    /// here would be exit 1, and the exit ladder is contract (spec §3.2).
    #[command(allow_negative_numbers = true)]
    Verify {
        name: String,
        /// Check only this disc instead of sweeping every live disc.
        #[arg(long)]
        disc: Option<i64>,
    },
    /// Show the full version history of one file.
    History { name: String, rel_path: String },
```

- [ ] **Step 4: Run the args tests**

Run: `cargo test -p diskrement-cli args`
Expected: PASS.

- [ ] **Step 5: Wire the dispatch**

In `crates/diskrement-cli/src/main.rs`:

Change the `main` match arm:

```rust
        Ok(cli) => match dispatch(cli) {
            Ok(code) => ExitCode::from(code),
            Err(err) => {
                eprintln!("{err}");
                ExitCode::from(err.exit_code() as u8)
            }
        },
```

Change the signature and add the three names to the `project_name` match:

```rust
fn dispatch(cli: args::Cli) -> Result<u8, PipelineError> {
    // The project name the default catalog/data-dir resolution needs.
    let project_name = match &cli.command {
        Command::Init { name, .. } => name,
        Command::Scan { name, .. } => name,
        Command::Plan { name, .. } => name,
        Command::Status { name, .. } => name,
        Command::Burn { name, .. } => name,
        Command::Reset { name, .. } => name,
        Command::Verify { name, .. } => name,
        Command::History { name, .. } => name,
    };
```

Change every existing arm's trailing `Ok(())` to `Ok(0)` — there are five
(`Init`, `Scan`, `Plan`, `Status`, and both exits of `Burn`, whose early
`return Ok(())` after "Aborted." becomes `return Ok(0)`).

Add a verifier resolver beside the existing helpers:

```rust
/// The verification backend, chosen by the same one decision that chooses the
/// burner (spec §5.2): verifying a simulated burn by mounting a physical disc
/// is meaningless, so the two arms move together.
fn resolve_verifier(
    burner: Option<args::BurnerKind>,
    device: Option<&str>,
    work_dir: &std::path::Path,
) -> Result<Verifier, PipelineError> {
    match backend::resolve(burner, device, &env_lookup) {
        backend::Backend::Simulated => Ok(Verifier::Staging(StagingVerifier::new(
            work_dir.join("staging"),
        ))),
        // P8 replaces this arm with the read-back disc verifier. Falling back
        // to the staging verifier would report a physical disc as verified
        // without ever reading it — a divergence no test could see.
        backend::Backend::Real(device) => Err(PipelineError::Failed(format!(
            "the real disc verifier is not implemented yet (device {}); \
             re-run with --burner simulated",
            device.xorriso_address()
        ))),
    }
}
```

Add the three dispatch arms at the end of the `match cli.command` block:

```rust
        Command::Reset { name } => {
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
            pipeline.reset(&name)?;
            say(&render::reset_done(&name));
            Ok(0)
        }
        Command::Verify { name, disc } => {
            let mut catalog = open_catalog(&resolved.catalog)?;
            let p = find_or_fail(&catalog, &name)?;
            let verifier =
                resolve_verifier(cli.burner, cli.device.as_deref(), std::path::Path::new(&p.work_dir))?;
            let mut pipeline = Pipeline::new(
                &mut catalog,
                PipelinePaths {
                    work_dir: PathBuf::from(&p.work_dir),
                    scan_cache: paths::scan_cache_path(&resolved.catalog, &name),
                },
                &NoopSink,
            );
            let reports = pipeline.verify(&name, disc, &verifier)?;
            say(&render::verify_report(&reports));
            // Result-driven: corruption found is a successful run with a bad
            // outcome, so it does not travel the error ladder.
            Ok(render::verify_exit(&reports))
        }
        Command::History { name, rel_path } => {
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
            let rel = diskrement_core::RelPath(rel_path);
            let entries = pipeline.history(&name, &rel)?;
            say(&render::history(&rel, &entries));
            Ok(0)
        }
```

Extend the imports at the top of `main.rs`:

```rust
use diskrement_shell::burn::verifier::{StagingVerifier, Verifier};
```

Finally, drop the four `#[allow(dead_code)]` attributes Task 6 added to
`verify_report`, `verify_exit`, `history` and `reset_done` in `render.rs` — they
now have callers, and `-D warnings` makes a redundant allow an error only if
`clippy::allow_attributes_without_reason` were on; either way a stale allow is
noise. If `scan_report` still carries one from P3, leave it: it is still the
convention there.

- [ ] **Step 6: Run the tests**

Run: `cargo test -p diskrement-cli`
Expected: PASS.

- [ ] **Step 7: Verify the whole workspace**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

- [ ] **Step 8: Commit**

```bash
git add crates/diskrement-cli/src/args.rs crates/diskrement-cli/src/main.rs crates/diskrement-cli/src/render.rs
git commit -m "feat(cli): the verify, history and reset commands and the exit-4 contract"
```

---

### Task 9: `verify`, `history` and `reset` end to end through the spawned binary

**Files:**
- Modify: `crates/diskrement-cli/tests/binary.rs`

Everything below is a transcript of the staged Scala launcher on the same
inputs; it is what the Rust binary must print.

`TempDir` lives on `$TMPDIR`. `init` requires a CoW-capable work dir, so this
module is only meaningful when `$TMPDIR` is on btrfs/XFS — the same condition
the sbt IT suite has. Rather than skipping silently, let the test fail loudly if
`init` cannot run: an unexplained skip is the failure mode the corpus rules
exist to prevent.

- [ ] **Step 1: Write the failing test**

Append to `crates/diskrement-cli/tests/binary.rs`:

```rust
mod p6 {
    use super::*;

    fn cmd() -> Command {
        let mut c = Command::cargo_bin("diskrement").expect("binary builds");
        c.env("DISKREMENT_BURNER", "simulated");
        c.env_remove("DISKREMENT_DEVICE");
        c
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        catalog: std::path::PathBuf,
        work: std::path::PathBuf,
    }

    /// init → scan → plan → burn, leaving one burned disc with two files
    /// staged. Every later assertion in this module starts here.
    fn burned_project() -> Fixture {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        let source = root.join("source");
        std::fs::create_dir_all(source.join("nested")).unwrap();
        std::fs::write(source.join("a.bin"), vec![b'a'; 800]).unwrap();
        std::fs::write(source.join("nested/c.bin"), vec![b'c'; 100]).unwrap();
        let catalog = root.join("catalog/cat.sqlite");
        std::fs::create_dir_all(catalog.parent().unwrap()).unwrap();
        let work = root.join("work");

        let base = |c: &std::path::Path| {
            let mut x = cmd();
            x.arg("--catalog").arg(c);
            x
        };
        base(&catalog)
            .args(["init", "p", "--source"])
            .arg(&source)
            .arg("--work-dir")
            .arg(&work)
            .assert()
            .success();
        base(&catalog).args(["scan", "p"]).assert().success();
        base(&catalog).args(["plan", "p"]).assert().success();
        base(&catalog)
            .args(["burn", "p", "--yes"])
            .assert()
            .success();

        Fixture {
            _dir: dir,
            catalog,
            work,
        }
    }

    fn run(f: &Fixture, args: &[&str]) -> assert_cmd::assert::Assert {
        let mut c = cmd();
        c.arg("--catalog").arg(&f.catalog);
        c.args(args).assert()
    }

    #[test]
    fn verify_of_a_clean_burn_exits_0_and_names_every_disc() {
        let f = burned_project();
        run(&f, &["verify", "p"])
            .success()
            .stdout("disc 1: 2 files OK ✓\n");
    }

    /// Result-driven exit: corruption found is a successful run with a bad
    /// outcome, and its code is 4, not one of the error-ladder codes.
    #[test]
    fn a_corrupted_staged_file_exits_4_and_lists_the_path() {
        let f = burned_project();
        std::fs::write(f.work.join("staging/disc-1/data/a.bin"), b"CORRUPT").unwrap();
        run(&f, &["verify", "p"])
            .failure()
            .code(4)
            .stdout("disc 1: 2 checked, 1 MISMATCH:\n  a.bin\n");
    }

    #[test]
    fn verifying_a_named_disc_that_does_not_exist_exits_2() {
        let f = burned_project();
        run(&f, &["verify", "p", "--disc", "99"])
            .failure()
            .code(2)
            .stderr(contains("no disc with ordinal 99"));
    }

    /// The parser must not turn a negative ordinal into an argument error:
    /// the Scala build answers it from the domain, with exit 2.
    #[test]
    fn a_negative_disc_ordinal_is_a_lookup_failure_not_an_argument_error() {
        let f = burned_project();
        run(&f, &["verify", "p", "--disc", "-1"])
            .failure()
            .code(2)
            .stderr(contains("no disc with ordinal -1"));
    }

    #[test]
    fn verify_before_any_burn_says_there_is_nothing_to_check() {
        let dir = tempfile::TempDir::new().unwrap();
        let catalog = dir.path().join("cat.sqlite");
        let source = dir.path().join("source");
        let work = dir.path().join("work");
        std::fs::create_dir_all(&source).unwrap();
        let mut c = cmd();
        c.arg("--catalog")
            .arg(&catalog)
            .args(["init", "p", "--source"])
            .arg(&source)
            .arg("--work-dir")
            .arg(&work)
            .assert()
            .success();
        let mut c = cmd();
        c.arg("--catalog")
            .arg(&catalog)
            .args(["verify", "p"])
            .assert()
            .success()
            .stdout("Nothing to verify: no burned discs.\n");
    }

    #[test]
    fn history_prints_one_line_per_version() {
        let f = burned_project();
        run(&f, &["history", "p", "nested/c.bin"])
            .success()
            .stdout(
                contains("nested/c.bin\n")
                    .and(contains("  v1  disc 1 / session 1   0.0 GB  "))
                    .and(contains("  CURRENT")),
            );
    }

    #[test]
    fn history_of_an_unknown_path_says_so_and_exits_0() {
        let f = burned_project();
        run(&f, &["history", "p", "nope.bin"])
            .success()
            .stdout("No history for nope.bin.\n");
    }

    /// `reset` destroys the staging trees, so the next verify skips the disc
    /// rather than reporting it clean — and the catalog still knows the disc
    /// was burned.
    #[test]
    fn reset_clears_the_work_dir_and_the_next_verify_skips() {
        let f = burned_project();
        assert!(f.work.join("staging/disc-1").is_dir());
        run(&f, &["reset", "p"]).success().stdout("Reset p.\n");
        assert!(!f.work.join("staging").exists());
        assert!(!f.work.join("snapshot").exists());
        run(&f, &["verify", "p"])
            .success()
            .stdout("disc 1: skipped (not checked)\n");
        run(&f, &["status", "p"])
            .success()
            .stdout(contains("  #1   "));
    }

    #[test]
    fn planning_after_a_reset_needs_a_fresh_scan() {
        let f = burned_project();
        run(&f, &["scan", "p"]).success();
        run(&f, &["reset", "p"]).success();
        run(&f, &["plan", "p"])
            .failure()
            .code(2)
            .stderr(contains("no staged scan for p (run scan)"));
    }

    #[test]
    fn the_three_new_commands_exit_2_on_an_unknown_project() {
        let dir = tempfile::TempDir::new().unwrap();
        let catalog = dir.path().join("cat.sqlite");
        for args in [
            vec!["verify", "nosuch"],
            vec!["history", "nosuch", "a.bin"],
            vec!["reset", "nosuch"],
        ] {
            let mut c = cmd();
            c.arg("--catalog")
                .arg(&catalog)
                .args(&args)
                .assert()
                .failure()
                .code(2)
                .stderr(contains("project not found: nosuch"));
        }
    }

    /// Until P8 lands the disc verifier, a real-backend verify must refuse
    /// rather than silently check the staging tree.
    #[test]
    fn verify_on_the_real_backend_refuses_until_p8() {
        let f = burned_project();
        let mut c = Command::cargo_bin("diskrement").expect("binary builds");
        c.env("DISKREMENT_BURNER", "real")
            .env("DISKREMENT_DEVICE", "/dev/sr0")
            .arg("--catalog")
            .arg(&f.catalog)
            .args(["verify", "p"])
            .assert()
            .failure()
            .code(5)
            .stderr(contains("the real disc verifier is not implemented yet"));
    }
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test -p diskrement-cli --test binary p6`
Expected: PASS, 11 tests.

If `burned_project` fails at `init` with "work dir is not CoW-capable", `$TMPDIR`
is not on btrfs/XFS. Re-run with `TMPDIR` pointed at a CoW filesystem — do not
weaken the test to accommodate the environment.

A caveat worth checking before debugging: `a.bin` (800 B) and `nested/c.bin`
(100 B) both fit one BD-R disc with no `DISKREMENT_DISC_BYTES` override, so the
first cycle burns both. If the planner disagrees, read the actual `plan` output
before touching an assertion.

- [ ] **Step 3: Verify the whole workspace**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

- [ ] **Step 4: Commit**

```bash
git add crates/diskrement-cli/tests/binary.rs
git commit -m "test(cli): verify, history and reset end to end through the spawned binary"
```

---

### Task 10: Open the differential gate on `verify`, `history` and `reset`

**Files:**
- Modify: `crates/diskrement-difftest/src/runner.rs`
- Modify: `crates/diskrement-difftest/src/corpus.rs`

Landing a command means appending it to `RUST_IMPLEMENTED` **in the same
commit** (CLAUDE.md). Until then the Rust and interleaved modes skip every
scenario that uses it, and a fully-skipped run still exits green.

`harvest_replay` will *still* skip in those two modes after this task: it also
uses `disc mark-dead`, which P7 lands. That is correct and the harness says so
out loud; do not add `"disc"` to the list.

- [ ] **Step 1: Open the gate**

In `crates/diskrement-difftest/src/runner.rs`:

```rust
pub const RUST_IMPLEMENTED: &[&str] = &[
    "burn", "frobnicate", "history", "init", "plan", "reset", "scan", "status", "verify",
];
```

(`"frobnicate"` is deliberately present: the `error-paths` scenario invokes it
as an unknown subcommand, and both binaries must agree on that exit code.)

- [ ] **Step 2: Add the scenario**

In `crates/diskrement-difftest/src/corpus.rs`, add to the `all()` list after
`burn_errors()`:

```rust
        verify_history_reset(),
```

and append the scenario itself:

```rust
/// `verify`, `history` and `reset`, and the state each leaves behind.
///
/// The interesting assertions are not the renders alone — they are the catalog
/// rows underneath. A `verify` advances a session `BURNED -> VERIFIED`, and the
/// state diff compares session states, so a port that marked the wrong session
/// (or marked one it should have skipped) fails here even though both binaries
/// printed the same line.
pub fn verify_history_reset() -> Scenario {
    Scenario {
        name: "verify-history-reset",
        disc_bytes: Some(1000),
        steps: vec![
            write("a.bin", 800),
            write("b.bin", 800),
            cmd(
                "init p",
                &["init", "p", "--source", "{source}", "--work-dir", "{work}"],
            ),
            // Nothing burned: the sweep has nothing to check, and the path has
            // no history. Both are exit-0 sentences, not errors.
            cmd("verify before any burn", &["verify", "p"]),
            cmd("history before any burn", &["history", "p", "a.bin"]),
            // cycle 1: a.bin fills disc 1, which is sealed; b.bin spills.
            cmd("scan p", &["scan", "p"]),
            cmd("plan p", &["plan", "p"]),
            cmd("burn p --yes", &["burn", "p", "--yes"]),
            cmd("verify p", &["verify", "p"]),
            // The second sweep finds the session already VERIFIED. It must be
            // a clean no-op on both sides, not an illegal transition.
            cmd("verify p again", &["verify", "p"]),
            cmd("verify p --disc 1", &["verify", "p", "--disc", "1"]),
            // A named ordinal that does not exist is an error, not a silent
            // exit 0 — including the negative one zio-cli let through.
            cmd("verify p --disc 99", &["verify", "p", "--disc", "99"]),
            cmd("verify p --disc -1", &["verify", "p", "--disc", "-1"]),
            cmd("history p a.bin", &["history", "p", "a.bin"]),
            cmd("history p nope.bin", &["history", "p", "nope.bin"]),
            // cycle 1b: b.bin lands on disc 2, so history spans two discs and
            // the sweep has two discs to report.
            cmd("scan p", &["scan", "p"]),
            cmd("plan p", &["plan", "p"]),
            cmd("burn p --yes", &["burn", "p", "--yes"]),
            cmd("verify p", &["verify", "p"]),
            cmd("history p b.bin", &["history", "p", "b.bin"]),
            // cycle 2: supersede a, delete b — history then shows SUPERSEDED
            // and DELETED alongside CURRENT.
            modify("a.bin", 800),
            delete("b.bin"),
            cmd("scan p", &["scan", "p"]),
            cmd("plan p", &["plan", "p"]),
            cmd("burn p --yes", &["burn", "p", "--yes"]),
            cmd("history p a.bin", &["history", "p", "a.bin"]),
            cmd("history p b.bin", &["history", "p", "b.bin"]),
            cmd("verify p", &["verify", "p"]),
            cmd("status p", &["status", "p"]),
            // reset drops the staged state *and* the staging trees, so every
            // disc then skips and `plan` needs a fresh scan.
            cmd("scan p", &["scan", "p"]),
            cmd("reset p", &["reset", "p"]),
            cmd("verify after reset", &["verify", "p"]),
            cmd("plan after reset", &["plan", "p"]),
            cmd("reset p again", &["reset", "p"]),
            cmd("reset on an unknown project", &["reset", "nosuch"]),
            cmd("status p", &["status", "p"]),
        ],
    }
}
```

- [ ] **Step 3: Add the two argument-error steps**

In `error_paths()`, before the final `status` step:

```rust
            // A non-numeric `--disc` fails inside the argument parser, the one
            // place spec §3.2 lets clap and zio-cli word things differently.
            cmd_exit_only(
                "verify with a non-numeric disc",
                &["verify", "p", "--disc", "abc"],
            ),
            // `history` needs both arguments; missing one is likewise an
            // argument-parser failure.
            cmd_exit_only("history with no path", &["history", "p"]),
```

- [ ] **Step 4: Run the differential suite in all three modes**

```bash
sbt cli/stage
cargo build -p diskrement-cli
DISKREMENT_SCALA_BIN=$PWD/cli/target/universal/stage/bin/diskrement \
  TMPDIR=${TMPDIR:-/tmp} \
  cargo test -p diskrement-difftest -- --ignored --nocapture
```

Expected: the bootstrap reproduces all 11 scenarios identically; `all-rust` and
`interleaved` each compare 10 and skip 1 (`harvest-replay`, which needs P7's
`disc`). **Read the printed counts, not the exit code** — a fully-skipped run
exits green.

If a diff fails, the failure is a porting bug until proven otherwise. Do not
widen a normalization rule in `normalize.rs` to make it pass: every rule there
is a hole in the oracle, and widening one is how this harness stops working
without anyone noticing (CLAUDE.md). Fix the Rust side.

- [ ] **Step 5: Verify the whole workspace**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

- [ ] **Step 6: Commit**

```bash
git add crates/diskrement-difftest/src/runner.rs crates/diskrement-difftest/src/corpus.rs
git commit -m "feat(difftest): teach the harness that verify, history and reset exist"
```

---

### Task 11: Land the phase

**Files:**
- Modify: `docs/superpowers/plans/2026-09-07-rust-rewrite-p6-verify-history-reset.md`

- [ ] **Step 1: Run the full verification**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

All clean. Record the new counts; they must exceed the 349 passed / 3 ignored
baseline.

- [ ] **Step 2: Confirm the corpus is untouched**

```bash
git status --porcelain fixtures/
sha256sum -c fixtures/SHA256SUMS --quiet
```

Expected: no output from either.

- [ ] **Step 3: Run the differential suite once more, in all three modes**

```bash
sbt cli/stage
cargo build -p diskrement-cli
DISKREMENT_SCALA_BIN=$PWD/cli/target/universal/stage/bin/diskrement \
  cargo test -p diskrement-difftest -- --ignored --nocapture
```

Check the printed scenario counts, not just the exit code.

- [ ] **Step 4: Confirm the Scala tree still builds**

```bash
sbt clean test
```

P6 touches nothing under `core/`, `shell/`, `cli/`, but the branch must leave
the Scala build green until P10.

- [ ] **Step 5: Append the execution deviations**

Fill in the `## Execution deviations` and `## Execution verification` sections
at the end of this document, recording every place the implementation diverged
from what is written above and why. Per CLAUDE.md, deviations live in the plan
doc, not in commit messages.

- [ ] **Step 6: Merge and clean up**

```bash
git add docs/superpowers/plans/2026-09-07-rust-rewrite-p6-verify-history-reset.md
git commit -m "docs(plan): record the P6 execution deviations"
git switch feat/rust-rewrite
git merge --no-ff feat/rewrite-p6 -m "Merge feat/rewrite-p6: the staging verifier, verify/history/reset, and the full render surface — every CLI line pinned by a hand-authored insta snapshot and matching run-manifest.txt, proven against the Scala binary in all three harness modes"
git branch -d feat/rewrite-p6
```

---

## Phase exit bar (spec §9, P6 row)

- [ ] `cargo test --workspace` green, above the 349/3 baseline
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean
- [ ] `cargo fmt --check` clean
- [ ] **insta snapshots for all CLI output** — eight hand-authored galleries
      covering every renderer the binary calls (Task 7), proven to fail when a
      line changes
- [ ] **Success-path renders match `run-manifest.txt`** — steps 05, 16, 21 and
      22 asserted byte for byte (Task 6), alongside the steps P3–P5 already
      pinned
- [ ] A `verify` sweep over Scala-written catalogs and Scala-staged trees is
      clean, and advances exactly the sessions still in `BURNED` (Task 5)
- [ ] A named dead or nonexistent disc is exit 2; a sweep skips dead discs
      silently (Tasks 3, 5, 9)
- [ ] A verify that finds corruption exits **4**, and a clean or skipped one
      exits 0 (Tasks 6, 9)
- [ ] `reset` destroys the staged scan/plan, `burn-catalog.sqlite`, `staging/`
      and `snapshot/`, and leaves the catalog untouched (Tasks 4, 9)
- [ ] The differential suite passes in all three modes for every scenario
      reachable with `init`/`scan`/`status`/`plan`/`burn`/`verify`/`history`/
      `reset` — Task 10
- [ ] `fixtures/` unchanged and `SHA256SUMS` still verifies
- [ ] `sbt clean test` still green

## Spec coverage

| Spec requirement | Task |
|---|---|
| §3.1 catalog schema frozen; `DEAD` derived from `disc_event` | 3, 5 |
| §3.2 `verify` / `history` / `reset` command surface | 8, 9 |
| §3.2 exit codes 2 and 5, and the result-driven 4 | 2, 6, 8, 9 |
| §4 corpus as the oracle; `run-manifest.txt` as the render oracle | 5, 6, 7 |
| §5.2 closed `Verifier` enum; one backend decision made in `cli` | 1, 8 |
| §5.3 `burn/verifier.rs` in the layout | 1 |
| §5.5 `thiserror` per module, one `exit_code()` | 2 |
| §5.5 `insta` in the dev-dependency set, used for CLI output | 7 |
| §8.1 the phase's differential task | 10 |
| §9 P6 scope and exit bar | all |
| CLAUDE.md: `StalePlan` never advises `reset` | 4 (unchanged assertion re-read) |
| CLAUDE.md: landing a command means appending to `RUST_IMPLEMENTED` | 10 |
| CLAUDE.md: a normalization rule is a hole in the oracle | 10 |

## Execution deviations

- **rustfmt reflows throughout.** The briefs' verbatim test/impl code was
  reformatted by `cargo fmt` in several tasks (wrapped `assert_eq!`s, the
  `RUST_IMPLEMENTED` const went one-per-line, minor call-site wrapping). All
  mechanical; no assertion or string changed.
- **Task 7 Step 5 restore command.** `git checkout` could not restore
  `reset.snap` because the file was untracked at that moment; an inverse `sed`
  restored the authored byte content instead. The committed `.snap` was
  verified byte-identical to the plan's authored text, and the FAIL→PASS proof
  was observed both ways.
- **Task 10, incidental dedupe.** While editing `runner.rs`, a pre-existing
  accidentally duplicated doc-comment block was removed (comment-only hunk,
  verified by review). No code semantics touched.
- **Final-review fix wave (`3a8fff0`), not in the original task list:**
  (a) `a_sweep_skips_a_dead_disc_without_mentioning_it` now also asserts disc
  3's **stored** `disc.status` is still `OPEN` after the sweep, via the
  pre-existing `#[doc(hidden)] raw_connection_for_tests()` — per CLAUDE.md's
  "also assert the row itself is unchanged" discipline (`read_disc_states`
  derives from `disc_event` and would mask an illegal update-in-place);
  (b) `cmd()` in `binary.rs` `mod p5` and `mod p6` now
  `env_remove("DISKREMENT_DISC_BYTES")` so an ambient value cannot split the
  fixture burn across two discs; (c) a doc comment was added to
  `naming_an_ordinal_the_corpus_never_had_is_unknown_disc` to match its
  siblings.
- **Ordinal-range divergence (sanctioned, recorded).** Scala's
  `Options.integer("disc").map(_.toInt)` truncates to 32 bits — `--disc
  4294967297` would wrap to 1 and *verify disc 1*. The Rust port's `i64`
  answers `UnknownDisc`, and clap exits 1 beyond `i64::MAX`. The `i64` choice
  is right; the divergence only manifests on ordinals no corpus ever has.
  **P7 corollary:** Scala's `disc mark-dead` validates positive-in-range at
  *parse* time (`Commands.scala`); P7 must mirror that boundary with a clap
  `value_parser`, or mark-dead's reject-vs-domain split will drift.
- **Verification command note.** `sha256sum -c fixtures/SHA256SUMS` must be
  run from *inside* `fixtures/` — the manifest's paths are relative to it.
  From the repo root every line fails spuriously.

## Execution verification

- `cargo test --workspace`: **413 passed, 0 failed, 3 ignored** (baseline
  349/3 → +64).
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo fmt --check`: clean.
- `git status --porcelain fixtures/`: empty; `sha256sum -c SHA256SUMS`
  (from `fixtures/`): verifies.
- Differential suite, all three modes
  (`DISKREMENT_SCALA_BIN` = staged launcher): **bootstrap reproduced 11
  scenarios identically; all-rust 10 compared / 1 skipped; interleaved
  10 compared / 1 skipped** — the skip is `harvest-replay`, which needs P7's
  `disc mark-dead`, exactly as the plan predicts.
- `sbt clean test`: 74 + 156 = **230 passed, 0 failed** — the Scala tree is
  untouched and green.

