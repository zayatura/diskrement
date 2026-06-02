# diskrement Plan 3a — Orchestration / Pipeline Service Design

**Date:** 2026-06-02
**Status:** Approved for planning
**Parent design:** `docs/superpowers/specs/2026-05-31-diskrement-core-engine-design.md` (§7.1, §8, §9.1, §9.2, §9.3, §9.4, §10.3)
**Predecessors:** Plans 1 (core), 2a (CatalogStore), 2b (read adapters), 2c (write adapters) — all done and merged.

---

## 1. Scope

Plan 3a is the **orchestration layer**: a single testable `Pipeline` service that wires the pure core (`Diff`, `Planner`, `ManifestBuilder`) to every adapter (`CatalogStore`, `FileScanner`, `SourceSnapshot`, `SimulatedBurner`, `SnapshotWriter`) and implements the staged `init`/`scan`/`plan`/`burn`/`reset`/`status`/`verify`/`history` workflow. Its methods take structured inputs and return structured result types — no argument strings, no interactive prompts, no console output. It is the unit the scripted full-loop integration test drives.

**In scope:**
- A `Pipeline` trait + `PipelineLive` + `ZLayer`, with the eight operations and their result types.
- A `StagedStore` for work-dir intermediate state (staged scan + staged plan as zio-json).
- The burn orchestration: assemble per-disc `DiscImage`s (2c) and one `CatalogUpdate` (2a) from a `BurnPlan` + manifests; **commit-after-success** (see §2.1).
- Source-snapshot lifecycle wiring (create on `scan`, reuse on `plan`/`burn`, release on `reset`/successful `burn`).
- Two new `CatalogStore` reads: `fileHistory` (powers `history`) and `markVerified` (powers `verify`).
- One new `SourceSnapshot` method: `checkCowCapable` (powers `init`'s upfront CoW check).
- A `PipelineError` ADT and the scripted full-loop integration test (§10.3).

**Deferred to Plan 3b:** CLI argument parsing + command dispatch, interactive insert/label prompts, output formatting + exit codes, and config/XDG/systemd path resolution (3a takes catalog path + work dir as explicit inputs). **Deferred to later cycles:** real ISO/UDF + hardware burning; real `/.archive` byte-linking; point-in-time restore; enforced `copies > 1`.

---

## 2. Architecture & conventions

Same functional-core / imperative-shell discipline as the earlier plans: the `Pipeline` is an imperative-shell orchestrator (it performs effects and sequences adapters), but it contains **no business rules** — diffing, planning, and manifest-building stay in the pure core. New code lives in a new package `diskrement.shell.pipeline`. ZIO 2.1.19; staged state via zio-json (already a dependency). Tests are integration tests against real temp dirs + real temp-file catalogs.

### 2.1 Key decisions

1. **Commit-after-success.** `burn` takes a **pre-burn** catalog snapshot, stages **all** discs via `SimulatedBurner`, and only on full success commits one `CatalogUpdate` (sessions written `BURNED`) in a single `applyUpdate`. Consequences: a crash before the commit leaves the catalog unchanged (the staged plan is the recovery unit); **no `PLANNED` rows ever exist**, so orphaned-`PLANNED` detection is intentionally dropped from `status`; each disc's embedded `catalog.sqlite` reflects the pre-burn state (the disc's own session is still captured by its `manifest.json`, and the host catalog remains the source of truth). This matches §9.4 and the `applyUpdate` built in Plan 2a.
2. **`Pipeline` returns data, not output.** Every method returns a structured result type; 3b renders it. No `println`/prompts in 3a.
3. **3a takes paths as inputs.** `ProjectPaths(catalogPath, workDir)` is provided to the service; 3b resolves it from XDG/systemd/args. 3a never reads environment variables.
4. **Staged scan = `ScannedFile`s; plan recomputes.** `scan` stages the `List[ScannedFile]`; `plan` re-runs `Diff.diff(currentView, stagedScan)` + `Planner.plan` and stages the `BurnPlan`; `burn` consumes the staged `BurnPlan` (which carries the files) and re-derives the per-disc added/modified split via `Diff.diff` for manifests. Because the catalog is unchanged between `scan` and the commit, these re-derivations are stable.

---

## 3. The `Pipeline` interface

`diskrement.shell.pipeline`:

