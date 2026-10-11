# diskrement Staging Cleanup After Real Burns Design

**Date:** 2026-10-11
**Status:** Approved for planning
**Issue:** https://github.com/zayatura/diskrement/issues/3
**Parent designs:** `2026-10-11-multiple-copies-design.md` (one disc per copy set per
cycle, commit each disc as it burns), `2026-08-30-diskrement-cycle-2-real-burning-design.md`
(real vs. simulated backend, `StagingVerifier`)
**Branch:** `feat/staging-cleanup`

---

## 1. Problem

A successful **real** burn leaves its staging tree, `<work-dir>/staging/disc-<name>/`, on
disk forever. `Pipeline::burn` releases the snapshot and clears the staged scan/plan
after the last disc commits, but never touches `staging/`. Only `reset` (all of
`staging/`) and `stager::clear_and_create` (the same disc staged again) remove a tree.

Staging is reflinked from the snapshot, so it is nearly free while the source is
unchanged. Once the source deletes or rewrites a file, though, the staging tree keeps the
old extents alive, and that space never comes back. If staging fell back to plain copies,
each disc's full size is held from the start. Nothing reports this.

Issue #3 was written before #1 (multiple copies). Since then, staging trees are named by
`DiscKey::name` (`staging/disc-3B`), and one `burn` commits up to one disc per copy set,
each as soon as it is burned. This design takes that into account.

## 2. Goal

After a real-backend disc has been burned **and committed**, its staging tree is gone.
The simulated backend keeps its trees, because they are the "disc" `StagingVerifier`
reads. A failed cleanup never fails a committed burn.

## 3. Decisions

| # | Question | Decision |
|---|---|---|
| D1 | Where the backend decision lives | **`Burner::retains_staging(&self) -> bool`** on the closed enum: `Simulated` → `true`, `Real` → `false`. The pipeline asks; it never matches on the variant. Rejected: an `after_commit` hook on the burner (the ordering rule would live in a callback that cannot see the commit), and a `BurnResult` flag (per-result data for a per-backend constant). |
| D2 | When | **Per disc, immediately after that disc's `catalog.apply_update` succeeds** (and after the temporary burn catalog is removed). Not at the end of the run: if 1A commits and 1B fails, 1A's tree is already gone and 1B's stays. |
| D3 | Resume path | When `burn_disc` finds the disc **already committed** by an earlier run (`Ok(None)`), it removes `staging/disc-<name>` too (real backend only). This closes the crash window between commit and removal. |
| D4 | Failure handling | **Best-effort.** `NotFound` is silent. Any other error prints `warning: could not remove staging tree <path>: <error>` on stderr and the burn continues. No `PipelineError`, no exit-code change. |
| D5 | Trees left by older builds | **Documented only.** No sweep: the backend is chosen per invocation, not stored per project, so a tree under `staging/` might be simulated evidence. README says that `reset` is safe on a real-backend project with nothing staged. |
| D6 | Burn report | **Silent on success.** `render::burn_report` and its snapshots are unchanged. |
| D7 | `DiscBurn.staging_dir` | Kept. Its doc comment says that with the real backend the path has already been removed by the time the report is returned. |

## 4. Invariants

- **Commit after success, extended.** A staging tree is removed only after its disc's
  commit. Any failure before or at the commit (`DiscFailure::Unwritten`,
  `DiscFailure::Unrecorded`) leaves the tree in place.
- **Staging reads only from the snapshot.** Every disc, including each copy-set twin,
  is staged from `work_dir/snapshot` (`stager::copy_all` reflinks or copies;
  `check_all_exist` reads the snapshot). It is never staged from another disc's tree and
  never hardlinks. That is why removing 1A's tree cannot affect 1B. The snapshot is
  released only after the whole set loop. A change that stages a twin from its sibling's
  tree, or hardlinks, breaks this design; the two-copy test in §6 pins it.
- **Unchanged:** `reset` behaviour, `StalePlan` advice ("re-run plan", never "reset"),
  `PartialBurn` advice, and exit codes.

## 5. Changes

### 5.1 `crates/diskrement-shell/src/burn/mod.rs`

```rust
impl Burner<'_> {
    /// Whether a disc's staging tree must outlive its commit. The simulated
    /// backend's tree *is* the disc `StagingVerifier` reads back; a real disc
    /// carries its own record, so its tree is only scratch space.
    pub fn retains_staging(&self) -> bool { matches!(self, Burner::Simulated(_)) }
}
```

### 5.2 `crates/diskrement-shell/src/pipeline/mod.rs`

- A private free function `discard_staging(dir: &Path)` implements D4.
- `burn_disc`, after the commit and the `tmp_catalog` removal:
  `if !burner.retains_staging() { discard_staging(&burned.staging_dir) }`.
- `burn_disc`'s already-committed branch (`sess_ord == sp.session_ordinal + 1`) does the
  same for `work_dir/staging/disc-<name>`. The staging-path construction is shared with
  the `DiscImage` build so the name is formatted in one place (via `DiscKey::name`).

### 5.3 Docs

- README, near the existing `reset` paragraph, gets a short note:
  - a real burn removes each disc's staging tree once it is committed;
  - trees from older builds remain;
  - `reset` removes them safely on a real-backend project with nothing staged.
- CLAUDE.md gets a gotcha line saying the staging tree is removed after commit for the
  real backend only, and that staging must keep reading from the snapshot.
- The issue's open point 4 is answered by D5; points 1–3 and 5 by D1–D4 and D6.

## 6. Testing

TDD, in `diskrement-shell`.

**Test enabler:** move the scripted `Exec` (`Script`) out of
`burn/xorriso/burner.rs`'s test module into `burn/testkit.rs`. Pipeline tests can then
drive a `Burner::Real` with a scripted operator and exec. The existing burner tests use
it from its new home, unchanged.

Pipeline tests, real backend via the scripted exec:

1. **Single copy:** a committed burn leaves no `staging/disc-1`.
2. **Two copies:** a committed burn leaves neither `staging/disc-1A` nor `staging/disc-1B`.
   Both discs commit. When the operator is asked for 1B, `staging/disc-1A` is already
   gone. A test operator records `staging/disc-1A`'s existence inside each
   `confirm_insert` call. 1B still
   stages with every hash matching (the stager's `HashMismatch` check).
3. **Commit failure:** using the existing temp-trigger trick that refuses 1B's commit,
   `staging/disc-1A` is gone, `staging/disc-1B/data/...` still exists, and the error is
   the same `PartialBurn` as today.
4. **Declined prompt:** an aborted real burn still stages nothing (the existing burner
   assertion is kept).
5. **Resume:** a leftover `staging/disc-1` for a disc the catalog already holds is
   removed by the `Ok(None)` path.
6. **Removal failure:** removal fails (e.g. a read-only `staging/` parent, skipped when
   running as root, where permissions do not bite). The burn still returns `Ok` and the
   catalog holds the session.

Simulated backend: the existing pipeline tests and CLI scenarios keep their staging trees
and `verify` passes. Add one explicit assertion that a simulated burn keeps
`staging/disc-1/data`. Evidence runs set `DISKREMENT_SCENARIOS_REQUIRED=1`.

Verification: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D
warnings`, `cargo fmt --check`. The test count goes up and nothing is skipped
unexplained.

## 7. Out of scope

- Sweeping staging trees left by earlier builds (D5).
- `status` reporting work-dir space.
- Any change to `reset`.
