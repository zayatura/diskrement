# Deferred Deletions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A cycle with nothing to place writes no disc, at every `copies`. Its deletions are recorded by the next disc that is burned anyway (issue #9).

**Architecture:** `plan_from` drops its set-A fallback, so a plan can have zero sets. Tombstones are computed once, by a new pure `diff::tombstones` over the catalog's not-deleted paths (`Catalog::fingerprints`), and passed into `plan_from`. The scan report counts deletions the same way. `burn` of a zero-set plan writes nothing, ends the cycle, and reports the deferred count.

**Tech Stack:** Rust workspace (`diskrement-core` → `diskrement-shell` → `diskrement-cli`), rusqlite, cargo test/clippy/fmt.

**Spec:** `docs/superpowers/specs/2026-10-11-deferred-deletions-design.md`

**Branch:** `feat/deferred-deletions` (already created off `main` at a74a542; the spec is committed there as c04d9c2).

**Verification (CLAUDE.md):**

```
DISKREMENT_SCENARIOS_REQUIRED=1 cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

The scenarios need a CoW `TMPDIR`. If `/tmp` is not CoW, point `TMPDIR` at a btrfs/xfs-reflink directory. With `DISKREMENT_SCENARIOS_REQUIRED=1` a non-CoW `TMPDIR` fails loudly instead of skipping.

Run `cargo fmt` before every commit; the snippets below are not guaranteed to be rustfmt-clean.

---

## File map

| File | Change |
|---|---|
| `crates/diskrement-core/src/diff.rs` | Add `pub fn tombstones`. |
| `crates/diskrement-shell/src/pipeline/planning.rs` | `plan_from` takes `tombstones`, drops the fallback; tests updated. |
| `crates/diskrement-shell/src/pipeline/mod.rs` | `plan` and `count` use `diff::tombstones`; `burn` sets `deferred_deletions`; new pipeline tests; three test call sites of `plan_from` updated. |
| `crates/diskrement-shell/src/pipeline/reports.rs` | `BurnReport.deferred_deletions`. |
| `crates/diskrement-cli/src/render.rs` | Zero-set plan text, "Nothing burned" text; test fixtures get the new field. |
| `crates/diskrement-cli/tests/scenarios/s08_idempotent.rs` | `burn` after a no-op plan writes nothing. |
| `crates/diskrement-cli/tests/scenarios/s14_deferred_deletions.rs` + `main.rs` | New copies=2 scenario. |
| `CLAUDE.md`, multiple-copies spec §6.1 and plan "Known limitations" | Docs. |

---

### Task 1: `diff::tombstones` in core

**Files:**
- Modify: `crates/diskrement-core/src/diff.rs`

- [ ] **Step 1: Write the failing tests**

Append inside `mod tests` in `crates/diskrement-core/src/diff.rs`:

```rust
    // --- tombstones ---

    fn rp(s: &str) -> RelPath {
        RelPath(s.to_string())
    }

    #[test]
    fn a_recorded_path_absent_from_the_scan_is_a_tombstone_and_output_is_sorted() {
        let recorded = [rp("z"), rp("keep"), rp("a")];
        let t = tombstones(recorded.iter(), &[sf_hash("keep", "h")]);
        assert_eq!(t, vec![rp("a"), rp("z")]);
    }

    #[test]
    fn a_scanned_path_that_was_never_recorded_is_not_a_tombstone() {
        let t = tombstones([rp("a")].iter(), &[sf_hash("a", "h"), sf_hash("new", "h")]);
        assert!(t.is_empty());
    }
```

- [ ] **Step 2: Run the tests to check that they fail**

Run: `cargo test -p diskrement-core diff::tests::a_recorded_path`
Expected: compile error, `cannot find function tombstones`.

- [ ] **Step 3: Implement**

In `crates/diskrement-core/src/diff.rs`, add after `pub fn diff`:

```rust
/// The cycle's deletions: every path the catalog still holds as not deleted
/// that is absent from the scan, sorted.
///
/// `recorded` is the catalog's history (latest change not `DELETED`), **not**
/// the live view: a version whose every copy died is gone from the view but
/// still current in the history, and its deletion must still be recorded.
/// Deletions are deferred to the next burned disc (issue #9), so this set is
/// re-derived on every scan until a commit records it.
pub fn tombstones<'a>(
    recorded: impl IntoIterator<Item = &'a RelPath>,
    scanned: &[ScannedFile],
) -> Vec<RelPath> {
    let scanned_paths: HashSet<&RelPath> = scanned.iter().map(|f| &f.rel_path).collect();
    let mut out: Vec<RelPath> = recorded
        .into_iter()
        .filter(|p| !scanned_paths.contains(p))
        .cloned()
        .collect();
    out.sort();
    out
}
```

- [ ] **Step 4: Run the tests to check that they pass**

Run: `cargo test -p diskrement-core diff::`
Expected: all `diff` tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/diskrement-core/src/diff.rs
git commit -m "feat(core): diff::tombstones from the catalog history (#9)"
```

