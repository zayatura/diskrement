# Coverage counts files with no live copy — design

**Date:** 2026-10-12
**Status:** Approved for planning
**Issue:** https://github.com/zayatura/diskrement/issues/6
**Parent design:** `2026-10-11-multiple-copies-design.md` (coverage, "Known limitations" in its plan)
**Branch:** `feat/coverage-no-live-copy`

---

## 1. Problem

`replication::coverage` counts only the entries in `read_view`. `read_view` deliberately
drops a current version that has no live copy left, so that the next scan re-adds it
(the REBURN path). Once every copy of a file is on a dead disc:

- `status` shows every set as `set A: complete`, and its `Current files:` count
  silently shrinks;
- `burn` reports "every current file has N copies", and can close with "No more
  changes — … up to date";
- a single-copy project says nothing at all.

This lasts until the next `scan` re-adds the file. `disc mark-dead` does list the file
under "Re-burn", so nothing is lost silently, but every later report overstates how
safe the archive is.

## 2. Decision

**Coverage is computed over every current version, not just the live ones.** A
current version with no live copy anywhere is missing from every set, and is also
counted on its own, as "no live copy". Both `status` and `burn` show one extra line
for those files, at every `copies`, but only when there are any.

`read_view`, `shortfall`, `classify` and `diff` are untouched: the REBURN path depends
on `read_view` leaving these versions out, and it keeps doing so.

## 3. Source of "every current version"

`Catalog::fingerprints` already returns every path whose latest change is not
`DELETED`, deliberately unfiltered by dead discs, with its size. A path present in
`fingerprints` but absent from `read_view` is exactly a current version with no live
copy (`read_view`'s query is `fingerprints`' query plus the live-copy filter). No new
SQL and no schema change.

## 4. Core: `replication::coverage`

```rust
/// Copy coverage of the whole project.
pub struct Coverage {
    /// Indexed by copy set: current versions with no live copy in that set,
    /// including those with no live copy anywhere.
    pub sets: Vec<SetCoverage>,
    /// Current versions with no live copy in any set: the next scan re-adds
    /// them.
    pub no_live_copy: SetCoverage,
}

pub fn coverage(view: &CatalogView, current: &BTreeMap<RelPath, u64>, copies: u32) -> Coverage
```

`current` maps every current version's path to its size. For each path in `current`
that is not in `view.current`, every set's `missing_*` and `no_live_copy` grow by one
file and its bytes. Entries in the view count per set exactly as today. A view entry
missing from `current` cannot happen (the view is a subset by construction) and still
counts per set from the view alone.

`SetCoverage` keeps its fields; for `no_live_copy`, "missing" means missing from every
set.

## 5. Pipeline

- `status` and the end of `burn` read `fingerprints` beside `read_view` and pass its
  sizes to `coverage`.
- `StatusReport.coverage` and `BurnReport.coverage` become `Coverage`.
- `StatusReport.total_current_files` becomes `fingerprints.len()`, so it counts the
  files the new line describes.

## 6. Rendering

One shared line, printed only when `no_live_copy.missing_files > 0`, worded after
`mark-dead`'s "Re-burn on next scan/plan/burn":

```
No live copy: 2 files (0.8 GB) — re-burn on next scan/plan/burn.
```

- **`status`**: the line sits right above `Current files:`, at every `copies`. With
  `copies > 1` the `Copies:` block is unchanged in form; the affected files simply
  count as missing in every set, so no set reads "complete".
- **`burn`**: the line follows the per-set coverage lines (with `copies = 1`, where no
  per-set lines are printed, it is the only coverage line). While it shows, the closing
  "No more changes — … up to date" line is omitted, since it would be false; a
  "~N more discs of changes remain" line is still printed.
- With nothing missing, every output is byte-identical to today, at every `copies`.

## 7. Known imprecision

A file deleted from the source whose tombstone is still deferred (#9) and whose last
copy then dies is reported as "no live copy — re-burn" until a burned disc records the
deletion; it is never actually re-burned. `mark-dead` already lists such a file under
"Re-burn", so the two agree. `status` has no knowledge of the source, and giving it
some is out of scope.

## 8. Testing

- **Core:** a version with no live copy is missing in every set and counted in
  `no_live_copy`; view entries count as before; a proptest pins that every set's
  `missing_files >= no_live_copy.missing_files` and that a fully covered catalog
  reports all zeros.
- **Pipeline:** after marking dead the only disc(s) holding a file, `status` reports it
  in `no_live_copy`, no set is complete, and `total_current_files` still counts it.
- **Render:** `status` and `burn` at `copies` 1 and 2 with a non-zero `no_live_copy`;
  the existing all-zero outputs stay unchanged.
- **Scenarios:** `s09_mark_dead` (single copy) asserts that `status` after the kill
  prints `No live copy: 2 files` and still counts `Current files: 2`, and that the
  line is gone after the recovery burn. `s13_copies` asserts the negative after 1B
  dies: every file still has a copy on 1A, so set B is missing files but there is no
  "No live copy" line. The `copies = 2` no-live-copy case is covered at the pipeline
  and render levels instead of stretching `s13` to kill both sets.
