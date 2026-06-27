# diskrement Post-Cycle-1 — Post-Burn Catalog Snapshot Design

**Date:** 2026-06-27
**Status:** Implemented (branch `feat/post-burn-catalog-snapshot`)
**Parent design:** `docs/superpowers/specs/2026-06-02-diskrement-plan-3a-orchestration-design.md` (burn orchestration, catalog-update ordering)
**Predecessor:** Cycle 1 complete (Plans 1–3b done and merged).

---

## 1. Scope

A focused correctness fix to the `burn` orchestration: the `catalog.sqlite` embedded under each disc's `/.diskrement` must reflect the **post-burn** catalog state — the disc the burn just produced, its session, and its files — so that a single disc is genuinely self-describing. This **reverses the Plan 3a decision** "per-disc embedded catalog is pre-burn."

**In scope:**
- `burn` materializes the post-burn catalog into the work-dir snapshot (`burn-catalog.sqlite`) that each disc embeds, before staging the discs.
- A new catalog-package helper, `CatalogStore.applyUpdateTo(dbPath, update)`, that applies a `CatalogUpdate` to a standalone catalog file (its own scoped connection pool) without touching the live catalog.
- Cleanup fix: `burn` deletes the work-dir `burn-catalog.sqlite` after committing (previously left behind; only `reset` removed it).
- A small refactor: per-disc burn data (`tombs`/`sessOrd`/`manifest`) is derived once into a `DiscPlanData` record and reused by the snapshot, the staged manifests, and the live commit.

**Out of scope (unchanged / deferred):**
- The crash-safety model: the live catalog is still committed **after** all discs are staged, via the existing `CatalogStore.applyUpdate`.
- `pointers.json` semantics: archive pointers still describe versions superseded/deleted **before** this burn (pre-burn by definition); not changed here.
- No literal database file-swap (see §2.1 decision 2).
- A command to **re-stage already-committed sessions** — a real gap surfaced by this work, left for Cycle 2 (see §6).
- The `Pipeline` interface: no methods added, no signatures changed.

---

## 2. Architecture & conventions

Same functional-core / imperative-shell discipline. The change lives entirely in the `shell` module: the burn orchestration in `diskrement.shell.pipeline.PipelineLive` and one new helper on the `diskrement.shell.catalog.CatalogStore` companion. `core` is untouched. ZIO 2.1.19; Scala 3.3.4; `-Wunused:all`.

### 2.1 Key decisions

1. **The post-burn catalog is built on a copy, not by committing early.** The crash-safety constraint — commit to the live catalog only *after* every disc is staged — means we cannot mutate the live catalog before the burn succeeds. So the post-burn state is materialized into the work-dir copy (`burn-catalog.sqlite`), embedded into the discs, and only then is the same update committed to the live catalog. Building on a copy is therefore the *natural* shape, not a workaround.

2. **Double-apply, not a file-swap.** The end state we need is "the embedded catalog equals the live catalog." Two ways to get there: (a) apply the update to the copy for embedding and apply it again to the live DB as the commit (chosen); (b) apply once to the copy and then atomically replace the live DB file with it. (b) was rejected: swapping a SQLite file underneath the still-open live connection pool, with WAL/`-shm` sidecars in play, is corruption-prone, and the work dir and catalog dir are frequently on different filesystems (no atomic `rename`). The update is deterministic (same `now`, content-hash `manifest_hash` — not a chain — same ordinals and insertion order over two byte-identical starting DBs), so (a) yields a live catalog logically identical to every embedded copy. Verified on a real 5428-file burn: all three DBs match row-for-row across `disc`/`session`/`file_version`/`placement`.

3. **Every disc embeds the same complete post-burn catalog.** A single `burn-catalog.sqlite` is built once and copied into all discs, so each disc knows about its sibling discs (disc 1 lists disc 2 and vice versa). `manifest.json` stays per-disc ("what is on *this* disc"); `catalog.sqlite` is uniform ("the whole archive"). This is the property that makes "rebuild the catalog from any one disc" actually hold.

4. **Reuse `CatalogStore.live` (with migrations) for the copy.** `applyUpdateTo` builds the standard `DbPath → Db.poolLayer → CatalogStore.live` stack against the snapshot file. Migrations are `create table if not exists` (idempotent) and the snapshot already carries the schema, so re-running them is a no-op; this keeps the helper to a few lines and avoids exposing the package-private `CatalogStoreLive`.

