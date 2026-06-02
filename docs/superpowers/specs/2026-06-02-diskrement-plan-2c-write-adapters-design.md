# diskrement Plan 2c — Write-Side Adapters Design

**Date:** 2026-06-02
**Status:** Approved for planning
**Parent design:** `docs/superpowers/specs/2026-05-31-diskrement-core-engine-design.md` (§6.2, §6.3, §9.3, §10.3)
**Predecessors:** Plan 2a (`CatalogStore` SQLite adapter) and Plan 2b (read-side adapters) — both done and merged.

---

## 1. Scope

Plan 2c implements the **write/output-side shell adapters**: everything needed to stage a burnable disc image (the on-disc tree) to a directory, given the core's plan + manifest and the catalog. It is the counterpart to Plan 2b's read side.

**In scope:**
- `SnapshotWriter` — writes the `/.diskrement/` metadata: `project.json`, `manifest.json`, `pointers.json` (via zio-json) and copies a prepared `catalog.sqlite` snapshot into place.
- `SimulatedBurner` — stages a full disc (`/data` + `/.diskrement`) to a per-disc directory in the work dir: copies each `/data` file from the frozen snapshot, **re-hashes on write** and aborts on mismatch (§6.2), delegates `/.diskrement` to `SnapshotWriter`, computes the would-be image size, and returns a `BurnResult`.
- Shell-side JSON DTOs (`ProjectJson`, `ManifestJson`, `PointersJson`) with zio-json codecs, plus an `ArchivePointer` model type and the burn input/output bundles (`DiscImage`, `DiscMetadata`, `BurnResult`) and a `BurnError` ADT.
- Two new `CatalogStore` read methods: `snapshotTo` (full DB snapshot via `VACUUM INTO`) and `archivePointers` (superseded/deleted versions with their physical locations, feeding `pointers.json`).
- Add the `dev.zio %% zio-json` dependency to the `shell` module.

**Deferred (explicitly NOT in this plan):**
- The CLI and the scripted **insert/label prompt interaction** (§9.3) — the burner is non-interactive here; prompts are **Plan 3**.
- Burn **orchestration**: assembling `DiscImage`s from a `BurnPlan` + manifests, the timing of catalog updates vs. snapshotting, and the `PLANNED → BURNED → VERIFIED` **crash-safety ordering** (§9.4) — **Plan 3**.
- Real ISO/UDF image generation and hardware burning; real same-disc `/.archive` byte-level linking (cycle 1 uses text pointers only).

---

## 2. Architecture & conventions

Follows the Plan 2a/2b precedent: ports/DTOs in the `shell` module (the pure `core` stays I/O-free and dependency-free); each adapter a `trait` + `final case class …Live` exposed via a `ZLayer`; ZIO 2.1.19; in-memory/real-temp-dir tests with ZIO Test.

New code lives in a new package `diskrement.shell.burn`; the new model type goes under `diskrement.shell.model`; the two catalog methods extend the existing `diskrement.shell.catalog` port.

### 2.1 Key decisions

1. **Shell-side JSON DTOs, not codecs on core types.** `manifest.json` is serialized from a shell `ManifestJson` DTO that mirrors the core `SessionManifest` (via a small mapping function), not by putting a zio-json codec on `SessionManifest`. Adding zio-json to `core` would give the pure core a dependency; the one-way `shell → core` rule forbids it. Same for `ProjectJson`/`PointersJson`.
2. **`VACUUM INTO` runs outside a transaction.** SQLite cannot `VACUUM` (or `VACUUM INTO`) inside a transaction, but the `CatalogStoreLive.tx` helper sets `autoCommit(false)`. So `snapshotTo` does **not** use `tx`; it executes the statement on a connection left in autocommit mode (no `setAutoCommit(false)` prepended).
3. **The burner is non-interactive.** `SimulatedBurner.burn` stages the disc and returns a `BurnResult`; it does not prompt. The §9.3 scripted insert/label interaction and the §9.4 catalog-update ordering are Plan 3's orchestration concerns.

---

## 3. Components

