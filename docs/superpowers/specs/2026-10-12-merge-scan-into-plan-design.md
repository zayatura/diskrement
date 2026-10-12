# Merge scan into plan — design

**Date:** 2026-10-12
**Status:** Approved for planning
**Issue:** https://github.com/zayatura/diskrement/issues/4 (both parts)
**Branch:** `feat/merge-scan-into-plan`

---

## 1. Problem

Without flags, `scan` reuses a staged scan if one exists (`Pipeline::scan`,
`crates/diskrement-shell/src/pipeline/mod.rs`). It neither snapshots nor walks the
source. It returns the old counts, and the only sign is the missing `Hashed N of M files`
line. Only a successful `burn` or `reset` clears the staged scan. `plan` never clears
it, and neither does "Nothing to burn".

So a periodic backup silently stops seeing changes. Month 1's `scan → plan` says
"Nothing to burn". In month 2, 500 new photos arrive, `scan` returns month 1's result,
and `plan` says "Nothing to burn" again. That repeats until someone runs
`scan --force` or `reset`. A declined burn and a burn that fails before its commit
lead to the same state. The live `immich-dsz` project currently works around this
with `scan --force` on every cycle.

The reuse rule (core-engine spec §9.1) made sense when every re-scan re-hashed every
unburned file, and one `burn` wrote the whole set. Neither is true now:

- With one disc per cycle, every successful burn clears the staged scan. So reuse
  only happens on the no-burn paths, which are exactly where a stale result hurts.
- The fingerprint cache covers scanned-but-unburned files too. On the real 76k-file /
  417 GB library, a warm re-scan costs a walk, a stat and a reflink snapshot (about
  4 s), not a re-hash.

Once scanning is always fresh and cheap, a separate required `scan` step has no
reason to exist. Operators always run `scan && plan`.

## 2. Decision

**`plan` scans.** It takes a fresh snapshot, hashes against the fingerprint cache,
plans, and stages both the scan and the plan. **The `scan` command is removed.** The
cycle becomes `plan → burn`.

This covers both parts of #4. Part 1 ("scan is always fresh") falls out of Part 2:
the only remaining way to scan is a `plan`, and a `plan` always scans. So nothing
needs a `--force` kept as a deprecated no-op, and the reuse path disappears instead
of being turned off.

### 2.1 Alternatives rejected

- **Keep `scan` as a read-only preview** (walk, count, stage nothing). Its only use
  is answering "is there anything new?" without staging a plan. `plan` answers the
  same question, and staging a plan writes no catalog rows.
- **Keep `scan` as `plan`'s first half** (always fresh, still stages `scan.json`).
  This keeps a redundant step, and `plan` would re-scan anyway.
- **A hidden `scan` stub that redirects to `plan`.** The tool has one operator, and
  a dead command would only need deleting later.
- **Two pipeline methods sequenced by the CLI.** This is the smallest diff, but the
  ordering rules (clear, then scan, then plan) would sit in `main.rs`. A public
  `Pipeline::scan` would also still let a caller stage a scan without a plan, which
  is the shape that caused #4.
- **Plan from the scan held in memory, and write `scan.json` only for `burn`.**
  `burn`'s `stale_detail` and its snapshot checks rely on `scan.json` describing
  `<work>/snapshot`. That role stays as it is.

## 3. CLI surface

- The `scan` subcommand is removed. The commands are `init`, `plan`, `burn`,
  `status`, `reset`, `verify`, `history` and `disc`, eight in all. `diskrement scan …`
  is now an unknown subcommand: clap's usage error, exit 1.
- `plan NAME [--disc-type T] [--finalize] [--rehash]`.
  - `--rehash` is the old `scan --verify`: it re-hashes every file, ignoring the
    size+mtime fast path and the fingerprint cache. It is named `--rehash` rather
    than `--verify` so it is not confused with the `verify` command, which reads
    burned discs back.
  - The about text changes from "Plan the next disc from the staged scan." to one
    that says it snapshots and scans the source, e.g. "Scan the source and plan
    the next disc."
- `scan --force` goes away without replacement, because every `plan` is a fresh
  scan.
- Exit codes keep their meanings. `plan` can now fail the way `scan` could, with
  the same codes: file names that are not UTF-8 exit 3, and snapshot or I/O failures
  exit 5 (`Failed`).

This amends the rewrite spec's §3.2 ("same nine commands"). That freeze was the
parity requirement for the Scala→Rust cutover, which is done.

## 4. Pipeline

`Pipeline::scan` is removed. `Pipeline::plan` becomes:

```rust
pub fn plan(
    &mut self,
    name: &str,
    disc_type_override: Option<DiscType>,
    finalize: bool,
    rehash: bool,
) -> Result<PlanOutcome, PipelineError>

pub struct PlanOutcome {
    pub scan: ScanReport,
    pub plan: PlanReport,
}
```

