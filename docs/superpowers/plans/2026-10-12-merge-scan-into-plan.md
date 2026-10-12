# Merge scan into plan Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `plan` snapshots and scans the source itself, so every scan is fresh, and
the `scan` command is removed. The cycle becomes `plan → burn` (issue #4, both parts).

**Architecture:** `Pipeline::plan` clears the staged scan and plan, runs the existing
`fresh_scan`, then runs the existing planning on the files it just scanned, and returns
`PlanOutcome { scan, plan }`. The CLI prints the scan summary, then the plan block. A
`#[cfg(test)]` method, `plan_staged`, plans a hand-staged `scan.json`, so the burn tests
that stage snapshots by hand keep working. The work is ordered so that every commit
leaves the workspace green: first `burn`'s error order, then `plan` scans, then the
integration tests stop calling `scan`, then `scan` is removed, then the wording and docs.

**Tech Stack:** Rust workspace (`diskrement-core` → `diskrement-shell` → `diskrement-cli`),
clap derive, rusqlite, insta (render snapshots), assert_cmd (`tests/binary.rs`), and the
CoW scenario harness (`tests/scenarios/`).

**Spec:** `docs/superpowers/specs/2026-10-12-merge-scan-into-plan-design.md`
**Branch:** `feat/merge-scan-into-plan` (already created; the spec is committed there as `b98fabd`).

---

## Ground rules for every task

- **Verified** means all three of these pass:

  ```bash
  DISKREMENT_SCENARIOS_REQUIRED=1 cargo test --workspace
  cargo clippy --workspace --all-targets -- -D warnings
  cargo fmt --check
  ```

  `DISKREMENT_SCENARIOS_REQUIRED=1` turns a skipped CoW scenario into a failure.
  Warnings are errors (`.cargo/config.toml`).
- **Baseline counts at `b98fabd`:**
  - `diskrement-core` 82;
  - `diskrement-shell`: lib 368, `fixture_catalog` 8, `fixture_verify` 6,
    `timestamps` 2, `xorriso_image` 5;
  - `diskrement-cli`: bin unit 108, `binary` 46, `scenarios` 20.

  Every task's report states its counts, and any change must be explained by tests
  the task added or deleted.
- **Commits:** conventional with a scope, ending with the trailer
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. One commit per task
  unless a task says otherwise.
- **Line numbers** below refer to the tree at `b98fabd`. They drift as tasks land, so
  find each site by the quoted code, not by the number.
- **Never** touch `fixtures/`. Never open a fixture catalog with `Catalog::open`.
- Insta snapshots: after changing a render gallery, run
  `cargo insta test -p diskrement-cli --accept` if `cargo-insta` is installed.
  Otherwise edit the `.snap` file by hand to the new output, then re-run the tests.
  Read the diff of every `.snap` change before committing.

---

## File map

| File | Change |
|---|---|
| `crates/diskrement-shell/src/pipeline/mod.rs` | `plan` scans; `plan_scanned` (private), `plan_staged` (`#[cfg(test)]`); `burn` reads the plan first; `Pipeline::scan` removed (Task 4); tests migrated |
| `crates/diskrement-shell/src/pipeline/reports.rs` | `PlanOutcome`; `ScanReport.hashed: usize` (Task 4) |
| `crates/diskrement-shell/src/error.rs` | `NoStagedScan` removed; `SnapshotMissing` wording and its `PartialBurn` advice |
| `crates/diskrement-cli/src/args.rs` | `Plan` gains `--rehash`; `Scan` removed (Task 4) |
| `crates/diskrement-cli/src/main.rs` | `plan` uses `ConsoleProgress` and renders both blocks; `Scan` arm removed |
| `crates/diskrement-cli/src/render.rs` (+ `snapshots/scan.snap`, `burn.snap`, `mark_dead.snap`, `status.snap`) | `hashed` always printed; "plan → burn" wording |
| `crates/diskrement-cli/tests/binary.rs` | no `scan`; new `scan` → exit 1 test |
| `crates/diskrement-cli/tests/scenarios/*.rs` | no `scan`; s07, s10 and s11 rewritten; s08 gains the #4 story |
| `scripts/perf-bench.sh` | `scan1`/`scan2` → `plan1`/`plan2` |
| `README.md`, `CLAUDE.md`, `docs/smoke-test-real-media.md`, three older specs | wording and amendments |

---

### Task 1: `burn` reads the staged plan first

After the merge, `plan` writes `scan.json` and `plan.json` together, so `burn` should
report any missing staged state as "no staged plan (run plan)". This task changes only
`burn`'s order. `plan` still reports `NoStagedScan` until Task 2.

**Files:**
- Modify: `crates/diskrement-shell/src/pipeline/mod.rs` (`burn`, lines ~279–291; tests ~1855–1870)
- Modify: `crates/diskrement-cli/tests/binary.rs` (`burning_twice_fails_with_the_staged_state_error`, ~509–519)

- [ ] **Step 1: Write the failing tests.** In `mod burn_tests`, replace
  `burning_without_a_staged_scan_is_a_state_error` with these two tests:

```rust
    /// Nothing staged at all (a second burn, or a failed `plan`) is "run
    /// plan": `plan` is the only command that stages anything (#4).
    #[test]
    fn burning_with_nothing_staged_is_a_state_error_that_says_run_plan() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], false);
        staged::clear(&env.work).unwrap();
        let err = run_burn(&mut env).unwrap_err();
        assert!(matches!(err, PipelineError::NoStagedPlan(_)), "{err}");
        assert_eq!(err.exit_code(), 2);
    }

    /// A plan without its scan can only come from deleting files by hand. The
    /// remedy is the same as for no plan at all, so it reads the same.
    #[test]
    fn a_staged_plan_without_its_scan_is_the_no_staged_plan_error() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], false);
        std::fs::remove_file(staged::scan_path(&env.work)).unwrap();
        let err = run_burn(&mut env).unwrap_err();
        assert!(matches!(err, PipelineError::NoStagedPlan(_)), "{err}");
        assert_eq!(err.exit_code(), 2);
    }
```

  `staged::scan_path` and `staged::plan_path` are both `pub` (`pipeline/staged.rs`).

- [ ] **Step 2: Run them and watch them fail.**

  Run: `cargo test -p diskrement-shell burn_tests::burning_with_nothing_staged burn_tests::a_staged_plan_without_its_scan`

  Expected: both FAIL with `NoStagedScan` where `NoStagedPlan` was expected.

- [ ] **Step 3: Implement.** In `Pipeline::burn`, replace the opening block

```rust
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
```

  with:

```rust
        let p = self.project(name)?;
        if staged::is_legacy_plan(&self.paths.work_dir) {
            return Err(PipelineError::StalePlan(
                "it was staged by an older version of diskrement".to_string(),
            ));
        }
        // The plan first: `plan` stages both files together, so whatever is
        // missing, the remedy is "run plan" (#4).
        let mut report = staged::read_plan(&self.paths.work_dir)
            .map_err(Self::failed)?
            .ok_or_else(|| PipelineError::NoStagedPlan(name.to_string()))?;
        let scanned = staged::read_scan(&self.paths.work_dir)
            .map_err(Self::failed)?
            .ok_or_else(|| PipelineError::NoStagedPlan(name.to_string()))?;
```

  `is_legacy_plan` must still run before `read_plan`: a v1 plan does not parse as the
  current format (see the `read_plan` doc comment in `staged.rs`).

- [ ] **Step 4: Update the CLI test.** In `tests/binary.rs`,
  `burning_twice_fails_with_the_staged_state_error`, change
  `.stderr(contains("no staged scan for p (run scan)"))` to
  `.stderr(contains("no staged plan for p (run plan)"))`.

- [ ] **Step 5: Verify.** Run the three verification commands.

  Expected counts: shell lib 369 (+1); all others at baseline.

- [ ] **Step 6: Commit.**

```bash
git add crates/diskrement-shell/src/pipeline/mod.rs crates/diskrement-cli/tests/binary.rs
git commit -m "fix(pipeline): burn reports missing staged state as no staged plan (#4)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: `plan` scans

This is the core change. `Pipeline::plan` clears staged state, scans, then plans, and
returns `PlanOutcome`. The CLI gets `--rehash` and prints both blocks. `NoStagedScan` is
removed, and `SnapshotMissing` says "run plan". **`Pipeline::scan` and the `scan`
command stay in this task** (Task 4 removes them), so every existing caller keeps
compiling and the integration tests stay green.

**Files:**
- Modify: `crates/diskrement-shell/src/pipeline/reports.rs`
- Modify: `crates/diskrement-shell/src/pipeline/mod.rs` (`plan` at ~221–258; every `.plan(` call in the test modules)
- Modify: `crates/diskrement-shell/src/error.rs`
- Modify: `crates/diskrement-cli/src/args.rs`, `crates/diskrement-cli/src/main.rs`
- Modify: `crates/diskrement-cli/tests/binary.rs`, `tests/scenarios/s07_reset.rs`, `tests/scenarios/s11_exit_codes.rs`

- [ ] **Step 1: Add `PlanOutcome`.** In `pipeline/reports.rs`, directly after
  `pub struct ScanReport { … }`:

```rust
/// What `plan` returns: the scan it took and the plan it staged (#4).
#[derive(Debug)]
pub struct PlanOutcome {
    pub scan: ScanReport,
    pub plan: PlanReport,
}
```

  Add `PlanOutcome` to the `use reports::{…}` list at the top of `pipeline/mod.rs`.

- [ ] **Step 2: Write the new pipeline tests (they will not compile yet).** In
  `pipeline/mod.rs` `mod tests`, replace
  `plan_without_a_staged_scan_is_exit_2_with_the_run_scan_advice` with the following
  four tests. Add `use std::os::unix::ffi::OsStrExt as _;` inside the non-UTF-8 test.

```rust
    /// `plan` needs no earlier step: it scans the source itself (#4).
    #[test]
    fn plan_scans_the_source_itself() {
        let mut e = setup();
        write_src(&e, "a.txt", b"aaaaa");
        e.pipeline.init(&project_init(&e)).unwrap();
        let out = e.pipeline.plan("p", None, false, false).unwrap();
        assert_eq!((out.scan.added, out.scan.scanned_files), (1, 1));
        assert_eq!(out.plan.sets[0].disc.files.len(), 1);
        assert_eq!(staged::read_scan(&e.work).unwrap().unwrap().len(), 1);
    }

    /// The #4 regression: a plan with nothing to burn used to leave its scan
    /// staged, and every later scan reused it, so new files were never seen.
    #[test]
    fn a_plan_after_nothing_to_burn_sees_a_file_added_since() {
        let mut e = setup();
        e.pipeline.init(&project_init(&e)).unwrap();
        let first = e.pipeline.plan("p", None, false, false).unwrap();
        assert!(first.plan.sets.is_empty(), "nothing to burn yet");
        write_src(&e, "new.jpg", b"photo");
        let second = e.pipeline.plan("p", None, false, false).unwrap();
        assert_eq!(second.scan.added, 1);
        assert_eq!(second.plan.sets[0].disc.files.len(), 1);
        assert_eq!(second.plan.sets[0].disc.files[0].rel_path.as_str(), "new.jpg");
    }

    /// A plan that fails partway leaves nothing staged, so `burn` cannot
    /// accept an older plan whose snapshot the failed scan already replaced.
    #[test]
    fn a_plan_that_fails_mid_scan_leaves_nothing_staged() {
        use std::os::unix::ffi::OsStrExt as _;
        let mut e = setup();
        write_src(&e, "a.txt", b"a");
        e.pipeline.init(&project_init(&e)).unwrap();
        e.pipeline.plan("p", None, false, false).unwrap();
        assert!(staged::read_plan(&e.work).unwrap().is_some());

        let bad = std::ffi::OsStr::from_bytes(b"bad\xffname.txt");
        std::fs::write(e.src.join(bad), b"x").unwrap();
        let err = e.pipeline.plan("p", None, false, false).unwrap_err();
        assert_eq!(err.exit_code(), 3, "{err}");
        assert!(staged::read_plan(&e.work).unwrap().is_none());
        assert!(staged::read_scan(&e.work).unwrap().is_none());
    }

    /// Each plan's snapshot replaces the last one rather than accumulating.
    #[test]
    fn a_re_plan_replaces_the_snapshot() {
        let mut e = setup();
        write_src(&e, "a.txt", b"a");
        e.pipeline.init(&project_init(&e)).unwrap();
        e.pipeline.plan("p", None, false, false).unwrap();
        std::fs::remove_file(e.src.join("a.txt")).unwrap();
        write_src(&e, "b.txt", b"b");
        e.pipeline.plan("p", None, false, false).unwrap();
        let snap = e.work.join("snapshot");
        assert!(snap.join("b.txt").is_file());
        assert!(!snap.join("a.txt").exists(), "the first snapshot is gone");
    }
```

- [ ] **Step 3: Implement `plan`.** In `pipeline/mod.rs`, replace the whole
  `pub fn plan(&self, …) -> Result<PlanReport, PipelineError> { … }` (doc comment
  included) with:

```rust
    /// `plan`: scan the source, then decide the disc each copy set writes this
    /// cycle. The staged scan and plan are cleared first, so a plan that fails
    /// at any step leaves nothing for `burn` to accept (#4); the fresh snapshot
    /// replaces the previous one. Stages `staged/scan.json` and
    /// `staged/plan.json` and writes **no catalog rows**. No `PLANNED` state is
    /// ever persisted: the staged plan in the work dir is the recovery unit.
    pub fn plan(
        &mut self,
        name: &str,
        disc_type_override: Option<DiscType>,
        finalize: bool,
        rehash: bool,
    ) -> Result<PlanOutcome, PipelineError> {
        let p = self.project(name)?;
        staged::clear(&self.paths.work_dir).map_err(Self::failed)?;
        let (files, hashed) = self.fresh_scan(&p, rehash)?;
        self.plan_scanned(&p, &files, Some(hashed), disc_type_override, finalize)
    }

    /// `plan` on files already scanned and staged: count them against the
    /// catalog, plan, and stage the plan.
    fn plan_scanned(
        &self,
        p: &Project,
        scanned: &[ScannedFile],
        hashed: Option<usize>,
        disc_type_override: Option<DiscType>,
        finalize: bool,
    ) -> Result<PlanOutcome, PipelineError> {
        let scan = self.count(p, scanned, hashed)?;
        let view = self.catalog.read_view(p.id).map_err(Self::failed)?;
        let states = self.catalog.read_disc_states(p.id).map_err(Self::failed)?;
        let recorded = self.catalog.fingerprints(p.id).map_err(Self::failed)?;
        let tombstones = diff::tombstones(recorded.keys(), scanned);
        let disc_type = disc_type_override.unwrap_or(p.default_disc_type);
        let cap = planning::effective_capacity(
            Capacity::from_disc_type(disc_type, p.margin_percent),
            planning::disc_bytes_from_env(),
        );
        let next = self.next_sessions(p.id)?;
        let report = planning::plan_from(
            &view,
            scanned,
            tombstones,
            &states,
            disc_type,
            cap,
            finalize,
            p.copies,
            |disc| next.get(&disc).copied().unwrap_or(1),
        );
        staged::write_plan(&self.paths.work_dir, &report).map_err(Self::failed)?;
        Ok(PlanOutcome { scan, plan: report })
    }

    /// `plan` without the scan: plans `scan.json` as it stands. The burn
    /// tests stage a snapshot and a scan by hand (fixed mtimes, files that
    /// never touch the source) and plan them through this.
    #[cfg(test)]
    fn plan_staged(
        &self,
        name: &str,
        disc_type_override: Option<DiscType>,
        finalize: bool,
    ) -> Result<PlanOutcome, PipelineError> {
        let p = self.project(name)?;
        let scanned = staged::read_scan(&self.paths.work_dir)
            .map_err(Self::corrupt_scan)?
            .expect("plan_staged plans a scan the test staged");
        self.plan_scanned(&p, &scanned, Some(0), disc_type_override, finalize)
    }
```

  Notes for the implementer:
  - `count` (unchanged) also writes `pending-deletions.json`. Before this task only
    `scan` wrote it; now every `plan` and `plan_staged` call does, which matches what
    `scan && plan` always did. If a burn test fails *because* that file now exists,
    stop and report it rather than working around it.
  - `next_sessions` and the other helpers take `&self`, so `plan_scanned` needs only
    `&self`. `plan` needs `&mut self` because `fresh_scan` does.

- [ ] **Step 4: Update every existing `.plan(` caller in the shell test modules.**
  Two rules:
  - **Real source** (the test wrote files with `write_src` or into `env.source`, and
    ran `Pipeline::scan` or `init` on that source): call
    `.plan("p", <override>, <finalize>, false)` and take `.plan` (or `.scan`) from the
    result. Keep the preceding `.scan(…)` call for now; Task 4 removes it. Tests that
    call it from a `let e = setup();` binding need `let mut e`.
  - **Hand-staged** (the test built `snapshot/` and `scan.json` itself through
    `staged_env`, `staged_env_with`, `restage`, `stage`, or `staged::write_scan`):
    call `.plan_staged("p", <override>, <finalize>)` and take `.plan`.

  The sites at `b98fabd`:

  | Where | Rule |
  |---|---|
  | `mod tests`: `plan_on_an_unknown_project_fails_projectnotfound` | real; `let mut e`; `.plan("nosuch", None, false, false)` |
  | `mod tests`: `plan_stages_the_report_it_returns`, `plan_uses_the_projects_default_disc_type_and_margin`, `an_explicit_disc_type_overrides_the_projects_default`, `finalize_seals_a_disc_that_would_have_been_left_open`, `a_spilling_plan_seals_this_cycles_disc_and_reports_the_rest`, `a_file_larger_than_any_disc_is_surfaced_as_oversized`, `re_planning_overwrites_the_staged_plan_and_leaves_the_staged_scan`, `plan_writes_no_catalog_rows` | real; `let report = e.pipeline.plan("p", …, false).unwrap().plan;` |
  | `mod burn_tests` ~2104 (the session-2 test), ~2274 (`restage`), ~2938 (verify of an empty sealing session) | hand-staged |
  | the module with `fn stage` / `fn cycle` / `fn run_plan` (~3136–3200), and the dead-before-burn test at ~3466 and ~3476 | hand-staged; `run_plan` returns `Result<PlanReport, PipelineError>`, so its body becomes `pipeline.plan_staged("p", None, false).map(\|o\| o.plan)` and its `let pipeline` stays immutable |
  | the copies module (`burned_twice`, ~3549 onward, and `run_to_completion` ~3826) | real |

  In `plan_stages_the_report_it_returns`, the staged-plan assertion compares with the
  returned plan: `assert_eq!(staged_report, report);` still holds when
  `report = …unwrap().plan`.

- [ ] **Step 5: Error wording.** In `crates/diskrement-shell/src/error.rs`:
  - Delete the `NoStagedScan(String)` variant with its `#[error]` line, and its arm
    in `exit_code` (the `| PipelineError::NoStagedScan(_)` line).
  - Change `#[error("source snapshot missing for {0} (run scan)")]` to
    `#[error("source snapshot missing for {0} (run plan)")]`.
  - In `partial_burn_advice`, replace

```rust
        // `plan` cannot recreate the snapshot; only a fresh scan does.
        PipelineError::SnapshotMissing(_) => "re-run scan --force, then plan and burn",
```

    with

```rust
        // `plan` takes a fresh snapshot (#4).
        PipelineError::SnapshotMissing(_) => "re-run plan, then burn",
```

  - Tests in the same file:
    - delete `assert_eq!(PipelineError::NoStagedScan("p".into()).exit_code(), 2);`;
    - in `the_staged_state_messages_match_the_scala_render_verbatim`, delete the
      `NoStagedScan` assertion;
    - in `burn_error_messages_match_the_scala_render_verbatim`, expect
      `"source snapshot missing for photos (run plan)"`;
    - in `a_partial_burn_on_a_stale_plan_advises_re_planning_never_reset`, expect
      `missing.ends_with("(re-run plan, then burn)")`.

  `partial_burn_advice` now returns the same text for `StalePlan` and
  `SnapshotMissing`. Merge the two arms into one
  (`PipelineError::StalePlan(_) | PipelineError::SnapshotMissing(_) => "re-run plan, then burn",`)
  and give it one comment covering both: the plan no longer holds or its snapshot is
  gone, and a fresh `plan` rebuilds both.

- [ ] **Step 6: CLI flag.** In `crates/diskrement-cli/src/args.rs`, the `Plan`
  variant becomes:

```rust
    /// Scan the source and plan the next disc.
    Plan {
        name: String,
        /// Override the project's default disc type for this plan only.
        #[arg(long)]
        disc_type: Option<DiscType>,
        /// Seal the disc even when everything fit.
        #[arg(long, default_value_t = false)]
        finalize: bool,
        /// Re-hash every file, ignoring the fingerprint cache and the size+mtime fast path.
        #[arg(long, default_value_t = false)]
        rehash: bool,
    },
```

  In `args.rs` tests, `plan_defaults_to_no_disc_type_override_and_no_finalize`
  destructures `Plan` exhaustively: add `rehash` to the pattern and `assert!(!rehash);`.
  Add this test after `plan_takes_a_disc_type_override_and_the_finalize_flag`:

```rust
    #[test]
    fn plan_takes_the_rehash_flag() {
        let cli = Cli::try_parse_from(["diskrement", "plan", "photos", "--rehash"]).unwrap();
        let Command::Plan { rehash, .. } = cli.command else {
            panic!("not plan")
        };
        assert!(rehash);
    }
```

- [ ] **Step 7: CLI dispatch.** In `crates/diskrement-cli/src/main.rs`, replace the
  `Command::Plan { … } => { … }` arm with:

```rust
        Command::Plan {
            name,
            disc_type,
            finalize,
            rehash,
        } => {
            // The work dir comes from the project row, not the CLI
            // (`Runner.withProject`). `plan` scans first, so hashing progress
            // goes to stderr like any long phase.
            let mut catalog = open_catalog(&resolved.catalog)?;
            let p = find_or_fail(&catalog, &name)?;
            let sink = progress::ConsoleProgress::new();
            let mut pipeline = Pipeline::new(
                &mut catalog,
                PipelinePaths {
                    work_dir: PathBuf::from(&p.work_dir),
                    scan_cache: paths::scan_cache_path(&resolved.catalog, &name),
                },
                &sink,
            );
            let outcome = pipeline.plan(&name, disc_type, finalize, rehash)?;
            say(&render::scan_report(&outcome.scan));
            say(&render::plan_report(&outcome.plan));
            Ok(0)
        }
```

  If `NoopSink` is now unused in `main.rs`, remove it from the import. (`status` still
  uses it at `b98fabd`, so it probably stays.)

- [ ] **Step 8: Run the shell and bin unit tests.**

  Run: `cargo test -p diskrement-shell && cargo test -p diskrement-cli --bin diskrement`

  Expected: PASS, including the four new pipeline tests.

- [ ] **Step 9: Fix the CLI tests whose meaning changed.** `plan` no longer fails
  without a scan or after a reset, so four tests are rewritten:

  - **`tests/binary.rs`, `mod plan`:** replace
    `plan_without_a_staged_scan_exits_2_and_says_to_run_scan` with:

```rust
    /// `plan` needs no earlier step: on an empty source it scans, finds
    /// nothing, and says so (#4).
    #[test]
    fn plan_on_a_fresh_project_scans_by_itself() {
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
            .success()
            .stdout(
                contains("Scanned: 0 added, 0 modified, 0 deleted, 0 unchanged.\n")
                    .and(contains("Hashed 0 of 0 files (0 fingerprints reused).\n"))
                    .and(contains("Nothing to burn: no changes to plan.")),
            );
    }
```

  - **`tests/binary.rs`:** replace `planning_after_a_reset_needs_a_fresh_scan` with:

```rust
    /// `reset` discards the staged plan; the next `plan` simply scans again.
    #[test]
    fn planning_after_a_reset_scans_again() {
        let f = burned_project();
        run(&f, &["plan", "p"]).success();
        run(&f, &["reset", "p"]).success();
        assert!(!f.work.join("staged").exists());
        run(&f, &["plan", "p"])
            .success()
            .stdout(contains("Scanned: 0 added, 0 modified, 0 deleted, 2 unchanged."));
    }
```

  - **`tests/scenarios/s07_reset.rs`:** the whole file becomes:

```rust
//! Scenario 7: `reset` discards the staged plan, so the next `burn` fails with
//! exit 2. This is the scenario that proves the CLI propagates application
//! exit codes rather than collapsing them to 0.

use crate::support;

#[test]
fn reset_clears_the_staged_plan_and_a_following_burn_errors() {
    let Some(s) = support::scratch_or_skip("s07 reset") else {
        return;
    };
    let cli = s.cli();
    support::write(&s.source, "a.txt", 64);

    cli.init(&s, "p");

    assert_eq!(cli.run(&["plan", "p"]).code, 0);

    let reset = cli.run(&["reset", "p"]);
    assert_eq!(reset.code, 0);
    assert!(reset.out_has("Reset p."));

    let burn = cli.run(&["burn", "p", "--yes"]);
    assert_eq!(burn.code, 2);
    assert!(burn.err_has("no staged plan for p (run plan)"), "{}", burn.stderr);
    // CLAUDE.md: staged-state advice is always "run plan", never "reset":
    // reset destroys staging the verifier needs.
    assert!(
        !burn.stderr.contains("reset"),
        "the advice must not suggest reset: {}",
        burn.stderr
    );
}
```

  - **`tests/scenarios/s11_exit_codes.rs`:** rename the test to
    `verify_of_an_unknown_project_and_burn_without_a_plan_both_exit_two` and replace
    its last line with `assert_eq!(cli.run(&["burn", "p", "--yes"]).code, 2);`.

- [ ] **Step 10: Verify.** Run the three verification commands.

  Expected counts:
  - shell lib 372 (Task 1's 369, −1 replaced, +4 new);
  - cli bin unit 109 (+1 `plan_takes_the_rehash_flag`);
  - `binary` 46;
  - `scenarios` 20.

- [ ] **Step 11: Commit.**

```bash
git add -A crates/
git commit -m "feat(pipeline): plan scans the source itself (#4)" -m "plan clears the staged scan and plan, takes a fresh snapshot, hashes against the fingerprint cache, and plans what it just scanned. --rehash forces a full re-hash. NoStagedScan is gone; a missing snapshot now advises re-running plan." -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: integration tests and the bench script stop calling `scan`

`plan` scans now, so every `scan` step in `tests/binary.rs`, the scenarios and
`scripts/perf-bench.sh` is either redundant or belongs on the `plan` that follows it.
The `scan` command still exists in this task.

**Files:**
- Modify: `crates/diskrement-cli/tests/binary.rs`
- Modify: `crates/diskrement-cli/tests/scenarios/s01`–`s06`, `s08`–`s10`, `s12`–`s14`
- Modify: `scripts/perf-bench.sh`

- [ ] **Step 1: The mechanical rule.** For every `scan` invocation:
  1. If its output is not asserted, delete the line.
  2. If its output is asserted (`Scanned: …`, `Hashed …`), delete the `scan` run and
     move those assertions onto the output of the `plan` that follows it. If no
     `plan` follows before the next `burn`/`status`, replace the `scan` with
     `plan` (same args, minus `--force`).
  3. Rename variables (`sc`, `sc2`, `scan1` …) to plan names (`pl`, `pl2`, `plan1`
     …) wherever that leaves two variables holding the same `plan` run.

  Rename helpers and tests whose name says `scan`: `init_scan_plan` → `init_and_plan`,
  `scanned_project` → `initialized_project` (update its doc comment: "already init'd;
  `plan` scans"), and `scan_plan_burn` (s13) → `plan_burn`. Update every caller.

- [ ] **Step 2: Sites that need more than the rule.**

  - **`binary.rs` `init_scan_status_round_trip`** → rename to
    `init_plan_status_round_trip`, and change its doc comment to "init → plan →
    status". The first `scan` becomes `plan` with the same two stdout assertions.
    `status` is unchanged. The staged-file block also asserts
    `work.join("staged/plan.json").is_file()`. The final "second scan reuses the
    staged scan" block becomes:

```rust
        // A second plan scans afresh; the sidecar answers every fingerprint.
        base(&catalog)
            .args(["plan", "p"])
            .assert()
            .success()
            .stdout(contains(
                "Scanned: 2 added, 0 modified, 0 deleted, 0 unchanged.",
            ))
            .stdout(contains("Hashed 0 of 2 files (2 fingerprints reused)."));
```

    Because the test no longer uses the `.not()` predicate, the `PredicateBooleanExt`
    import may become unused. Remove it only if clippy says so.
  - **`binary.rs` `scan_on_an_unknown_project_also_exits_2`:** delete it.
    `plan_on_an_unknown_project_exits_2` covers it. `binary` drops to 45 here and is
    back to 46 after Task 4.
  - **`binary.rs` `an_argument_parse_error_exits_1_with_clap_wording`:**
    `.args(["scan"])` → `.args(["plan"])` (a missing NAME is still clap's exit 1).
  - **`binary.rs` `a_catch_up_copy_too_large_is_skipped_and_the_set_reads_incomplete`:**
    delete both `scan` lines, including `["scan", "p", "--force"]`.
  - **`s08_idempotent.rs`** gains the #4 story. Rewrite its body from `cli.init` on:

```rust
    cli.init(&s, "p");
    cli.run(&["plan", "p"]);
    cli.run(&["burn", "p", "--yes"]);

    // Every plan scans afresh.
    let pl2 = cli.run(&["plan", "p"]);
    assert_eq!(pl2.code, 0);
    assert!(pl2.out_has("Scanned: 0 added, 0 modified, 0 deleted, 2 unchanged."));
    assert!(pl2.out_has("Nothing to burn: no changes to plan."));

    // Burning that plan writes nothing: disc 1 gets no second session.
    let b2 = cli.run(&["burn", "p", "--yes"]);
    assert_eq!(b2.code, 0, "{}{}", b2.stdout, b2.stderr);
    assert!(
        b2.out_has("Nothing burned: no disc in this plan."),
        "{}",
        b2.stdout
    );
    assert!(!b2.out_has("Wrote disc"), "{}", b2.stdout);
    assert_eq!(disc::manifest(&s.disc_dir(1)).session_ordinal, 1);
    assert!(!s.disc_dir(2).exists());

    // #4: a plan with nothing to burn left its scan staged, and the next
    // cycle reused it and never saw new files. Now the next plan does.
    support::write(&s.source, "c.txt", 64);
    let pl3 = cli.run(&["plan", "p"]);
    assert_eq!(pl3.code, 0, "{}{}", pl3.stdout, pl3.stderr);
    assert!(pl3.out_has("Scanned: 1 added, 0 modified, 0 deleted, 2 unchanged."));
    assert!(!pl3.out_has("Nothing to burn"), "{}", pl3.stdout);
```

    Update the module doc comment so that it also mentions the new file being seen
    after a nothing-to-burn cycle. Rename the test to
    `replanning_an_unchanged_source_plans_nothing_and_a_new_file_is_then_seen`.
  - **`s10_warm_rescan.rs`:** the whole file becomes:

```rust
//! Scenario 10: the fingerprint cache, end to end.
//!
//! Every `plan` scans, and the scan merges two sources: fingerprints read back
//! from the catalog (populated once a session is burned) and the sidecar cache
//! (written after every scan, burn or no burn). A rescan taken after a burn is
//! explained by either, so it alone proves nothing about the sidecar. Three
//! rescans are pinned instead: one before any burn, one after, and one after a
//! file's size changes.

use crate::support;

#[test]
fn plans_reuse_fingerprints_before_and_after_a_burn_and_a_touched_file_rehashes() {
    let Some(s) = support::scratch_or_skip("s10 warm-cache rescan") else {
        return;
    };
    let cli = s.cli();
    support::write(&s.source, "a.txt", 100);
    support::write(&s.source, "b.txt", 200);
    support::write(&s.source, "c.txt", 300);

    cli.init(&s, "p");

    let plan1 = cli.run(&["plan", "p"]);
    assert_eq!(plan1.code, 0);
    assert!(plan1.out_has("Hashed 3 of 3 files (0 fingerprints reused)."));

    // The discriminating step: nothing is burned yet, so only the sidecar
    // can explain the reuse.
    let plan1b = cli.run(&["plan", "p"]);
    assert_eq!(plan1b.code, 0);
    assert!(plan1b.out_has("Hashed 0 of 3 files (3 fingerprints reused)."));

    cli.run(&["burn", "p", "--yes"]);

    let plan2 = cli.run(&["plan", "p"]);
    assert_eq!(plan2.code, 0);
    assert!(plan2.out_has("Scanned: 0 added, 0 modified, 0 deleted, 3 unchanged."));
    assert!(plan2.out_has("Hashed 0 of 3 files (3 fingerprints reused)."));

    // A size change is a fingerprint miss whichever source answered.
    support::modify(&s.source, "b.txt", 999);
    let plan3 = cli.run(&["plan", "p"]);
    assert_eq!(plan3.code, 0);
    assert!(plan3.out_has("Scanned: 0 added, 1 modified, 0 deleted, 2 unchanged."));
    assert!(plan3.out_has("Hashed 1 of 3 files (2 fingerprints reused)."));

    // Pins the sidecar naming convention the CLI resolves. Mere existence
    // proves nothing about persistence — the file is created on demand by
    // any cache call, a plain read included — so this is a path assertion
    // and nothing more.
    assert!(
        s.catalog
            .parent()
            .unwrap()
            .join("cat.p.scancache.sqlite")
            .exists(),
        "the sidecar is not where the CLI resolves it"
    );
}
```

  - **`s12_realistic_tree.rs`:**
    - `a_file_name_that_is_not_utf8_is_refused_at_scan_with_exit_3` → rename to
      `…_refused_at_plan_with_exit_3`. Run `plan` instead of `scan`, keep the exit-3
      and stderr assertions, and extend the final assertion to
      `!s.work.join("staged").exists()` with the message "a refused plan stages
      nothing".
    - In `a_messy_tree_scans_plans_burns_and_verifies`, the first `scan`'s
      `Scanned: 4 added` assertion moves onto `pl`. The closing `sc2` (no plan
      follows) becomes `let pl2 = cli.run(&["plan", "p"]);` with the same two
      assertions. Rename that test to `a_messy_tree_plans_burns_and_verifies`.
  - **`s14_deferred_deletions.rs`:** drop `&["scan", "p"][..],` from the step array.
    That leaves `&["plan", "p", "--finalize"][..]` as the first element, so it needs
    the `[..]`. Both `sc`/`sc2` assertions move onto `pl`/`pl2`.
  - **`s01_happy.rs`:** rename the test to
    `init_plan_burn_verify_and_the_disc_contents_equal_the_source`. Its doc comment
    loses "scan →".
  - **`s09_mark_dead.rs`, `s13_copies.rs`:** rule only. Leave the "Re-burn on next
    scan/plan/burn" and "restored on next scan/plan/burn" assertions alone; Task 5
    changes that wording.

- [ ] **Step 3: Check no `scan` invocation is left.**

  Run: `grep -rn '"scan"' crates/diskrement-cli/tests/`

  Expected: no output.

- [ ] **Step 4: Bench script.** In `scripts/perf-bench.sh`:
  - The header line `# Runs init -> scan -> plan -> burn (simulated) -> scan against SOURCE_DIR`
    becomes `# Runs init -> plan -> burn (simulated) -> plan against SOURCE_DIR`.
    (Each `plan` scans first; the second one is the warm rescan.)
  - In the header comments, replace `scan2` with `plan2` and `scan1` with `plan1`.
  - The steps become:

```bash
run_step plan1 "${GLOBAL_ARGS[@]}" plan "$PROJECT"
run_step burn  "${GLOBAL_ARGS[@]}" burn "$PROJECT" --yes
run_step plan2 "${GLOBAL_ARGS[@]}" plan "$PROJECT"
```

  - The evidence block becomes:

```bash
echo "cache evidence (P2: plan2 should show ~0 hashed):"
echo -n "  plan1: "; grep '^Hashed' "$LOG_DIR/plan1.stdout" || echo "(no Hashed line found)"
echo -n "  plan2: "; grep '^Hashed' "$LOG_DIR/plan2.stdout" || echo "(no Hashed line found)"
```

    and the next note reads `read plan2's rchar as '<< source size', not '== 0'.`.
  - Grep the script for any other `scan1`/`scan2` (for example in a summary table or
    in `drop_caches_if_requested`'s message "before scan1") and rename them.
  - Run `bash -n scripts/perf-bench.sh`. Expected: no output.

- [ ] **Step 5: Verify.** Run the three verification commands.

  Expected counts: `binary` 45 (−1 `scan_on_an_unknown_project_also_exits_2`),
  `scenarios` 20; everything else as after Task 2.

- [ ] **Step 6: Commit.**

```bash
git add -A crates/diskrement-cli/tests scripts/perf-bench.sh
git commit -m "test(cli): drive every cycle through plan, which now scans (#4)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: remove `scan`

Remove `Pipeline::scan`, the `scan` subcommand and the reuse path's `Option`, and
move the shell unit tests that still call `scan` over to `plan`.

**Files:**
- Modify: `crates/diskrement-shell/src/pipeline/mod.rs`, `pipeline/reports.rs`
- Modify: `crates/diskrement-cli/src/args.rs`, `main.rs`, `render.rs`, `snapshots/scan.snap`
- Modify: `crates/diskrement-cli/tests/binary.rs`

- [ ] **Step 1: Write the failing CLI test.** In `tests/binary.rs` `mod init_scan_status`
  (rename the module to `init_plan_status`), add:

```rust
    /// `scan` was merged into `plan` (#4); it is now an unknown subcommand,
    /// which is clap's argument error, exit 1.
    #[test]
    fn scan_is_no_longer_a_command() {
        cmd()
            .args(["scan", "p"])
            .assert()
            .failure()
            .code(1)
            .stderr(contains("unrecognized subcommand"));
    }
```

- [ ] **Step 2: Run it and watch it fail.**

  Run: `cargo test -p diskrement-cli --test binary scan_is_no_longer_a_command`

  Expected: FAIL. `scan p` exits 2 ("project not found").

- [ ] **Step 3: Remove the CLI command.**
  - `args.rs`: delete the `Scan { … }` variant with its doc comment, and the test
    `scan_flags_default_off_and_parse_on`.
  - `main.rs`: delete the `Command::Scan { … } => { … }` arm and the
    `Command::Scan { name, .. } => name,` line near the top of `dispatch`.

- [ ] **Step 4: Remove `Pipeline::scan` and the `Option`.**
  - `pipeline/mod.rs`: delete `pub fn scan(…)` with its doc comment. The module doc
    at the top says "— for scan — **writes no catalog rows**". Change it to "— for
    plan — …".
  - `pipeline/reports.rs`: `ScanReport`'s doc becomes
    `/// What \`plan\`'s scan found. \`hashed\` counts the files hashed this run; the rest reused a fingerprint.`
    and the field becomes `pub hashed: usize,`.
  - `count(&self, p, files, hashed: usize)`, with `hashed` passed through unchanged.
  - `plan`: `self.plan_scanned(&p, &files, hashed, …)`; `plan_scanned`'s parameter
    becomes `hashed: usize`; `plan_staged` passes `0`.
  - `PipelinePaths`' doc mentions "`work_dir` for `scan`/`status`". Change it to
    `plan`/`status`.

- [ ] **Step 5: Move the shell unit tests off `scan`.** In `pipeline/mod.rs` `mod tests`:

  | Test at `b98fabd` | Becomes |
  |---|---|
  | `scan_on_a_fresh_project_reports_all_added_and_stages_them` | `plan_on_a_fresh_project_reports_all_added_and_stages_the_scan`: `let r = e.pipeline.plan("p", None, false, false).unwrap().scan;`, `r.hashed == 2` |
  | `an_unexpected_scan_failure_names_the_file_and_the_os_error` | same name; `e.pipeline.plan("p", None, false, false).unwrap_err()` |
  | `a_second_scan_without_burn_reuses_the_staged_scan` | **delete** (Task 2's `a_plan_after_nothing_to_burn_sees_a_file_added_since` is its opposite) |
  | `a_forced_rescan_hashes_nothing_once_the_cache_is_primed` | `a_second_plan_hashes_nothing_once_the_cache_is_primed`: two `plan` calls; `hashed` 3 then 0 |
  | `a_forced_rescan_rehashes_only_a_modified_file` | `a_re_plan_rehashes_only_a_modified_file` |
  | `verify_forces_a_full_rehash_even_with_a_primed_cache` | `rehash_forces_a_full_rehash_even_with_a_primed_cache`: second call `plan("p", None, false, true)`, `hashed == 2` |
  | `verify_with_a_staged_scan_and_no_force_still_rehashes` | **delete** (that path no longer exists) |
  | `a_failing_cache_read_still_succeeds_and_hashes_everything` | `pl.plan("p", None, false, false).unwrap().scan` |
  | `a_failing_cache_write_still_succeeds_and_nothing_was_cached` | two `plan` calls, both `hashed == 3`; the comment says "so the second plan re-hashes everything" |
  | `re_planning_overwrites_the_staged_plan_and_leaves_the_staged_scan` | `re_planning_replaces_both_the_staged_scan_and_the_staged_plan`: below |
  | the other `plan_*` tests from Task 2's table | delete the `e.pipeline.scan(…)` line before the `plan` call |

  `re_planning_replaces_both_the_staged_scan_and_the_staged_plan`:

```rust
    #[test]
    fn re_planning_replaces_both_the_staged_scan_and_the_staged_plan() {
        let mut e = setup();
        write_src(&e, "a.txt", b"a");
        e.pipeline.init(&project_init(&e)).unwrap();
        e.pipeline.plan("p", None, false, false).unwrap();
        write_src(&e, "b.txt", b"b");
        let second = e.pipeline.plan("p", None, true, false).unwrap().plan;
        assert!(second.sets[0].disc.will_finalize);
        assert_eq!(staged::read_plan(&e.work).unwrap().unwrap(), second);
        assert_eq!(staged::read_scan(&e.work).unwrap().unwrap().len(), 2);
    }
```

  In the burn and copies modules:
  - `a_deletion_deferred_past_its_discs_death_is_still_recorded` (~2445): replace the
    `Pipeline::new(…).scan("p", false, false).unwrap()` expression with
    `Pipeline::new(…).plan_staged("p", None, false).unwrap().scan` and keep
    `assert_eq!(scan.deleted, 1, "the scan agrees with the plan");`.
  - `burned_twice`: delete `pipeline.scan("p", false, false).unwrap();`.
  - Every `pipeline(&mut env).scan("p", false, true).unwrap();` directly followed by a
    `plan` call: delete it.
  - `mark_dead_after_a_scan_saw_the_deletion_reports_the_file_lost` →
    `mark_dead_after_a_plan_saw_the_deletion_reports_the_file_lost`. Its lone `scan`
    becomes `pipeline(&mut env).plan("p", None, false, false).unwrap();`. Update the
    doc comment ("once a plan has seen …").
  - `a_file_back_before_a_rescan_is_reported_lost_but_burned_again`: the first lone
    `scan` becomes a `plan`. Replace the second `let scan = …scan(…)` together with
    the `plan` that follows it with:

```rust
        let out = pipeline(&mut env).plan("p", None, false, false).unwrap();
        assert_eq!(out.scan.added, 1, "no live copy, so the file is new again");
```

    Its doc comment's "without a rescan" / "the next scan" become "without a re-plan"
    / "the next plan".
  - `run_to_completion` (proptest): delete the `scan` line. The doc comment
    "scan → plan → burn" becomes "plan → burn".

  Then check that nothing is left:

  Run: `grep -n '\.scan(' crates/diskrement-shell/src/pipeline/mod.rs`

  Expected: no output. (`scanner::scan(` in `fresh_scan` does not match `.scan(`.)

- [ ] **Step 6: Render.** In `crates/diskrement-cli/src/render.rs`:

```rust
pub fn scan_report(r: &ScanReport) -> String {
    format!(
        "Scanned: {} added, {} modified, {} deleted, {} unchanged.\n\
         Hashed {} of {} files ({} fingerprints reused).",
        r.added,
        r.modified,
        r.deleted,
        r.unchanged,
        r.hashed,
        r.scanned_files,
        r.scanned_files - r.hashed
    )
}
```

  - Tests: delete `a_reused_staged_scan_omits_the_hashed_line`. In
    `a_fresh_scan_reports_hashed_and_reused`, write `hashed: 3`.
  - Change `scan_of`'s `hashed` parameter to `usize`.
  - In `scan_gallery`, delete the "a reused staged scan (no Hashed line)" entry and
    pass `3` and `0` for the others.
  - Update `snapshots/scan.snap`: the "## a reused staged scan (no Hashed line)" section
    and its line disappear, and nothing else changes.

- [ ] **Step 7: Make the test from Step 1 pass and check for leftovers.**

  Run: `cargo test -p diskrement-cli --test binary scan_is_no_longer_a_command`

  Expected: PASS. If clap's wording differs from "unrecognized subcommand", match
  what clap 4 prints, from the test output. The exit code is the contract; the
  wording only pins that it is the unknown-subcommand error.

  Run: `grep -rn 'NoStagedScan\|Command::Scan\|hashed: Some\|hashed: None\|scan --force\|scan --verify' crates/`

  Expected: no output.

- [ ] **Step 8: Verify.** Run the three verification commands.

  Expected counts:
  - shell lib 370 (372, −2 deleted reuse tests);
  - cli bin unit 107 (109, −1 `scan_flags…`, −1 `a_reused_staged_scan…`);
  - `binary` 46 (45 + 1);
  - `scenarios` 20.

- [ ] **Step 9: Commit.**

```bash
git add -A crates/
git commit -m "feat(cli)!: remove the scan command; plan scans (#4)" -m "The reuse path and its Option<usize> hashed count go with it. scan is now an unknown subcommand (exit 1)." -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: user-facing wording says "plan → burn"

**Files:**
- Modify: `crates/diskrement-cli/src/render.rs` and its snapshots (`burn.snap`, plus `status.snap` / `mark_dead.snap` if they hold these lines)
- Modify: `crates/diskrement-cli/tests/binary.rs`, `tests/scenarios/s09_mark_dead.rs`, `tests/scenarios/s13_copies.rs`

- [ ] **Step 1: Change the expectations first.** Find every expected string:

  Run: `grep -rn 'scan → plan → burn\|scan/plan/burn' crates/`

  In every **test** (inline `assert_eq!` in `render.rs`, `binary.rs`, s09 and s13),
  make these replacements:
  - `Run: scan → plan → burn` → `Run: plan → burn`;
  - `re-burn on next scan/plan/burn` → `re-burn on next plan/burn`;
  - `Re-burn on next scan/plan/burn` → `Re-burn on next plan/burn`;
  - `restored on next scan/plan/burn` → `restored on next plan/burn`.

- [ ] **Step 2: Run them and watch them fail.**

  Run: `cargo test -p diskrement-cli`

  Expected: FAIL in the updated render tests, the `binary` test and s09/s13.

- [ ] **Step 3: Change the four format strings in `render.rs`:**
  - `"~{} more discs of changes remain. Run: plan → burn"` (~206);
  - `"Copy sets are incomplete. Run: plan → burn"` (~216);
  - `"No live copy: {} files ({}) — re-burn on next plan/burn."` (~259);
  - `"Re-burn on next plan/burn ({}):"` (~461);
  - `"Short of a copy — restored on next plan/burn ({}):"` (~475).

  Fix the comments beside them as well. "Worded after `mark_dead`'s 'Re-burn on next
  scan/plan/burn'" becomes "… 'Re-burn on next plan/burn'". "the operator's next
  action is `scan`" becomes "… is `plan`".

- [ ] **Step 4: Update the snapshots.** Use `cargo insta test -p diskrement-cli --accept`
  or edit the files by hand. Then run `git diff crates/diskrement-cli/src/snapshots/`.
  It should show only the wording swaps above.

  Run: `grep -rn 'scan → plan\|scan/plan' crates/`

  Expected: no output.

- [ ] **Step 5: Verify.** Run the three verification commands. Expected counts: unchanged from Task 4.

- [ ] **Step 6: Commit.**

```bash
git add -A crates/diskrement-cli
git commit -m "fix(cli): next-step advice says plan → burn (#4)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: docs

**Files:**
- Modify: `README.md`, `CLAUDE.md`, `docs/smoke-test-real-media.md`
- Modify: `docs/superpowers/specs/2026-05-31-diskrement-core-engine-design.md` (§9.1)
- Modify: `docs/superpowers/specs/2026-06-02-diskrement-plan-3a-orchestration-design.md`
- Modify: `docs/superpowers/specs/2026-09-05-rust-rewrite-design.md` (§3.2)

- [ ] **Step 1: README.**
  - "What it does", step 1: "**Scans** the source tree …" stays, but step 2 becomes
    "**Plans** a burn (the same command scans first): …".
  - "a full scan → plan → burn → verify → history loop" → "a full plan → burn →
    verify → history loop".
  - Workflow block: delete `diskrement scan photos`.
  - Dead discs: "the next `scan` / `plan` / `burn` writes them" → "the next `plan` /
    `burn` writes them".
  - Status line example: `re-burn on next scan/plan/burn.` → `re-burn on next plan/burn.`
  - The stale-plan paragraph: "re-run `plan`, which is the whole recovery: the staged
    scan and the source snapshot both survive, and `plan` re-reads the disc states
    and targets fresh media." becomes "re-run `plan`, which is the whole recovery: it
    scans the source again, re-reads the disc states and targets fresh media."
  - Performance: "`scan` hashes new or changed content" → "`plan` hashes new or
    changed content (it scans the source before planning)". "`scan` also keeps a
    fingerprint cache" → "`plan`'s scan also keeps a fingerprint cache".
  - Replace the sentence "The first scan of a large library is the expensive one; …
    (omitted when a previously staged scan is reused, since no scan ran — `scan
    --force` rescans instead of reusing it)." with "The first scan of a large library
    is the expensive one; later ones hash only what changed, and `plan` reports the
    split, e.g. `Hashed 12 of 75618 files (75606 fingerprints reused).` Every `plan`
    scans afresh, so a cycle that found nothing to burn never hides later changes."
  - "one full re-hash on the next scan" → "one full re-hash on the next plan".
    "`scan --verify` re-hashes everything regardless of the cache" → "`plan --rehash`
    re-hashes everything regardless of the cache".
  - Bench paragraph: "a full init → scan → plan → burn → scan cycle" → "a full init →
    plan → burn → plan cycle (each plan scans; the second is the warm rescan)".
    "before the first scan" → "before the first plan".
  - Building: `cargo run -p diskrement-cli -- scan photos` → `cargo run -p diskrement-cli -- plan photos`.

  Run: `grep -n '\bscan\b' README.md`

  Every remaining hit must describe scanning as part of `plan`, not a command.

- [ ] **Step 2: CLAUDE.md.**
  - "One disc per copy set per cycle": "Each scan→plan→burn writes" → "Each
    plan→burn writes". "\"re-run scan --force, then plan and burn\" for a missing
    snapshot" → "\"re-run plan, then burn\" for a stale plan or a missing snapshot",
    merged with the stale-plan clause before it, so the sentence names the advice
    once.
  - "Deletions ride the next burned disc": "on every scan until" → "on every plan
    until". "Meanwhile `scan` writes those deletions" → "Meanwhile `plan` writes those
    deletions".
  - The Build section's commands do not mention `scan`; leave them alone.

- [ ] **Step 3: Smoke test.** In `docs/smoke-test-real-media.md`, step 1 "`scan`,
  `plan`," → "`plan`,", and step 3 "`scan`/`plan`/`burn` again" → "`plan`/`burn`
  again". Leave the dated result notes under each step unchanged: they record what was
  run then.

- [ ] **Step 4: Spec amendments.** Do not rewrite history. Append a dated note.
  - `2026-05-31-diskrement-core-engine-design.md`, directly under the
    "**Restage policy:**" bullet in §9.1:

```markdown
  > **Amended 2026-10-12 (#4):** superseded. `plan` now snapshots and scans the
  > source itself on every run, and the `scan` command is gone; nothing is reused.
  > See `2026-10-12-merge-scan-into-plan-design.md`.
```

  - `2026-06-02-diskrement-plan-3a-orchestration-design.md`, directly under the
    `**plan(name, discTypeOverride)**` paragraph:

```markdown
> **Amended 2026-10-12 (#4):** `scan` and `plan` are one operation. `plan` clears the
> staged scan and plan, snapshots, scans and plans; `NoStagedScan` no longer exists.
> See `2026-10-12-merge-scan-into-plan-design.md`.
```

  - `2026-09-05-rust-rewrite-design.md`, at the end of §3.2:

```markdown
> **Amended 2026-10-12 (#4):** eight commands, not nine. `scan` was merged into
> `plan` (`scan --verify` became `plan --rehash`). That freeze was the parity
> requirement for the cutover, which is complete. See
> `2026-10-12-merge-scan-into-plan-design.md`.
```

- [ ] **Step 5: Verify.** Run `cargo fmt --check`. Docs-only changes cannot break the
  build, so the full verification waits for Task 7.

- [ ] **Step 6: Commit.**

```bash
git add README.md CLAUDE.md docs/
git commit -m "docs: plan scans; scan is gone (#4)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: final verification and merge

- [ ] **Step 1: Full verification on the branch tip.**

```bash
DISKREMENT_SCENARIOS_REQUIRED=1 cargo test --workspace 2>&1 | grep -E '^test result|Running'
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
cargo build --release
```

  Expected counts:

  | Suite | Baseline | Final | Why |
  |---|---|---|---|
  | core | 82 | 82 | |
  | shell lib | 368 | 370 | +1 Task 1, +3 net Task 2, −2 Task 4 |
  | shell integration (8/6/2/5) | 21 | 21 | |
  | cli bin unit | 108 | 107 | +1 `--rehash`, −1 `scan` args, −1 reused render |
  | `binary` | 46 | 46 | −1 `scan_on_an_unknown_project…`, +1 `scan_is_no_longer_a_command` |
  | `scenarios` | 20 | 20 | |

  `xorriso_image` must report 5 passed, not skipped. If it skipped, say so.

- [ ] **Step 2: Release-binary smoke test.** This uses the simulated backend in the
  scratchpad, never the live `immich-dsz` project:

```bash
T=$(mktemp -d)   # must be on a CoW filesystem, as the scenarios' TMPDIR is
mkdir -p "$T/src" && echo hi > "$T/src/a.txt"
B=target/release/diskrement
$B --catalog "$T/c.sqlite" init p --source "$T/src" --work-dir "$T/work"
$B --catalog "$T/c.sqlite" --burner simulated plan p
$B --catalog "$T/c.sqlite" scan p; echo "exit $?"
$B --catalog "$T/c.sqlite" --burner simulated burn p --yes
$B --catalog "$T/c.sqlite" --burner simulated plan p
echo new > "$T/src/b.txt"
$B --catalog "$T/c.sqlite" --burner simulated plan p
rm -rf "$T"
```

  Expected:
  - the first `plan` prints `Scanned: 1 added …`, `Hashed 1 of 1 …` and a disc 1 plan;
  - `scan` exits 1;
  - the second `plan` prints `Nothing to burn: no changes to plan.`;
  - the third `plan` prints `Scanned: 1 added, 0 modified, 0 deleted, 1 unchanged.`
    and plans `b.txt`.

- [ ] **Step 3: Record deviations.** Append an "Execution deviations" section to this
  plan for anything that differed from what is written here. If nothing did, write
  "None". Commit it as `docs(plan): merge-scan-into-plan execution notes (#4)`.

- [ ] **Step 4: Merge (after the user confirms).**

```bash
git switch main
git merge --no-ff feat/merge-scan-into-plan -m "Merge feat/merge-scan-into-plan: plan scans; scan is removed (#4)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push origin main
git branch -d feat/merge-scan-into-plan
git push origin --delete feat/merge-scan-into-plan   # only if it was pushed
gh issue close 4 --reason completed --comment "Fixed in the merge of feat/merge-scan-into-plan: plan now scans every time, and scan is removed."
```

- [ ] **Step 5: Live-backup note.** Update the `photo-backup-live` memory. The cycle is
  now `plan` → `burn` → `verify NA/NB`, and the `scan --force` workaround is gone.
  `--rehash` replaces `scan --verify`.

---

## Execution deviations

Executed subagent-driven on 2026-10-12. Each task had a spec review and a code-quality review, and the whole branch had a final review. These are the places where the work differs from the text above.

- **Task 1 (f201e88, 0eae71e).** The doc comment on `a_staged_plan_without_its_scan_is_the_no_staged_plan_error` said a plan without its scan "can only come from deleting files by hand". That is wrong: `staged::clear` removes `scan.json` before `plan.json`, so an interrupted clear also leaves that state. The comment was corrected, and the sibling test `burning_without_a_staged_plan_is_a_state_error` gained its `exit_code() == 2` assertion.
- **Task 2 (39ee53a, ef9e8e0).** The review fixes:
  - `burn` now reads `scan.json` through `corrupt_scan`, as spec §5 says. The plan left this out, and without it Task 4 would have turned `corrupt_scan` into dead code. A new test covers it: `burning_with_a_corrupt_staged_scan_says_so`.
  - A new helper, `clear_staged`, used by both `plan` and `reset`, names the `staged/` directory when a stray file blocks the clear. Previously the error was a bare "Directory not empty". A new test covers it: `a_stray_file_in_staged_fails_plan_naming_the_directory`.
  - `plan_on_a_fresh_project_scans_by_itself` also checks that the scan block comes before the plan block.
  - The `StalePlan` doc comment was reworded, because `plan` now scans afresh.

  The two new tests are why the shell lib ends at **372**, not the 370 this plan expected.
- **Task 3 (40bc26e, 03edeb1).**
  - The s08 tail in this plan did not discriminate. Its intervening "Nothing burned" `burn` clears the staged state, so a regression back to reuse would still pass. The fix adds a nothing-to-burn `plan` that is not burned, before the new file appears. The test now asserts that `scan.json` is staged at that point.
  - The `binary.rs` module `init_scan_status` was renamed here rather than in Task 4.
  - `perf-bench.sh` notes in its header that a plan step's time includes planning.
- **Task 4 (2d7bb47).** Further stale comments were fixed:
  - `fresh_scan`'s snapshot comment, which wrongly said "only `reset` removes it";
  - the `stale_detail` doc and a drift test's doc;
  - the mark-dead module's `env()` doc, which said it used the real `plan` when it uses `plan_staged`;
  - `pending_deletions_path`.

  The `count` stderr warning now reads "until the next plan".
- **Task 5 (fe01d8e).** The Task 4 review's comment items were folded in:
  - the `staged.rs` module doc and `timestamps.rs`;
  - `count`'s "the plan it is part of";
  - `fresh_scan`'s parameter `verify` renamed to `rehash`;
  - the test renamed `a_file_back_before_a_re_plan_is_reported_lost_but_burned_again`;
  - the `ScanReport` doc wrap.

  A sweep also changed the `reset` help to "Discard staged plan state.", plus the `ReburnEntry` and `replication.rs` comments.
- **Task 6 (55cb803).** This commit's trailer names Claude Sonnet 5.5, the model that wrote it.
- **Task 7.** The final review found stale comments in `planning.rs`'s module doc and the `restage` helper, and a split sentence in the `perf-bench.sh` header. All three are fixed in the commit that adds this section.
- **Left open, for the user to decide:** a stray file in `staged/` (e.g. a vim swap file) now blocks every `plan` with exit 5, and the message ends "(this is a bug)". The message names the directory, and nothing is lost. Before #4 the same check ran only in `burn` and `reset`.

**Final counts** (`DISKREMENT_SCENARIOS_REQUIRED=1`, xorriso present):

| Suite | Baseline | Final | Why |
|---|---|---|---|
| core | 82 | 82 | |
| shell lib | 368 | 372 | Task 1 +1, Task 2 +3 net and +2 from review fixes, Task 4 −2 |
| shell integration (8/6/2/5) | 21 | 21 | |
| cli bin unit | 108 | 107 | +1 `--rehash`, −1 `scan` args, −1 reused render |
| `binary` | 46 | 46 | −1 `scan_on_an_unknown_project…`, +1 `scan_is_no_longer_a_command` |
| `scenarios` | 20 | 20 | |
| **total** | 645 | 648 | |

clippy (`-D warnings`) and `cargo fmt --check` are clean, and `cargo build --release` succeeds. The release-binary smoke test (Step 2) behaved as expected.
