# Rust Rewrite P7 — the coaster workflow: `disc mark-dead`, the lost report, and the re-plan Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land P7 — `PipelineError::DiscAlreadyDead`, `Pipeline::mark_dead`,
`render::mark_dead`, and the `disc mark-dead` command with its parse-time
ordinal validation and its confirmation prompt — ending with a Rust binary that
completes the coaster workflow: a dead disc's live files re-plan onto a fresh
disc, its superseded versions read `LOST` forever, and the `disc` row it was
recorded against is never touched.

**Architecture:** The catalog half of this phase already exists. P2 landed
`Catalog::mark_dead`, the `disc_event` / `lost_version` tables, the
derived-`DEAD` reads (`read_disc_states`, `read_view`, `file_history`) and
`LostReport`, all pinned by unit tests. P5 landed the `StalePlan` refusal for a
plan whose target disc died after staging. What P7 adds is the thin layer above
them: one pipeline method with two pre-checks that turn a catalog-level
`IllegalStatusTransition` (exit 5) into the contract's exit-2 answers, one
renderer, one nested clap subcommand, and the tests that prove the whole
coaster round trip — mark dead → re-plan → recovery burn → `LOST`-per-copy
history — behaves as the Scala build does. Landing it also unblocks the
`harvest-replay` differential scenario, which has been skipping since P6
because it needs `disc mark-dead`.

**Tech Stack:** Rust 2021; `serde`, `serde_json`, `rusqlite` (bundled), `sha2`,
`rayon`, `reflink-copy`, `walkdir`, `chrono`, `clap` 4 derive, `tempfile`,
`assert_cmd`, `predicates`, `insta`. **No new dependencies.**

**Spec:** `docs/superpowers/specs/2026-09-05-rust-rewrite-design.md` — §3.1 (the
catalog schema is frozen; `DEAD` is derived from `disc_event`, never stored),
§3.2 (CLI surface, environment, exit codes), §4 (the corpus and the
`run-manifest.txt` render oracle), §5.5 (`thiserror` per module, one
`exit_code()`), §8.1 (the differential task every phase from P3 carries), §9
(the P7 row: *"Coaster workflow: `disc mark-dead`, `disc_event` /
`lost_version`, derived `DEAD`, lost report, re-plan"*, ending with
*"`LOST`-per-copy and unchanged-disc-row invariant tests green"*). The Scala
sources are the behavioral oracle: **read the file named in each task before
porting it.**

---

## Global Constraints

- **Branch:** `feat/rewrite-p7`, forked from `feat/rust-rewrite`. Never commit
  directly to `feat/rust-rewrite`. Merge back at the end with an explicit
  `Merge feat/rewrite-p7: <summary>` commit and delete the phase branch.

```bash
git switch feat/rust-rewrite && git switch -c feat/rewrite-p7
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
  **413 passed, 3 ignored** (the three `#[ignore]`d differential entry points).
- **Commit scopes:** `feat(pipeline):`, `test(pipeline):`, `feat(cli):`,
  `test(cli):`, `test(catalog):`, `feat(difftest):`, `docs(plan):`.
- **User-visible strings and exit codes are contract** (§3.2). Copy them
  verbatim from the Scala source named in each task. Every expected string in
  this plan was captured by running the staged Scala launcher
  (`sbt cli/stage`) against a scratch project — they are transcripts, not
  guesses. The only sanctioned divergence is clap's own usage/argument-error
  wording.
- **The catalog is append-only.** `mark_dead` inserts one `disc_event` row and
  N `lost_version` rows and updates nothing. In particular it must never touch
  `disc.status`, whose `check (status in ('OPEN','FINALIZED'))` constraint has
  no `DEAD` member by design.
- **`DEAD` is derived, never stored.** Every test that asserts dead-ness
  through a derived read (`read_disc_states`, `status`, `file_history`,
  `read_view`) must **also assert the `disc` row itself is unchanged**
  (CLAUDE.md): a derived-value-only assertion would pass just as happily
  against an illegal update-in-place.
- **`LOST` is per physical copy, not per content hash.** After a recovery burn
  two versions sharing one content hash legitimately read `LOST` and `CURRENT`
  (CLAUDE.md, and `fixtures/run-manifest.txt` step 16). No test may "fix" that
  by deduplicating on hash.
- **Deferred debt is ported as-is** (§1): `archivePointers` has no dead flag,
  `writeDisc` does not refuse a dead disc, and `copies>1` redundancy degrades
  silently when a disc dies. Do not repair any of them in this phase.
- **Time formatting only through `diskrement_shell::catalog::time`**
  (`format`/`parse`). Never `chrono`'s serde or `Display` for stored values.
  The `disc_event.at` value in the corpus carries **nine** fractional digits
  (`2026-09-05T20:01:13.784981123Z`), so this phase re-proves the variable
  precision rule §3.1 calls out on the write path.
- **`Pipeline` returns data and does no I/O path resolution**; the CLI resolves
  paths, prompts, and renders (CLAUDE.md invariant, enforced by the crate
  boundary). `mark_dead` therefore takes its timestamp from the caller, exactly
  as `burn` takes `now`.
- **Appending `"disc"` to `runner::RUST_IMPLEMENTED` happens in the same commit
  that lands the command in the binary** (Task 9). Otherwise the Rust and
  interleaved differential modes keep skipping `harvest-replay`, and a
  fully-skipped run still exits green.

---

## What already exists (do not rebuild it)

P7 is a thin phase because P2 landed the whole catalog half. Read this table
before writing anything, and delete nothing it names.

| Already landed | Where | Pinned by |
|---|---|---|
| `disc_event` / `lost_version` DDL, the `MARKED_DEAD` and `LOST`/`REBURN` check constraints | `crates/diskrement-shell/src/catalog/schema.rs:77-89` | `tests/fixture_schema.rs` |
| `Catalog::mark_dead` — the event, the casualty split, the already-dead refusal | `crates/diskrement-shell/src/catalog/store.rs:149-215` | `store.rs` tests `mark_dead_splits_versions_into_lost_and_reburn_and_records_both`, `deadness_is_derived_so_the_disc_row_is_untouched`, `several_placements_of_one_version_make_one_casualty`, `marking_a_dead_disc_again_fails_and_so_does_an_unknown_ordinal` |
| `dead_disc_versions` — the "no live copy elsewhere" filter, `order by fv.rel_path, sess_ord` | `crates/diskrement-shell/src/catalog/store.rs:463-495` | `a_version_alive_on_another_disc_is_neither_lost_nor_reburned`, `a_copy_on_an_already_dead_disc_no_longer_shields_a_version`, `a_tombstoned_path_is_lost_never_reburned` |
| `LostReport`, `LostVersionEntry`, `ReburnEntry` | `crates/diskrement-shell/src/catalog/model.rs:222-247` | — |
| Derived `DEAD` in `read_disc_states`, `read_view`, `file_history` | `crates/diskrement-shell/src/catalog/read.rs:53,189,319` | `read_view_drops_current_versions_whose_only_copy_is_dead`, `file_history_marks_dead_disc_versions_lost`, `a_current_version_on_a_dead_disc_still_renders_current` |
| `DiscStatus::Dead` and the planner ignoring a dead disc when choosing the open one | `crates/diskrement-core/src/planner.rs:15,159` | `planner.rs:417` |
| `PipelineError::UnknownDisc` / `DiscIsDead`, both exit 2 | `crates/diskrement-shell/src/error.rs` | `disc_state_errors_are_exit_2_with_their_messages` |
| `StalePlan` when the staged plan's disc died after planning | `crates/diskrement-shell/src/pipeline/mod.rs` | `a_disc_marked_dead_after_planning_makes_the_plan_stale` |
| The `status` render of a `DEAD` row | `crates/diskrement-cli/src/render.rs` | `src/snapshots/status.snap` ("a dead disc in the table") |
| The `history` render of `LOST` | `crates/diskrement-cli/src/render.rs` | `src/snapshots/history.snap` ("three versions, one lost") |
| The `harvest-replay` differential scenario, including its `disc mark-dead` step | `crates/diskrement-difftest/src/corpus.rs:344-393` | skipping since P6, for want of `"disc"` in `RUST_IMPLEMENTED` |

**What is missing, and is the whole of P7:** `PipelineError::DiscAlreadyDead`
(Task 1), `Pipeline::mark_dead` (Task 2), the coaster round-trip and corpus
oracles (Tasks 3, 4), `render::mark_dead` and its snapshot (Tasks 5, 6), the
`disc mark-dead` command (Task 7), the end-to-end suite (Task 8), and the
differential gate (Task 9).

---

## Scala oracle files (read before porting each area)

| Area | File |
|---|---|
| `LostReport`, `LostVersionEntry`, `ReburnEntry` | `shell/src/main/scala/diskrement/shell/model/LostReport.scala` |
| `markDead` orchestration and its two pre-checks | `shell/src/main/scala/diskrement/shell/pipeline/PipelineLive.scala` (`def markDead`, line 280) |
| `PipelineError.DiscAlreadyDead` | `shell/src/main/scala/diskrement/shell/pipeline/PipelineError.scala` |
| The pipeline mark-dead spec (port it) | `shell/src/test/scala/diskrement/shell/pipeline/PipelineMarkDeadSpec.scala` |
| `Render.markDead` | `cli/src/main/scala/diskrement/cli/Render.scala` (lines 70-81) |
| The exit ladder, incl. `DiscAlreadyDead` → 2 | `cli/src/main/scala/diskrement/cli/Render.scala` (`def handle`, line 95) |
| The `disc` / `mark-dead` command, and the parse-time ordinal check | `cli/src/main/scala/diskrement/cli/Commands.scala` (lines 94-112) |
| The confirmation prompt and dispatch | `cli/src/main/scala/diskrement/cli/Runner.scala` (the `DiskrementCommand.MarkDead` case, line 115) |
| Command parsing spec (port it) | `cli/src/test/scala/diskrement/cli/CommandsSpec.scala` (lines 72-85) |
| Render spec (port it) | `cli/src/test/scala/diskrement/cli/RenderSpec.scala` (lines 131-147) |
| Exit ladder spec (port it) | `cli/src/test/scala/diskrement/cli/ExitMappingSpec.scala` (line 26) |

## Corpus oracles used by this phase

| Fixture | Used by | What it pins |
|---|---|---|
| `fixtures/catalog/07-after-burn3.sqlite` → `fixtures/catalog/08-after-mark-dead.sqlite` | Task 4 | the whole write: applying the Rust `mark_dead` to the Scala catalog at state 07 must reproduce state 08 **row for row** |
| `fixtures/run-manifest.txt` step 12 | Tasks 5, 6, 8 | the byte-exact `disc mark-dead` render |
| `fixtures/run-manifest.txt` steps 13-16, 20 | Task 8 (shape), Task 9 (agreement) | the recovery burn, `LOST`-per-copy history, and the `DEAD` row in `status` |

Facts about those fixtures that the tasks below rely on, confirmed by query:

- `08-after-mark-dead.sqlite` differs from `07-after-burn3.sqlite` by **exactly
  two rows**: `disc_event(id=1, disc_id=3, kind='MARKED_DEAD',
  at='2026-09-05T20:01:13.784981123Z')` and
  `lost_version(id=1, event_id=1, file_version_id=3, fate='REBURN')`.
  (`sqlite_sequence` also gains two rows; it is not in `CATALOG_TABLES` and is
  not compared.)
- Disc 3's `disc.status` column in `08` still reads **`OPEN`**. Its dead-ness
  lives only in `disc_event` — this is the unchanged-disc-row invariant, in the
  corpus, written by the Scala build.
- `file_version` id 3 is `a.bin` at hash
  `e148cf0e8c66a7c18ea6be52a5215dd2a770d74b6c014d51d1e1e0463e882e77` — the
  current version of `a.bin` when disc 3 died, hence `REBURN` and not `LOST`.
  Nothing on disc 3 was superseded, which is why step 12 prints "No versions
  permanently lost."
- The project in every corpus catalog is named `p` and has `id = 1`.

---

## File Structure

**Created:**

| Path | Responsibility |
|---|---|
| `crates/diskrement-shell/tests/fixture_mark_dead.rs` | the corpus oracle: 07 + `mark_dead` == 08, row for row |
| `crates/diskrement-cli/src/snapshots/mark_dead.snap` | the hand-authored `insta` gallery for `render::mark_dead` |

**Modified:**

| Path | Change |
|---|---|
| `crates/diskrement-shell/src/error.rs` | `PipelineError::DiscAlreadyDead`, its exit code, its tests |
| `crates/diskrement-shell/src/pipeline/mod.rs` | `Pipeline::mark_dead`; a new `mod mark_dead_tests` |
| `crates/diskrement-cli/src/render.rs` | `mark_dead`, its `assert_eq!` oracles, `mark_dead_gallery` |
| `crates/diskrement-cli/src/args.rs` | the `Disc` subcommand, `DiscCommand::MarkDead`, the `disc_ordinal` value parser |
| `crates/diskrement-cli/src/main.rs` | the `Disc` arms in `project_name` and `dispatch` |
| `crates/diskrement-cli/tests/binary.rs` | the `p7` end-to-end module |
| `crates/diskrement-difftest/src/runner.rs` | `RUST_IMPLEMENTED` += `"disc"` |
| `crates/diskrement-difftest/src/corpus.rs` | the `mark-dead-errors` scenario, and its guard test |

---

### Task 1: `PipelineError::DiscAlreadyDead`

The catalog already refuses a second `MARKED_DEAD` event — but it refuses it as
`CatalogError::IllegalStatusTransition`, which the pipeline wraps in
`Failed(..)` and the ladder maps to **5**. The Scala build answers **2**,
because a repeated mark-dead is a lookup-shaped mistake, not a bug. That gap is
closed by a dedicated variant plus the pre-check in Task 2.

**Files:**
- Modify: `crates/diskrement-shell/src/error.rs`
- Test: `crates/diskrement-shell/src/error.rs` (the `mod tests` at the bottom)

- [ ] **Step 1: Read the Scala oracle**

```bash
sed -n '20,26p' shell/src/main/scala/diskrement/shell/pipeline/PipelineError.scala
sed -n '95,112p' cli/src/main/scala/diskrement/cli/Render.scala
sed -n '20,30p' cli/src/test/scala/diskrement/cli/ExitMappingSpec.scala
```

Expected: the message is `s"disc $ordinal is already marked dead"`, and
`Render.handle` maps it to `ExitCode(2)` — the same code as `UnknownDisc` and
`DiscIsDead`, with a different sentence.

- [ ] **Step 2: Write the failing test**

Append to the `mod tests` block at the bottom of
`crates/diskrement-shell/src/error.rs`:

```rust
    /// A second `disc mark-dead` on the same disc. The catalog would refuse it
    /// too, but as `IllegalStatusTransition` — which travels as `Failed` and
    /// exits 5. The Scala build answers 2, so the pipeline pre-checks and
    /// raises this instead.
    #[test]
    fn marking_a_dead_disc_again_is_exit_2_with_its_own_message() {
        assert_eq!(PipelineError::DiscAlreadyDead(1).exit_code(), 2);
        assert_eq!(
            PipelineError::DiscAlreadyDead(1).to_string(),
            "disc 1 is already marked dead"
        );
    }

    /// Three exit-2 disc errors, three distinct sentences. `DiscIsDead` is
    /// what a *named* `verify` target answers; `DiscAlreadyDead` is what a
    /// repeated mark-dead answers; `UnknownDisc` is neither. Collapsing any
    /// two would lose the operator's only clue about which mistake they made.
    #[test]
    fn the_three_disc_errors_read_differently_at_the_same_exit_code() {
        let messages = [
            PipelineError::UnknownDisc(3).to_string(),
            PipelineError::DiscIsDead(3).to_string(),
            PipelineError::DiscAlreadyDead(3).to_string(),
        ];
        assert_eq!(
            messages,
            [
                "no disc with ordinal 3",
                "disc 3 is marked dead",
                "disc 3 is already marked dead",
            ]
        );
        assert!(
            [
                PipelineError::UnknownDisc(3).exit_code(),
                PipelineError::DiscIsDead(3).exit_code(),
                PipelineError::DiscAlreadyDead(3).exit_code(),
            ]
            .iter()
            .all(|c| *c == 2)
        );
    }
```

- [ ] **Step 3: Run it and watch it fail**

```bash
cargo test -p diskrement-shell marking_a_dead_disc_again 2>&1 | tail -20
```

Expected: a compile error — `no variant or associated item named
DiscAlreadyDead found for enum PipelineError`.

- [ ] **Step 4: Add the variant**

In `crates/diskrement-shell/src/error.rs`, immediately after the `DiscIsDead`
variant:

```rust
    /// A `disc mark-dead` naming a disc that already carries a `MARKED_DEAD`
    /// event.
    ///
    /// The ordinal is `u32`, not the `i64` `UnknownDisc` and `DiscIsDead`
    /// carry: `disc mark-dead`'s ordinal is validated in the *parser*
    /// (`Commands.scala`), so a non-positive value never reaches the domain.
    /// `verify --disc`'s does, which is why that one stays signed.
    #[error("disc {0} is already marked dead")]
    DiscAlreadyDead(u32),
```

and add it to the exit-2 arm of `exit_code`:

```rust
            PipelineError::ProjectNotFound(_)
            | PipelineError::NoStagedScan(_)
            | PipelineError::NoStagedPlan(_)
            | PipelineError::SnapshotMissing(_)
            | PipelineError::UnknownDisc(_)
            | PipelineError::DiscIsDead(_)
            | PipelineError::DiscAlreadyDead(_)
            | PipelineError::StalePlan(_) => 2,
```

- [ ] **Step 5: Run it and watch it pass**

```bash
cargo test -p diskrement-shell --lib error:: 2>&1 | tail -20
```

Expected: all `error::tests::*` pass, including the two new ones.

- [ ] **Step 6: Full check and commit**

```bash
cargo test --workspace && \
cargo clippy --workspace --all-targets -- -D warnings && \
cargo fmt --check
git add crates/diskrement-shell/src/error.rs
git commit -m "feat(pipeline): the already-dead disc error and its exit-2 rung

A repeated \`disc mark-dead\` is a lookup-shaped mistake, not a bug: the
catalog's IllegalStatusTransition would exit 5, and Render.handle maps
DiscAlreadyDead to 2. The pre-check that raises it lands next.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_019vNyFqvFhs9JPHi7phvauJ"
```

---

### Task 2: `Pipeline::mark_dead`

**Files:**
- Modify: `crates/diskrement-shell/src/pipeline/mod.rs`
- Test: `crates/diskrement-shell/src/pipeline/mod.rs` (a new `mod mark_dead_tests`)

- [ ] **Step 1: Read the Scala oracle**

```bash
sed -n '278,292p' shell/src/main/scala/diskrement/shell/pipeline/PipelineLive.scala
```

Expected shape: resolve the project, read the disc states, fail `UnknownDisc`
when the ordinal is absent and `DiscAlreadyDead` when its derived status is
`DEAD`, take the clock, then delegate to `catalog.markDead`. Note that the
pre-check is over `readDiscStates` — the *derived* status — not over
`disc.status`, which never says `DEAD`.

- [ ] **Step 2: Write the failing test**

Add a new test module at the very bottom of
`crates/diskrement-shell/src/pipeline/mod.rs`, after `mod verify_tests`. This
module's helpers are used again in Task 3, so write them in full now.

```rust
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
    /// this phase's behaviour depends on. Every cycle below drives the real
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

    fn run_mark_dead(env: &mut Env, ordinal: u32) -> Result<LostReport, PipelineError> {
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
        pipeline.mark_dead("p", ordinal, at(500))
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

    fn run_verify(env: &mut Env, disc: Option<i64>) -> Result<Vec<VerifyReport>, PipelineError> {
        let work = env.work.clone();
        let scan_cache = env.scan_cache.clone();
        let verifier = Verifier::Staging(StagingVerifier::new(work.join("staging")));
        let mut pipeline = Pipeline::new(
            &mut env.catalog,
            PipelinePaths {
                work_dir: work,
                scan_cache,
            },
            &NoopSink,
        );
        pipeline.verify("p", disc, &verifier)
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
        let err = run_mark_dead(&mut e, 9).unwrap_err();
        assert!(matches!(err, PipelineError::UnknownDisc(9)), "{err:?}");
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
        run_mark_dead(&mut e, 1).expect("the first mark-dead succeeds");

        let err = run_mark_dead(&mut e, 1).unwrap_err();
        assert!(matches!(err, PipelineError::DiscAlreadyDead(1)), "{err:?}");
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
        let err = pipeline.mark_dead("nosuch", 1, at(500)).unwrap_err();
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

        run_mark_dead(&mut e, 1).expect("mark-dead succeeds");

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
}
```

- [ ] **Step 3: Run it and watch it fail**

```bash
cargo test -p diskrement-shell mark_dead_tests 2>&1 | tail -20
```

Expected: a compile error — `no method named mark_dead found for struct
Pipeline`.

If instead the error names `raw_connection_for_tests`, confirm the accessor's
exact name before adapting the helper:

```bash
grep -n 'doc(hidden)' -A 4 crates/diskrement-shell/src/catalog/mod.rs
```

- [ ] **Step 4: Implement `Pipeline::mark_dead`**

In `crates/diskrement-shell/src/pipeline/mod.rs`, extend the model import to
carry `LostReport`:

```rust
use crate::catalog::model::{
    CatalogUpdate, DiscWrite, FileFingerprint, FileHistoryEntry, FileWrite, LostReport, Project,
    ProjectId, ProjectInit, Session, SessionState, SessionWrite,
};
```

and add the method immediately after `reset`, as the last method of the `impl`:

```rust
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
    /// `disc_ordinal` is `u32` where `verify`'s is `i64`: zio-cli let a
    /// negative `--disc` reach the domain, but `disc mark-dead`'s ordinal is
    /// validated in the parser (`Commands.scala`), because mark-dead is
    /// irreversible and a silently wrapped ordinal would destroy a different
    /// disc than the one named.
    ///
    /// `at` comes from the caller, as `burn`'s `now` does: the pipeline reads
    /// no clock, which is what lets the corpus replay compare the committed
    /// timestamp exactly.
    pub fn mark_dead(
        &mut self,
        name: &str,
        disc_ordinal: u32,
        at: DateTime<Utc>,
    ) -> Result<LostReport, PipelineError> {
        let p = self.project(name)?;
        let states = self.catalog.read_disc_states(p.id).map_err(Self::failed)?;
        match states.iter().find(|s| s.ordinal == disc_ordinal) {
            None => return Err(PipelineError::UnknownDisc(i64::from(disc_ordinal))),
            Some(s) if s.status == DiscStatus::Dead => {
                return Err(PipelineError::DiscAlreadyDead(disc_ordinal))
            }
            Some(_) => {}
        }
        self.catalog
            .mark_dead(p.id, disc_ordinal, &at)
            .map_err(Self::failed)
    }
```

- [ ] **Step 5: Run it and watch it pass**

```bash
cargo test -p diskrement-shell mark_dead_tests 2>&1 | tail -20
```

Expected: `4 passed`.

- [ ] **Step 6: Full check and commit**

```bash
cargo test --workspace && \
cargo clippy --workspace --all-targets -- -D warnings && \
cargo fmt --check
git add crates/diskrement-shell/src/pipeline/mod.rs
git commit -m "feat(pipeline): mark a disc dead and report the casualties

The two pre-checks are the point: the catalog refuses an unknown or
already-dead ordinal as IllegalStatusTransition, which exits 5, and the
contract answer for both is 2. Both are checked against the derived disc
states, because disc.status never says DEAD.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_019vNyFqvFhs9JPHi7phvauJ"
```

---

### Task 3: The coaster round trip — re-plan, recovery burn, `LOST` per copy

This is the port of `PipelineMarkDeadSpec`'s first test, and it is the phase's
central behavioral claim: killing a disc does not lose the *live* files on it,
it re-queues them. The third Scala test (a plan staged before the disc died) is
already ported as `burn_tests::a_disc_marked_dead_after_planning_makes_the_plan_stale`;
do not duplicate it.

**Files:**
- Modify: `crates/diskrement-shell/src/pipeline/mod.rs` (`mod mark_dead_tests`)

- [ ] **Step 1: Read the Scala oracle**

```bash
sed -n '16,50p' shell/src/test/scala/diskrement/shell/pipeline/PipelineMarkDeadSpec.scala
```

Expected assertions: the report names disc 1, lists `b.txt` as lost and both
paths as re-burn; `status` reports the disc `DEAD` with no open disc; the next
scan sees both files as *added* (the view dropped them); the plan opens a new
disc 2 holding both; the verify sweep names only disc 2; and `b.txt`'s history
reads `LOST, LOST, CURRENT`.

- [ ] **Step 2: Confirm the expectation against the real Scala binary**

The plan's numbers were captured this way; re-running it is the cheapest way to
be sure nothing has drifted.

```bash
sbt cli/stage
S=$PWD/cli/target/universal/stage/bin/diskrement
T=$(mktemp -d); mkdir -p $T/source $T/work $T/cat
python3 -c "
open('$T/source/a.bin','wb').write(b'a'*800)
open('$T/source/b.bin','wb').write(b'b'*800)"
export DISKREMENT_BURNER=simulated; unset DISKREMENT_DISC_BYTES
r() { $S --catalog $T/cat/c.sqlite "$@"; }
r init p --source $T/source --work-dir $T/work >/dev/null
r scan p >/dev/null; r plan p >/dev/null; r burn p --yes >/dev/null
python3 -c "open('$T/source/b.bin','wb').write(b'B'*800)"
r scan p >/dev/null; r plan p >/dev/null; r burn p --yes >/dev/null
r disc mark-dead p 1 --yes
r scan p; r plan p; r burn p --yes >/dev/null; r history p b.bin
```

Expected, verbatim:

```
Disc 1 marked DEAD.
Permanently lost (1):
  b.bin  0.0 GB  b9749f157163b4bbb7faec0a55cfb88c69968a25eac4cfe13e45d6286874709a  (was session 1)
Re-burn on next scan/plan/burn (2):
  a.bin
  b.bin
Scanned: 2 added, 0 modified, 0 deleted, 0 unchanged.
Hashed 0 of 2 files (2 fingerprints reused).
Plan: disc 2 (new disc, BD_R) — 2 files, 0.0 GB, left OPEN
This disc completes the set.
b.bin
  v1  disc 1 / session 1   0.0 GB  b9749f157163b4bbb7faec0a55cfb88c69968a25eac4cfe13e45d6286874709a  LOST
  v2  disc 1 / session 2   0.0 GB  49a7906775663af7970f00f946d1d540822233477e75f5b2912ca497780a01bc  LOST
  v3  disc 2 / session 1   0.0 GB  49a7906775663af7970f00f946d1d540822233477e75f5b2912ca497780a01bc  CURRENT
```

Note v2 and v3: one content hash, two physical copies, `LOST` and `CURRENT`.
That is the `LOST`-per-copy invariant, from the reference implementation.

- [ ] **Step 3: Write the failing test**

Append to `mod mark_dead_tests` in
`crates/diskrement-shell/src/pipeline/mod.rs`:

```rust
    /// The whole coaster workflow in one test (`PipelineMarkDeadSpec`'s first
    /// case). Disc 1 takes two sessions, so it holds one superseded version
    /// (b@1) and two current ones (a@1, b@2) — one of each casualty class.
    #[test]
    fn a_dead_discs_live_files_re_plan_onto_a_fresh_disc() {
        let mut e = env();
        cycle(&mut e, &[("a.txt", b"aaa"), ("b.txt", b"bbb")], at(10));
        // A second session on the same open disc supersedes b.txt in place.
        cycle(&mut e, &[("a.txt", b"aaa"), ("b.txt", b"bbbb")], at(20));

        let report = run_mark_dead(&mut e, 1).expect("mark-dead succeeds");
        assert_eq!(report.disc_ordinal, 1);
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
            status.discs.iter().find(|d| d.ordinal == 1).map(|d| d.status),
            Some(DiscStatus::Dead)
        );
        assert_eq!(status.open_disc_ordinal, None);
        assert_eq!(
            status.total_current_files, 0,
            "the view drops versions whose only copy is dead"
        );

        // The recovery cycle plans both live files onto a brand-new disc 2.
        stage(&e, &[("a.txt", b"aaa"), ("b.txt", b"bbbb")]);
        let plan = run_plan(&mut e).expect("plan succeeds");
        assert!(plan.disc.is_new);
        assert_eq!(plan.disc.ordinal, 2);
        assert_eq!(
            plan.disc
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
            reports.iter().map(|r| r.disc_ordinal).collect::<Vec<_>>(),
            vec![2]
        );
        assert!(reports.iter().all(|r| !r.skipped && r.mismatches.is_empty()));

        // Naming the dead disc, though, is an assertion of intent and an error.
        let named = run_verify(&mut e, Some(1)).unwrap_err();
        assert!(matches!(named, PipelineError::DiscIsDead(1)), "{named:?}");
    }

    /// LOST is per physical copy, not per content hash (CLAUDE.md). b@2 and
    /// b@3 share one hash: the copy on the dead disc reads LOST and the copy
    /// on the recovery disc reads CURRENT, and no test may collapse them.
    #[test]
    fn two_copies_of_one_hash_read_lost_and_current() {
        let mut e = env();
        cycle(&mut e, &[("b.txt", b"bbb")], at(10));
        cycle(&mut e, &[("b.txt", b"bbbb")], at(20));
        run_mark_dead(&mut e, 1).expect("mark-dead succeeds");
        cycle(&mut e, &[("b.txt", b"bbbb")], at(30));

        let history = e
            .catalog
            .file_history(ProjectId(1), &RelPath("b.txt".to_string()))
            .expect("history reads");

        assert_eq!(
            history
                .iter()
                .map(|h| (h.disc_ordinal, h.session_ordinal, h.status))
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
```

- [ ] **Step 4: Run it and watch it fail**

```bash
cargo test -p diskrement-shell mark_dead_tests 2>&1 | tail -30
```

Expected: both new tests fail. They exercise only code that Task 2 already
landed, so a failure here is a real disagreement with the Scala transcript —
read the assertion message before changing anything, and do not weaken an
assertion to make it pass.

If `plan.disc.is_new` does not compile, check the field's name:

```bash
grep -n 'is_new\|pub struct PlannedDisc' -A 12 crates/diskrement-core/src/planner.rs | head -30
```

- [ ] **Step 5: Make them pass**

No production change should be needed: P2's catalog reads and P4's planner
already implement this. If a test genuinely fails, the bug is in Task 2's
pre-check ordering or in the helper, not in the invariant — fix the cause, and
record what it was for the deviations section.

```bash
cargo test -p diskrement-shell mark_dead_tests 2>&1 | tail -20
```

Expected: `6 passed`.

- [ ] **Step 6: Full check and commit**

```bash
cargo test --workspace && \
cargo clippy --workspace --all-targets -- -D warnings && \
cargo fmt --check
git add crates/diskrement-shell/src/pipeline/mod.rs
git commit -m "test(pipeline): the coaster round trip and LOST per physical copy

Killing a disc does not lose its live files, it re-queues them: the next
plan opens a fresh disc holding both, the sweep skips the dead one, and
b.txt's two surviving copies of one content hash read LOST and CURRENT.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_019vNyFqvFhs9JPHi7phvauJ"
```

---

### Task 4: The corpus oracle — 07 + `mark_dead` == 08

The strongest test in this phase. The corpus holds the catalog immediately
before and immediately after the Scala build marked disc 3 dead; applying the
Rust `mark_dead` to the first must reproduce the second, row for row. It also
re-proves §3.1's variable-precision timestamp rule on the write path: the
event's `at` carries nine fractional digits.

**Files:**
- Create: `crates/diskrement-shell/tests/fixture_mark_dead.rs`

- [ ] **Step 1: Confirm the two fixtures differ by exactly the two rows**

```bash
cd fixtures/catalog
for db in 07-after-burn3 08-after-mark-dead; do
  sqlite3 "$db.sqlite" ".dump" | grep '^INSERT' | sort > "/tmp/$db.dump"
done
diff /tmp/07-after-burn3.dump /tmp/08-after-mark-dead.dump
cd ../..
git status --porcelain fixtures/
```

Expected: four added lines — the `disc_event` row, the `lost_version` row, and
two `sqlite_sequence` rows — and **no output** from `git status` (`.dump` is a
read; if `-wal`/`-shm` files appear, delete them and use `open_fixture`'s
`immutable=1` route instead).

- [ ] **Step 2: Write the failing test**

Create `crates/diskrement-shell/tests/fixture_mark_dead.rs`:

```rust
//! `disc mark-dead` against the harvested corpus (spec §4).
//!
//! `07-after-burn3.sqlite` is the catalog the moment before the Scala build
//! marked disc 3 dead; `08-after-mark-dead.sqlite` is the moment after. The
//! Rust `mark_dead`, applied to a copy of the first at the same instant, must
//! reproduce the second row for row — which is the §11 failure this whole
//! corpus exists to catch, on the one write path P7 adds.

mod support;

use diskrement_shell::catalog::model::ProjectId;
use diskrement_shell::catalog::{time, Catalog};
use diskrement_shell::pipeline::{Pipeline, PipelinePaths};
use diskrement_shell::progress::NoopSink;
use diskrement_core::DiscStatus;

use support::{assert_dumps_match, dump_catalog, fixture_copy, open_fixture};

/// The instant the Scala build recorded, read back out of the corpus rather
/// than typed here: nine fractional digits, which is exactly the variable
/// precision §3.1 says `Instant.toString` produces and `chrono`'s
/// `SecondsFormat::AutoSi` must reproduce.
fn harvested_event_time() -> String {
    open_fixture("catalog/08-after-mark-dead.sqlite")
        .query_row(
            "select at from disc_event where kind = 'MARKED_DEAD'",
            [],
            |row| row.get::<_, String>(0),
        )
        .expect("the corpus recorded exactly one MARKED_DEAD event")
}

#[test]
fn the_harvested_event_time_has_nanosecond_precision() {
    // If this ever stops holding, the round-trip assertion below stops
    // testing the hard case and the failure would be silent.
    assert_eq!(harvested_event_time(), "2026-09-05T20:01:13.784981123Z");
}

#[test]
fn marking_disc_3_dead_reproduces_the_scala_catalog_row_for_row() {
    let at = time::parse(&harvested_event_time()).expect("the fixture timestamp parses");

    let (_dir, path) = fixture_copy("catalog/07-after-burn3.sqlite");
    let mut catalog = Catalog::open(&path).expect("the copied catalog opens");
    let work = _dir.path().join("work");

    let report = {
        let mut pipeline = Pipeline::new(
            &mut catalog,
            PipelinePaths {
                work_dir: work.clone(),
                scan_cache: work.join("unused.scancache.sqlite"),
            },
            &NoopSink,
        );
        pipeline.mark_dead("p", 3, at).expect("mark-dead succeeds")
    };

    // Step 12 of the render oracle: nothing lost, one file to re-burn.
    assert_eq!(report.disc_ordinal, 3);
    assert!(report.lost.is_empty(), "{:?}", report.lost);
    assert_eq!(
        report
            .reburn
            .iter()
            .map(|r| (r.rel_path.as_str(), r.content_hash.as_str()))
            .collect::<Vec<_>>(),
        vec![(
            "a.bin",
            "e148cf0e8c66a7c18ea6be52a5215dd2a770d74b6c014d51d1e1e0463e882e77"
        )]
    );

    let expected = dump_catalog(&open_fixture("catalog/08-after-mark-dead.sqlite"));
    let actual = dump_catalog(catalog.raw_connection_for_tests());
    assert_dumps_match(&expected, &actual);
}

/// The unchanged-disc-row invariant, checked against the corpus: the Scala
/// build left disc 3's stored status at OPEN, and so must this one.
#[test]
fn the_dead_discs_row_still_reads_open_in_both_implementations() {
    let scala: String = open_fixture("catalog/08-after-mark-dead.sqlite")
        .query_row(
            "select status from disc where project_id = 1 and ordinal = 3",
            [],
            |row| row.get(0),
        )
        .expect("the corpus has disc 3");
    assert_eq!(scala, "OPEN");

    let at = time::parse(&harvested_event_time()).expect("the fixture timestamp parses");
    let (_dir, path) = fixture_copy("catalog/07-after-burn3.sqlite");
    let mut catalog = Catalog::open(&path).expect("the copied catalog opens");
    let work = _dir.path().join("work");
    {
        let mut pipeline = Pipeline::new(
            &mut catalog,
            PipelinePaths {
                work_dir: work.clone(),
                scan_cache: work.join("unused.scancache.sqlite"),
            },
            &NoopSink,
        );
        pipeline.mark_dead("p", 3, at).expect("mark-dead succeeds");
    }

    let rust: String = catalog
        .raw_connection_for_tests()
        .query_row(
            "select status from disc where project_id = 1 and ordinal = 3",
            [],
            |row| row.get(0),
        )
        .expect("the copy has disc 3");
    assert_eq!(rust, "OPEN", "dead-ness is derived, never stored");

    // …and the derived read must nonetheless say DEAD, or the "unchanged row"
    // half would be satisfied by simply doing nothing.
    let states = catalog.read_disc_states(ProjectId(1)).expect("states read");
    assert_eq!(
        states.iter().find(|s| s.ordinal == 3).map(|s| s.status),
        Some(DiscStatus::Dead)
    );
}
```

- [ ] **Step 3: Run it and watch it fail (for the right reason)**

```bash
cargo test -p diskrement-shell --test fixture_mark_dead 2>&1 | tail -30
```

Before Task 2 this would not compile; after it, the expectation is that all
three tests **pass** on the first run. If `marking_disc_3_dead_reproduces…`
fails, read the reported diverging line: a difference in `disc_event.at` is a
timestamp-formatting bug (§3.1), and a difference in `lost_version` is a
casualty-classification bug. Neither is fixed by editing the test.

- [ ] **Step 4: Confirm the corpus is still untouched**

```bash
git status --porcelain fixtures/
cd fixtures && sha256sum -c SHA256SUMS --quiet; cd ..
```

Expected: no output from either. (`SHA256SUMS`'s paths are relative to
`fixtures/`; running it from the repo root fails every line spuriously.)

- [ ] **Step 5: Full check and commit**

```bash
cargo test --workspace && \
cargo clippy --workspace --all-targets -- -D warnings && \
cargo fmt --check
git add crates/diskrement-shell/tests/fixture_mark_dead.rs
git commit -m "test(catalog): replay the harvested mark-dead against the corpus

07-after-burn3 plus the Rust mark_dead, at the instant the Scala build
recorded, must equal 08-after-mark-dead row for row -- and leave disc 3's
stored status at OPEN in both implementations.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_019vNyFqvFhs9JPHi7phvauJ"
```

---

### Task 5: `render::mark_dead`

**Files:**
- Modify: `crates/diskrement-cli/src/render.rs`

- [ ] **Step 1: Read the Scala oracle**

```bash
sed -n '70,82p' cli/src/main/scala/diskrement/cli/Render.scala
sed -n '131,147p' cli/src/test/scala/diskrement/cli/RenderSpec.scala
sed -n '76,84p' fixtures/run-manifest.txt
```

Note the column shape of a lost line: two spaces of indent, then the path, two
spaces, the GB value, two spaces, the hash, two spaces, `(was session N)`. A
re-burn line carries the path and nothing else.

- [ ] **Step 2: Write the failing test**

Append to the `mod tests` block in `crates/diskrement-cli/src/render.rs`,
just before its closing brace (immediately after
`reset_names_the_project_it_cleared`):

```rust
    fn lost(rel: &str, size: u64, hash: &str, session: u32) -> LostVersionEntry {
        LostVersionEntry {
            rel_path: RelPath(rel.to_string()),
            content_hash: ContentHash(hash.to_string()),
            size_bytes: size,
            session_ordinal: session,
        }
    }

    fn reburn(rel: &str, size: u64, hash: &str) -> ReburnEntry {
        ReburnEntry {
            rel_path: RelPath(rel.to_string()),
            content_hash: ContentHash(hash.to_string()),
            size_bytes: size,
        }
    }

    /// `run-manifest.txt` step 12, byte for byte. Disc 3 held one current
    /// version and nothing superseded, so the lost half is the "nothing" line.
    #[test]
    fn the_mark_dead_render_matches_the_oracle_for_step_12() {
        let a2 = "e148cf0e8c66a7c18ea6be52a5215dd2a770d74b6c014d51d1e1e0463e882e77";
        assert_eq!(
            mark_dead(&LostReport {
                disc_ordinal: 3,
                lost: vec![],
                reburn: vec![reburn("a.bin", 800, a2)],
            }),
            "Disc 3 marked DEAD.\n\
             No versions permanently lost.\n\
             Re-burn on next scan/plan/burn (1):\n\
             \x20 a.bin"
        );
    }

    /// Both halves populated. Captured from the staged Scala launcher against
    /// a two-session disc: b's first version is superseded and permanently
    /// gone, while both current versions re-queue.
    #[test]
    fn both_casualty_classes_render_with_their_counts() {
        let b1 = "b9749f157163b4bbb7faec0a55cfb88c69968a25eac4cfe13e45d6286874709a";
        let b2 = "49a7906775663af7970f00f946d1d540822233477e75f5b2912ca497780a01bc";
        let a1 = "0868513521f77faae1cf5200d9ff9603007fa04e0caef9f81183b44d46cbba86";
        assert_eq!(
            mark_dead(&LostReport {
                disc_ordinal: 1,
                lost: vec![lost("b.bin", 800, b1, 1)],
                reburn: vec![reburn("a.bin", 800, a1), reburn("b.bin", 800, b2)],
            }),
            format!(
                "Disc 1 marked DEAD.\n\
                 Permanently lost (1):\n\
                 \x20 b.bin  0.0 GB  {b1}  (was session 1)\n\
                 Re-burn on next scan/plan/burn (2):\n\
                 \x20 a.bin\n\
                 \x20 b.bin"
            )
        );
    }

    /// `RenderSpec`'s "markDead with no casualties": a disc whose every
    /// version had a live copy elsewhere costs nothing at all.
    #[test]
    fn a_disc_that_cost_nothing_says_so_twice() {
        assert_eq!(
            mark_dead(&LostReport {
                disc_ordinal: 2,
                lost: vec![],
                reburn: vec![],
            }),
            "Disc 2 marked DEAD.\n\
             No versions permanently lost.\n\
             No live files to re-burn."
        );
    }

    /// The GB column is the same one-decimal format every other renderer uses.
    #[test]
    fn the_lost_line_uses_the_shared_gb_format() {
        assert_eq!(
            mark_dead(&LostReport {
                disc_ordinal: 9,
                lost: vec![lost("big/movie.mkv", 12_300_000_000, "a1b2c3", 4)],
                reburn: vec![],
            }),
            "Disc 9 marked DEAD.\n\
             Permanently lost (1):\n\
             \x20 big/movie.mkv  12.3 GB  a1b2c3  (was session 4)\n\
             No live files to re-burn."
        );
    }
```

and extend that module's `use` lines so the new types are in scope. The block
currently opens:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use diskrement_core::{DiscState, DiscStatus, DiscType};
```

Change the `diskrement_core` line and add the model line below it — `RelPath`
already arrives through `use super::*`, so do not import it again:

```rust
    use diskrement_core::{ContentHash, DiscState, DiscStatus, DiscType};
    use diskrement_shell::catalog::model::{LostReport, LostVersionEntry, ReburnEntry};