All paths are `java.nio.file.Path`. Core types in use: `ScannedFile`, `SessionManifest`/`ManifestEntry`, `ManifestChangeKind`, `RelPath`, `ContentHash`, `PlannedDisc` (consumed by Plan 3, not directly here).

### 3.1 `ArchivePointer` (model)

`diskrement.shell.model.ArchivePointer`:

```
final case class ArchivePointer(
    relPath: RelPath,
    contentHash: ContentHash,
    sizeBytes: Long,
    discOrdinal: Int,
    sessionOrdinal: Int,
    discPath: String,
    kind: ManifestChangeKind   // ADDED/MODIFIED => superseded older version; DELETED => tombstoned
)
```

One non-current (superseded or deleted) file version and where its bytes physically live. Produced by `CatalogStore.archivePointers`; serialized into `pointers.json`.

### 3.2 Input/output bundles

`diskrement.shell.burn`:

```
/** Project + disc identity written to project.json. */
final case class DiscIdentity(
    projectName: String,
    discOrdinal: Int,
    discCount: Int,        // the "M" in "disc N-of-M" (total discs in the project after this burn)
    sessionOrdinal: Int,
    createdAt: Instant
)

/** Everything that goes under /.diskrement for one disc. */
final case class DiscMetadata(
    identity: DiscIdentity,
    manifest: SessionManifest,        // core type; mapped to ManifestJson on write
    catalogSnapshot: Path,            // a prepared catalog.sqlite (from CatalogStore.snapshotTo)
    pointers: List[ArchivePointer]
)

/** A full disc to stage. */
final case class DiscImage(
    stagingDir: Path,                 // per-disc staging root in the work dir
    snapshotRoot: Path,               // the frozen source root (Plan 2b SnapshotHandle.root)
    files: List[ScannedFile],         // the /data payload for this disc (from PlannedDisc.files)
    metadata: DiscMetadata
)

/** Result of staging one disc. */
final case class BurnResult(
    stagingDir: Path,
    imageBytes: Long                  // total bytes staged (/data files + /.diskrement files)
)
```

### 3.3 `BurnError`

```
sealed abstract class BurnError(msg: String, cause: Throwable = null)
    extends Exception(msg, cause)              // mirrors CatalogError
object BurnError:
  final case class HashMismatch(relPath: RelPath, expected: ContentHash, actual: ContentHash)
      extends BurnError(s"content hash mismatch for ${relPath.value}: expected ${expected.value}, got ${actual.value}")
  final case class SourceFileMissing(relPath: RelPath)
      extends BurnError(s"source file missing from snapshot: ${relPath.value}")
  final case class BurnIoError(cause: Throwable)
      extends BurnError("burn I/O error", cause)
```

### 3.4 `SnapshotWriter` (port + live)

```
trait SnapshotWriter:
  def write(diskrementDir: Path, meta: DiscMetadata): IO[BurnError, Unit]
```

