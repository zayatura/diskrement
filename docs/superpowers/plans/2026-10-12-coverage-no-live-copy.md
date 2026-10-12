# Coverage Counts Files With No Live Copy Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `status` and `burn` never report a copy set as complete, or the archive as up to date, while a current version has no live copy left (issue #6).

**Architecture:** `replication::coverage` takes every current version's size (from `Catalog::fingerprints`, which is unfiltered by dead discs) beside the live `CatalogView`. A path in the first but not the second is missing from every set and also counted in a new `Coverage::no_live_copy`. The CLI prints one `No live copy: …` line, at every `copies`, only when that count is non-zero. `read_view`, `shortfall`, `classify` and `diff` are untouched.

**Tech Stack:** Rust workspace (`diskrement-core` → `diskrement-shell` → `diskrement-cli`), proptest, assert_cmd.

**Spec:** `docs/superpowers/specs/2026-10-12-coverage-no-live-copy-design.md`

**Branch:** `feat/coverage-no-live-copy` (already created off `main` at d54467f; the spec is committed there as f4539ac).

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
| `crates/diskrement-core/src/replication.rs` | `Coverage` struct; `coverage` takes `current` sizes; tests + proptest. |
| `crates/diskrement-core/src/lib.rs` | Re-export `Coverage`. |
| `crates/diskrement-shell/src/pipeline/reports.rs` | `StatusReport.coverage`, `BurnReport.coverage` become `Coverage`. |
| `crates/diskrement-shell/src/pipeline/mod.rs` | `status` and `burn` pass `fingerprints` sizes; `total_current_files` from `fingerprints`; pipeline tests. |
| `crates/diskrement-cli/src/render.rs` | `no_live_copy_line`; status and burn print it; burn drops "up to date" while it shows; tests. |
| `crates/diskrement-cli/tests/binary.rs` | The post-mark-dead `status` contract changes. |
| `crates/diskrement-cli/tests/scenarios/s09_mark_dead.rs`, `s13_copies.rs` | End-to-end assertions. |
| `docs/superpowers/plans/2026-10-11-multiple-copies.md` | Mark the known limitation resolved. |

---

### Task 1: `Coverage` in core, plumbed through unchanged output

This task changes the type and the inputs, but no rendered output: the CLI reads `coverage.sets` where it read `coverage`. Everything compiles and every existing test passes at the end.

**Files:**
- Modify: `crates/diskrement-core/src/replication.rs`
- Modify: `crates/diskrement-core/src/lib.rs:31`
- Modify: `crates/diskrement-shell/src/pipeline/reports.rs:9,31,99`
- Modify: `crates/diskrement-shell/src/pipeline/mod.rs` (`burn` tail ~line 612, `status` ~line 588, test module imports ~line 1349, test ~line 2011)
- Modify: `crates/diskrement-cli/src/render.rs` (`burn_report`, `status_report`, test constructors)

- [ ] **Step 1: Write the failing core tests**

In `crates/diskrement-core/src/replication.rs`, replace the existing test `coverage_counts_files_and_bytes_missing_per_set` with these, and add `use std::collections::BTreeMap;` plus a `sizes` helper inside `mod tests` (below the existing `paths` helper):

```rust
    fn sizes(entries: &[(&str, u64)]) -> BTreeMap<RelPath, u64> {
        entries
            .iter()
            .map(|(p, s)| (RelPath((*p).to_string()), *s))
            .collect()
    }

    fn missing(files: usize, bytes: u64) -> SetCoverage {
        SetCoverage {
            missing_files: files,
            missing_bytes: bytes,
        }
    }

    #[test]
    fn coverage_counts_files_and_bytes_missing_per_set() {
        let view = view_of(&[entry_in("a", "h", 10, &[0]), entry_in("b", "h", 5, &[1])]);
        let c = coverage(&view, &sizes(&[("a", 10), ("b", 5)]), 2);
        assert_eq!(c.sets, vec![missing(1, 5), missing(1, 10)]);
        assert_eq!(c.no_live_copy, SetCoverage::default());
    }

    #[test]
    fn a_current_version_with_no_live_copy_is_missing_from_every_set() {
        // "gone" is current in the history, but every copy of it died, so
        // the view leaves it out.
        let view = view_of(&[entry_in("a", "h", 10, &[0, 1])]);
        let c = coverage(&view, &sizes(&[("a", 10), ("gone", 7)]), 2);
        assert_eq!(c.no_live_copy, missing(1, 7));
        assert_eq!(c.sets, vec![missing(1, 7), missing(1, 7)]);
    }

    #[test]
    fn a_single_copy_project_counts_a_file_with_no_live_copy() {
        let c = coverage(&view_of(&[]), &sizes(&[("gone", 7)]), 1);
        assert_eq!(c.sets, vec![missing(1, 7)]);
        assert_eq!(c.no_live_copy, missing(1, 7));
    }

    #[test]
    fn a_fully_covered_catalog_has_nothing_missing() {
        let view = view_of(&[entry_in("a", "h", 10, &[0, 1])]);
        assert_eq!(
            coverage(&view, &sizes(&[("a", 10)]), 2),
            Coverage {
                sets: vec![SetCoverage::default(); 2],
                no_live_copy: SetCoverage::default(),
            }
        );
    }
```