---

### Task 2: `plan_from` takes tombstones and drops the set-A fallback

**Files:**
- Modify: `crates/diskrement-shell/src/pipeline/planning.rs` (`plan_from` at ~line 41-120, tests)
- Modify: `crates/diskrement-shell/src/pipeline/mod.rs` (`Pipeline::plan` ~line 219; test call sites ~1388, ~1814, ~2375)

- [ ] **Step 1: Write the failing tests**

In `planning.rs` `mod tests`:

(a) Replace `a_deletion_only_cycle_still_plans_one_set_a_disc_for_its_tombstones` with:

```rust
    #[test]
    fn a_deletion_only_cycle_plans_no_disc_and_keeps_its_tombstones() {
        let kept = vec![file("a.bin", 300)];
        let view = live_in(&[file("a.bin", 300), file("gone.bin", 10)], &[0, 1]);
        let states = [open(1, 0, 310), open(1, 1, 310)];
        let report = plan_from(
            &view,
            &kept,
            vec![RelPath("gone.bin".into())],
            &states,
            DiscType::BdR,
            Capacity::new(1_000),
            false,
            2,
            |_| 7,
        );
        assert!(report.sets.is_empty(), "no disc, in either set");
        assert_eq!(report.tombstones, vec![RelPath("gone.bin".into())]);
    }

    #[test]
    fn a_single_copy_deletion_only_cycle_plans_no_disc_either() {
        let kept = vec![file("a.bin", 300)];
        let view = live_in(&[file("a.bin", 300), file("gone.bin", 10)], &[0]);
        let report = plan_from(
            &view,
            &kept,
            vec![RelPath("gone.bin".into())],
            &[],
            DiscType::BdR,
            Capacity::new(1_000),
            false,
            1,
            |_| 1,
        );
        assert!(report.sets.is_empty(), "no fresh empty disc for tombstones");
        assert_eq!(report.tombstones.len(), 1);
    }

    #[test]
    fn a_two_copy_no_op_cycle_plans_no_disc() {
        let files = vec![file("a.bin", 300)];
        let view = live_in(&files, &[0, 1]);
        let report = plan2(&view, &files, &[open(1, 0, 300), open(1, 1, 300)], false);
        assert!(report.sets.is_empty());
        assert!(report.tombstones.is_empty());
    }

    #[test]
    fn finalize_with_deletions_only_carries_the_tombstones_on_the_seals() {
        let kept = vec![file("a.bin", 300)];
        let view = live_in(&[file("a.bin", 300), file("gone.bin", 10)], &[0, 1]);
        let states = [open(1, 0, 310), open(1, 1, 310)];
        let report = plan_from(
            &view,
            &kept,
            vec![RelPath("gone.bin".into())],
            &states,
            DiscType::BdR,
            Capacity::new(1_000),
            true,
            2,
            |_| 7,
        );
        assert_eq!(report.sets.len(), 2);
        assert!(report
            .sets
            .iter()
            .all(|sp| sp.disc.will_finalize && sp.disc.files.is_empty()));
        assert_eq!(report.tombstones, vec![RelPath("gone.bin".into())]);
    }
```

(b) Replace `a_no_op_scan_plans_an_empty_disc_rather_than_panicking` with:

```rust
    #[test]
    fn a_no_op_scan_plans_no_disc() {
        let files = vec![file("a.bin", 300)];
        let report = plan_from(
            &live_in(&files, &[0]),
            &files,
            Vec::new(),
            &[],
            DiscType::BdR,
            Capacity::new(1_000),
            false,
            1,
            |_| 1,
        );
        assert!(report.sets.is_empty());
        assert!(report.tombstones.is_empty());
    }
```

(c) Replace `deletions_become_tombstones_on_this_cycles_disc` with:

