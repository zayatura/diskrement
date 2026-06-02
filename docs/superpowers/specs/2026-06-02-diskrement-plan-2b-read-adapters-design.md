# diskrement Plan 2b — Read-Side Adapters Design

**Date:** 2026-06-02
**Status:** Approved for planning
**Parent design:** `docs/superpowers/specs/2026-05-31-diskrement-core-engine-design.md` (§4.2, §6, §7.1, §10.3)
**Predecessor:** Plan 2a (`CatalogStore` SQLite adapter) — done and merged.

---

## 1. Scope

Plan 2b implements the **read/input-side shell adapters** of the functional-core/imperative-shell architecture: everything needed to turn a real source directory into a frozen, consistent `List[ScannedFile]` that the pure core (`Diff` → `Planner`) can consume.

**In scope:**
- `Hasher` — streaming SHA-256 of a file (port + live impl).
- `FileSystem` — read-only recursive walk + stat (port + live impl).
- `SourceSnapshot` — reflink (CoW) freeze of the source tree into the work dir, with a hard-fail when the filesystem is not CoW-capable (port + live impl + internal `Cloner` seam).
- `FileScanner` — walks a frozen snapshot, applies fast-path hashing (reuse a stored hash when size+mtime match), and produces fully-hashed `ScannedFile`s; a `verify` flag forces a full re-hash.
- `FileFingerprint` — a shell-model record carrying `(sizeBytes, mtime, contentHash)`.
- `CatalogStore.fingerprints` — a new read method on the existing Plan 2a port that returns the current version per path **including mtime**, sourcing the scanner's prior-fingerprint map.

**Deferred (explicitly NOT in this plan):**
- `SnapshotWriter` and `SimulatedBurner` (the write/output side) → **Plan 2c**.
- CLI, staged-pipeline orchestration, when create/release/scan actually fire, config/XDG/systemd path resolution → **Plan 3**.
- btrfs subvolume-snapshot `SourceSnapshot` implementation; ignore/exclude patterns; symlink and special-file handling (cycle-1 backs up regular files only).

---

## 2. Architecture & conventions

Follows the Plan 2a precedent without deviation in style:

- **Ports live in the `shell` module**, not `core`. The core remains pure (data in, data out, no effect types). Each port is a Scala 3 `trait`; each live implementation is a `final case class … extends <Port>` exposed through a `ZLayer`.
- **Effect type:** ZIO 2.1.19 (same as the rest of the shell).
- **Testing:** in-memory **fakes** back the unit tests of any adapter that composes other ports (`FileScanner`); **real impls** are integration-tested against real temp directories. ZIO Test.
- **New code** lives in a new package `diskrement.shell.fs`; the new model type goes under the existing `diskrement.shell.model`.
- **No new library dependencies.** `Hasher` uses `java.security.MessageDigest`; `FileSystem` uses `java.nio.file`; the reflink `Cloner` uses `java.lang.ProcessBuilder` (`cp`). No `zio-streams`.

### 2.1 Deviations from the parent engine design (intentional)

1. **No custom `Clock` port.** The engine design (§4.2) listed `Clock`; ZIO already provides `zio.Clock`, so a bespoke port adds nothing. Timestamps come from `zio.Clock`. (No `Clock` is actually consumed *within* Plan 2b — `mtime` comes from the filesystem, not a clock — but this records the decision so Plan 3 uses `zio.Clock` directly.)
2. **mtime lives shell-side, not in the core view.** The core's `CatalogEntry`/`CatalogView` stay hash-only (they are the `Diff` input and need nothing else). The mtime needed for fast-path hashing is exposed via a separate shell-side read (`CatalogStore.fingerprints`) returning `FileFingerprint`. The pure core is untouched by Plan 2b.
3. **Snapshot lifecycle is explicit create/release persisted on disk** — not a process-scoped `ZIO` `Scope` resource. The CLI pipeline (Plan 3) is staged across separate process invocations (`scan` creates the snapshot; `plan`/`burn` reuse it; `reset` or a successful `burn` releases it), which a `Scope`-bound resource cannot span. Plan 2b therefore builds `create`/`release` against a deterministic work-dir location; Plan 3 owns *when* they fire.