Add this proptest inside the existing `proptest! { … }` block, after `each_set_receives_exactly_what_it_does_not_already_hold`:

```rust
        /// Coverage against an independent oracle: a set misses exactly the
        /// current files with no live copy in it, and `no_live_copy` is
        /// exactly the current files with none in any set — which every set
        /// therefore also misses.
        #[test]
        fn coverage_counts_every_current_file_missing_from_a_set(
            files in scanned_files_strategy(),
            held in proptest::collection::vec(0u8..8, 0..12),
            copies in 1u32..=3,
        ) {
            // File i is current, live in the sets whose bit is set in
            // held[i]; no bit (within `copies`) means no live copy, so the
            // view leaves it out. Files beyond `held` are not current at all.
            let live = |bits: u8, set: u32| bits & (1 << set) != 0;
            let mut entries = Vec::new();
            let mut current = BTreeMap::new();
            for (f, &bits) in files.iter().zip(held.iter()) {
                current.insert(f.rel_path.clone(), f.size_bytes);
                let sets: Vec<u32> = (0..copies).filter(|&s| live(bits, s)).collect();
                if !sets.is_empty() {
                    entries.push(entry_in(f.rel_path.as_str(), f.content_hash.as_str(), f.size_bytes, &sets));
                }
            }
            let c = coverage(&view_of(&entries), &current, copies);
            let pairs: Vec<(&ScannedFile, u8)> =
                files.iter().zip(held.iter().copied()).collect();

            prop_assert_eq!(c.sets.len(), copies as usize);
            for set in 0..copies {
                let lacking: Vec<&&ScannedFile> = pairs.iter()
                    .filter(|(_, bits)| !live(*bits, set))
                    .map(|(f, _)| f)
                    .collect();
                prop_assert_eq!(c.sets[set as usize].missing_files, lacking.len());
                prop_assert_eq!(
                    c.sets[set as usize].missing_bytes,
                    lacking.iter().map(|f| f.size_bytes).sum::<u64>()
                );
                prop_assert!(c.sets[set as usize].missing_files >= c.no_live_copy.missing_files);
            }
            let orphans = pairs.iter()
                .filter(|(_, bits)| (0..copies).all(|s| !live(*bits, s)))
                .count();
            prop_assert_eq!(c.no_live_copy.missing_files, orphans);
        }
```

- [ ] **Step 2: Run the core tests to verify they fail**

Run: `cargo test -p diskrement-core replication`
Expected: compile errors: `Coverage` not found, and `coverage` takes 2 arguments but 3 were supplied.

- [ ] **Step 3: Implement `Coverage` and the new `coverage`**

In `crates/diskrement-core/src/replication.rs`, change the imports at the top to:

```rust
use std::collections::BTreeMap;

use crate::diff::ChangeSet;
use crate::model::{CatalogView, RelPath, ScannedFile};
```

Replace the existing `/// Coverage of every copy set, indexed by set.` function with:

```rust
/// Copy coverage of the whole project.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Coverage {
    /// Indexed by copy set: current versions with no live copy in that set,
    /// including those with no live copy anywhere.
    pub sets: Vec<SetCoverage>,
    /// Current versions with no live copy in any set. The next scan re-adds
    /// them (the REBURN path), but until then no set holds them.
    pub no_live_copy: SetCoverage,
}

/// Coverage of every copy set over every current version.
///
/// `current` maps each current version's path to its size: every path whose
/// latest change is not `DELETED`, whether or not a live copy remains
/// (`Catalog::fingerprints`). `view` leaves out a version with no live copy,
/// so a path in `current` but not in `view` is missing from every set and is
/// counted in `no_live_copy`.
pub fn coverage(view: &CatalogView, current: &BTreeMap<RelPath, u64>, copies: u32) -> Coverage {
    let mut no_live_copy = SetCoverage::default();
    for (path, &size) in current {
        if !view.current.contains_key(path) {
            no_live_copy.missing_files += 1;
            no_live_copy.missing_bytes += size;
        }
    }
    let sets = (0..copies)
        .map(|set| {
            let mut c = no_live_copy;
            for e in view.current.values() {
                if !e.live_sets.contains(&set) {
                    c.missing_files += 1;
                    c.missing_bytes += e.size_bytes;
                }
            }
            c
        })
        .collect();
    Coverage { sets, no_live_copy }
}
```

In `crates/diskrement-core/src/lib.rs`, line 31, change the re-export to:

```rust
pub use replication::{Coverage, DiscContents, SetCoverage};
```

- [ ] **Step 4: Run the core tests to verify they pass**

Run: `cargo test -p diskrement-core replication`
Expected: all `replication` tests PASS (both proptests included).

- [ ] **Step 5: Plumb `Coverage` through the shell**

In `crates/diskrement-shell/src/pipeline/reports.rs`, replace `SetCoverage` with `Coverage` in the `diskrement_core` import on line 9 (the two fields below are its only uses). Change both fields:

```rust
    pub coverage: Coverage,
```

(in `StatusReport`, line 31, and in `BurnReport`, line 99).

In `crates/diskrement-shell/src/pipeline/mod.rs`, the `status` function (~line 588): after `let view = …read_view…?;` add

```rust
        let current = self.catalog.fingerprints(p.id).map_err(Self::failed)?;
```

and change the `StatusReport` literal's coverage field to:

```rust
            coverage: replication::coverage(&view, &sizes(&current), p.copies),
```

Leave `total_current_files: view.current.len()` alone in this task (Task 2 changes it, test first).

The `burn` tail (~line 607): after the `let view = …read_view…;` block add

```rust
        let current = self
            .catalog
            .fingerprints(p.id)
            .map_err(|e| partial(&done, None, &[], Self::failed(e)))?;
```

and change the `BurnReport` literal's coverage field to:

```rust
            coverage: replication::coverage(&view, &sizes(&current), p.copies),
```

Add this private free function to `pipeline/mod.rs`, next to the other module-level helpers (e.g. just above `fn discard_staging`; `grep -n "^fn " crates/diskrement-shell/src/pipeline/mod.rs` lists them):

```rust
/// Each current version's size, for `replication::coverage`.
fn sizes(current: &BTreeMap<RelPath, FileFingerprint>) -> BTreeMap<RelPath, u64> {
    current
        .iter()
        .map(|(path, f)| (path.clone(), f.size_bytes))
        .collect()
}
```

`BTreeMap`, `RelPath` and `FileFingerprint` are already imported at the top of `pipeline/mod.rs`.

In the `pipeline/mod.rs` test module whose import is `use diskrement_core::{DiscKey, DiscType, RelPath, ScannedFile, SetCoverage};` (~line 1349), add `Coverage`, and change the assertion at ~line 2011 to:

```rust
        assert_eq!(
            report.coverage,
            Coverage {
                sets: vec![SetCoverage::default(); 2],
                no_live_copy: SetCoverage::default(),
            }
        );
```

In the `copies_tests` module (~line 3351), update the two existing coverage assertions:

```rust
        assert!(s.coverage.sets.iter().all(|c| c.missing_files == 0));
```

```rust
        assert_eq!(s.coverage.sets[1].missing_files, 1);
        assert_eq!(s.coverage.no_live_copy.missing_files, 0, "a.bin is still on 1A");
```

- [ ] **Step 6: Plumb `Coverage` through the CLI, output unchanged**

In `crates/diskrement-cli/src/render.rs`:

- `burn_report`: `lines.extend(coverage_lines(&r.coverage));` → `lines.extend(coverage_lines(&r.coverage.sets));`
- `status_report`: `for (set, c) in r.coverage.iter().enumerate()` → `for (set, c) in r.coverage.sets.iter().enumerate()`

Update the test constructors. The single-copy ones never render coverage, so `Coverage::default()` (no sets) renders identically:

```bash
sed -i 's/coverage: vec!\[SetCoverage::default()\],/coverage: diskrement_core::Coverage::default(),/' crates/diskrement-cli/src/render.rs
sed -i 's/coverage: vec!\[\],/coverage: diskrement_core::Coverage::default(),/' crates/diskrement-cli/src/render.rs
```

Then hand-edit the three two-copy constructors:

- `a_two_copy_status_names_discs_by_set_and_shows_coverage` (~line 508):

```rust
            coverage: diskrement_core::Coverage {
                sets: vec![
                    SetCoverage::default(),
                    SetCoverage {
                        missing_files: 2,
                        missing_bytes: 300_000_000,
                    },
                ],
                no_live_copy: SetCoverage::default(),
            },
```

- `deferred_burn` (~line 770):

```rust
            coverage: diskrement_core::Coverage {
                sets: vec![SetCoverage::default(); copies],
                no_live_copy: SetCoverage::default(),
            },
```

- `a_two_copy_burn_lists_both_discs_and_the_coverage` (~line 961):

```rust
            coverage: diskrement_core::Coverage {
                sets: vec![
                    SetCoverage::default(),
                    SetCoverage {
                        missing_files: 3,
                        missing_bytes: 2_000_000_000,
                    },
                ],
                no_live_copy: SetCoverage::default(),
            },
```

Check nothing was missed: `grep -n "coverage: vec" crates/diskrement-cli/src/render.rs` must print nothing.

- [ ] **Step 7: Run the whole workspace**

Run: `cargo fmt && cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: everything PASSES, and no existing test changed its expected output.

- [ ] **Step 8: Commit**

```bash
git add crates/diskrement-core/src/replication.rs crates/diskrement-core/src/lib.rs \
  crates/diskrement-shell/src/pipeline/reports.rs crates/diskrement-shell/src/pipeline/mod.rs \
  crates/diskrement-cli/src/render.rs
git commit -m "feat(core): coverage counts current versions with no live copy (#6)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Pipeline — status and burn see files with no live copy

**Files:**
- Modify: `crates/diskrement-shell/src/pipeline/mod.rs` (`status`; `copies_tests`; `mark_dead_tests`)

- [ ] **Step 1: Write the failing status test**

Add to `mod copies_tests` in `crates/diskrement-shell/src/pipeline/mod.rs`, after `marking_one_twin_dead_degrades_and_status_shows_the_gap`:

```rust
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
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p diskrement-shell marking_both_twins_dead`
Expected: FAIL on `total_current_files`: `left: 0, right: 1` (the coverage assertions already pass after Task 1).

- [ ] **Step 3: Count every current version**

In `status`, change

```rust
            total_current_files: view.current.len(),
```

to

```rust
            total_current_files: current.len(),
```

and in `crates/diskrement-shell/src/pipeline/reports.rs` give the `StatusReport` field (line 29, currently undocumented) a doc comment:

```rust
    /// Every current version, including those with no live copy left.
    pub total_current_files: usize,
```

- [ ] **Step 4: Run it to verify it passes**

Run: `cargo test -p diskrement-shell marking_both_twins_dead`
Expected: PASS.

- [ ] **Step 5: Write the burn test**

Add to `mod mark_dead_tests` (copies = 1), after the existing tests:

```rust
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
```

- [ ] **Step 6: Run it**