```rust
    #[test]
    fn tombstones_ride_this_cycles_disc_when_one_is_planned() {
        let report = plan_from(
            &CatalogView::from_scanned(&[file("gone.bin", 100)]),
            &[file("a.bin", 100)],
            vec![RelPath("gone.bin".into())],
            &[],
            DiscType::BdR,
            Capacity::new(1_000),
            false,
            1,
            |_| 1,
        );
        assert_eq!(report.sets.len(), 1);
        assert_eq!(report.sets[0].disc.files[0].rel_path.as_str(), "a.bin");
        assert_eq!(report.tombstones, vec![RelPath("gone.bin".into())]);
    }
```

(d) Every other `plan_from(` call in `planning.rs` tests (the direct calls at ~255, 284, 312, 322, 347, 379, 402, 441 and the helpers `plan_of` ~457, `plan1` ~507, `plan2` ~525): insert `Vec::new(),` as the new third argument, right after the scanned-files argument. For example, `plan2` becomes:

```rust
        plan_from(
            view,
            files,
            Vec::new(),
            states,
            DiscType::BdR,
            Capacity::new(1_000),
            finalize,
            2,
            |_| 7,
        )
```

(e) The same `Vec::new(),` insertion in the three test call sites in `pipeline/mod.rs` (`crate::pipeline::planning::plan_from(` at ~1388, ~1814, ~2375). Each passes `&scanned` as the second argument; insert after it.

- [ ] **Step 2: Run the tests to check that they fail**

Run: `cargo test -p diskrement-shell pipeline::planning`
Expected: compile error, `plan_from` takes 8 arguments but 9 were supplied.

- [ ] **Step 3: Implement `plan_from`**

In `planning.rs`, replace the doc comment, the signature and the body of `plan_from` up to the `PlanReport` literal:

```rust
/// Diff once, plan every copy set, and reduce each set's full plan to the
/// **one disc that set writes this cycle**.
///
/// Per set: burn the first disc that actually carries files. The planner
/// keeps a set's open disc first by contract; when a new file cannot top it
/// off, that open disc is an empty finalizing placeholder — skip it so this
/// cycle writes the new disc instead of wasting a cycle on an empty session.
/// The skipped disc stays open.
///
/// A set with nothing to place is left out, unless `finalize` and it has an
/// open disc to seal. That can leave **no set at all** (a no-op scan, or a
/// deletion-only one): the plan then writes nothing, and its `tombstones` are
/// deferred to the next disc that is burned anyway (issue #9). Never fall back
/// to a disc just to carry tombstones: at `copies > 1` that opened an untwinned
/// disc.
///
/// `tombstones` are the cycle's deletions (`diff::tombstones`, computed
/// against the catalog history rather than the live view), carried as-is.
///
/// `next_session` returns the session ordinal an existing disc gets next; it
/// is recorded per set so `burn` can tell a disc it already committed from one
/// it has not.
#[allow(clippy::too_many_arguments)]
pub fn plan_from(
    view: &CatalogView,
    scanned: &[ScannedFile],
    tombstones: Vec<RelPath>,
    states: &[DiscState],
    disc_type: DiscType,
    cap: Capacity,
    finalize: bool,
    copies: u32,
    next_session: impl Fn(DiscKey) -> u32,
) -> PlanReport {
    let changes = diff::diff(view, scanned);
    let new_files: Vec<ScannedFile> = changes
        .added
        .iter()
        .chain(changes.modified.iter())
        .cloned()
        .collect();
    let catch_up = replication::shortfall(view, &changes, copies);

    let mut sets = Vec::new();
    let mut oversized: Vec<ScannedFile> = Vec::new();
    for (copy_set, extra) in (0..copies).zip(catch_up.iter()) {
        let to_place: Vec<ScannedFile> = new_files.iter().chain(extra.iter()).cloned().collect();
        let full = planner::plan_set(copy_set, &to_place, &tombstones, states, cap);
        for f in full.oversized {
            if !oversized.iter().any(|o| o.rel_path == f.rel_path) {
                oversized.push(f);
            }
        }
        // `plan_set` always returns at least one disc.
        let chosen = match full.discs.iter().position(|d| !d.files.is_empty()) {
            Some(i) => Some(i),
            None if finalize && !full.discs[0].is_new => Some(0),
            None => None,
        };
        if let Some(i) = chosen {
            sets.push(set_plan(&full.discs, i, finalize, extra, &next_session));
        }
    }
    oversized.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));

    PlanReport {
        copies,
        sets,
        disc_type,
        tombstones,
        oversized,
    }
}
```

