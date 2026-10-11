# Deferred deletions — design

**Date:** 2026-10-11
**Status:** Approved for planning
**Issue:** https://github.com/zayatura/diskrement/issues/9
**Parent design:** `2026-10-11-multiple-copies-design.md` (§6.1, the set-A fallback)
**Branch:** `feat/deferred-deletions`

---

## 1. Problem

When no copy set has anything to place, `pipeline::planning::plan_from` falls back to
set A's head disc, so that the cycle's tombstones still have a session to ride on.
`burn` then writes that disc, with no emptiness check. As a result:

- A **deletion-only** cycle burns a session that carries no data onto set A's open
  disc. If set A has no open disc, it opens a fresh, empty disc. At `copies > 1` that
  new disc (`NA`) has no `NB` twin, so the sets drift in disc count and 1A/1B stop
  being reliable twins (#9).
- A **no-op** cycle does the same, even though `plan` prints "Nothing to burn: no
  changes to plan."
- The same happens at `copies = 1`: an empty session, or a fresh empty disc.

The live `immich-dsz` project (copies=2, sealed BD-Rs) would hit this on its first
deletion-only cycle after the initial run.

## 2. Decision

**Deletions are deferred to the next disc that is burned anyway.** A cycle with
nothing to place writes no disc at all, at every `copies`. Its deletions are not
recorded. The next scan derives them again, because they are still in the catalog
and not in the source, and the next disc that carries data records them through the
existing path.

This works because a deletion needs no storage of its own to survive: `diff`
recomputes it on every scan until a commit records it.

### 2.1 Alternatives rejected

| Option | Why not |
|---|---|
| Record deletions in the catalog without burning a disc | `change_record.session_id` is `not null references session(id)`, so this needs a V4 migration: rebuild `change_record`, or add a sessionless deletion table. Every "latest change is DELETED" query (`read_view`, `fingerprints`, history) would have to change. The live catalog would also hold history that no disc's embedded `catalog.sqlite` has, which breaks "each disc is self-describing". The only gain is a fresher `status`/`history` between burns. |
| Add an empty session to each set's open disc, and skip sets with none | It still burns sessions that carry no data. With A open and B sealed, the sets drift in session count. On sealed media it ends up the same as deferral anyway. |
| Refuse to open a fresh, empty disc just for tombstones | This is only the guard half of the previous option, and it still needs a rule for what happens to the tombstones, which is this design. |

### 2.2 Accepted costs

- Between a deletion-only cycle and the next burned disc, `status` and `history`
  still show the deleted files as current. That is true of the archive: no disc says
  otherwise yet.
- Each scan in that window reports the deletions again.
- A file that is deleted and then restored unchanged before the next burn is never
  recorded as deleted. A file that is deleted and then re-created with new content is
  recorded as `MODIFIED`, not `DELETED` + `ADDED`. Both are faithful to what the
  archive holds.

## 3. Behaviour

### 3.1 Which paths are tombstones

A tombstone is a path whose **latest change is not `DELETED`** (the keys of
`Catalog::fingerprints`, which ignores dead discs) and that is **absent from the
scan**.

Today tombstones come from `diff` against `read_view`, which leaves out versions with
no live copy. Under deferral that would be a hole. Take a deferred deletion whose
discs all die before the next burn: it drops out of the view, is never tombstoned,
and reads as a current, lost version forever. The same hole exists today for a file
deleted after its only disc died. `burn` already uses `fingerprints` for exactly this
reason when it decides which tombstones to record (`pipeline/mod.rs`, "Still current
is the catalog's history").

The rule is one pure function, `diff::tombstones(recorded, scanned)`, in
`diskrement-core`. `plan` uses it, and so does the scan report's `deleted` count, so
the two always agree. `diff::diff` itself is unchanged: added, modified and unchanged
still come from the view, and its `deleted` field is no longer used by the shell.

Every view path is also a `fingerprints` key, so the new set is a superset of the old
one. It differs only by versions with no live copy.

### 3.2 Plan

`plan_from` drops the fallback. A set is in the plan iff:

- it has files to place (new files or catch-up copies), or
- `--finalize` is given and the set has an open disc to seal.

A plan can therefore have **zero sets**. `tombstones` is still set at plan level,
whatever the number of sets. When at least one set is planned, every planned disc
lists the tombstones in its manifest, and the first disc to commit records them,
exactly as today. `--finalize` with deletions only seals every open disc and the
tombstones ride those seal sessions. If no set has an open disc, the plan has zero
sets and the deletions are deferred.

The plan report:

| Plan | Output |
|---|---|
| zero sets, no tombstones | `Nothing to burn: no changes to plan.` (unchanged) |
| zero sets, N tombstones | `Nothing to burn: N deletions wait for the next disc.` (`1 deletion waits`) |
| seals only | unchanged |
| anything else | unchanged |

The rule is the same at every `copies`. The single-copy behaviour changes on
purpose, because the fallback wasted a session or a disc there too.

### 3.3 Burn

When a staged plan has zero sets, `burn` has nothing to write. It:

- calls no burner and opens no device (the real backend prompts per disc, so no
  insert prompt appears);
- writes nothing to the catalog;
- releases the snapshot and clears the staged state, like any finished cycle;
- reports "Nothing burned", with the deferred deletion count when there is one:
  - `Nothing burned: no disc in this plan.`
  - `Nothing burned: N deletions will be recorded with the next disc.`
  - followed by the usual coverage lines (copies > 1) and "up to date" line.

A zero-set `plan.json` can only come from `plan`. After a commit, `burn` writes the
plan back to disk only while sets remain. So "zero sets" and "every planned disc was
already committed" (a resumed burn) cannot be confused. `BurnReport` gets a
`deferred_deletions: Option<usize>` field: `Some(n)` iff the plan had zero sets.
`render::burn_report` uses it to choose the first line.

The confirmation prompt (`Burn the staged plan for …?`) is unchanged. Exit code is 0.

## 4. Invariants

These hold without extra work, because an unburned cycle writes nothing:

- **Commit after success:** there is no commit.
- **The staged plan is the recovery unit:** a zero-set plan has nothing to recover;
  clearing it is the normal end of a cycle.
- **The embedded `catalog.sqlite` reflects post-burn state:** unchanged. The next
  burned disc embeds the catalog with the deferred deletions applied.
- **Append-only history, the two allowed in-place updates, no schema change, exit
  codes:** untouched.
- **One disc per copy set per cycle** becomes "at most one", which CLAUDE.md already
  says.

CLAUDE.md gains one line under the invariants: deletions are recorded by the next
burned disc, so a cycle that burns nothing defers them. Never add a sessionless
deletion record.

## 5. Docs

- Multiple-copies spec §6.1: a note that the set-A fallback is replaced by this
  design.
- Multiple-copies plan, "Known limitations": the no-op/deletion-only item is marked
  resolved by #9, with a link to this spec.

## 6. Tests

**Core (`diff`)**
- `tombstones` returns recorded paths that are absent from the scan, sorted.
- `tombstones` ignores scanned paths that were never recorded.

**Planning (`planning.rs`)**
- The test `a_deletion_only_cycle_still_plans_one_set_a_disc_for_its_tombstones` is
  replaced: a deletion-only cycle plans zero sets and keeps its tombstones, at
  copies 1 and 2.
- A no-op cycle plans zero sets, at copies 1 and 2.
- `--finalize` with deletions only plans one seal per set with an open disc and keeps
  the tombstones.
- A path whose only copy is on a dead disc and that is absent from the scan is a
  tombstone.

**Pipeline (`pipeline/mod.rs`)**
- Burning a zero-set plan: every catalog table's row count is unchanged, no staging
  dir is created, the staged scan/plan are cleared, and the report says
  `deferred_deletions == Some(n)`.
- Deferred, then recorded: at copies=2 the deletion-only cycle writes nothing. The
  next cycle adds a file, so 1A and 1B both carry it and both manifests list the
  deletion, which is recorded once, on 1A's session.
- Recording after the last copy died: a deletion deferred while its disc is marked
  dead is still recorded by the next burned disc.

**Render (`render.rs`)**
- The plan line for deferred deletions, singular and plural.
- The two "Nothing burned" lines.

**Scenarios (`DISKREMENT_SCENARIOS_REQUIRED=1`)**
- s08: `burn` after a no-op plan writes no disc and exits 0.
- s13: at copies=2, a deletion-only cycle creates no new disc in either set; the next
  cycle with a new file records the deletion and both discs' manifests list it.
