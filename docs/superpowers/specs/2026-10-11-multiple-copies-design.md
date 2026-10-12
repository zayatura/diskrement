# diskrement Enforced Redundant Copies Design

**Date:** 2026-10-11
**Status:** Approved for planning
**Issue:** https://github.com/zayatura/diskrement/issues/1
**Parent designs:** `2026-05-31-diskrement-core-engine-design.md` (§out-of-scope: enforced
`copies > 1`), `2026-08-30-diskrement-cycle-2-real-burning-design.md` (dead-disc filter
activates together with copies), `2026-09-05-rust-rewrite-design.md` (§3.1 data formats)
**Branch:** `feat/multiple-copies`

---

## 1. Problem

`diskrement init <name> --copies N` stores `N` in `project.copies` and nothing reads it.
The planner places each file once and `disc_write` hard-codes `copy_index: 0`, so
`--copies 2` silently produces one copy. Separately, when a disc holding one of several
copies is marked dead, nothing re-plans a replacement: `read_view` is a path→entry map
with no copy count, so redundancy quietly degrades.

## 2. Goal

`copies = N` means every current version of every file has N burned copies on N distinct
physical discs. Losing any one disc loses no data, and the next cycle restores the
missing copy.

## 3. Decisions

| # | Question | Decision |
|---|---|---|
| D1 | What does one cycle write? | **One disc per copy set.** A single `burn` writes the set-A disc, then the set-B disc, and so on. New data never waits a cycle for its second copy. |
| D2 | Partial failure inside a cycle | **Commit each disc as soon as it burns.** A burned disc cannot be un-burned, so all-or-nothing is impossible to honour. A re-run of `burn` resumes at the first uncommitted disc. |
| D3 | Disc sets and naming | **Separate series per set, numbered per set: 1A/1B, 2A/2B.** Twins share a number. Plain `3` means `3A`. With `copies = 1`, names are unchanged ("disc 3"). |
| D4 | Changing `copies` on an existing project | **Out of scope.** `copies` is fixed at `init`. No real discs exist from this version yet; a raise command can come later and the deficit model below already supports back-fill. |
| D5 | `archive_pointers` | **Out of scope.** It already emits one pointer per placement, which is correct for several copies. The known debt (no dead flag) stays. |
| D6 | Interim `--copies > 1` refusal | **Dropped.** This feature replaces it. |

### 3.1 Invariants that change

- **"One disc per cycle"** becomes **"one disc per copy set per cycle"**. CLAUDE.md is
  updated to say so.
- **The frozen catalog schema (rewrite spec §3.1)** is relaxed for one migration (V3).
  That contract existed so the Scala and Rust binaries could share a catalog; the Scala
  binary was deleted at P10. Existing catalogs migrate forward on open. Discs already
  burned keep their embedded V2 catalog, which remains readable.
- **The staged `plan.json` and on-disc `/.diskrement` JSON** change shape (§6.1, §7.5).
  Both were frozen by the same §3.1 contract, relaxed for the same reason.
- **The Scala-comparison fixture tests are retired.** Seven tests compared Rust
  output byte for byte or row for row against `fixtures/` (schema, manifest
  hash, burn replay, catalog replay, mark-dead replay, plan replay, pointers);
  V3 and the new JSON fields break each of them by design. The corpus stays as
  input: V2 catalogs that must migrate and read, staged discs the verifier
  must accept, and timestamp strings.

### 3.2 Invariants that hold

- The catalog history is append-only. Copy placements are inserts. The only in-place
  writes stay the two forward-only status transitions.
- `DEAD` is derived, never stored. "Degraded" (§8.1) is derived the same way.
- Commit after success, per disc. No `PLANNED` rows.
- `Pipeline` returns data and does no I/O.
- Exit codes are unchanged.
- `LOST` is per physical copy; a current version with no live copy anywhere is still
  re-added as a new `file_version` (REBURN), exactly as today.

## 4. Vocabulary

- **Copy set:** one of `copies` independent disc series, numbered `0..copies`, displayed
  as letters `A`, `B`, `C`…. Every disc belongs to exactly one set.
- **Live copy:** a placement of a version on a disc that has no `MARKED_DEAD` event.
- **`live_sets(v)`:** the copy sets holding at least one live copy of version `v`.
- **Under-replicated in set k:** a current version with at least one live copy whose
  `live_sets` does not contain `k`.
- **Copy placement:** a `placement` row that points at an existing `file_version` and
  has no `change_record` of its own.