(`set_a_discs` and the `if sets.is_empty()` fallback are gone.)

- [ ] **Step 4: Compute tombstones in `Pipeline::plan`**

In `pipeline/mod.rs` `Pipeline::plan`, after `let states = ...;` add:

```rust
        let recorded = self.catalog.fingerprints(p.id).map_err(Self::failed)?;
        let tombstones = diff::tombstones(recorded.keys(), &scanned);
```

and pass `tombstones,` as the third argument of `planning::plan_from(` (after `&scanned,`).

- [ ] **Step 5: Run the planning tests**

Run: `cargo test -p diskrement-shell pipeline::planning`
Expected: PASS.

- [ ] **Step 6: Run the shell crate and see what else depended on the fallback**

Run: `cargo test -p diskrement-shell`
Expected: PASS. If a pipeline test fails because it burned a no-op or deletion-only plan and expected a session, it was pinning the old fallback. Change it to expect zero sets, and record the test's name under "Execution deviations" at the bottom of this plan. A failure of any other kind is a bug: stop and investigate.

- [ ] **Step 7: Commit**

```bash
git add crates/diskrement-shell/src/pipeline/planning.rs crates/diskrement-shell/src/pipeline/mod.rs
git commit -m "feat(plan): a cycle with nothing to place plans no disc (#9)"
```

---

### Task 3: The scan report counts deletions the same way

**Files:**
- Modify: `crates/diskrement-shell/src/pipeline/mod.rs` (`fn count`, ~line 179)

- [ ] **Step 1: Implement**

Replace the body of `fn count` with:

```rust
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
```

This is tested in Task 4, Step 1, test (c), through the reuse path of `scan`.

- [ ] **Step 2: Build**

Run: `cargo build -p diskrement-shell`
Expected: builds with no warnings.

- [ ] **Step 3: Commit**

```bash
git add crates/diskrement-shell/src/pipeline/mod.rs
git commit -m "feat(scan): count deletions against the catalog history (#9)"
```

---

### Task 4: `burn` of a zero-set plan writes nothing and reports the deferral

**Files:**
- Modify: `crates/diskrement-shell/src/pipeline/reports.rs` (`BurnReport`)
- Modify: `crates/diskrement-shell/src/pipeline/mod.rs` (`Pipeline::burn`, tests)
- Modify: `crates/diskrement-cli/src/render.rs` (the four `BurnReport {` literals in tests)

- [ ] **Step 1: Write the failing pipeline tests**

Add to the test module in `pipeline/mod.rs` that defines `staged_env_with`, `restage`, `run_burn` and `deletions_of` (the same module as `a_two_copy_deletion_is_recorded_once_on_set_a_and_listed_by_both_discs`), right after that test:

```rust
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
    #[test]
    fn a_deletion_only_cycle_burns_nothing_and_ends_the_cycle() {
        let mut env = staged_env_with(&[("a.bin", b"aaaa"), ("gone.bin", b"gggg")], false, 2);
        run_burn(&mut env).unwrap();
        let before = row_counts(&env);

        let plan = restage(&mut env, &[("a.bin", b"aaaa")]);
        assert!(plan.sets.is_empty());
        assert_eq!(plan.tombstones, vec![RelPath("gone.bin".into())]);
        let report = run_burn(&mut env).unwrap();

        assert!(report.discs.is_empty());
        assert_eq!(report.deferred_deletions, Some(1));
        assert_eq!(row_counts(&env), before, "nothing reached the catalog");
        assert!(deletions_of(&env, "gone.bin").is_empty());
        assert!(!env.work.join("staging/disc-2A").exists());
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
```

If the module doesn't already import `ProjectId`, `DiscKey` or `NoopSink`, add them to its `use` lines. The neighbouring test `a_tombstone_whose_version_lost_its_last_copy_is_still_recorded` already uses all three.

- [ ] **Step 2: Run the tests to check that they fail**

Run: `cargo test -p diskrement-shell deferred`
Expected: compile error, no field `deferred_deletions` on `BurnReport`.

- [ ] **Step 3: Add the field**

In `pipeline/reports.rs`, replace the `BurnReport` doc comment and struct with:

```rust
/// The result of one burn cycle: every disc written this run, the largest
/// remaining estimate among their sets, and each set's coverage afterwards.
///
/// `discs` can be empty for two reasons. Either every planned disc had already
/// been committed by an earlier, interrupted run (`deferred_deletions` is
/// `None`), or the plan had no disc at all (`deferred_deletions` is `Some`,
/// holding the number of deletions left for the next burned disc, issue #9).
/// A zero-set `plan.json` can only come from `plan`: `burn` rewrites the plan
/// only while discs remain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BurnReport {
    pub discs: Vec<DiscBurn>,
    pub remaining: RemainingEstimate,
    pub coverage: Vec<SetCoverage>,
    pub deferred_deletions: Option<usize>,
}
```

- [ ] **Step 4: Set it in `Pipeline::burn`**

In `pipeline/mod.rs` `Pipeline::burn`, directly after the `if !report.oversized.is_empty() { ... }` block, add:

```rust
        // A plan with no disc is a finished cycle with nothing to write: the
        // loop below does not run, and the tail ends the cycle as usual. Its
        // deletions wait for the next burned disc (issue #9).
        let deferred_deletions = report.sets.is_empty().then_some(report.tombstones.len());
```

and in the final `Ok(BurnReport { ... })` add `deferred_deletions,` after `coverage: ...,`.

- [ ] **Step 5: Fix the render test fixtures**

In `crates/diskrement-cli/src/render.rs`, add `deferred_deletions: None,` to each of the four `BurnReport {` literals (~852, ~880, ~917, ~1422).

- [ ] **Step 6: Run the tests**

Run: `cargo test -p diskrement-shell deferred && cargo test -p diskrement-shell pipeline && cargo test -p diskrement-cli --lib`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/diskrement-shell/src/pipeline/reports.rs crates/diskrement-shell/src/pipeline/mod.rs crates/diskrement-cli/src/render.rs
git commit -m "feat(burn): a plan with no disc writes nothing and defers its deletions (#9)"
```

---

### Task 5: Render the zero-set plan and the deferred burn

**Files:**
- Modify: `crates/diskrement-cli/src/render.rs` (`plan_report` ~line 35, `burn_report` ~line 121, tests)

- [ ] **Step 1: Write the failing tests**

In the render test module that contains `a_plan_with_nothing_to_do_says_so` (it imports `PlanReport`, `SetPlan`, `DiscType`, `RelPath`, `BurnReport`, `RemainingEstimate` and has `scanned(...)`), add:

```rust
    fn zero_set_plan(tombstones: &[&str], oversized: Vec<ScannedFile>) -> PlanReport {
        PlanReport {
            copies: 2,
            sets: vec![],
            disc_type: DiscType::BdR,
            tombstones: tombstones.iter().map(|t| RelPath((*t).into())).collect(),
            oversized,
        }
    }

    #[test]
    fn a_plan_with_no_disc_and_no_deletions_has_nothing_to_burn() {
        assert_eq!(
            plan_report(&zero_set_plan(&[], vec![])),
            "Nothing to burn: no changes to plan."
        );
    }

    #[test]
    fn a_deletion_only_plan_says_the_deletions_wait_for_the_next_disc() {
        assert_eq!(
            plan_report(&zero_set_plan(&["gone"], vec![])),
            "Nothing to burn: 1 deletion waits for the next disc."
        );
        assert_eq!(
            plan_report(&zero_set_plan(&["a", "b"], vec![])),
            "Nothing to burn: 2 deletions wait for the next disc."
        );
    }

    #[test]
    fn a_plan_with_no_disc_still_lists_oversized_files() {
        assert_eq!(
            plan_report(&zero_set_plan(&[], vec![scanned("huge.bin", 99_000_000_000)])),
            "Oversized (cannot be placed):\n\
             \x20 huge.bin  99.0 GB"
        );
    }

    fn deferred_burn(n: usize) -> BurnReport {
        BurnReport {
            discs: vec![],
            remaining: RemainingEstimate {
                more_discs: 0,
                more_files: 0,
                more_bytes: 0,
            },
            coverage: vec![SetCoverage::default(), SetCoverage::default()],
            deferred_deletions: Some(n),
        }
    }

    #[test]
    fn a_burn_of_a_plan_with_no_disc_says_nothing_was_burned() {
        assert_eq!(
            burn_report(&deferred_burn(0), 2),
            "Nothing burned: no disc in this plan.\n\
             Copies: every current file has 2 copies.\n\
             No more changes — every copy set is up to date."
        );
        assert!(burn_report(&deferred_burn(1), 2)
            .starts_with("Nothing burned: 1 deletion will be recorded with the next disc.\n"));
        assert!(burn_report(&deferred_burn(3), 1)
            .starts_with("Nothing burned: 3 deletions will be recorded with the next disc.\n"));
    }