```

- [ ] **Step 3: Run it and watch it fail**

```bash
cargo test -p diskrement-cli --lib render:: 2>&1 | tail -20
```

Expected: a compile error — `cannot find function mark_dead in this scope`.

- [ ] **Step 4: Implement the renderer**

In `crates/diskrement-cli/src/render.rs`, extend the model import at the top:

```rust
use diskrement_shell::catalog::model::{
    FileHistoryEntry, LostReport, Project, ProjectId,
};
```

and add the function immediately after `history`:

```rust
/// `Render.markDead`. A header, then two blocks: what is gone forever, and
/// what will be written again.
///
/// Each block is either a count and its list, or a single "nothing" line —
/// the two are never both empty *and* silent, because an operator who just
/// destroyed a disc deserves to be told explicitly that it cost nothing. The
/// two-space indent and the double-space column gaps are contract
/// (`run-manifest.txt` step 12).
pub fn mark_dead(r: &LostReport) -> String {
    let mut lines = vec![format!("Disc {} marked DEAD.", r.disc_ordinal)];

    if r.lost.is_empty() {
        lines.push("No versions permanently lost.".to_string());
    } else {
        lines.push(format!("Permanently lost ({}):", r.lost.len()));
        lines.extend(r.lost.iter().map(|e| {
            format!(
                "  {}  {}  {}  (was session {})",
                e.rel_path.as_str(),
                gb(e.size_bytes),
                e.content_hash,
                e.session_ordinal
            )
        }));
    }

    if r.reburn.is_empty() {
        lines.push("No live files to re-burn.".to_string());
    } else {
        lines.push(format!(
            "Re-burn on next scan/plan/burn ({}):",
            r.reburn.len()
        ));
        // Only the path: the hash and size are already in the catalog, and
        // the operator's next action is `scan`, not an inspection.
        lines.extend(r.reburn.iter().map(|e| format!("  {}", e.rel_path.as_str())));
    }

    lines.join("\n")
}
```

- [ ] **Step 5: Run it and watch it pass**

```bash
cargo test -p diskrement-cli --lib render:: 2>&1 | tail -20
```

Expected: every `render::tests::*` passes, including the four new ones.

- [ ] **Step 6: Full check and commit**

```bash
cargo test --workspace && \
cargo clippy --workspace --all-targets -- -D warnings && \
cargo fmt --check
git add crates/diskrement-cli/src/render.rs
git commit -m "feat(cli): render the lost report

Two blocks, each either a count and its list or an explicit \"nothing\"
line: an operator who just destroyed a disc deserves to be told that it
cost nothing, not to be met with silence. Pinned against run-manifest
step 12.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_019vNyFqvFhs9JPHi7phvauJ"
```

---

### Task 6: The `mark_dead` snapshot gallery

The eight galleries P6 authored cover every line the CLI printed at that point.
This adds the ninth, so the "all CLI output" claim survives P7. As in P6, the
`.snap` file is **written by hand first** and the test is proven to fail
against a deliberate corruption — a snapshot generated from the code under test
asserts nothing.

**Files:**
- Create: `crates/diskrement-cli/src/snapshots/mark_dead.snap`
- Modify: `crates/diskrement-cli/src/render.rs` (`mod snapshots`)

- [ ] **Step 1: Author the snapshot by hand**

Create `crates/diskrement-cli/src/snapshots/mark_dead.snap` with exactly this
content:

```
---
source: crates/diskrement-cli/src/render.rs
expression: doc
---
## nothing lost, nothing to re-burn
Disc 2 marked DEAD.
No versions permanently lost.
No live files to re-burn.

## one live file re-queued (run-manifest step 12)
Disc 3 marked DEAD.
No versions permanently lost.
Re-burn on next scan/plan/burn (1):
  a.bin

## a superseded version gone forever
Disc 1 marked DEAD.
Permanently lost (1):
  b.bin  0.0 GB  b9749f157163b4bbb7faec0a55cfb88c69968a25eac4cfe13e45d6286874709a  (was session 1)
No live files to re-burn.

## both halves, with the GB column
Disc 4 marked DEAD.
Permanently lost (2):
  b.bin  0.0 GB  b9749f157163b4bbb7faec0a55cfb88c69968a25eac4cfe13e45d6286874709a  (was session 1)
  big/movie.mkv  12.3 GB  a1b2c3  (was session 2)
Re-burn on next scan/plan/burn (2):
  a.bin
  nested/c.bin
```

- [ ] **Step 2: Write the gallery test**

Append to `mod snapshots` in `crates/diskrement-cli/src/render.rs`, after
`reset_gallery`:

```rust
    fn lost_entry(rel: &str, size: u64, hash: &str, session: u32) -> LostVersionEntry {
        LostVersionEntry {
            rel_path: RelPath(rel.to_string()),
            content_hash: ContentHash(hash.to_string()),
            size_bytes: size,
            session_ordinal: session,
        }
    }

    fn reburn_entry(rel: &str, size: u64, hash: &str) -> ReburnEntry {
        ReburnEntry {
            rel_path: RelPath(rel.to_string()),
            content_hash: ContentHash(hash.to_string()),
            size_bytes: size,
        }
    }

    #[test]
    fn mark_dead_gallery() {
        let a1 = "0868513521f77faae1cf5200d9ff9603007fa04e0caef9f81183b44d46cbba86";
        let a2 = "e148cf0e8c66a7c18ea6be52a5215dd2a770d74b6c014d51d1e1e0463e882e77";
        let b1 = "b9749f157163b4bbb7faec0a55cfb88c69968a25eac4cfe13e45d6286874709a";
        check(
            "mark_dead",
            gallery(&[
                (
                    "nothing lost, nothing to re-burn",
                    mark_dead(&LostReport {
                        disc_ordinal: 2,
                        lost: vec![],
                        reburn: vec![],
                    }),
                ),
                (
                    "one live file re-queued (run-manifest step 12)",
                    mark_dead(&LostReport {
                        disc_ordinal: 3,
                        lost: vec![],
                        reburn: vec![reburn_entry("a.bin", 800, a2)],
                    }),
                ),
                (
                    "a superseded version gone forever",
                    mark_dead(&LostReport {
                        disc_ordinal: 1,
                        lost: vec![lost_entry("b.bin", 800, b1, 1)],
                        reburn: vec![],
                    }),
                ),
                (
                    "both halves, with the GB column",
                    mark_dead(&LostReport {
                        disc_ordinal: 4,
                        lost: vec![
                            lost_entry("b.bin", 800, b1, 1),
                            lost_entry("big/movie.mkv", 12_300_000_000, "a1b2c3", 2),
                        ],
                        reburn: vec![
                            reburn_entry("a.bin", 800, a1),
                            reburn_entry("nested/c.bin", 100, "c0ffee"),
                        ],
                    }),
                ),
            ]),
        );
    }
```

and extend that module's imports so the new types are in scope. The block
currently opens with:

```rust
    use diskrement_core::{
        ContentHash, DiscState, DiscStatus, DiscType, PlannedDisc, RelPath, ScannedFile,
    };
    use diskrement_shell::catalog::model::{FileHistoryEntry, VersionStatus};
```

Change the second line to:

```rust
    use diskrement_shell::catalog::model::{
        FileHistoryEntry, LostReport, LostVersionEntry, ReburnEntry, VersionStatus,
    };
```

- [ ] **Step 3: Run it and watch it pass against the hand-authored file**

```bash
cargo test -p diskrement-cli --lib snapshots::mark_dead_gallery 2>&1 | tail -20
```

Expected: PASS. A failure here means the renderer and the hand-authored
expectation disagree — read the diff and decide which is wrong; do **not** run
`cargo insta accept`.

- [ ] **Step 4: Prove the snapshot actually asserts something**

```bash
sed -i 's/^Disc 2 marked DEAD\.$/Disc 2 marked UNDEAD./' \
  crates/diskrement-cli/src/snapshots/mark_dead.snap
cargo test -p diskrement-cli --lib snapshots::mark_dead_gallery 2>&1 | tail -20
```

Expected: FAIL, with a diff naming that line. Then restore it:

```bash
sed -i 's/^Disc 2 marked UNDEAD\.$/Disc 2 marked DEAD./' \
  crates/diskrement-cli/src/snapshots/mark_dead.snap