- Creates `diskrementDir` (the disc's `/.diskrement`) and writes four files:
  - `project.json` — from `ProjectJson(meta.identity)`.
  - `manifest.json` — from `ManifestJson(meta.manifest)` (mapped from the core `SessionManifest`).
  - `pointers.json` — from `PointersJson(meta.pointers)`.
  - `catalog.sqlite` — copied from `meta.catalogSnapshot`.
- JSON is written with zio-json (pretty-printed for human readability). I/O failures and a missing catalog-snapshot source map to `BurnError.BurnIoError`.
- **Live (`SnapshotWriterLive`)** uses `java.nio.file` + zio-json encoders. No other ports needed.

### 3.5 `SimulatedBurner` (port + live)

```
trait SimulatedBurner:
  def burn(image: DiscImage): IO[BurnError, BurnResult]
```

Behavior:
1. Create `image.stagingDir` (cleared first if a stale one exists, mirroring `SourceSnapshot`'s deterministic-location discipline).
2. For each `ScannedFile` in `image.files`: resolve source `image.snapshotRoot.resolve(relPath)`; if absent → `SourceFileMissing`. Copy it to `image.stagingDir/data/<relPath>` (creating parent dirs). **Re-hash the written file** with the `Hasher` port; if it differs from `ScannedFile.contentHash` → `HashMismatch` and abort (the whole staging is left for inspection; Plan 3 decides cleanup/retry).
3. Write `/.diskrement` via `SnapshotWriter.write(stagingDir/".diskrement", image.metadata)`.
4. Compute `imageBytes` = sum of all staged file sizes under `stagingDir` (data + metadata).
5. Return `BurnResult(stagingDir, imageBytes)`.

- **Depends on** the `Hasher` port (Plan 2b) and `SnapshotWriter`.
- **Live (`SimulatedBurnerLive`)** uses `java.nio.file` for copying and directory walking; `Hasher` for the verify; `SnapshotWriter` for metadata.

### 3.6 JSON DTOs (`diskrement.shell.burn`)

Shell DTOs with derived zio-json codecs (`DeriveJsonEncoder`/`DeriveJsonDecoder`). `RelPath`/`ContentHash` are serialized as their underlying `String`; `ManifestChangeKind` as its name; `Instant` via its ISO-8601 string.

- `ProjectJson(projectName, discOrdinal, discCount, sessionOrdinal, createdAt)` — from `DiscIdentity`.
- `ManifestEntryJson(relPath, contentHash, sizeBytes)`; `ManifestJson(projectName, discOrdinal, sessionOrdinal, added, modified, deleted)` — mapped from `SessionManifest`.
- `ArchivePointerJson(relPath, contentHash, sizeBytes, discOrdinal, sessionOrdinal, discPath, kind)`; `PointersJson(pointers: List[ArchivePointerJson])` — from `List[ArchivePointer]`.

Decoders exist too (not just encoders), so `verify`/recovery (later cycles) and the tests can re-read what was written.

### 3.7 New `CatalogStore` methods

Add to the `CatalogStore` trait and `CatalogStoreLive`:

```
def snapshotTo(dest: Path): IO[CatalogError, Unit]
def archivePointers(project: ProjectId): IO[CatalogError, List[ArchivePointer]]
```

- **`snapshotTo`** runs `VACUUM INTO '<dest>'` to write a consistent full-database snapshot to `dest` (which must not already exist). It executes **outside** the `tx` helper — on a connection in autocommit mode — because SQLite forbids `VACUUM` inside a transaction (decision §2.1.2). Result is a self-contained `.sqlite` file openable on its own.
- **`archivePointers`** returns every file version that is **not** the current live version of its path, joined to its placement and change-record kind: i.e. older (superseded) versions of paths plus the latest version of any tombstoned (deleted) path. Implemented as the complement of the latest-non-tombstoned-per-path set used by `readView`/`fingerprints`. Each row → `ArchivePointer(relPath, contentHash, sizeBytes, discOrdinal, sessionOrdinal, discPath, kind)` (disc ordinal via `placement → session → disc`).

---

## 4. On-disc layout produced (per disc)

```
<stagingDir>/
  data/<relPath...>                  # current versions, mirroring source layout
  .diskrement/
    project.json
    manifest.json
    catalog.sqlite                   # full snapshot (VACUUM INTO)
    pointers.json
```

---

## 5. Data flow (the slice Plan 2c enables)

```
Plan 3 assembles, per PlannedDisc:
  files          = plannedDisc.files
  manifest       = ManifestBuilder.build(...)            (core)
  catalogSnapshot= CatalogStore.snapshotTo(tmp) -> tmp   (this plan)
  pointers       = CatalogStore.archivePointers(project) (this plan)
  -> DiscImage
        │  SimulatedBurner.burn
        ▼
   BurnResult(stagingDir, imageBytes)   (Plan 3: prompts, catalog update ordering, verify)
```

Plan 2c builds and tests each adapter in isolation; Plan 3 wires them and owns timing/crash-safety.

---

## 6. Dependency

Add to the `shell` module in `build.sbt`: `"dev.zio" %% "zio-json" % zioJsonVersion` (a `zioJsonVersion` val pinned to a release compatible with ZIO 2.1.x). First new dependency since Plan 1; `core` gains nothing.

---

## 7. Error handling

Typed errors for expected conditions — `BurnError` (`HashMismatch`, `SourceFileMissing`, `BurnIoError`) for the burn/write side; `CatalogError` for the two catalog reads. Anything unexpected is a defect, consistent with `CatalogStoreLive` and the Plan 2b adapters.

---

## 8. Testing strategy (engine design §10.3)

- **`SnapshotWriter`** — write `/.diskrement` to a real temp dir from a hand-built `DiscMetadata` (with a small real `catalog.sqlite` file as the snapshot source); assert all four files exist, `catalog.sqlite` is byte-identical to the source, and `project.json`/`manifest.json`/`pointers.json` **round-trip** (decode with zio-json and compare to the expected DTOs).
- **`SimulatedBurner`** — stage a disc from a real temp "snapshot" tree: assert the `/data` tree mirrors the inputs, `/.diskrement` is present and correct, and `imageBytes` equals the sum of staged file sizes. A file whose actual bytes don't match its `ScannedFile.contentHash` triggers `HashMismatch` and aborts; a missing source file triggers `SourceFileMissing`. Uses the real `Hasher.live` and `SnapshotWriter.live`.
- **`CatalogStore.snapshotTo`** — against a populated temp-file catalog, snapshot to a new path, open the snapshot as its own DB, and assert its `fingerprints`/`readView` match the source (proving a consistent, self-contained copy). Verifies it works despite the no-transaction constraint.
- **`CatalogStore.archivePointers`** — after a multi-session history (add, modify, delete), assert the returned set is exactly the superseded + deleted versions with correct disc/session/hash, and excludes current live versions.

---

## 9. Module / file structure

New package `diskrement.shell.burn`:
- `shell/src/main/scala/diskrement/shell/burn/Json.scala` — `ProjectJson`, `ManifestJson`/`ManifestEntryJson`, `PointersJson`/`ArchivePointerJson` + zio-json codecs + mapping functions from core/model types.
- `shell/src/main/scala/diskrement/shell/burn/Burn.scala` — `DiscIdentity`, `DiscMetadata`, `DiscImage`, `BurnResult`, `BurnError`.
- `shell/src/main/scala/diskrement/shell/burn/SnapshotWriter.scala` — port + `SnapshotWriterLive` + layer.
- `shell/src/main/scala/diskrement/shell/burn/SimulatedBurner.scala` — port + `SimulatedBurnerLive` + layer.
- `shell/src/main/scala/diskrement/shell/model/ArchivePointer.scala` — the model record.
- Modify `shell/src/main/scala/diskrement/shell/catalog/CatalogStore.scala` + `CatalogStoreLive.scala` — add `snapshotTo`, `archivePointers`.
- Modify `build.sbt` — add zio-json + a version val.

Tests under `shell/src/test/scala/diskrement/shell/burn/` (SnapshotWriter, SimulatedBurner) and `…/catalog/` (snapshotTo, archivePointers).

---

## 10. Mapping back to the engine design

| Engine design | Plan 2c delivery |
|---|---|
| §6.3 on-disc layout (`project.json`/`manifest.json`/`catalog.sqlite`/`pointers.json` + `/data`) | §3.4 `SnapshotWriter` + §4 |
| §6.3 full catalog snapshot on every disc | §3.7 `snapshotTo` (VACUUM INTO) |
| §6.3 `/.archive` text pointers (cycle 1) | §3.7 `archivePointers` + `pointers.json` |
| §6.2 burner re-reads/re-hashes each file, aborts on mismatch | §3.5 `SimulatedBurner` step 2 (`HashMismatch`) |
| §9.3 `SimulatedBurner` stages `/data` + `/.diskrement`, computes image size | §3.5 (prompts deferred to Plan 3, §2.1.3) |
| §10.3 integration tests for `SimulatedBurner` | §8 |

Plan 3 (CLI + orchestration + crash-safety ordering + full-loop test) consumes these adapters.