```

Before relying on the "Copies: every current file has 2 copies." line, check it against `coverage_lines` in `render.rs`. The s13 scenario asserts that exact text for a complete coverage.

- [ ] **Step 2: Run the tests to check that they fail**

Run: `cargo test -p diskrement-cli --lib render`
Expected: the new tests FAIL. A zero-set plan currently renders with a "Deletions: 1" line, and the burn report says "No disc written: every planned disc was already committed."

- [ ] **Step 3: Implement `plan_report`**

In `plan_report`, insert at the top (right after `let copies = p.copies;`):

```rust
    if p.sets.is_empty() {
        // No disc this cycle (issue #9): deletions wait for the next one.
        let mut lines = Vec::new();
        match p.tombstones.len() {
            0 => {}
            1 => lines.push("Nothing to burn: 1 deletion waits for the next disc.".to_string()),
            n => lines.push(format!(
                "Nothing to burn: {n} deletions wait for the next disc."
            )),
        }
        lines.extend(oversized_lines(p));
        if lines.is_empty() {
            return "Nothing to burn: no changes to plan.".to_string();
        }
        return lines.join("\n");
    }
```

Replace the oversized block at the end of the function:

```rust
    if !p.oversized.is_empty() {
        lines.push("Oversized (cannot be placed):".to_string());
        for f in &p.oversized {
            lines.push(format!("  {}  {}", f.rel_path.as_str(), gb(f.size_bytes)));
        }
    }
    lines.join("\n")
```

with:

```rust
    lines.extend(oversized_lines(p));
    lines.join("\n")
```

and add, below `plan_report`:

```rust
/// The oversize block: a header and one line per file, or nothing.
fn oversized_lines(p: &PlanReport) -> Vec<String> {
    if p.oversized.is_empty() {
        return Vec::new();
    }
    let mut lines = vec!["Oversized (cannot be placed):".to_string()];
    for f in &p.oversized {
        lines.push(format!("  {}  {}", f.rel_path.as_str(), gb(f.size_bytes)));
    }
    lines
}
```

- [ ] **Step 4: Implement `burn_report`**

Replace

```rust
    if r.discs.is_empty() {
        lines.push("No disc written: every planned disc was already committed.".to_string());
    }
```

with

```rust
    match r.deferred_deletions {
        Some(0) => lines.push("Nothing burned: no disc in this plan.".to_string()),
        Some(1) => lines
            .push("Nothing burned: 1 deletion will be recorded with the next disc.".to_string()),
        Some(n) => lines.push(format!(
            "Nothing burned: {n} deletions will be recorded with the next disc."
        )),
        None if r.discs.is_empty() => lines
            .push("No disc written: every planned disc was already committed.".to_string()),
        None => {}
    }
```

Update the `burn_report` doc comment's first line to: `/// `Render.burn`: one block per disc written (or why none was), the copy`.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p diskrement-cli --lib`
Expected: PASS. That includes `plan.snap`: its "Nothing to burn: no changes to plan." is unchanged.

- [ ] **Step 6: Commit**

```bash
git add crates/diskrement-cli/src/render.rs
git commit -m "feat(cli): say when a cycle burns nothing and its deletions wait (#9)"
```

---

### Task 6: Scenarios

**Files:**
- Modify: `crates/diskrement-cli/tests/scenarios/s08_idempotent.rs`
- Create: `crates/diskrement-cli/tests/scenarios/s14_deferred_deletions.rs`
- Modify: `crates/diskrement-cli/tests/scenarios/main.rs` (add `mod s14_deferred_deletions;` after `mod s13_copies;`)

- [ ] **Step 1: Extend s08**

Append to the end of `rescanning_an_unchanged_source_reports_all_unchanged_and_plans_nothing` in `s08_idempotent.rs`:

```rust

    // Burning that plan writes nothing: disc 1 gets no second session.
    let b2 = cli.run(&["burn", "p", "--yes"]);
    assert_eq!(b2.code, 0, "{}{}", b2.stdout, b2.stderr);
    assert!(b2.out_has("Nothing burned: no disc in this plan."), "{}", b2.stdout);
    assert!(!b2.out_has("Wrote disc"), "{}", b2.stdout);
    assert_eq!(disc::manifest(&s.disc_dir(1)).session_ordinal, 1);
    assert!(!s.disc_dir(2).exists());
```