- **Disc name:** `<ordinal><letter>` when `copies > 1` (e.g. `3B`); `<ordinal>` when
  `copies = 1`.

## 5. Data model and schema

### 5.1 `init`

`--copies` accepts `1..=26` (one letter per set). `0` and anything above 26 are refused
by the argument parser as a usage error, before the catalog is opened.

### 5.2 Migration V3

Rebuild `disc` to:

- add `copy_set integer not null default 0 check (copy_set >= 0)`;
- replace `unique (project_id, ordinal)` with `unique (project_id, copy_set, ordinal)`.

Every other column and the `status in ('OPEN','FINALIZED')` check are carried over
verbatim. Disc `id`s are preserved, so `session.disc_id` and `disc_event.disc_id` stay
valid.

SQLite cannot alter a constraint in place, so V3 follows SQLite's documented table
rebuild procedure: `PRAGMA foreign_keys=OFF` (it is a no-op inside a transaction, so it
is issued before the migration transaction begins), then in one transaction create
`disc_new`, copy every row, drop `disc`, rename `disc_new` to `disc`, run
`PRAGMA foreign_key_check` and fail the migration on any row, commit, then
`PRAGMA foreign_keys=ON`. The migration runner grows whatever hook it needs for the
pragma steps; V1 and V2 are untouched.

After V3, every existing disc is in set 0, so a `copies = 1` project behaves exactly as
before.

### 5.3 Placements

`placement.copy_index` always equals the `copy_set` of the placement's disc. The write
path enforces this and refuses a mismatch as a constraint violation.

A copy placement inserts one `placement` row against an existing `file_version`, with no
new `file_version` and no `change_record`.

### 5.4 Read model

- `DiscState` gains `copy_set: u32`. `read_disc_states` returns it.
- `CatalogEntry` gains `live_sets: BTreeSet<u32>`.
- `read_view` keeps its existing rule: a current version with **no** live copy anywhere
  is dropped from the view, so `diff` re-adds it as a new version. For every version it
  keeps, it reports `live_sets`.
- `fingerprints` is unchanged.

### 5.5 Names and arguments

- Disc labels: `"<project> disc <name>"`, so `"photos disc 3B"`, or `"photos disc 3"` at
  `copies = 1`.
- `disc mark-dead <disc>` and `verify --disc <disc>` accept `3B` or `3` (meaning `3A`).
  A letter beyond the project's `copies` is an unknown disc (exit 2), as is an unknown
  ordinal today. A malformed value (`B3`, `3BB`, `-1B`) is refused by the parser as
  today's malformed ordinal is. `verify --disc -1` keeps reaching the domain and
  answering exit 2 (CLAUDE.md, `args.rs`).

## 6. Planning

### 6.1 Per-set plan

`plan` diffs the staged scan against the view once. For each copy set `k` it places:

- **new files:** `added ∪ modified` — identical for every set;
- **catch-up copies for k:** `unchanged` files whose view entry's `live_sets` lacks `k`.

Normally the catch-up list is empty. It is non-empty after a failed or skipped burn of
set `k`, or after a set-`k` disc is marked dead.

`plan_from` runs the planner once per set, against only that set's discs, and reduces
each set's full plan to the one disc that set writes this cycle (the existing rule: the
first disc carrying files). A set with nothing to place produces no entry, unless
`--finalize` is given, in which case each set's open disc is sealed. If no set has
anything to place, the plan holds no set and writes nothing; its deletions wait for the
next burned disc. (Superseded by `2026-10-11-deferred-deletions-design.md`, issue #9: the
original set-A fallback opened untwinned discs.) `--disc-type` applies to every set.

The staged `plan.json` becomes:

- `sets`: one entry per set being written, each with `copy_set`, the planned disc, its
  `remaining` estimate, and the disc's expected prior session count (§7.3). Each planned
  file carries a tag, `new` or `copy`, describing how it was planned.
- `tombstones` and `oversized`, once, at the plan level. (Since #12, also
  `unplaceableCatchUp`: catch-up copies too large for the disc type, skipped rather than
  refused.)

A `plan.json` staged by the previous binary fails to parse; `burn` reports it as a
`StalePlan` whose advice is "re-run plan" (never "reset").

### 6.2 Core changes (`diskrement-core`)

- New pure module `replication`: `shortfall(view, changes, copies) -> Vec<Vec<ScannedFile>>`,
  the catch-up list per set, each sorted by rel path.