---

## 3. Components

All paths below are `java.nio.file.Path`. `RelPath`, `ContentHash`, `ScannedFile` are the existing `diskrement.core` types.

### 3.1 `FileFingerprint` (model)

`diskrement.shell.model.FileFingerprint`:

```
final case class FileFingerprint(
    sizeBytes: Long,
    mtime: Instant,
    contentHash: ContentHash
)
```

The prior-version identity the scanner needs to decide whether a file is unchanged. Keyed by `RelPath` in the scanner's `prior` map.

### 3.2 `Hasher` (port + live)

```
trait Hasher:
  def hash(path: Path): IO[IOException, ContentHash]
```

- **Live (`HasherLive`)**: opens the file, streams it through a SHA-256 `MessageDigest` in fixed-size buffered chunks (constant memory regardless of file size), returns the lowercase hex digest as `ContentHash`. I/O failures surface as `IOException` in the typed channel; anything else is a defect.
- Port retained so an alternate algorithm can be introduced later (engine design §5.3).

### 3.3 `FileSystem` (port + live)

```
final case class FileStat(sizeBytes: Long, mtime: Instant)

trait FileSystem:
  def walk(root: Path): IO[IOException, List[Path]]
  def stat(path: Path): IO[IOException, FileStat]
```

- **`walk`**: returns every **regular file** under `root`, recursively. Symlinks are **not followed**; symlink entries and special files (devices, sockets, FIFOs) are **skipped**. Hidden/dotfiles **are included**. Order is deterministic (sorted by path string) so downstream behavior is reproducible.
- **`stat`**: `sizeBytes` and `mtime` (as `Instant`) for a regular file.
- **Live (`FileSystemLive`)** uses `java.nio.file.Files` (`walkFileTree`/`Files.walk` with `BasicFileAttributes`, no `FOLLOW_LINKS`).
- The scanner derives `RelPath` from an absolute path via `root.relativize(p)`, rendering with `/` separators.

### 3.4 `SourceSnapshot` (port + live + `Cloner` seam)

```
final case class SnapshotHandle(root: Path)   // the frozen source root

sealed abstract class SnapshotError(msg: String, cause: Throwable = null)
    extends Exception(msg, cause)              // mirrors CatalogError
object SnapshotError:
  case class NotCowCapable(workDir: Path, detail: String) extends SnapshotError(...)
  case class SourceMissing(sourceRoot: Path)              extends SnapshotError(...)
  case class SnapshotIoError(cause: Throwable)            extends SnapshotError(...)

trait SourceSnapshot:
  def create(sourceRoot: Path, workDir: Path): IO[SnapshotError, SnapshotHandle]
  def release(handle: SnapshotHandle): UIO[Unit]
```

- **`create`** reflink-copies `sourceRoot` into a **deterministic** location under `workDir` (e.g. `workDir/snapshot`), returning a `SnapshotHandle` whose `root` is that frozen path. If a stale snapshot dir already exists it is cleared first (so a crashed prior run does not wedge the next). The deterministic location lets a later process (Plan 3 `plan`/`burn`) reconstruct the handle without shared in-memory state.
- **CoW is a hard requirement.** If the reflink cannot be performed because the work-dir filesystem lacks CoW support, `create` fails with `NotCowCapable` carrying an actionable message (which filesystems qualify: btrfs, XFS-with-reflink). There is no full-copy or live-tree fallback in cycle 1.
- **`release`** removes the snapshot directory; best-effort and total (`UIO`) — a failed cleanup is logged, not surfaced as an error.
- **Internal `Cloner` seam** (package-private):

  ```
  trait Cloner:
    def reflinkCopy(src: Path, dst: Path): IO[SnapshotError, Unit]
  ```

  - **Live (`CpCloner`)** shells out to `cp --reflink=always -a <src> <dst>` via `ProcessBuilder`. A non-zero exit whose stderr indicates the reflink/clone is unsupported maps to `NotCowCapable`; a missing source maps to `SourceMissing`; other failures map to `SnapshotIoError`.
  - The seam exists so `SourceSnapshot`'s orchestration (deterministic-path resolution, stale-dir clearing, release, error mapping) is unit-tested with a **fake `Cloner`**, and the real `cp` is exercised only by an environment-gated integration test (§5).