Its steps, in order:

1. Look up the project (`ProjectNotFound`, exit 2).
2. **`staged::clear`**: remove `staged/scan.json`, `staged/plan.json`, their `.tmp`
   files and the empty `staged/` directory. This leaves `staging/`,
   `pending-deletions.json`, `snapshot/` and `burn-catalog.sqlite` alone. It is the
   same function `burn` and `reset` use, so it needs no new partial clear.
3. `fresh_scan(rehash)`. It is unchanged: snapshot (which already replaces an existing
   `snapshot/`), merge the catalog fingerprints with the cache, hash, write
   `scan.json`, and write the cache back.
4. `count`. It is unchanged: diff against the view, derive tombstones from
   `fingerprints`, and write `pending-deletions.json`.
5. `planning::plan_from` on the files just scanned (not a re-read of `scan.json`),
   then `staged::write_plan`.

**Why clear first.** Today a `scan --force` that fails partway (a non-UTF-8 name, an
I/O error mid-hash) has already removed and partly recreated `snapshot/`, but an
older `plan.json` and `scan.json` survive it. `burn` would then accept that plan,
because its stale check compares it with the old `scan.json`, and stage from a
snapshot that no longer matches. The stager's existence and re-hash checks
(`burn/stager.rs`) stop it with a missing-file or `HashMismatch` error before
anything reaches the media. So no wrong data is burned, but the error points at
staging rather than at the failed scan. Clearing first means a `plan` that fails at
any step leaves no staged plan, and `burn` says "no staged plan (run plan)".

The other invariants are unchanged:

- `plan` writes no catalog rows, and the catalog is committed only after success.
- The staged plan stays the recovery unit.
- `Pipeline` returns data and the CLI renders it.
- `ScanReport.hashed` becomes `usize` instead of `Option<usize>`, because the reuse
  path that produced `None` is gone.

`private fn fresh_scan` and `fn count` keep their current bodies. `plan` takes
`&mut self` because `fresh_scan` does.

**Re-running `plan`.** Every re-plan re-scans: after `mark-dead`, after `StalePlan`,
and when trying `--finalize` or `--disc-type`. That costs a warm scan (seconds) and
replaces the snapshot. It is the desired behaviour, because recovery then sees the
current source. Repeated `plan` runs never leave a second snapshot behind, because
`snapshot::create` removes the old tree first.

**`burn` without a fresh `plan` is unchanged.** It still reads the staged plan and
scan, and it still refuses a stale plan.

## 5. Errors

- **`NoStagedScan` is removed.** `plan` can no longer hit it. `burn` reads the staged
  plan *first*: a missing `plan.json` is `NoStagedPlan` ("no staged plan for p (run
  plan)", exit 2, as before). `plan` writes the two files together, so a `plan.json`
  without a `scan.json` can only come from someone deleting files by hand. `burn`
  reports that as `NoStagedPlan` too: the remedy is the same, and naming a file the
  operator never creates would only confuse.
- **`SnapshotMissing`** changes from "source snapshot missing for p (run scan)" to
  "… (run plan)". Its `PartialBurn` advice (`error::partial_burn_advice`) changes from
  "re-run scan --force, then plan and burn" to "re-run plan, then burn", the same
  advice a `StalePlan` gets.
- **`StalePlan`** keeps "re-run plan" and never mentions "reset" (`s07_reset` still
  asserts that). A re-plan now also re-scans.
- A corrupt `scan.json` can no longer reach `plan`, because `plan` deletes it first.
  `corrupt_scan` stays for `burn`'s read.

Every exit code keeps its meaning. `NoStagedScan` and `NoStagedPlan` were both exit 2,
so `burn` without staged state still exits 2.

## 6. Output and progress

- `plan` uses the `ConsoleProgress` sink instead of `NoopSink`, so hashing progress
  goes to stderr as it did for `scan`.
- On stdout, `plan` prints the scan summary first, then the plan block, which is
  unchanged:

  ```
  Scanned: 12 added, 0 modified, 1 deleted, 75606 unchanged.
  Hashed 12 of 75619 files (75607 fingerprints reused).
  <plan block, every existing shape unchanged>
  ```

  `render::scan_report` and `render::plan_report` stay separate functions, and the
  CLI prints one after the other. `scan_report` always prints the `Hashed` line now.
- Every user-facing "scan/plan/burn" or "scan → plan → burn" becomes "plan/burn" or
  "plan → burn":
  - burn report: "~N more discs of changes remain. Run: plan → burn" and "Copy sets
    are incomplete. Run: plan → burn";
  - status: "No live copy: N files (X GB) — re-burn on next plan/burn.";
  - mark-dead report: "Re-burn on next plan/burn (N):" and "Short of a copy —
    restored on next plan/burn (N):". The code comment saying "the operator's next
    action is `scan`" says `plan`.