- `planner::plan` takes the explicit files to place (instead of reading
  `added ∪ modified` from the `ChangeSet`), the tombstones, the set's discs and the
  capacity. Packing is unchanged: top off the set's newest open disc, then spill to fresh
  discs. Fresh ordinals are one above the highest ordinal **in that set**.
- `PlannedDisc` gains `copy_set`.

No disc ever holds two copies of a version: each disc belongs to one set, and set `k`
only receives versions without a live copy in `k`.

### 6.3 Typical cycles at `copies = 2`

- **Steady state:** 1A and 1B are open. New files `x`, `y` top off both. They fill at the
  same rate and spill together onto 2A and 2B.
- **After 1B is marked dead:** everything that was on 1B has no live copy in set B. Set
  A's disc carries new files only; set B's disc carries new files plus catch-up copies.
  If 1B was full, set B catches up over several cycles, one disc per cycle, and the
  remaining estimate says how many.

## 7. Burn

### 7.1 Order

`burn` writes the planned discs in set order. Each disc runs today's single-disc flow
unchanged: stale checks → post-burn catalog snapshot on a work copy → stage and burn →
commit to the live catalog. The next set starts only after the previous one commits. On
real media each disc has its own insert prompt naming it (e.g. "insert a blank for
photos disc 1B").

### 7.2 Classification at commit time

Each file is classified against the live catalog at the moment its disc commits:

- the path's current version has the same content hash → **copy placement** of that
  version, with `copy_index` = the disc's set;
- otherwise → a new `file_version` with an `ADDED` or `MODIFIED` change record, as today.

In a normal cycle 1A creates the versions and 1B's files become copy placements. If 1A
fails and is marked dead, 1B creates the versions itself. The outcome does not depend on
which set commits first.

**Tombstones** are written to the catalog once, by the first disc to commit in the cycle:
like files, each tombstone is classified at commit time, and one whose path no longer
has a current version is not written again. Every disc's manifest lists the cycle's
deletions.

### 7.3 Partial failure and resume

`burn` stops at the first failing disc. Discs already committed stay committed. The
staged plan is rewritten to hold only the uncommitted discs. The exit code is the failing
disc's, as today (6 for media/burner, 0 for a declined prompt), and the message names
what committed and what is left: "1A committed; 1B not written — re-run burn".

Crash safety: each planned disc records the session count its disc had at plan time. If
the catalog shows that disc with a later session already, the disc was committed before
the plan was rewritten, and `burn` skips it. Commit-after-success holds per disc, and the
staged plan stays the recovery unit.

### 7.4 Staleness

The existing checks (disc marked dead since planning, planned file changed in the scan,
tombstoned path back in the scan) run per disc just before that disc burns. A stale disc
stops the burn there; earlier discs stay committed. The advice is still "re-run plan".

### 7.5 Self-describing discs

- The embedded `catalog.sqlite` reflects post-burn state including every commit made
  earlier in the same cycle, so 1B's embedded catalog knows about 1A.
- `manifest.json` gains `copy_set` and a `copied` list beside `added`, `modified` and
  `deleted`.
- The identity in `project.json` gains `copy_set` and `copies`.
- The new fields are always written, including at `copies = 1` (`copy_set` 0, `copied`
  empty, `copies` 1), so readers never special-case a missing field.

## 8. Reporting, verify, history

### 8.1 `disc mark-dead`

The report has three groups:

- **Lost:** a non-current version with no other live copy. Unchanged.
- **Re-burn:** a current version with no other live copy. Unchanged.
- **Degraded (new):** a current version that still has a live copy on another disc, but
  none in the dying disc's copy set (the next cycle restores that set's copy). A current
  version still covered within its own set is not reported. Reported as e.g. "212 files now at 1 of 2 copies; restored on the next burn".

A non-current version that still has a live copy elsewhere is neither lost nor restored
(the source has moved past it); the report prints its count on its own line. The
`lost_version` table and its `LOST`/`REBURN` vocabulary are unchanged; degraded is
derived and not stored.

### 8.2 `status`

At `copies > 1`, a per-set block: open disc, disc count, and under-replicated files and
bytes. After a full cycle that is 0; after a partial burn or a dead disc it shows the gap
until a burn fills it. At `copies = 1`, output is unchanged.

### 8.3 `plan` and `burn`

`plan` prints one line per set, e.g. `1A: 14 new · 0 catch-up` and
`1B: 14 new · 212 catch-up`, with each set's remaining estimate. `burn` reports each
disc's outcome, then the overall copy coverage.

### 8.4 `verify`