### 3.5 `FileScanner`

```
sealed abstract class ScanError(msg: String, cause: Throwable = null)
    extends Exception(msg, cause)                // mirrors CatalogError
object ScanError:
  // One failure case: the path that failed and the underlying cause. This
  // covers an I/O error and the (snapshot-prevented but defensive) case of a
  // file disappearing mid-walk, without speculating extra cases.
  case class FileFailed(relPath: RelPath, cause: Throwable)
      extends ScanError(s"failed scanning ${relPath.value}", cause)

trait FileScanner:
  def scan(
      snapshotRoot: Path,
      prior: Map[RelPath, FileFingerprint],
      verify: Boolean
  ): IO[ScanError, List[ScannedFile]]
```

- For each regular file under `snapshotRoot` (via `FileSystem.walk`):
  1. Compute `RelPath` relative to `snapshotRoot`.
  2. `FileSystem.stat` → `(size, mtime)`.
  3. **Fast path:** if `!verify` and `prior(relPath)` exists with **both** `sizeBytes` and `mtime` equal, reuse its `contentHash` (no read of file bytes).
  4. **Slow path** (added file, changed size/mtime, or `verify == true`): `Hasher.hash(absolutePath)`.
  5. Emit `ScannedFile(relPath, hash, size, mtime)`.
- Returns the list **sorted by `relPath`** (deterministic; the core also sorts, but a stable scanner output keeps tests and logs predictable).
- **Depends only on the `FileSystem` and `Hasher` ports** — not on `CatalogStore`. The `prior` map is an explicit input, so the scanner is unit-tested with in-memory fakes and a hand-built map. Wiring `CatalogStore.fingerprints` → `prior` is Plan 3's job.
- **Deletions are not the scanner's concern.** The scanner reports only what exists in the snapshot; a path present in `prior` but absent from the snapshot simply does not appear in the output. Classifying it as a deletion is the pure `Diff`'s job.
- **mtime matching precision:** `mtime` is compared as the `Instant` produced by `FileSystem.stat` against the `Instant` stored in `FileFingerprint`. To make the fast path reliable, mtime is normalized to a single fixed precision (milliseconds) **consistently** on both the persist side (when the catalog records `file_version.mtime`) and the read side (`stat`). Plan 2b establishes this normalization in `FileSystem.stat` and documents it; the catalog already stores mtime as an ISO-8601 string, so `fingerprints` parses it back to an `Instant` at the same precision. When in doubt the fast path errs toward a full re-hash (a precision mismatch costs performance, never correctness).

### 3.6 `CatalogStore.fingerprints` (new read on the Plan 2a port)

Add to the `CatalogStore` trait and `CatalogStoreLive`:

```
def fingerprints(project: ProjectId): IO[CatalogError, Map[RelPath, FileFingerprint]]
```

- Returns the **current** (latest non-tombstoned) `FileVersion` per `relPath`, as `FileFingerprint(sizeBytes, mtime, contentHash)`.
- Same latest-change-per-path window logic as `readView` (§ Plan 2a), but selecting `size_bytes`, `mtime`, `content_hash`; `mtime` (stored ISO-8601 text) is parsed to `Instant`.
- Sources the scanner's `prior` map in Plan 3.

---

## 4. Data flow (the slice Plan 2b enables)

```
(sourceRoot, workDir)
        │  SourceSnapshot.create
        ▼
   SnapshotHandle(frozenRoot)
        │  FileScanner.scan(frozenRoot, CatalogStore.fingerprints(project), verify)
        ▼
   List[ScannedFile]          ──►  (Plan 3: core Diff → Planner → ManifestBuilder)
```