and change its `use` line to `use crate::{disc, support};`. Update the module doc to: `//! Scenario 8: after a full burn, re-scanning an untouched source reports everything unchanged, there is nothing left to plan, and burning that plan writes nothing.`

- [ ] **Step 2: Create s14**

`crates/diskrement-cli/tests/scenarios/s14_deferred_deletions.rs`:

```rust
//! Scenario 14 (issue #9): at copies=2 a deletion-only cycle writes no disc
//! in either set, so 1A/1B stay twins; the deletion is recorded by the next
//! cycle that burns anyway, and both twins' manifests list it.

use crate::{disc, support};

#[test]
fn a_deletion_only_cycle_writes_no_disc_and_the_next_burn_records_it() {
    let Some(s) = support::scratch_or_skip("s14 deferred deletions") else {
        return;
    };
    let cli = s.cli();
    support::write(&s.source, "a.txt", 128);
    support::write(&s.source, "gone.txt", 128);
    let init = cli.run(&[
        "init",
        "p",
        "--source",
        &s.source.display().to_string(),
        "--work-dir",
        &s.work.display().to_string(),
        "--copies",
        "2",
    ]);
    assert_eq!(init.code, 0, "{}{}", init.stdout, init.stderr);
    for step in [&["scan", "p"][..], &["plan", "p"], &["burn", "p", "--yes"]] {
        let r = cli.run(step);
        assert_eq!(r.code, 0, "{step:?}: {}{}", r.stdout, r.stderr);
    }

    // Deletion-only cycle: nothing burned, nothing recorded.
    support::delete(&s.source, "gone.txt");
    let sc = cli.run(&["scan", "p"]);
    assert!(sc.out_has("Scanned: 0 added, 0 modified, 1 deleted, 1 unchanged."), "{}", sc.stdout);
    let pl = cli.run(&["plan", "p"]);
    assert_eq!(pl.code, 0, "{}{}", pl.stdout, pl.stderr);
    assert!(pl.out_has("Nothing to burn: 1 deletion waits for the next disc."), "{}", pl.stdout);
    let b = cli.run(&["burn", "p", "--yes"]);
    assert_eq!(b.code, 0, "{}{}", b.stdout, b.stderr);
    assert!(
        b.out_has("Nothing burned: 1 deletion will be recorded with the next disc."),
        "{}",
        b.stdout
    );
    for name in ["1A", "1B"] {
        assert_eq!(disc::manifest(&s.disc_dir_named(name)).session_ordinal, 1, "{name}");
    }
    assert!(!s.disc_dir_named("2A").exists());
    assert!(!s.disc_dir_named("2B").exists());
    let h = cli.run(&["history", "p", "gone.txt"]);
    assert!(!h.out_has("DELETED"), "not recorded yet: {}", h.stdout);

    // The next real cycle carries it on both twins and records it once.
    support::write(&s.source, "b.txt", 128);
    let sc2 = cli.run(&["scan", "p"]);
    assert!(sc2.out_has("Scanned: 1 added, 0 modified, 1 deleted, 1 unchanged."), "{}", sc2.stdout);
    let pl2 = cli.run(&["plan", "p"]);
    assert_eq!(pl2.code, 0, "{}{}", pl2.stdout, pl2.stderr);
    let b2 = cli.run(&["burn", "p", "--yes"]);
    assert_eq!(b2.code, 0, "{}{}", b2.stdout, b2.stderr);
    assert!(b2.out_has("Wrote disc 1A"), "{}", b2.stdout);
    assert!(b2.out_has("Wrote disc 1B"), "{}", b2.stdout);
    for name in ["1A", "1B"] {
        let m = disc::manifest(&s.disc_dir_named(name));
        assert_eq!(m.session_ordinal, 2, "{name}");
        assert_eq!(disc::manifest_deleted(&m), support::set(&["gone.txt"]), "{name}");
    }
    let h2 = cli.run(&["history", "p", "gone.txt"]);
    assert!(h2.out_has("DELETED"), "{}", h2.stdout);
}
```

Register it in `main.rs`: add `mod s14_deferred_deletions;` after `mod s13_copies;`.

- [ ] **Step 3: Run the scenarios**

Run: `DISKREMENT_SCENARIOS_REQUIRED=1 cargo test -p diskrement-cli --test scenarios`
Expected: PASS, with neither scenario reported as `SKIPPED`. If `history` doesn't print `DELETED` for a deleted path, check the s04 scenario (`hb.out_has("DELETED")`) for the exact text, and fix the assertion rather than the code.