Unchanged in mechanism. It verifies per disc and already checks every placement on a
disc, so each copy is verified independently. `verify --disc 1B` checks only 1B; a sweep
covers every live disc in every set. A test pins copy coverage.

### 8.5 `history`

One row per placement, as today, so a version on 1A and 1B appears twice, each row naming
its disc. Status precedence is unchanged.

## 9. Testing

- **Core (proptest):** every placement lands on a disc of its own set; no disc holds two
  copies of a version; after a simulated full cycle every current version has `copies`
  live copies; planning is deterministic in the set of inputs.
- **Catalog:** V3 on a `TempDir` copy of a V2 fixture preserves every disc id, puts
  every disc in set 0, and passes `foreign_key_check`; a copy placement adds no
  `file_version` and no `change_record`; `read_view` reports `live_sets`; mark-dead
  classifies lost, re-burn and degraded; a `copy_index` mismatch is refused.
- **Pipeline:** a full `copies = 2` cycle; partial failure then resume; a crash between
  commit and plan rewrite; recovery after mark-dead; `--finalize` sealing every set; an
  old-format `plan.json` reported stale.
- **CLI:** parsing `2B` and `2`; `--copies 0` and `27` refused; status and plan output;
  a new scenario under `crates/diskrement-cli/tests/scenarios/` covering a `copies = 2`
  cycle with a coaster (run with `DISKREMENT_SCENARIOS_REQUIRED=1`).
- **Docs:** CLAUDE.md's "one disc per cycle" invariant and the "`copies>1` degrades
  silently" debt entry are updated; the stale comments at `pipeline/mod.rs` (`disc_write`)
  and `catalog/store.rs` (`dead_disc_versions`) are rewritten.

## 10. Deviations recorded during planning

- **`placement.copy_index` is enforced by construction, not checked.**
  `FileWrite` has no `copy_index`; the store writes the disc's `copy_set`, so a
  mismatch cannot be expressed (§5.3).
- **The staged plan carries counts, not per-file tags.** Each set entry holds
  `catchUp` (how many of its files are catch-up copies) and `sessionOrdinal`
  (the session the burn writes, used to skip a disc committed before the plan
  was rewritten). Files are classified at commit time anyway (§7.2), so a tag
  would never be read (§6.1).
- **`pointers.json` gains `copySet`.** A pointer naming only an ordinal is
  ambiguous between twins; this narrows D5 without touching the known
  dead-flag debt.
- **`planner::plan` remains** as the single-set wrapper over `plan_set`, so the
  ported planner tests and properties keep their meaning.
- **`-1B` is refused by `DiscArg` parsing everywhere**, and a bare `-1` still
  reaches the domain for `verify --disc` (exit 2).

## 11. Deviations recorded during execution

Detail and rationale are in the plan's "Execution deviations" section
(`docs/superpowers/plans/2026-10-11-multiple-copies.md`).

- **Degraded is per set.** A current version counts as degraded only when no
  live copy is left in the dying disc's own copy set; one still covered within
  its set is in no group. §8.1 already reads this way (clarified during
  Task 10).
- **`status` per-set block is thinner than §8.2.** Each set reads only
  `set A: complete` or `set B: N files (X GB) missing`. The open disc is marked
  inline in the disc list and the disc count is derivable from it, so neither
  is repeated in the block.
- **Mark-dead's degraded group** prints
  `Short of a copy — restored on next scan/plan/burn (N):` followed by one line
  per file, not §8.1's "N files now at 1 of 2 copies" summary.
- **Partial-burn wording differs from §7.3's single message.** The tail is
  `1A committed; 1B not written (re-run burn)`, and the advice follows the
  cause: "re-run plan, then burn" for a stale plan, "re-run scan --force, then
  plan and burn" for a missing snapshot, "nothing is left to write" when only
  cleanup failed. A disc the burner wrote but the catalog did not record reads
  "written but not recorded"; one a failed burner may have touched reads "may
  have been written but was not recorded". No advice says "reset".
- **`plan` output differs from §8.3's example.** Each set prints
  `Plan: disc 1A (…) — N files (x new, y catch-up)`, and at `copies > 1` the
  no-remainder line reads `Nothing more for set L after this disc.`
  (`copies = 1` output is unchanged).
- **Staged files are written atomically.** `plan.json` and `scan.json` are
  written to a temp file, fsynced and renamed over the target (then the
  directory is fsynced), because `burn` rewrites `plan.json` inside the crash
  window §7.3 describes.