```
final case class ProjectPaths(catalogPath: Path, workDir: Path)

trait Pipeline:
  def init(p: ProjectInit): IO[PipelineError, ProjectId]
  def scan(name: String, verify: Boolean, force: Boolean): IO[PipelineError, ScanResult]
  def plan(name: String, discTypeOverride: Option[DiscType]): IO[PipelineError, BurnPlan]
  def burn(name: String): IO[PipelineError, BurnReport]
  def reset(name: String): IO[PipelineError, Unit]
  def status(name: String): IO[PipelineError, StatusReport]
  def verify(name: String, discOrdinal: Option[Int]): IO[PipelineError, List[VerifyReport]]
  def history(name: String, relPath: RelPath): IO[PipelineError, List[FileHistoryEntry]]
```

Result types (fields are the design intent; the plan finalizes exact shapes):

```
final case class ScanResult(added: Int, modified: Int, deleted: Int, unchanged: Int)

final case class DiscBurn(ordinal: Int, stagingDir: Path, imageBytes: Long)
final case class BurnReport(discs: List[DiscBurn], oversized: List[RelPath])

final case class StatusReport(
    discs: List[DiscState],            // core type (ordinal, status, usable, used)
    openDiscOrdinal: Option[Int],
    totalCurrentFiles: Int,
    lastBurnAt: Option[Instant])

final case class VerifyReport(discOrdinal: Int, filesChecked: Int, mismatches: List[RelPath])

enum VersionStatus:
  case CURRENT, SUPERSEDED, DELETED
final case class FileHistoryEntry(
    contentHash: ContentHash, sizeBytes: Long,
    discOrdinal: Int, sessionOrdinal: Int, status: VersionStatus)
```

`Pipeline.live` is a `ZLayer` requiring `ProjectPaths & CatalogStore & FileScanner & SourceSnapshot & SimulatedBurner & StagedStore & Hasher` (the pure core objects are called directly — no layer). `SnapshotWriter` is **not** a direct dependency: it is consumed inside `SimulatedBurner.live`. `Hasher` is a direct dependency because `verify` re-hashes files itself.

---

## 4. Operations

**`init(p)`** — run the filesystem checks (§7), then `CatalogStore.createProject`. `p.sourceRoot`/`p.workDir` come from `ProjectPaths`/`ProjectInit`. Fails `NotSameFilesystem` / `NotCowCapable` before creating any row.

**`scan(name, verify, force)`** — resolve the project (`findProject`, else `ProjectNotFound`). If a staged scan exists and `!force`, reuse it (return its counts) without re-snapshotting. Otherwise: `SourceSnapshot.create(sourceRoot, workDir)`; `FileScanner.scan(snapshotRoot, CatalogStore.fingerprints(pid), verify)`; stage the `List[ScannedFile]`; compute and return `ScanResult` counts via `Diff.diff(readView(pid), scanned)`.

**`plan(name, discTypeOverride)`** — require a staged scan (`NoStagedScan`). `Diff.diff(readView(pid), stagedScan)` → `ChangeSet`; `Planner.plan(changeSet, readDiscStates(pid), Capacity.fromDiscType(discType, project.marginPercent))` where `discType = discTypeOverride orElse project.defaultDiscType`; stage and return the `BurnPlan`. If `BurnPlan.oversized` is non-empty, still stage but the result surfaces them (a hard error is raised at `burn`, or `plan` returns them for the caller to see — see §6).

**`burn(name)`** — require a staged plan (`NoStagedPlan`) and the live snapshot (`SnapshotMissing`). Pre-burn: `snapshotTo(tmpCatalog)` and `archivePointers(pid)` once. Recompute `ChangeSet` from staged scan. For each `PlannedDisc` (tombstones assigned to the last disc): `ManifestBuilder.build` → `SessionManifest`; build a `DiscImage` (staging dir `workDir/staging/disc-<ordinal>`, `snapshotRoot`, `files`, `DiscMetadata`(identity, manifest, tmpCatalog, pointers)); `SimulatedBurner.burn`. After all succeed, assemble one `CatalogUpdate` (DiscWrite per disc with `status = if willFinalize FINALIZED else OPEN`, label `"<name> disc <ordinal>"`, `SessionWrite` with next session ordinal, `burnedAt = now`, `manifestHash = sha256(manifestJson)`, added/modified `FileWrite`s split from `disc.files` with `discPath = "/data/" + relPath`, `deleted = tombstones`), `applyUpdate(update)`, then `SourceSnapshot.release` and clear staged state. Return `BurnReport`.