`SourceSnapshot.release(handle)` is called by Plan 3 on `reset` or after a successful burn. Plan 2b delivers and tests each arrow's adapter in isolation; the wiring and the staged lifecycle belong to Plan 3.

---

## 5. Testing strategy (per engine design §10.3)

- **`Hasher` (`HasherLive`)** — known SHA-256 test vectors (e.g. empty file, `"abc"`); a large streamed input (multi-buffer) to confirm chunked digesting matches a one-shot digest; `IOException` on a missing file.
- **`FileSystem` (`FileSystemLive`)** — real temp tree: regular files found, dotfiles included, symlinks not followed and their entries skipped, special-file behavior (skip), nested directories, deterministic ordering; `stat` returns correct size and mtime.
- **`FileScanner`** — in-memory `FileSystem` + `Hasher` fakes; assert: fast-path reuse when size+mtime match (Hasher **not** called); full re-hash when size or mtime differs; `verify = true` forces re-hash of everything (fast path bypassed) and agrees with the fast-path result for unchanged files; a path in `prior` but absent from the tree does not appear; output sorted by `relPath`.
- **`SourceSnapshot`** — unit tests with a **fake `Cloner`**: `create` resolves the deterministic path, clears a stale snapshot dir, returns the handle; `NotCowCapable`/`SourceMissing`/`SnapshotIoError` mapping; `release` removes the dir and is total. Plus a **real-`cp` integration test that auto-skips** when the work dir is not CoW-capable (capability detected at runtime), and a **documented manual test** on a real btrfs filesystem (hard-fail demo on ext4).
- **`CatalogStore.fingerprints`** — integration test against real temp-file SQLite: round-trips `(size, mtime, hash)`; latest version wins after a modify; tombstoned paths excluded; mtime parses back to the same `Instant` at the normalized precision.

---

## 6. Module / file structure

New package `diskrement.shell.fs`:

- `shell/src/main/scala/diskrement/shell/fs/Hasher.scala` — port + `HasherLive` + `ZLayer`.
- `shell/src/main/scala/diskrement/shell/fs/FileSystem.scala` — `FileStat` + port + `FileSystemLive` + `ZLayer`.
- `shell/src/main/scala/diskrement/shell/fs/SourceSnapshot.scala` — `SnapshotHandle`, `SnapshotError`, `Cloner` (seam) + `CpCloner`, port + `SourceSnapshotLive` + `ZLayer`.
- `shell/src/main/scala/diskrement/shell/fs/FileScanner.scala` — `ScanError`, port + `FileScannerLive` (depends on `FileSystem` + `Hasher`) + `ZLayer`.
- `shell/src/main/scala/diskrement/shell/model/FileFingerprint.scala` — the model record.
- Modify `shell/src/main/scala/diskrement/shell/catalog/CatalogStore.scala` + `CatalogStoreLive.scala` — add `fingerprints`.

Tests under `shell/src/test/scala/diskrement/shell/fs/` (with in-memory fakes for `FileSystem`/`Hasher`/`Cloner`), plus the `fingerprints` test under `shell/src/test/scala/diskrement/shell/catalog/`.

---

## 7. Mapping back to the engine design

| Engine design | Plan 2b delivery |
|---|---|
| §4.2 ports: `FileSystem`, `Hasher`, `SourceSnapshot` | §3.2–3.4 (port + live each); `Clock` dropped in favor of `zio.Clock` (§2.1) |
| §6 source snapshot (reflink, CoW hard-fail, work-dir location, release) | §3.4 `SourceSnapshot` + `Cloner` seam |
| §7.1 fast-path hashing + `--verify` | §3.5 `FileScanner` |
| §5.2 derived "current version" view (+ mtime for fast path) | §3.6 `CatalogStore.fingerprints` |
| §10.3 integration tests for `FileScanner`, `SourceSnapshot` | §5 |

Plan 2c (write side) and Plan 3 (CLI + orchestration + full-loop test) consume these adapters.