## 7. Docs

- **README.** The workflow block becomes `init`, `plan`, `burn`, …, with no `scan`.
  - "the next `scan` / `plan` / `burn` writes them" becomes "the next `plan` / `burn`".
  - The mark-dead recovery prose: re-running `plan` is the whole recovery, because it
    re-scans the source and re-reads the disc states. The passage that says "the
    staged scan and the source snapshot both survive" is reworded.
  - The fingerprint-cache paragraph says "`plan` hashes …". The parenthetical "omitted
    when a previously staged scan is reused … `scan --force` rescans instead of
    reusing it" is removed, and `scan --verify` becomes `plan --rehash`.
  - The `cargo run -p diskrement-cli -- scan photos` example and the "full scan →
    plan → burn → verify → history loop" sentence lose their scan step.
- **Specs** get dated amendment notes, not rewrites:
  - the core-engine spec §9.1 restage policy (2026-05-31);
  - the 3a orchestration spec's `scan` and `plan` descriptions (2026-06-02);
  - the rust-rewrite spec §3.2, which goes from nine commands to eight (2026-09-05).
- **`docs/smoke-test-real-media.md`**: steps 1 and 3 read `plan`, not `scan`, `plan`.
- **`scripts/perf-bench.sh`**: the `scan1` step folds into `plan`, and `scan2` becomes
  a second `plan` after the burn. Each still greps its `Hashed` line as evidence that
  the cache works. Its header comments are updated to match.
- **CLAUDE.md**: the "one disc per copy set" bullet ("Each scan→plan→burn", the
  "re-run scan --force, then plan and burn" advice) and the "deletions ride" bullet
  ("on every scan", "`scan` writes those deletions") say `plan`.

## 8. Tests

**Pipeline (`pipeline/mod.rs`):**

- `a_second_scan_without_burn_reuses_the_staged_scan` turns into its opposite:
  `a_second_plan_picks_up_a_file_added_after_the_first`.
- **The #4 regression test:** a plan says nothing to burn → a file is added → the
  next plan places it.
- **Failed plan leaves nothing to burn:** with a staged plan from an earlier `plan`,
  a `plan` that fails mid-scan (a non-UTF-8 file name) leaves no `plan.json`. `burn`
  then fails with `NoStagedPlan`, not with a staging error.
- **Re-plan replaces the snapshot:** two `plan` runs leave exactly one `snapshot/`,
  holding the second run's content.
- `a_forced_rescan_hashes_nothing_once_the_cache_is_primed` and
  `a_forced_rescan_rehashes_only_a_modified_file` become plain `plan` tests.
- `verify_forces_a_full_rehash_even_with_a_primed_cache` becomes the `rehash` test.
- **Deleted** as unreachable:
  - `verify_with_a_staged_scan_and_no_force_still_rehashes`: there is no
    staged-scan-and-no-force path;
  - `plan_without_a_staged_scan_is_exit_2_with_the_run_scan_advice`.
- `re_planning_overwrites_the_staged_plan_and_leaves_the_staged_scan` becomes
  "re-planning replaces both the staged scan and the staged plan".
- The remaining `.scan(` / `.plan(` call pairs (about 31 sites) collapse into one
  `plan` call. Tests that inspect the scan counts read `PlanOutcome.scan`.

**Errors (`error.rs`):** the `NoStagedScan` display test is removed. The
`SnapshotMissing` display and `PartialBurn` advice tests change to the "plan"
wording.

**Render (`render.rs`, insta):**

- In `scan_gallery`, the "a reused staged scan (no Hashed line)" entry is removed
  along with the `Option`.
- `burn.snap` and the inline expectations get "Run: plan → burn" and "next
  plan/burn".

**CLI (`tests/binary.rs`, scenarios):**

- The three `"no staged scan for p (run scan)"` asserts become `NoStagedPlan`
  asserts, or the scan setup step is dropped where it was only there to reach a
  later error.
- New: `diskrement scan p` exits 1 (unknown subcommand). `plan --rehash` parses.
- Scenarios s01–s14 drop their `scan` steps.
- s10 (warm rescan) is rewritten around repeated `plan` calls, with no `--force`.
  Its point stands: a plan before any burn, a plan after one, and a plan after a
  file's size changes each report the expected `Hashed` split.
- s07 keeps asserting that the `StalePlan` advice does not mention "reset".

**Test count.** It drops by the deleted tests listed above and rises by the new ones.
The plan document records the before and after counts per crate.

## 9. Out of scope

- **Releasing the snapshot when `plan` finds nothing to burn.** The snapshot pins
  source data deleted later until the next `plan` replaces it. Freeing it early is a
  separate, small change, not part of #4.
- The deferred debt listed in CLAUDE.md (`archive_pointers`, `write_disc` and dead
  discs, the fingerprint cache never pruning) is untouched.