**Staging-dir lifecycle.** `burn` clears only the staged *inputs* (`staged/scan.json`, `staged/plan.json`) and releases the snapshot on success; it **leaves the per-disc staging output** (`workDir/staging/disc-<ordinal>`) in place, because in cycle 1 those directories *are* the simulated discs that `verify` re-reads. `reset` removes everything — staged inputs, the `staging/` output dirs, and the snapshot.

**`reset(name)`** — `StagedStore.clear(workDir)`, remove `workDir/staging/`, and `SourceSnapshot.release` (best-effort).

**`status(name)`** — `readDiscStates` + `readView` (count) + sessions (last burn time); assemble `StatusReport`. (No orphaned-`PLANNED` line — §2.1.)

**`verify(name, discOrdinal?)`** — for each burned disc (or the one requested): re-read its staging dir's `/data` tree, re-hash each file (`Hasher`), compare to the catalog's current content hashes; report mismatches; on a clean disc, `markVerified(pid, discOrdinal, sessionOrdinal)`. Returns `List[VerifyReport]`.

**`history(name, relPath)`** — `CatalogStore.fileHistory(pid, relPath)`.

---

## 5. `StagedStore`

```
trait StagedStore:
  def writeScan(workDir: Path, scanned: List[ScannedFile]): IO[PipelineError, Unit]
  def readScan(workDir: Path): IO[PipelineError, Option[List[ScannedFile]]]
  def writePlan(workDir: Path, plan: BurnPlan): IO[PipelineError, Unit]
  def readPlan(workDir: Path): IO[PipelineError, Option[BurnPlan]]
  def clear(workDir: Path): IO[PipelineError, Unit]
```

- Stages to `workDir/staged/scan.json` and `workDir/staged/plan.json` via **zio-json**, using shell DTOs that mirror `ScannedFile`/`BurnPlan`/`PlannedDisc` (no codecs on core types, per the 2c precedent). `readScan`/`readPlan` return `None` when the file is absent.
- "Fresh" in cycle 1 means simply "present" (no TTL). `reset` / `clear` removes the `staged/` dir.

> The `StagedStore` is a separate port so `PipelineLive` is testable with an in-memory fake and the real (filesystem) impl is integration-tested on its own.

---

## 6. New `CatalogStore` / `SourceSnapshot` methods

**`CatalogStore.fileHistory(project, relPath): IO[CatalogError, List[FileHistoryEntry]]`** — every `file_version` for `relPath` (ordered oldest→newest) joined to its placement (disc/session), with `status`: `CURRENT` for the live version, `DELETED` when the path's latest change-record is a tombstone (applied to that version), else `SUPERSEDED`. Built from the same latest-change-per-path window logic as `readView`/`archivePointers`.

**`CatalogStore.markVerified(project, discOrdinal, sessionOrdinal): IO[CatalogError, Unit]`** — `update session set state = 'VERIFIED'` for that disc+session where `state = 'BURNED'`; a 0-row update raises `IllegalStatusTransition` (mirrors `finalizeDisc`). The append-only fact tables are untouched; only the session lifecycle column moves forward, which §5.1 already models as mutable.

**`SourceSnapshot.checkCowCapable(workDir): IO[SnapshotError, Unit]`** — reflink-probe the work dir (clone a throwaway temp file via the existing `Cloner` seam); succeed if CoW works, else fail `NotCowCapable`. Used by `init`.

---

## 7. `init` filesystem checks (§9.1)

Before creating the project row, `init` verifies:
1. **Same filesystem:** `Files.getFileStore(sourceRoot) == Files.getFileStore(workDir)` — else `PipelineError.NotSameFilesystem`.
2. **CoW-capable:** `SourceSnapshot.checkCowCapable(workDir)` — else `PipelineError.NotCowCapable`.

Both hard-fail with an actionable message; neither leaves a project row behind.

---

## 8. Error model