cargo test -p diskrement-cli --lib snapshots::mark_dead_gallery 2>&1 | tail -5
rm -f crates/diskrement-cli/src/snapshots/*.snap.new
```

Expected: PASS again, and no `.snap.new` left behind.

- [ ] **Step 5: Full check and commit**

```bash
cargo test --workspace && \
cargo clippy --workspace --all-targets -- -D warnings && \
cargo fmt --check
git add crates/diskrement-cli/src/render.rs crates/diskrement-cli/src/snapshots/mark_dead.snap
git commit -m "test(cli): snapshot the lost report

The ninth hand-authored gallery, so \"every line the CLI prints is
pinned\" still holds after P7. Authored by hand and proven to fail
against a one-word corruption, because a snapshot generated from the
code under test asserts nothing.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_019vNyFqvFhs9JPHi7phvauJ"
```

---

### Task 7: The `disc mark-dead` command

The last of the nine commands. Two things make it different from every
subcommand landed so far: it is **nested** (`disc` is a group with one
subcommand), and its ordinal is validated in the **parser**, not the domain.

**Files:**
- Modify: `crates/diskrement-cli/src/args.rs`
- Modify: `crates/diskrement-cli/src/main.rs`

- [ ] **Step 1: Read the Scala oracle**

```bash
sed -n '94,112p' cli/src/main/scala/diskrement/cli/Commands.scala
sed -n '115,126p' cli/src/main/scala/diskrement/cli/Runner.scala
sed -n '72,85p' cli/src/test/scala/diskrement/cli/CommandsSpec.scala
```

Note the comment above the ordinal: the check is `n.isValidInt && n > 0`, done
in the parser *"rather than letting `BigInt#toInt` truncate: mark-dead is
irreversible, and a silently wrapped ordinal would destroy a different disc
than the one named."* The P6 deviations record this as the corollary P7 owes:
mirror the boundary with a clap `value_parser`, or the reject-vs-domain split
drifts.

- [ ] **Step 2: Confirm the boundary and the prompt against the real binary**

```bash
S=$PWD/cli/target/universal/stage/bin/diskrement
for n in 0 -1 4294967296 4294967297 2147483647 abc; do
  printf '%s -> ' "$n"
  $S --catalog /nonexistent/c.sqlite disc mark-dead p "$n" --yes >/dev/null 2>&1
  echo "exit $?"
done
```

Expected: `0`, `-1`, `4294967296`, `4294967297` and `abc` all exit **1** (they
fail in the parser, before the catalog is ever opened); `2147483647` gets past
the parser and fails later. `i32::MAX` is the inclusive upper bound.

```bash
T=$(mktemp -d); mkdir -p $T/source $T/work $T/cat
printf 'aaa' > $T/source/a.txt
export DISKREMENT_BURNER=simulated
r() { $S --catalog $T/cat/c.sqlite "$@"; }
r init p --source $T/source --work-dir $T/work >/dev/null
r scan p >/dev/null; r plan p >/dev/null; r burn p --yes >/dev/null
echo n | r disc mark-dead p 1; echo "[exit $?]"
```

Expected, verbatim (one line, no newline before `Aborted.`):

```
Mark disc 1 of p DEAD? Superseded versions on it are lost forever; its live files will re-plan. [y/N] Aborted.
[exit 0]
```

- [ ] **Step 3: Write the failing test**

Append to the `mod tests` block in `crates/diskrement-cli/src/args.rs`:

```rust
    #[test]
    fn disc_mark_dead_parses_the_name_the_ordinal_and_the_yes_flag() {
        let plain =
            Cli::try_parse_from(["diskrement", "disc", "mark-dead", "photos", "1"]).unwrap();
        let Command::Disc {
            command: DiscCommand::MarkDead { name, ordinal, yes },
        } = plain.command
        else {
            panic!("not disc mark-dead")
        };
        assert_eq!(name, "photos");
        assert_eq!(ordinal, 1);
        assert!(!yes);

        for argv in [
            vec!["diskrement", "disc", "mark-dead", "photos", "2", "--yes"],
            vec!["diskrement", "disc", "mark-dead", "photos", "2", "-y"],
        ] {
            let cli = Cli::try_parse_from(argv).unwrap();
            let Command::Disc {
                command: DiscCommand::MarkDead { ordinal, yes, .. },
            } = cli.command
            else {
                panic!("not disc mark-dead")
            };
            assert_eq!(ordinal, 2);
            assert!(yes);
        }
    }

    /// `Commands.scala` validates the ordinal in the parser rather than
    /// letting `BigInt#toInt` truncate: mark-dead is irreversible, and
    /// `4294967297` wrapping to `1` would destroy a different disc than the
    /// one named. The Rust port mirrors the boundary exactly — 1..=i32::MAX.
    #[test]
    fn disc_mark_dead_rejects_a_non_positive_or_out_of_range_ordinal() {
        for bad in ["0", "-1", "4294967296", "4294967297", "9223372036854775808", "abc"] {
            assert!(
                Cli::try_parse_from(["diskrement", "disc", "mark-dead", "p", bad]).is_err(),
                "{bad} should be rejected by the parser"
            );
        }
    }

    #[test]
    fn disc_mark_dead_accepts_the_whole_int_range_the_scala_parser_did() {
        let cli =
            Cli::try_parse_from(["diskrement", "disc", "mark-dead", "p", "2147483647"]).unwrap();
        let Command::Disc {
            command: DiscCommand::MarkDead { ordinal, .. },
        } = cli.command
        else {
            panic!("not disc mark-dead")
        };
        assert_eq!(ordinal, 2_147_483_647);
    }

    /// A bare `disc` is a group, not a command: it must not run anything.
    #[test]
    fn disc_requires_a_subcommand_and_mark_dead_requires_both_arguments() {
        assert!(Cli::try_parse_from(["diskrement", "disc"]).is_err());
        assert!(Cli::try_parse_from(["diskrement", "disc", "mark-dead"]).is_err());
        assert!(Cli::try_parse_from(["diskrement", "disc", "mark-dead", "p"]).is_err());
        assert!(Cli::try_parse_from(["diskrement", "disc", "bury", "p", "1"]).is_err());
    }
```

- [ ] **Step 4: Run it and watch it fail**

```bash
cargo test -p diskrement-cli --lib args:: 2>&1 | tail -20
```

Expected: compile errors — `no variant named Disc`, `cannot find type
DiscCommand`.

- [ ] **Step 5: Add the command surface**

In `crates/diskrement-cli/src/args.rs`, add the variant at the end of `enum
Command`:

```rust
    /// Operations on individual burned discs.
    Disc {
        #[command(subcommand)]
        command: DiscCommand,
    },
```

and, after the `Command` enum, the nested group and its value parser:

```rust
/// `Commands.scala`'s `disc` group. One subcommand today; the group exists
/// because the Scala surface has it, and §3.2 freezes the surface.
#[derive(Subcommand, Debug)]
pub enum DiscCommand {
    /// Mark a disc dead (destroyed/unreadable); its live files re-plan.
    ///
    /// `allow_negative_numbers` so that `-1` reaches `disc_ordinal` and is
    /// refused with the same sentence the Scala parser used, rather than
    /// clap's "unexpected argument". Both exit 1; matching the wording costs
    /// one attribute.
    #[command(allow_negative_numbers = true)]
    MarkDead {
        name: String,
        /// The ordinal of the disc to mark dead.
        #[arg(value_parser = disc_ordinal)]
        ordinal: u32,
        #[arg(long, short = 'y', default_value_t = false)]
        yes: bool,
    },
}

/// The `disc mark-dead` ordinal, validated at parse time.
///
/// The mirror of `Commands.scala`'s `mapOrFail`: positive, and inside `Int`.
/// Validating here rather than in the domain is deliberate and is the one
/// place this binary's two disc-ordinal arguments differ — `verify --disc`
/// hands a negative ordinal to the domain (exit 2), because that is what
/// zio-cli did and the exit ladder is contract. Mark-dead is irreversible, so
/// its bad ordinals are refused before anything opens the catalog.
fn disc_ordinal(raw: &str) -> Result<u32, String> {
    match raw.parse::<i128>() {
        Ok(n) if n > 0 && n <= i128::from(i32::MAX) => Ok(n as u32),
        // One message for every rejection, including a non-numeric argument:
        // Scala words that case differently ("abc is not a integer."), which
        // §3.2 sanctions, and a single sentence is the clearer one.
        _ => Err(format!(
            "disc ordinal must be a positive whole number, got {raw}"
        )),
    }
}
```

- [ ] **Step 6: Run the parser tests and watch them pass**

```bash
cargo test -p diskrement-cli --lib args:: 2>&1 | tail -20
```

Expected: all `args::tests::*` pass. The binary will not compile yet —
`dispatch`'s match is non-exhaustive — which Step 7 fixes.

- [ ] **Step 7: Wire up `dispatch`**

In `crates/diskrement-cli/src/main.rs`, add the arm to the `project_name`
match:

```rust
        Command::History { name, .. } => name,
        Command::Disc {
            command: args::DiscCommand::MarkDead { name, .. },
        } => name,
```

and the dispatch arm, as the last arm of the `match cli.command` block:

```rust
        Command::Disc {
            command: args::DiscCommand::MarkDead { name, ordinal, yes },
        } => {
            // The project lookup comes first (`Runner.withProject` resolves
            // the project before `Prompt.confirm`), so an unknown project
            // fails with exit 2 without asking anything.
            let mut catalog = open_catalog(&resolved.catalog)?;
            let p = find_or_fail(&catalog, &name)?;

            if !prompt::confirm(
                &format!(
                    "Mark disc {ordinal} of {name} DEAD? Superseded versions on it are \
                     lost forever; its live files will re-plan."
                ),
                yes,
            ) {
                say("Aborted.");
                return Ok(0);
            }

            let mut pipeline = Pipeline::new(
                &mut catalog,
                PipelinePaths {
                    work_dir: PathBuf::from(&p.work_dir),
                    scan_cache: paths::scan_cache_path(&resolved.catalog, &name),
                },
                &NoopSink,
            );
            let report = pipeline.mark_dead(&name, ordinal, chrono::Utc::now())?;
            say(&render::mark_dead(&report));
            Ok(0)
        }
```

`mark-dead` reports no progress, so it takes `NoopSink` rather than
`ConsoleProgress`, exactly as `plan`, `status`, `reset` and `history` do.

- [ ] **Step 8: Check the prompt string is byte-identical**

The `\` line continuation strips the newline *and* the following indentation,
so the space before it is load-bearing. Verify against the transcript:

```bash
cargo build -p diskrement-cli 2>&1 | tail -5
T=$(mktemp -d); mkdir -p $T/source $T/work $T/cat
printf 'aaa' > $T/source/a.txt
export DISKREMENT_BURNER=simulated
R=./target/debug/diskrement
$R --catalog $T/cat/c.sqlite init p --source $T/source --work-dir $T/work >/dev/null
$R --catalog $T/cat/c.sqlite scan p >/dev/null
$R --catalog $T/cat/c.sqlite plan p >/dev/null
$R --catalog $T/cat/c.sqlite burn p --yes >/dev/null
echo n | $R --catalog $T/cat/c.sqlite disc mark-dead p 1; echo "[exit $?]"
```

Expected, character for character:

```
Mark disc 1 of p DEAD? Superseded versions on it are lost forever; its live files will re-plan. [y/N] Aborted.
[exit 0]
```

- [ ] **Step 9: Full check and commit**

```bash
cargo test --workspace && \
cargo clippy --workspace --all-targets -- -D warnings && \
cargo fmt --check
git add crates/diskrement-cli/src/args.rs crates/diskrement-cli/src/main.rs
git commit -m "feat(cli): the disc mark-dead command and its parse-time ordinal

The ninth and last command. Its ordinal is validated in the parser, not
the domain -- the one place this binary's two disc-ordinal arguments
differ, and deliberately so: verify --disc hands a negative to the domain
because zio-cli did, while mark-dead is irreversible and 4294967297 must
never wrap to 1.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_019vNyFqvFhs9JPHi7phvauJ"
```

---

### Task 8: `disc mark-dead` end to end through the spawned binary

**Files:**
- Modify: `crates/diskrement-cli/tests/binary.rs`

- [ ] **Step 1: Read the module this one mirrors**

```bash
sed -n '493,553p' crates/diskrement-cli/tests/binary.rs
```

Note `cmd()`: it pins `DISKREMENT_BURNER=simulated` and **removes**
`DISKREMENT_DEVICE` and `DISKREMENT_DISC_BYTES`, so an ambient value in the
developer's shell cannot split a fixture burn across two discs. The `p7`
module needs the same guard.

- [ ] **Step 2: Write the failing tests**

Append a new module at the end of `crates/diskrement-cli/tests/binary.rs`:

```rust
/// P7: `disc mark-dead`, the lost report, and the recovery cycle.
mod p7 {
    use super::*;

    /// The content hashes of `b.bin`'s two versions, which the lost report and
    /// `history` print verbatim. Written out, not computed at test time: a
    /// helper that hashed the same bytes the fixture wrote would agree with
    /// itself no matter what the binary printed.
    const B1: &str = "b9749f157163b4bbb7faec0a55cfb88c69968a25eac4cfe13e45d6286874709a";
    const B2: &str = "49a7906775663af7970f00f946d1d540822233477e75f5b2912ca497780a01bc";

    fn cmd() -> Command {
        let mut c = Command::cargo_bin("diskrement").expect("binary builds");
        c.env("DISKREMENT_BURNER", "simulated");
        c.env_remove("DISKREMENT_DEVICE");
        c.env_remove("DISKREMENT_DISC_BYTES");
        c
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        catalog: std::path::PathBuf,
        source: std::path::PathBuf,
    }

    fn run(f: &Fixture, args: &[&str]) -> assert_cmd::assert::Assert {
        let mut c = cmd();
        c.arg("--catalog").arg(&f.catalog);
        c.args(args).assert()
    }

    /// Two burned sessions on one open disc: `a.bin` current from session 1,
    /// `b.bin` superseded in session 2. Disc 1 therefore holds one of each
    /// casualty class, which is what makes both halves of the report non-empty.
    fn two_session_disc() -> Fixture {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        let source = root.join("source");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("a.bin"), vec![b'a'; 800]).unwrap();
        std::fs::write(source.join("b.bin"), vec![b'b'; 800]).unwrap();
        let catalog = root.join("catalog/cat.sqlite");
        std::fs::create_dir_all(catalog.parent().unwrap()).unwrap();
        let work = root.join("work");

        let f = Fixture {
            _dir: dir,
            catalog,
            source,
        };
        let mut c = cmd();
        c.arg("--catalog")
            .arg(&f.catalog)
            .args(["init", "p", "--source"])
            .arg(&f.source)
            .arg("--work-dir")
            .arg(&work)
            .assert()
            .success();
        run(&f, &["scan", "p"]).success();
        run(&f, &["plan", "p"]).success();
        run(&f, &["burn", "p", "--yes"]).success();

        std::fs::write(f.source.join("b.bin"), vec![b'B'; 800]).unwrap();
        run(&f, &["scan", "p"]).success();
        run(&f, &["plan", "p"]).success();
        run(&f, &["burn", "p", "--yes"]).success();
        f
    }

    #[test]
    fn mark_dead_prints_both_casualty_blocks_and_exits_0() {
        let f = two_session_disc();
        run(&f, &["disc", "mark-dead", "p", "1", "--yes"])
            .success()
            .stdout(format!(
                "Disc 1 marked DEAD.\n\
                 Permanently lost (1):\n\
                 \x20 b.bin  0.0 GB  {B1}  (was session 1)\n\
                 Re-burn on next scan/plan/burn (2):\n\
                 \x20 a.bin\n\
                 \x20 b.bin\n"
            ));
    }

    /// The disc becomes DEAD in `status` — the one place the CLI shows the
    /// derived state — while the row behind it is untouched (asserted in the
    /// shell's `fixture_mark_dead` and `mark_dead_tests`, which can see SQL).
    #[test]
    fn status_reports_the_disc_dead_and_drops_its_files_from_the_view() {
        let f = two_session_disc();
        run(&f, &["disc", "mark-dead", "p", "1", "--yes"]).success();
        run(&f, &["status", "p"])
            .success()
            .stdout(contains("  #1   DEAD").and(contains("Current files: 0")));
    }

    #[test]
    fn marking_the_same_disc_twice_is_exit_2() {
        let f = two_session_disc();
        run(&f, &["disc", "mark-dead", "p", "1", "--yes"]).success();
        run(&f, &["disc", "mark-dead", "p", "1", "--yes"])
            .failure()
            .code(2)
            .stderr(contains("disc 1 is already marked dead"));
    }

    #[test]
    fn an_ordinal_no_disc_has_is_exit_2() {
        let f = two_session_disc();
        run(&f, &["disc", "mark-dead", "p", "9", "--yes"])
            .failure()
            .code(2)
            .stderr(contains("no disc with ordinal 9"));
    }

    #[test]
    fn mark_dead_on_an_unknown_project_is_exit_2() {
        let f = two_session_disc();
        run(&f, &["disc", "mark-dead", "nosuch", "1", "--yes"])
            .failure()
            .code(2)
            .stderr(contains("project not found: nosuch"));
    }

    /// The parse-time boundary, through the real binary: exit **1**, and the
    /// catalog is never opened. Compare `verify --disc -1`, which is exit 2 —
    /// the two ordinals are deliberately different, and this is the assertion
    /// that keeps them from drifting together.
    #[test]
    fn a_non_positive_or_out_of_range_ordinal_is_an_argument_error() {
        let f = two_session_disc();
        for bad in ["0", "-1", "4294967297", "abc"] {
            run(&f, &["disc", "mark-dead", "p", bad, "--yes"])
                .failure()
                .code(1);
        }
        run(&f, &["verify", "p", "--disc", "-1"])
            .failure()
            .code(2)
            .stderr(contains("no disc with ordinal -1"));
    }

    /// Refusing the prompt is a successful no-op: exit 0, nothing appended,
    /// and the disc still reads OPEN.
    #[test]
    fn declining_the_prompt_leaves_the_disc_alive() {
        let f = two_session_disc();
        let mut c = cmd();
        c.arg("--catalog")
            .arg(&f.catalog)
            .args(["disc", "mark-dead", "p", "1"])
            .write_stdin("n\n")
            .assert()
            .success()
            .stdout(
                "Mark disc 1 of p DEAD? Superseded versions on it are lost forever; \
                 its live files will re-plan. [y/N] Aborted.\n",
            );

        run(&f, &["status", "p"]).success().stdout(contains("  #1   OPEN"));
    }

    /// The whole point of the phase: the live files come back. After the
    /// coaster, `scan` sees both paths as *added*, `plan` opens a fresh disc
    /// 2, and the recovery burn restores them — while `history` keeps both
    /// dead copies visible as LOST.
    #[test]
    fn the_live_files_re_plan_onto_a_fresh_disc_and_history_shows_lost_per_copy() {
        let f = two_session_disc();
        run(&f, &["disc", "mark-dead", "p", "1", "--yes"]).success();

        run(&f, &["scan", "p"])
            .success()
            .stdout(contains("Scanned: 2 added, 0 modified, 0 deleted, 0 unchanged."));
        run(&f, &["plan", "p"])
            .success()
            .stdout(contains("Plan: disc 2 (new disc, BD_R) — 2 files"));
        run(&f, &["burn", "p", "--yes"])
            .success()
            .stdout(contains("Wrote disc 2"));

        // LOST is per physical copy: v2 and v3 share one content hash.
        run(&f, &["history", "p", "b.bin"]).success().stdout(format!(
            "b.bin\n\
             \x20 v1  disc 1 / session 1   0.0 GB  {B1}  LOST\n\
             \x20 v2  disc 1 / session 2   0.0 GB  {B2}  LOST\n\
             \x20 v3  disc 2 / session 1   0.0 GB  {B2}  CURRENT\n"
        ));

        // The sweep skips the dead disc silently; naming it is an error.
        run(&f, &["verify", "p"])
            .success()
            .stdout("disc 2: 2 files OK ✓\n");
        run(&f, &["verify", "p", "--disc", "1"])
            .failure()
            .code(2)
            .stderr(contains("disc 1 is marked dead"));
    }

    /// Every disc dead: the sweep has nothing left to check and says so,
    /// rather than reporting a clean run over an empty list.
    #[test]
    fn a_project_whose_only_disc_died_has_nothing_to_verify() {
        let f = two_session_disc();
        run(&f, &["disc", "mark-dead", "p", "1", "--yes"]).success();
        run(&f, &["verify", "p"])
            .success()
            .stdout("Nothing to verify: no burned discs.\n");
    }
}
```

- [ ] **Step 3: Run them and watch them pass**

```bash
cargo test -p diskrement-cli --test binary p7:: 2>&1 | tail -30
```

Expected: `9 passed`. These exercise only code Tasks 2, 5 and 7 landed, so a
failure is a real disagreement — compare against the Step 2 transcript in
Task 3 before touching an assertion.

If `a_project_whose_only_disc_died_has_nothing_to_verify` fails, confirm the
exact empty-sweep wording:

```bash
grep -n 'Nothing to verify' crates/diskrement-cli/src/render.rs
```

- [ ] **Step 4: Full check and commit**

```bash
cargo test --workspace && \
cargo clippy --workspace --all-targets -- -D warnings && \
cargo fmt --check
git add crates/diskrement-cli/tests/binary.rs
git commit -m "test(cli): the coaster workflow end to end through the binary

Nine cases through the spawned binary, including the assertion that
keeps the two disc-ordinal arguments from drifting together: mark-dead's
bad ordinal is exit 1 from the parser, verify --disc -1 is exit 2 from
the domain.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_019vNyFqvFhs9JPHi7phvauJ"
```

---

### Task 9: Open the differential gate on `disc mark-dead`

Landing a command means appending it to `runner::RUST_IMPLEMENTED` **in the
same commit** (CLAUDE.md). Until this lands, `harvest-replay` — the scenario
that mirrors the harvest script step for step — keeps skipping in the Rust and
interleaved modes, and a fully-skipped run still exits green.

**Files:**
- Modify: `crates/diskrement-difftest/src/runner.rs`
- Modify: `crates/diskrement-difftest/src/corpus.rs`

- [ ] **Step 1: Confirm `harvest-replay` is currently skipping for this reason**

```bash
sbt cli/stage
cargo build -p diskrement-cli
DISKREMENT_SCALA_BIN=$PWD/cli/target/universal/stage/bin/diskrement \
  cargo test -p diskrement-difftest -- --ignored --nocapture 2>&1 | grep -i 'skip\|harvest'
```

Expected: a skip naming `harvest-replay` and the missing command `disc`.

- [ ] **Step 2: Add the scenario and its guard test**

In `crates/diskrement-difftest/src/corpus.rs`, add the scenario after
`verify_history_reset()`:

```rust
/// The coaster's failure modes and its recovery, compared by agreement.
///
/// `harvest_replay` already covers the happy path in the corpus's own shape;
/// this one exists for the error ladder, which the harvest never exercises —
/// and for the boundary between an ordinal the parser refuses and one the
/// domain refuses, which is the single place P7's two disc-ordinal arguments
/// behave differently.
pub fn mark_dead_errors() -> Scenario {
    Scenario {
        name: "mark-dead-errors",
        disc_bytes: None,
        steps: vec![
            write("a.bin", 800),
            write("b.bin", 800),
            cmd(
                "init p",
                &["init", "p", "--source", "{source}", "--work-dir", "{work}"],
            ),
            cmd("scan p", &["scan", "p"]),
            cmd("plan p", &["plan", "p"]),
            cmd("burn p --yes", &["burn", "p", "--yes"]),
            // A second session on the open disc, so the disc holds one
            // superseded version and two current ones.
            modify("b.bin", 800),
            cmd("scan p", &["scan", "p"]),
            cmd("plan p", &["plan", "p"]),
            cmd("burn p --yes", &["burn", "p", "--yes"]),
            // An ordinal no disc has, and an unknown project: both exit 2,
            // and the project lookup happens first.
            cmd(
                "mark-dead an unknown ordinal",
                &["disc", "mark-dead", "p", "9", "--yes"],
            ),
            cmd(
                "mark-dead on an unknown project",
                &["disc", "mark-dead", "nosuch", "1", "--yes"],
            ),
            cmd(
                "mark-dead p 1 --yes",
                &["disc", "mark-dead", "p", "1", "--yes"],
            ),
            // Twice: the second is refused, and appends no second event —
            // which the end-state diff checks, not just the exit code.
            cmd(
                "mark-dead p 1 again",
                &["disc", "mark-dead", "p", "1", "--yes"],
            ),
            // A sweep skips a dead disc silently; naming one is an error.
            cmd("verify p", &["verify", "p"]),
            cmd("verify p --disc 1", &["verify", "p", "--disc", "1"]),
            // Zero, and a value that would wrap through 32 bits, both fail in
            // the argument parser — zio-cli's `mapOrFail` and clap's
            // `value_parser` may word it differently, which §3.2 sanctions.
            // The exit code is the contract and is compared.
            cmd_exit_only(
                "mark-dead ordinal 0",
                &["disc", "mark-dead", "p", "0", "--yes"],
            ),
            cmd_exit_only(
                "mark-dead ordinal 4294967297",
                &["disc", "mark-dead", "p", "4294967297", "--yes"],
            ),
            // The recovery: the live files re-plan onto a fresh disc, and the
            // dead copies stay visible in history as LOST.
            cmd("scan p", &["scan", "p"]),
            cmd("plan p", &["plan", "p"]),
            cmd("burn p --yes", &["burn", "p", "--yes"]),
            cmd("history p b.bin", &["history", "p", "b.bin"]),
            cmd("status p", &["status", "p"]),
        ],
    }
}
```

Register it in `all()`, before `harvest_replay()`:

```rust
        verify_history_reset(),
        mark_dead_errors(),
        harvest_replay(),
```

and add a guard test to that file's `mod tests`:

```rust
    /// The scenario must actually reach both refusals it is named for: a
    /// second mark-dead of the same disc, and a *named* dead disc at verify.
    /// A scenario that stopped exercising them would still pass, silently.
    #[test]
    fn the_mark_dead_scenario_marks_one_disc_twice_and_then_names_it() {
        let s = mark_dead_errors();
        let marks = s
            .steps
            .iter()
            .filter(|step| match step {
                Step::Cmd { args, .. } => {
                    args.starts_with(&["disc".to_string(), "mark-dead".to_string()])
                        && args.get(2).is_some_and(|n| n == "p")
                        && args.get(3).is_some_and(|o| o == "1")
                }
                _ => false,
            })
            .count();
        assert_eq!(marks, 2, "the same disc must be marked dead twice");
        assert!(
            s.steps.iter().any(|step| matches!(
                step,
                Step::Cmd { args, .. }
                    if args == &["verify", "p", "--disc", "1"]
                        .map(str::to_string)
                        .to_vec()
            )),
            "the scenario must also name the dead disc at verify"
        );
    }
```

This needs `Step` in scope; extend the file's `use` line:

```rust
use crate::scenario::{cmd, cmd_exit_only, delete, modify, write, Scenario, Step};
```

- [ ] **Step 3: Run the harness and watch the new scenario skip**

```bash
DISKREMENT_SCALA_BIN=$PWD/cli/target/universal/stage/bin/diskrement \
  cargo test -p diskrement-difftest -- --ignored --nocapture 2>&1 | tail -30
```

Expected: the all-scala mode runs `mark-dead-errors` and reproduces it
identically; the all-rust and interleaved modes **skip** both it and
`harvest-replay`, naming `disc` as the missing command. That skip is the
failing test.

- [ ] **Step 4: Open the gate**

In `crates/diskrement-difftest/src/runner.rs`, add `"disc"` to
`RUST_IMPLEMENTED`, keeping the list alphabetical:

```rust
pub const RUST_IMPLEMENTED: &[&str] = &[
    "burn",
    "disc",
    "frobnicate",
    "history",
    "init",
    "plan",
    "reset",
    "scan",
    "status",
    "verify",
];
```

(`"frobnicate"` is not a command; it is the deliberately unknown subcommand the
`error-paths` scenario uses, and both binaries must reject it identically.)

- [ ] **Step 5: Run all three modes and watch them compare**

```bash
cargo build -p diskrement-cli
DISKREMENT_SCALA_BIN=$PWD/cli/target/universal/stage/bin/diskrement \
  cargo test -p diskrement-difftest -- --ignored --nocapture 2>&1 | tail -40
```

Expected: **12 scenarios compared, 0 skipped** in every mode — the twelve are
the eleven that existed plus `mark-dead-errors`, and `harvest-replay` now runs
for the first time in the Rust and interleaved modes. Read the printed counts;
do not trust the exit code alone.

If a diff appears, it is a real divergence between the two binaries. Fix the
Rust side. **Do not widen a normalization rule to make it pass** — each rule
is a hole in the oracle (CLAUDE.md), and the corpus's `run-manifest.txt`
remains an independent byte-for-byte check that no normalization can loosen.

- [ ] **Step 6: Full check and commit**

```bash
cargo test --workspace && \
cargo clippy --workspace --all-targets -- -D warnings && \
cargo fmt --check
git add crates/diskrement-difftest/src/runner.rs crates/diskrement-difftest/src/corpus.rs
git commit -m "feat(difftest): teach the harness that disc mark-dead exists

Appending \"disc\" to RUST_IMPLEMENTED in the commit that lands the
command, per CLAUDE.md: without it the Rust and interleaved modes keep
skipping harvest-replay, and a fully-skipped run still exits green. That
scenario now runs end to end for the first time.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_019vNyFqvFhs9JPHi7phvauJ"
```

---

### Task 10: Land the phase

**Files:**
- Modify: `docs/superpowers/plans/2026-09-07-rust-rewrite-p7-coaster-workflow.md`

- [ ] **Step 1: Run the full verification**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

All clean. Record the new counts; they must exceed the 413 passed / 3 ignored
baseline.

- [ ] **Step 2: Confirm the corpus is untouched**

```bash
git status --porcelain fixtures/
cd fixtures && sha256sum -c SHA256SUMS --quiet; cd ..
```

Expected: no output from either. (`SHA256SUMS` must be checked from inside
`fixtures/`; its paths are relative to that directory.)

- [ ] **Step 3: Run the differential suite once more, in all three modes**

```bash
sbt cli/stage
cargo build -p diskrement-cli
DISKREMENT_SCALA_BIN=$PWD/cli/target/universal/stage/bin/diskrement \
  cargo test -p diskrement-difftest -- --ignored --nocapture
```

Check the printed scenario counts, not just the exit code. Every mode must now
report **0 skipped**: P7 is the phase after which no scenario in the corpus is
gated on an unimplemented command.

- [ ] **Step 4: Confirm the Scala tree still builds**

```bash
sbt clean test
```

P7 touches nothing under `core/`, `shell/`, `cli/`, but the branch must leave
the Scala build green until P10. Expected: 230 passed, 0 failed.

- [ ] **Step 5: Append the execution deviations**

Fill in the `## Execution deviations` and `## Execution verification` sections
at the end of this document, recording every place the implementation diverged
from what is written above and why. Per CLAUDE.md, deviations live in the plan
doc, not in commit messages. Carry forward anything P8 will need — in
particular, any observation about the real-backend arms of `resolve_verifier`
and the burner resolution, which P8 replaces.

- [ ] **Step 6: Merge and clean up**

```bash
git add docs/superpowers/plans/2026-09-07-rust-rewrite-p7-coaster-workflow.md
git commit -m "docs(plan): record the P7 execution deviations"
git switch feat/rust-rewrite
git merge --no-ff feat/rewrite-p7 -m "Merge feat/rewrite-p7: the coaster workflow — disc mark-dead, the lost report, and the re-plan, with the dead disc's row proven untouched against the corpus and LOST pinned per physical copy; the ninth and last command, so every differential scenario now runs in all three modes"
git branch -d feat/rewrite-p7
```

---

## Phase exit bar (spec §9, P7 row)

- [ ] `cargo test --workspace` green, above the 413/3 baseline
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean
- [ ] `cargo fmt --check` clean
- [ ] **`LOST`-per-copy invariant test green** — two versions sharing one
      content hash read `LOST` and `CURRENT` after a recovery burn, asserted at
      the pipeline layer (Task 3) and through the binary (Task 8)
- [ ] **Unchanged-disc-row invariant test green** — `disc.status` still reads
      `OPEN` after the event, asserted against a synthetic catalog (Task 2) and
      against the corpus, alongside the Scala build's own row (Task 4)
- [ ] `disc mark-dead` reproduces the harvested catalog **row for row**:
      `07-after-burn3` + `mark_dead` == `08-after-mark-dead` (Task 4)
- [ ] The lost report renders byte-for-byte as `run-manifest.txt` step 12
      (Tasks 5, 6, 8)
- [ ] A dead disc's live files re-plan onto a fresh disc; the sweep skips the
      dead disc silently and a *named* dead disc is exit 2 (Tasks 3, 8)
- [ ] Marking a disc dead twice is exit **2** and appends nothing (Tasks 1, 2, 8)
- [ ] A non-positive or out-of-`Int` ordinal is refused by the **parser**
      (exit 1) while `verify --disc -1` is still refused by the **domain**
      (exit 2) — the P6 deviations' corollary, discharged (Tasks 7, 8)
- [ ] The differential suite passes in all three modes with **0 skipped**,
      including `harvest-replay` for the first time (Task 9)
- [ ] `fixtures/` unchanged and `SHA256SUMS` still verifies
- [ ] `sbt clean test` still green

## Spec coverage

| Spec requirement | Task |
|---|---|
| §1 full feature parity: the ninth command, `disc mark-dead` | 7 |
| §1 deferred debt ported as-is (`archivePointers` has no dead flag; `writeDisc` does not refuse a dead disc; `copies>1` degrades silently) | none — deliberately untouched, stated in Global Constraints |
| §3.1 catalog schema frozen; `DEAD` derived from `disc_event`, never stored | 2, 4 |
| §3.1 timestamps: variable fractional precision on the write path | 4 |
| §3.2 `disc mark-dead` surface, `--yes`/`-y`, the confirmation prompt | 7, 8 |
| §3.2 exit codes: 2 for the three disc errors, 1 for an argument error, 0 for an abort | 1, 7, 8 |
| §4 the corpus as the oracle; `run-manifest.txt` step 12 as the render oracle | 4, 5, 6, 8 |
| §5.5 `thiserror` per module, one `exit_code()` | 1 |
| §5.5 `insta` for CLI output | 6 |
| §8.1 the phase's differential task | 9 |
| §9 P7 scope and exit bar (`LOST`-per-copy, unchanged disc row) | 3, 4, all |
| CLAUDE.md: assert the row itself is unchanged, not only the derived value | 2, 4 |
| CLAUDE.md: `LOST` is per physical copy, not per content hash | 3, 8 |
| CLAUDE.md: landing a command means appending to `RUST_IMPLEMENTED` | 9 |
| CLAUDE.md: a normalization rule is a hole in the oracle | 9 |
| P6 deviations: mirror Scala's parse-time ordinal boundary with a `value_parser` | 7, 8 |

## Execution deviations

_(fill in during execution — see Task 10, Step 5)_

## Execution verification

_(fill in during execution — see Task 10, Step 5)_