5. **WAL checkpoint on close is sufficient.** `applyUpdateTo` is fully scoped: its single-connection pool closes before the staging loop copies the file, and SQLite checkpoints the WAL into the main DB on last-connection close. The integration tests read the embedded catalog's rows (not just file existence), so a stale/empty embed would fail them — and does not.

---

## 3. Components

### 3.1 `CatalogStore.applyUpdateTo` (new)

`diskrement.shell.catalog.CatalogStore` companion:

```
def applyUpdateTo(dbPath: Path, update: CatalogUpdate): IO[CatalogError, Unit]
```

Opens `dbPath` as a standalone catalog (its own scoped `ZConnectionPool` via `Db.poolLayer` + `CatalogStore.live`), applies `update`, and closes it. Non-`CatalogError` layer/setup failures are mapped to `CatalogError.MigrationFailed`. Used only to produce the disc-embedded snapshot; it never touches the live catalog.

### 3.2 `DiscPlanData` (new, private to pipeline)

```
private final case class DiscPlanData(
    disc: PlannedDisc, tombs: List[RelPath], sessOrd: Int, manifest: SessionManifest)
```

Per-disc data derived once during `burn`. Eliminates the previous duplication where the staging loop and the `CatalogUpdate` builder each recomputed `tombs`/`sessOrd`/`manifest`, guaranteeing the embedded catalog, the staged manifests, and the live commit are built from identical inputs.

### 3.3 `PipelineLive.burn` (changed)

New ordering:

1. Read staged scan + plan; oversized/snapshot guards; `readView`; `Diff`; `now`; `existingSessions`.
2. Derive `discData: List[DiscPlanData]` once; build the `CatalogUpdate` from it.
3. `snapshotTo(burn-catalog.sqlite)` — copy the **live (pre-burn)** catalog into the work dir.
4. **`CatalogStore.applyUpdateTo(burn-catalog.sqlite, update)`** — make the copy post-burn.
5. `archivePointers` (pre-burn, unchanged); stage each disc, embedding the post-burn copy.
6. `applyUpdate(update)` against the **live** catalog — the crash-safe commit point.
7. `snapshot.release`; `staged.clear`; **delete `burn-catalog.sqlite`** (cleanup fix).

Crash before step 6 leaves the live catalog untouched, exactly as before; the staged plan remains the recovery unit.

---

## 4. Testing strategy

TDD: the row-count assertions were written and watched fail (embedded `disc`/`file_version`/`session` were `0` — the pre-burn snapshot) before the fix.

1. **Post-burn embed (`PipelineBurnSpec`)** — after `init → scan → plan → burn`, open the disc's embedded `catalog.sqlite` over raw JDBC and assert `disc == 1`, `file_version == 2`, `session == 1` (was `0/0/0`).
2. **Cleanup (`PipelineBurnSpec`)** — after `burn`, assert `burn-catalog.sqlite` no longer exists in the work dir.
3. **Self-describing across discs (`PipelineSpillSpec`)** — in the two-disc spill case, assert disc 1's embedded catalog reports `disc == 2` and `file_version == 2`, proving each disc carries the full post-burn state including its sibling.
4. **No regressions** — full `core` (50) + `shell` (66) suites green; the existing `burn` test (embedded `catalog.sqlite` exists), full-loop, verify, and append-only specs unchanged.

Verified additionally in the real CLI on `test2` (5428 files): both embedded catalogs and the live catalog agree row-for-row across all session/disc/file tables.

---

## 5. Mapping back to the Plan 3a design

| Plan 3a decision | Status after this change |
|---|---|
| commit-after-success; staged plan is the recovery unit | **Unchanged** — live commit still last, via `applyUpdate` |
| per-disc embedded catalog is **pre-burn** | **Reversed** — embedded catalog is now **post-burn** (§2.1) |
| `Pipeline` returns data, no I/O; takes `ProjectPaths` | Unchanged |
| each disc embeds the project's catalog snapshot (`snapshotTo` / `VACUUM INTO`) | Unchanged as the *source*; now post-update before embedding |

---

## 6. Follow-ups (Cycle 2)

- **Re-stage committed sessions.** With the simulated burner the work-dir `staging/` tree *is* the output; once a burn is committed and `staging/` is cleared or `reset`, there is no supported way to regenerate those disc images (a fresh scan diffs to zero against the already-committed catalog). The real burner (writing to media at burn time) removes the need; absent that, a dedicated re-stage command would fill the gap.
- Revisit whether `pointers.json` should also reflect post-burn placements once `copies > 1` is activated (related to the existing duplicate-pointer note).