```
sealed abstract class PipelineError(msg: String, cause: Throwable = null) extends Exception(msg, cause)
object PipelineError:
  final case class ProjectNotFound(name: String)        extends PipelineError(...)
  final case class NoStagedScan(name: String)           extends PipelineError(...)
  final case class NoStagedPlan(name: String)           extends PipelineError(...)
  final case class SnapshotMissing(name: String)        extends PipelineError(...)
  final case class NotSameFilesystem(src: Path, work: Path) extends PipelineError(...)
  final case class NotCowCapable(workDir: Path, detail: String) extends PipelineError(...)
  final case class OversizedFiles(paths: List[RelPath]) extends PipelineError(...)
  final case class Failed(cause: Throwable)             extends PipelineError("pipeline failure", cause)
```

Adapter errors (`CatalogError`/`BurnError`/`SnapshotError`/`ScanError`) are mapped into `PipelineError` (the specific cases above where meaningful, `Failed` otherwise). Truly unexpected conditions remain defects.

---

## 9. Testing strategy (§10.3)

- **`StagedStore`** — round-trip `List[ScannedFile]` and `BurnPlan` through real work-dir JSON; `read*` returns `None` when absent; `clear` removes staged files.
- **`CatalogStore.fileHistory`** — multi-session history (add → modify → delete → re-add) returns the right ordered versions with correct `CURRENT`/`SUPERSEDED`/`DELETED` status and disc/session.
- **`CatalogStore.markVerified`** — flips `BURNED → VERIFIED`; a non-existent/again-verified session raises `IllegalStatusTransition`.
- **`Pipeline` per-operation** — integration tests on real temp dirs + a real temp-file catalog, using the real adapters (and an in-memory `StagedStore` fake where convenient): `init` happy path + `NotSameFilesystem`/`NotCowCapable`; `scan` produces correct counts and stages; `scan` reuse vs `force`; `plan` from staged scan (incl. an oversized file surfaced); `burn` stages discs and commits the catalog (post-burn `readView`/`readDiscStates` reflect the burn); `reset` clears state; `status`/`verify`/`history` shapes.
- **Full-loop integration test (headline, §10.3):** `init → scan → plan → burn → modify source → scan → plan(--disc-type BD_R_DL) → burn → verify → history` on a real CoW work dir, asserting multi-disc spill, mixed capacity, `reset`, and that a catalog written by one `burn` is read by the next `scan`. (This test requires a CoW filesystem; it auto-skips on non-CoW, like the Plan 2b reflink test.)

---

## 10. Module / file structure

New package `diskrement.shell.pipeline`:
- `pipeline/Pipeline.scala` — `ProjectPaths`, the `Pipeline` trait, result types (`ScanResult`/`BurnReport`/`DiscBurn`/`StatusReport`/`VerifyReport`/`FileHistoryEntry`/`VersionStatus`), and `Pipeline.live`.
- `pipeline/PipelineLive.scala` — the orchestration.
- `pipeline/StagedStore.scala` — port + live + the staged-scan/plan JSON DTOs + `StagedStore.live`.
- `pipeline/PipelineError.scala` — the error ADT.
- Modify `catalog/CatalogStore.scala` + `CatalogStoreLive.scala` — add `fileHistory`, `markVerified`.
- Modify `fs/SourceSnapshot.scala` — add `checkCowCapable`.

Tests under `shell/src/test/scala/diskrement/shell/pipeline/` (StagedStore, per-operation Pipeline, full-loop) and `…/catalog/` (fileHistory, markVerified).

---

## 11. Mapping back to the engine design

| Engine design | Plan 3a delivery |
|---|---|
| §9.1 staged pipeline (scan→plan→burn, restage policy, work-dir state) | §4 + §5 `StagedStore` |
| §9.2 commands `init/scan/plan/burn/reset/status/verify/history` | §3 `Pipeline` (as service methods; CLI veneer is 3b) |
| §9.3 `SimulatedBurner` drives staging | §4 `burn` (prompts deferred to 3b) |
| §9.4 crash safety / commit-after-success | §2.1 decision 1 |
| §8 planner consumed with per-disc capacity | §4 `plan` |
| §7.1 fast-path scan + `--verify` | §4 `scan` |
| §9.2 `verify` sets VERIFIED; `history` | §6 `markVerified` / `fileHistory` |
| §10.3 full-loop integration test | §9 |

Plan 3b (CLI adapter: arg parsing, prompts, output, XDG/systemd paths) consumes this service.