- [ ] **Step 4: Commit**

```bash
git add crates/diskrement-cli/tests/scenarios/
git commit -m "test(cli): deletion-only and no-op cycles burn nothing (#9)"
```

---

### Task 7: Docs

**Files:**
- Modify: `CLAUDE.md` (Invariants section)
- Modify: `docs/superpowers/specs/2026-10-11-multiple-copies-design.md` (§6.1)
- Modify: `docs/superpowers/plans/2026-10-11-multiple-copies.md` ("Known limitations")

- [ ] **Step 1: CLAUDE.md**

Under "## Invariants — do not "fix" these", after the "**One disc per copy set per cycle.**" bullet, add:

```markdown
- **Deletions ride the next burned disc.** A cycle with nothing to place plans no disc at all (any `copies`), and `burn` writes nothing; its tombstones are re-derived by `diff::tombstones` (against `fingerprints`, not the live view) on every scan until a disc that is burned anyway records them. Never fall back to a disc just to carry tombstones (that opened untwinned discs, #9), and never add a sessionless deletion record.
```

- [ ] **Step 2: Multiple-copies spec §6.1**

In `docs/superpowers/specs/2026-10-11-multiple-copies-design.md` §6.1, replace the sentence `If no set has anything to place, the plan holds a single set-A entry, exactly today's no-op plan, so a deletion-only cycle still has a session to carry its tombstones.` with:

```markdown
If no set has anything to place, the plan holds no set and writes nothing; its
deletions wait for the next burned disc. (Superseded by
`2026-10-11-deferred-deletions-design.md`, issue #9: the original set-A fallback
opened untwinned discs.)
```

- [ ] **Step 3: Multiple-copies plan, known limitations**

In `docs/superpowers/plans/2026-10-11-multiple-copies.md`, change the line `  - A no-op or deletion-only cycle writes a set-A session only.` to:

```markdown
  - ~~A no-op or deletion-only cycle writes a set-A session only.~~ Resolved by #9: such a cycle writes no disc (`docs/superpowers/specs/2026-10-11-deferred-deletions-design.md`).
```

- [ ] **Step 4: Check user-facing docs**

Run: `grep -rn -i "nothing to burn\|deletion-only\|tombstone" README* docs/*.md 2>/dev/null`
If any user-facing doc says a deletion-only or no-op `burn` writes a session, correct it to the new behaviour. If nothing matches, there's nothing to change.

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md docs/
git commit -m "docs(plan): deletions ride the next burned disc (#9)"
```

---

### Task 8: Verify, merge, close

- [ ] **Step 1: Full verification**

```bash
DISKREMENT_SCENARIOS_REQUIRED=1 cargo test --workspace 2>&1 | grep -E "^test result|SKIPPED|FAILED"
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

Expected: every `test result: ok`, no `SKIPPED`, clippy clean, fmt clean. Compare the total passed count with `main`: it should rise by about 14. It must not drop, apart from the tests this plan replaces one for one. If `xorriso` is missing, `xorriso_image.rs` skips; say so in the report.

- [ ] **Step 2: Append execution deviations**

Add an `## Execution deviations` section at the end of this plan. Record any deviation, or write "None."; then commit with `docs(plan): deferred deletions execution notes`.

- [ ] **Step 3: Merge**

```bash
git checkout main
git merge --no-ff feat/deferred-deletions -m "Merge feat/deferred-deletions: cycles with nothing to place burn no disc (#9)"
git push origin main
git branch -d feat/deferred-deletions
git push origin --delete feat/deferred-deletions   # only if it was pushed
```

- [ ] **Step 4: Close #9**

```bash
gh issue close 9 --comment "Fixed in <merge-sha> (https://github.com/zayatura/diskrement/commit/<merge-sha>). A no-op or deletion-only cycle now plans no disc at any copies; deletions are recorded by the next disc burned anyway. Design: docs/superpowers/specs/2026-10-11-deferred-deletions-design.md."
```

Replace `<merge-sha>` with `git rev-parse --short HEAD` on `main`.

## Execution deviations

- `pipeline::verify_tests::a_sealed_disc_with_no_files_verifies_clean_with_zero_checked` relied on the removed set-A fallback: `burned_env(&[], true)` planned a sealed empty disc 1 from an empty scan. It was rebuilt to produce a file-less seal session the operator way (burn `a.txt`, restore the snapshot and staged scan, `plan --finalize`, burn, verify), keeping its assertions.