Run: `cargo test -p diskrement-shell a_burn_after_the_last_copy_died`
Expected: PASS (Task 1 already passes `fingerprints` to burn's coverage; this test pins it). If the burn fails instead, e.g. with a `StalePlan`, stop and report it: the spec assumes a dead disc that is not the planned disc does not invalidate the plan, so the test, not the code, is what needs rethinking.

To prove the test bites, temporarily change burn's `sizes(&current)` to `&BTreeMap::new()`, rerun, and confirm it FAILS on `no_live_copy`; then revert.

- [ ] **Step 7: Run the shell suite and commit**

Run: `cargo fmt && cargo test -p diskrement-shell && cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS.

```bash
git add crates/diskrement-shell/src/pipeline/mod.rs crates/diskrement-shell/src/pipeline/reports.rs
git commit -m "fix(pipeline): status counts current files with no live copy (#6)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Render the "No live copy" line

**Files:**
- Modify: `crates/diskrement-cli/src/render.rs`
- Modify: `crates/diskrement-cli/tests/binary.rs:~815-823`

- [ ] **Step 1: Write the failing render tests**

In `crates/diskrement-cli/src/render.rs`, in the status test module (the one containing `a_two_copy_status_names_discs_by_set_and_shows_coverage`), add after that test:

```rust
    #[test]
    fn a_single_copy_status_names_the_files_with_no_live_copy() {
        let gone = SetCoverage {
            missing_files: 2,
            missing_bytes: 800_000_000,
        };
        let r = StatusReport {
            discs: vec![],
            open_discs: vec![],
            total_current_files: 2,
            last_burn_at: None,
            coverage: diskrement_core::Coverage {
                sets: vec![gone],
                no_live_copy: gone,
            },
        };
        assert_eq!(
            status_report(&project(), &r),
            "Project photos  (source /srv/photos, BD_R, margin 2.0%)\n\
             Discs:\n\
             No live copy: 2 files (0.8 GB) — re-burn on next scan/plan/burn.\n\
             Current files: 2      Last burn: never"
        );
    }

    #[test]
    fn a_two_copy_status_with_no_live_copy_shows_no_set_complete() {
        let mut p = project();
        p.copies = 2;
        let r = StatusReport {
            discs: vec![],
            open_discs: vec![],
            total_current_files: 3,
            last_burn_at: None,
            coverage: diskrement_core::Coverage {
                sets: vec![
                    SetCoverage {
                        missing_files: 2,
                        missing_bytes: 800_000_000,
                    },
                    SetCoverage {
                        missing_files: 3,
                        missing_bytes: 900_000_000,
                    },
                ],
                no_live_copy: SetCoverage {
                    missing_files: 2,
                    missing_bytes: 800_000_000,
                },
            },
        };
        assert_eq!(
            status_report(&p, &r),
            "Project photos  (source /srv/photos, BD_R, margin 2.0%, 2 copies)\n\
             Discs:\n\
             Copies:\n  \
             set A: 2 files (0.8 GB) missing\n  \
             set B: 3 files (0.9 GB) missing\n\
             No live copy: 2 files (0.8 GB) — re-burn on next scan/plan/burn.\n\
             Current files: 3      Last burn: never"
        );
    }
```

In the module containing `a_two_copy_burn_lists_both_discs_and_the_coverage`, add after it:

```rust
    #[test]
    fn a_two_copy_burn_lists_lagging_sets_then_the_files_with_no_live_copy() {
        let gone = SetCoverage {
            missing_files: 1,
            missing_bytes: 4_000_000_000,
        };
        let r = BurnReport {
            discs: vec![DiscBurn {
                disc: DiscKey::new(2, 0),
                label: "p disc 2A".to_string(),
                finalized: false,
                staging_dir: "/w/staging".into(),
                image_bytes: 1,
            }],
            remaining: RemainingEstimate {
                more_discs: 0,
                more_files: 0,
                more_bytes: 0,
            },
            coverage: diskrement_core::Coverage {
                sets: vec![gone, gone],
                no_live_copy: gone,
            },
            deferred_deletions: None,
        };
        assert_eq!(
            burn_report(&r, 2),
            "✔ Wrote disc 2A  —  label: \"p disc 2A\"\n\
             Status: left OPEN — may receive more sessions later.\n\
             Copies: set A is missing 1 files (4.0 GB).\n\
             Copies: set B is missing 1 files (4.0 GB).\n\
             No live copy: 1 files (4.0 GB) — re-burn on next scan/plan/burn."
        );
    }
```

In `mod burn_render_tests` (it has the `report(finalized, more_discs)` helper), add:

```rust
    #[test]
    fn a_single_copy_burn_with_no_live_copy_does_not_claim_up_to_date() {
        let gone = SetCoverage {
            missing_files: 1,
            missing_bytes: 4_000_000_000,
        };
        let mut r = report(false, 0);
        r.coverage = diskrement_core::Coverage {
            sets: vec![gone],
            no_live_copy: gone,
        };
        assert_eq!(
            burn_report(&r, 1),
            "✔ Wrote disc 1  —  label: \"p disc 1\"\n\
             Status: left OPEN — may receive more sessions later.\n\
             No live copy: 1 files (4.0 GB) — re-burn on next scan/plan/burn."
        );

        // More discs to come: the line stays, and so does the remaining one.
        let mut r = report(false, 1);
        r.coverage = diskrement_core::Coverage {
            sets: vec![gone],
            no_live_copy: gone,
        };
        assert!(
            burn_report(&r, 1).ends_with(
                "No live copy: 1 files (4.0 GB) — re-burn on next scan/plan/burn.\n\
                 ~1 more discs of changes remain. Run: scan → plan → burn"
            ),
            "{}",
            burn_report(&r, 1)
        );
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p diskrement-cli no_live_copy`
Expected: the four new tests FAIL: no "No live copy" line, and the burn ones end with "No more changes — …".

- [ ] **Step 3: Implement the line**

In `crates/diskrement-cli/src/render.rs`, add below `coverage_lines`:

```rust
/// The files with no live copy in any set, or nothing when there are none.
/// Worded after `mark_dead`'s "Re-burn on next scan/plan/burn".
fn no_live_copy_line(c: &SetCoverage) -> Option<String> {
    (c.missing_files > 0).then(|| {
        format!(
            "No live copy: {} files ({}) — re-burn on next scan/plan/burn.",
            c.missing_files,
            gb(c.missing_bytes)
        )
    })
}
```

In `burn_report`, replace everything from `if copies > 1 {` (the coverage block) through the end of the `lines.push(if r.remaining.more_discs == 0 { … });` statement with:

```rust
    if copies > 1 {
        lines.extend(coverage_lines(&r.coverage.sets));
    }
    // While a file has no live copy, "up to date" would be false.
    let no_live_copy = no_live_copy_line(&r.coverage.no_live_copy);
    let covered = no_live_copy.is_none();
    lines.extend(no_live_copy);
    if r.remaining.more_discs > 0 {
        lines.push(format!(
            "~{} more discs of changes remain. Run: scan → plan → burn",
            r.remaining.more_discs
        ));
    } else if covered {
        lines.push(if copies > 1 {
            "No more changes — every copy set is up to date.".to_string()
        } else {
            "No more changes — the set is up to date.".to_string()
        });
    }
```

In `status_report`, immediately before the `let last = r` line, add:

```rust
    if let Some(line) = no_live_copy_line(&r.coverage.no_live_copy) {
        out.push_str(&line);
        out.push('\n');
    }
```

Update the doc comment on `burn_report` to read: `/// `Render.burn`: one block per disc written (or why none was), the copy\n/// coverage when the project keeps more than one copy, any files with no live\n/// copy at all, then what to do next.`

- [ ] **Step 4: Run the CLI unit tests**

Run: `cargo test -p diskrement-cli --lib`
Expected: PASS, including every existing render and gallery test unchanged.

- [ ] **Step 5: Update the binary contract test**

In `crates/diskrement-cli/tests/binary.rs`, replace the test `status_reports_the_disc_dead_and_drops_its_files_from_the_view` (~line 815) with:

```rust
    /// The disc becomes DEAD in `status` — the one place the CLI shows the
    /// derived state — while the row behind it is untouched (asserted in the
    /// shell's pipeline `mark_dead_tests`, which can see SQL). Its files are
    /// still current versions, now with no live copy (issue #6).
    #[test]
    fn status_reports_the_disc_dead_and_its_files_as_having_no_live_copy() {
        let f = two_session_disc();
        run(&f, &["disc", "mark-dead", "p", "1", "--yes"]).success();
        run(&f, &["status", "p"]).success().stdout(
            contains("  #1   DEAD")
                .and(contains(
                    "No live copy: 2 files (0.0 GB) — re-burn on next scan/plan/burn.\n",
                ))
                .and(contains("Current files: 2")),
        );
    }
```

- [ ] **Step 6: Run the CLI suite**

Run: `cargo fmt && DISKREMENT_SCENARIOS_REQUIRED=1 cargo test -p diskrement-cli && cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS. If a scenario fails on an exact-output assertion after a mark-dead, report it before changing it; Task 4 owns the scenario edits.

- [ ] **Step 7: Commit**

```bash
git add crates/diskrement-cli/src/render.rs crates/diskrement-cli/tests/binary.rs
git commit -m "feat(cli): status and burn name files with no live copy (#6)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: End-to-end scenarios

**Files:**
- Modify: `crates/diskrement-cli/tests/scenarios/s09_mark_dead.rs`
- Modify: `crates/diskrement-cli/tests/scenarios/s13_copies.rs`

- [ ] **Step 1: s09 — the line appears after the kill and clears after recovery**

In `s09_mark_dead.rs`, replace

```rust
    let status = cli.run(&["status", "p"]);
    assert!(status.out_has("DEAD"));
```

with

```rust
    let status = cli.run(&["status", "p"]);
    assert!(status.out_has("DEAD"));
    // a.txt and b.txt are still current, but neither has a copy left.
    assert!(
        status.out_has("No live copy: 2 files"),
        "{}",
        status.stdout
    );
    assert!(status.out_has("Current files: 2"), "{}", status.stdout);
```

After the recovery burn's `assert_eq!(disc::data_files(&s.disc_dir(2)), …);`, add:

```rust
    let status = cli.run(&["status", "p"]);
    assert!(!status.out_has("No live copy"), "{}", status.stdout);
```

- [ ] **Step 2: s13 — a lagging set is not "no live copy"**

In `s13_copies.rs`, after the `mark-dead … 1B` block's last assertion (`Superseded versions still held on another disc: 1`), add:

```rust
    // Set B is short both files, but each still has its copy on 1A.
    let st = cli.run(&["status", "p"]);
    assert!(st.out_has("set B: 2 files"), "{}", st.stdout);
    assert!(st.out_has("set A: complete"), "{}", st.stdout);
    assert!(!st.out_has("No live copy"), "{}", st.stdout);
```

- [ ] **Step 3: Run the scenarios**

Run: `cargo fmt && DISKREMENT_SCENARIOS_REQUIRED=1 cargo test -p diskrement-cli --test scenarios`
Expected: PASS (not SKIPPED; check the output). If the test target is named differently, `ls crates/diskrement-cli/tests/` and use the directory's `main.rs` target name.

- [ ] **Step 4: Commit**

```bash
git add crates/diskrement-cli/tests/scenarios/s09_mark_dead.rs crates/diskrement-cli/tests/scenarios/s13_copies.rs
git commit -m "test(cli): status shows files with no live copy end to end (#6)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Docs and full verification

**Files:**
- Modify: `docs/superpowers/plans/2026-10-11-multiple-copies.md:4546`

- [ ] **Step 1: Mark the known limitation resolved**

In `docs/superpowers/plans/2026-10-11-multiple-copies.md`, replace line 4546

```
  - Copy coverage (`status`, `burn`) counts only versions that still have a live copy. A file whose every copy died reads as "complete" until the next scan re-adds it; `disc mark-dead` lists it under Re-burn.
```

with

```
  - ~~Copy coverage (`status`, `burn`) counts only versions that still have a live copy.~~ Resolved by #6: a current version with no live copy is missing from every set and named on a "No live copy" line (`docs/superpowers/specs/2026-10-12-coverage-no-live-copy-design.md`).
```

Then `grep -rn "complete\" until\|counts only versions" docs/ CLAUDE.md README.md` and fix any other place that still describes the old behaviour.

- [ ] **Step 2: Full verification**

Run:

```
DISKREMENT_SCENARIOS_REQUIRED=1 cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

Expected: all pass; test count is the `main` baseline plus the new tests (core +4 unit +1 proptest, shell +2, cli +4 render; binary.rs count unchanged), with no SKIPPED scenarios.

- [ ] **Step 3: Commit**

```bash
git add docs/superpowers/plans/2026-10-11-multiple-copies.md
git commit -m "docs(plan): copy coverage counts files with no live copy (#6)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 4: Finish the branch**

Use superpowers:finishing-a-development-branch. Repo etiquette: merge into `main` with `git merge --no-ff feat/coverage-no-live-copy -m "Merge feat/coverage-no-live-copy: coverage counts files with no live copy (#6)"`, push, delete the branch locally and on `origin`, and close issue #6.

---

## Deviations applied during execution

- **Task 2:** an existing test, `mark_dead_tests::a_dead_discs_live_files_re_plan_onto_a_fresh_disc`, asserted `total_current_files == 0` after the kill ("the view drops versions whose only copy is dead"), the very behaviour #6 changes. It now asserts `2` and `no_live_copy == {2 files, 7 bytes}`. The plan missed it because it grepped for `coverage`, not `total_current_files`.
- **Task 2:** the open assumption held: a dead disc other than the planned one does not stale the plan, and burn writes disc 2. The mutation check (burn given an empty size map) made the new burn test fail as intended.
- **Task 3:** `diskrement-cli` has no library target; its unit tests run with `cargo test -p diskrement-cli --bin diskrement`, not `--lib`.
- **Task 5:** `README.md`'s coaster-workflow section said `Current files` drops by the re-burn list after a mark-dead; it now describes the "No live copy" line.
