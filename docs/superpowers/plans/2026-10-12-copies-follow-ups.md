# Multiple-copies follow-ups (#7, #8, #10, #11, #12)

**Parent:** `2026-10-11-multiple-copies.md` ("Known limitations"). #6 and #9 have their own
specs and plans; #13 (raising `copies`) is deferred.

Each issue is a small, self-contained change on its own `feat/` branch, reviewed by a
read-only subagent before merging. This file records what each one decided; execution
deviations go under the issue's heading.

## #11 — mid-write I/O is not "not written"

Branch `feat/burn-io-phase`.

- New `BurnError::WriteInterrupted(io::Error)` (exit 6): `XorrisoBurner::write` maps a
  plain `Io` from running the burn command (pipe read, `wait()`) to it. `ToolMissing`
  from a failed launch passes through unchanged.
- `post_burn_check` turns every failure of the identity read-back into
  `PostBurnCheckFailed("could not read the identity back: …")`: it runs after a
  successful write, so it can never be a pre-write `Io`.
- `pipeline::burner_failure` classes `WriteInterrupted` with `BurnFailed` and
  `PostBurnCheckFailed` ("may have been written but was not recorded").
- A spawn failure other than "not found" at the write step is also `WriteInterrupted`.
  That is conservative: preflight and media inspection have just launched xorriso, so
  this is near-impossible, and over-warning costs a check of the disc, while
  under-warning costs a spent BD-R being treated as blank.

### Execution deviations (#11)

- Review: added `an_io_failure_mid_write_reports_the_disc_as_maybe_written`, a pipeline
  test through the real burner (copies=2, 1B's write fails with `Io` after 1A commits),
  so the chain `XorrisoBurner` → `burner_failure` → `PartialBurn` is pinned end to end.
  Confirmed it fails with the write's mapping removed.
- Left as is (pre-existing, over-warns): an unclassified `-toc` failure during media
  inspection is `BurnFailed`, so it reads "may have been written" although nothing was
  written yet. The comment in `burner_failure` now says so.

## #12 — an oversized catch-up copy is skipped, not refused

Branch `feat/catch-up-oversize`. The user chose "skip with a warning" over "refuse with
better advice".

- `plan_from` sorts each set's oversized files in two: a path in that set's `shortfall`
  list goes to the new `PlanReport::unplaceable_catch_up`, and everything else stays in
  `oversized`, which still refuses the burn (exit 3). `shortfall` only draws on unchanged
  files, so a path can never be both. A REBURN (a version with no live copy anywhere) is
  "added", so a REBURN too large for the disc is still oversized.
- `plan.json` gains `unplaceableCatchUp` (`serde(default)`). A plan staged before this
  change lists those files under `oversized` and is refused until it is re-planned.
- `plan` prints "Catch-up copies too large for <type> (skipped; plan with a larger
  --disc-type to place them):" with one line per file. `burn` writes the rest of the plan.

### Execution deviations (#12)

- Review: `burn` printed "Copies: set B is missing 1 files" followed by "No more changes —
  every copy set is up to date". That contradiction was already pinned by
  `a_two_copy_burn_lists_both_discs_and_the_coverage`, and #12 made it reachable with
  nothing left to plan. When no discs remain, there is no "no live copy" line, and a set
  is still short, the last line is now "Copy sets are incomplete. Run: scan → plan → burn".
- Added a CLI test that runs the real flow: copies=2, 1B marked dead, then plan with
  `DISKREMENT_DISC_BYTES` below the file size, then burn.

## #7 — `history` numbers versions, not copies

Branch `feat/history-copies`.

- `FileHistoryEntry` gains `version` (a 1-based `dense_rank()` over `file_version.id`
  in the query) and `on_dead_disc`. `file_history` now orders rows by
  `fv.id, d.ordinal, d.copy_set, s.ordinal`, so twins sit together (1A, 1B, 2A).
- At `copies = 1` the output is byte-identical to `fixtures/run-manifest.txt` (steps
  16 and 22). At `copies > 1` each version prints `v<n>  <size>  <hash>` once, with one
  `disc <name> / session <n>   <status>` line per copy under it.
- The status precedence is unchanged. A `CURRENT` copy on a dead disc now renders
  `CURRENT (on a dead disc)`, at every `copies`.

### Execution deviations (#7)

- Review: at `copies > 1` a dead disc's copy of a still-current version reads
  `CURRENT (on a dead disc)` permanently. Catching up the set adds a copy placement of
  the same version rather than creating a new version. The README now says so.
- Added a `copies = 1` render case for the annotation and a CLI test that runs
  `diskrement history` on a two-copy project.

## #8 — store errors name discs through `DiscKey::name`

Branch `feat/store-disc-names`. A `disc_name(tx, project, disc)` helper reads the
project's `copies` and returns `DiscKey::name(copies)`. It is used by `mark_verified`,
`mark_dead`, `disc_id`, `finalize_disc` and `write_disc`. If the project row is
unreadable (unreachable in practice), it falls back to the lettered form. Variants and
exit codes are unchanged.

### Execution deviations (#8)

- Review: `disc_id` keeps the pipeline's "no disc with ordinal …" wording. Added a
  `finalize_disc` (`plan --finalize`) assertion to the test.

## #10 — the full-cycle replication property

Branch `feat/full-cycle-property`. The proptest
`after_a_full_cycle_every_current_version_has_copies_live_copies` lives in
`pipeline/mod.rs` (`copies_tests`) and runs 24 cases. Each case:

- picks `copies` in 1..=3 and 2–5 files of 40–200 KB, on a project whose margin leaves
  250 000 usable bytes, so most sets span several discs;
- runs scan → plan → burn with the simulated burner until a plan is empty (at most 40
  cycles);
- then runs up to three rounds of writes, deletions and `mark-dead` on a random live
  disc, each followed by another run to completion.

After every completion, each file still in the source has a live copy in every set.
When no deletion is waiting, `status` coverage also reads complete with no "no live
copy" files. Mutation check: with `shortfall` replaced by empty lists, the test fails
("a.bin is short of copies"). 300 cases pass.

### Execution deviations (#10)

- The property's first form asserted the full view and coverage. It failed on a real
  case: a file deleted from the source whose deletion waits for the next burned disc
  (#9), and whose last copy then dies. It stays catalog-current with no live copy, and
  `status` reads "No live copy … re-burn on next scan/plan/burn", although there is
  nothing to re-burn. This is the imprecision #6 recorded. The property now checks files
  still in the source, and checks coverage only when no deletion is waiting. The gap is
  reported as a candidate follow-up instead of being fixed here.
- Review: the first oracle was `read_view`'s `live_sets`, the same view that drives
  catch-up. A mutant that stopped `read_view` seeing dead discs still passed. The test
  now also tracks the discs it kills and checks through `file_history` that each source
  file's bytes sit on an unkilled disc in every set. That catches the mutant. Files are
  now 40–200 KB (2–5 initially) so sets spill more often. The coverage exemption is
  bounded: each set may miss at most the number of pending deletions.
