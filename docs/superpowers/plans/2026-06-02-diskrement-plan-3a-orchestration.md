# diskrement Plan 3a — Orchestration / Pipeline Service Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the `Pipeline` orchestration service that wires the pure core (`Diff`/`Planner`/`ManifestBuilder`) to every adapter and implements the staged `init`/`scan`/`plan`/`burn`/`reset`/`status`/`verify`/`history` workflow, returning structured results (no CLI, no prompts).

**Architecture:** Imperative-shell orchestrator in a new `diskrement.shell.pipeline` package. It sequences effects but holds no business rules (those stay in the pure core). Commit-after-success burn model. Staged scan/plan persisted as zio-json in the work dir. The `Pipeline` trait grows method-by-method across tasks (no stubs). Tests are integration tests on real temp dirs + temp-file catalogs, using a fake `SourceSnapshot` (plain copy) so they need no CoW filesystem.

**Tech Stack:** Scala 3 (3.3.4 LTS), sbt, ZIO 2.1.19, zio-jdbc 0.1.2, sqlite-jdbc 3.49.1.0, zio-json 0.7.3, zio-test 2.1.19. JDK 21. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-06-02-diskrement-plan-3a-orchestration-design.md` (and parent engine design §7.1, §8, §9, §10.3).

**Refinements vs. spec (discovered while planning):**
- `FileHistoryEntry` + `VersionStatus` live in `diskrement.shell.model` (not `pipeline`), so `CatalogStore.fileHistory` can return them without the `catalog` package depending on `pipeline`.
- The staged plan also stores the chosen `DiscType` (the core `BurnPlan` doesn't record it, and `burn` needs it for new discs): `StagedStore.writePlan(workDir, plan, discType)` / `readPlan → Option[(BurnPlan, DiscType)]`.

---

## Testing-environment note (applies to every test in this plan)

In this project's ZIO-test specs, calling `java.nio.file.Files` static methods **directly inside `assertTrue(...)`** throws `NoClassDefFoundError` under sbt's classloader. Always hoist `Files` calls into their own `ZIO.attempt(...)` binding and pass the resulting value to `assertTrue`.

---

## File Structure

**Main:**
- `shell/src/main/scala/diskrement/shell/model/FileHistory.scala` — `VersionStatus` enum + `FileHistoryEntry`.
- `shell/src/main/scala/diskrement/shell/pipeline/PipelineError.scala` — error ADT.
- `shell/src/main/scala/diskrement/shell/pipeline/StagedStore.scala` — port + live + staged JSON DTOs.
- `shell/src/main/scala/diskrement/shell/pipeline/Pipeline.scala` — `ProjectPaths`, trait, result types, `Pipeline.live`.
- `shell/src/main/scala/diskrement/shell/pipeline/PipelineLive.scala` — the orchestration.
- Modify `catalog/CatalogStore.scala` + `CatalogStoreLive.scala` — add `fileHistory`, `markVerified`.
- Modify `fs/SourceSnapshot.scala` — add `checkCowCapable`.

**Test:**
- `shell/src/test/scala/diskrement/shell/catalog/FileHistorySpec.scala`, `MarkVerifiedSpec.scala`
- `shell/src/test/scala/diskrement/shell/fs/CheckCowCapableSpec.scala`
- `shell/src/test/scala/diskrement/shell/pipeline/StagedStoreSpec.scala`
- `shell/src/test/scala/diskrement/shell/pipeline/PipelineTestKit.scala` (shared support)
- `shell/src/test/scala/diskrement/shell/pipeline/PipelineInitStatusHistorySpec.scala`, `PipelineScanResetSpec.scala`, `PipelinePlanSpec.scala`, `PipelineBurnSpec.scala`, `PipelineVerifySpec.scala`, `PipelineFullLoopSpec.scala`

---

## Environment prerequisite

Plans 1, 2a, 2b, 2c are merged and green. Verify:

```bash
sbt "shell/test"     # expect: 41 tests passed
```

---

### Task 1: `CatalogStore.fileHistory` + `markVerified` (+ model types)

**Files:**
- Create: `shell/src/main/scala/diskrement/shell/model/FileHistory.scala`
- Modify: `shell/src/main/scala/diskrement/shell/catalog/CatalogStore.scala`
- Modify: `shell/src/main/scala/diskrement/shell/catalog/CatalogStoreLive.scala`
- Test: `shell/src/test/scala/diskrement/shell/catalog/FileHistorySpec.scala`, `shell/src/test/scala/diskrement/shell/catalog/MarkVerifiedSpec.scala`

- [ ] **Step 1: Write the model types**

`shell/src/main/scala/diskrement/shell/model/FileHistory.scala`:

```scala
package diskrement.shell.model

import diskrement.core.ContentHash

/** Lifecycle status of one file version relative to its path's current state. */
enum VersionStatus:
  case CURRENT, SUPERSEDED, DELETED

/** One version of a path in its full history, with where its bytes live. */
final case class FileHistoryEntry(
    contentHash: ContentHash,
    sizeBytes: Long,
    discOrdinal: Int,
    sessionOrdinal: Int,
    status: VersionStatus
)
```

- [ ] **Step 2: Write the failing tests**

`shell/src/test/scala/diskrement/shell/catalog/FileHistorySpec.scala`:

```scala
package diskrement.shell.catalog

import diskrement.core._
import diskrement.shell.model._
import zio._
import zio.test._
import java.time.Instant

object FileHistorySpec extends ZIOSpecDefault:
  private val init = ProjectInit("p", "/src", "/work", DiscType.BD_R, 2.0, 1,
    Instant.parse("2026-06-01T00:00:00Z"))
  private val ts = Instant.parse("2026-06-01T10:00:00Z")
  private def fw(path: String, hash: String, size: Long) =
    FileWrite(RelPath(path), ContentHash(hash), size, ts, "/data/" + path, 0)
  private def session(ord: Int, added: List[FileWrite] = Nil,
                      modified: List[FileWrite] = Nil, deleted: List[RelPath] = Nil) =
    CatalogUpdate(ProjectId(0), Nil, List(DiscWrite(
      1, isNew = ord == 1, DiscType.BD_R, 1_000_000L, DiscStatus.OPEN, "d1",
      SessionWrite(ord, ts, "m" + ord, added, modified, deleted))))

  def spec = suite("CatalogStore.fileHistory")(
    test("orders versions oldest->newest with CURRENT/SUPERSEDED/DELETED status"):
      for
        pid <- ZIO.serviceWithZIO[CatalogStore](_.createProject(init))
        st   = ZIO.serviceWithZIO[CatalogStore]
        _   <- st(_.applyUpdate(session(1, added = List(fw("a","a1",10), fw("b","b1",20))).copy(projectId = pid)))
        _   <- st(_.applyUpdate(session(2, modified = List(fw("a","a2",15))).copy(projectId = pid)))
        _   <- st(_.applyUpdate(session(3, deleted = List(RelPath("b"))).copy(projectId = pid)))
        ha  <- st(_.fileHistory(pid, RelPath("a")))
        hb  <- st(_.fileHistory(pid, RelPath("b")))
      yield assertTrue(
        ha == List(
          FileHistoryEntry(ContentHash("a1"), 10, 1, 1, VersionStatus.SUPERSEDED),
          FileHistoryEntry(ContentHash("a2"), 15, 1, 2, VersionStatus.CURRENT)),
        hb == List(
          FileHistoryEntry(ContentHash("b1"), 20, 1, 1, VersionStatus.DELETED)))
  ).provideLayer(TestDb.layer)
```

`shell/src/test/scala/diskrement/shell/catalog/MarkVerifiedSpec.scala`:

```scala
package diskrement.shell.catalog

import diskrement.core._
import diskrement.shell.model._
import zio._
import zio.test._
import java.time.Instant

object MarkVerifiedSpec extends ZIOSpecDefault:
  private val init = ProjectInit("p", "/src", "/work", DiscType.BD_R, 2.0, 1,
    Instant.parse("2026-06-01T00:00:00Z"))
  private val ts = Instant.parse("2026-06-01T10:00:00Z")
  private val u = CatalogUpdate(ProjectId(0), Nil, List(DiscWrite(
    1, isNew = true, DiscType.BD_R, 100L, DiscStatus.OPEN, "d1",
    SessionWrite(1, ts, "m1",
      List(FileWrite(RelPath("a"), ContentHash("h"), 10L, ts, "/data/a", 0)), Nil, Nil))))

  def spec = suite("CatalogStore.markVerified")(
    test("flips a BURNED session to VERIFIED"):
      for
        pid <- ZIO.serviceWithZIO[CatalogStore](_.createProject(init))
        _   <- ZIO.serviceWithZIO[CatalogStore](_.applyUpdate(u.copy(projectId = pid)))
        _   <- ZIO.serviceWithZIO[CatalogStore](_.markVerified(pid, 1, 1))
        v   <- ZIO.serviceWithZIO[CatalogStore](_.sessionsByState(pid, SessionState.VERIFIED))
        b   <- ZIO.serviceWithZIO[CatalogStore](_.sessionsByState(pid, SessionState.BURNED))
      yield assertTrue(
        v.map(s => (s.discOrdinal, s.ordinal)) == List((1, 1)),
        b.isEmpty),
    test("marking a non-BURNED session fails with IllegalStatusTransition"):
      for
        pid <- ZIO.serviceWithZIO[CatalogStore](_.createProject(init))
        err <- ZIO.serviceWithZIO[CatalogStore](_.markVerified(pid, 1, 1)).flip
      yield assertTrue(err.isInstanceOf[CatalogError.IllegalStatusTransition])
  ).provideLayer(TestDb.layer)
```

- [ ] **Step 3: Run them to verify they fail**

Run: `sbt "shell/testOnly diskrement.shell.catalog.FileHistorySpec diskrement.shell.catalog.MarkVerifiedSpec"`
Expected: FAIL — `fileHistory`/`markVerified` not members.

- [ ] **Step 4: Add both methods to the trait**

In `CatalogStore.scala`, add `FileHistoryEntry` is available via `import diskrement.shell.model._`; add to `trait CatalogStore`:

```scala
  def fileHistory(project: ProjectId, relPath: RelPath): IO[CatalogError, List[FileHistoryEntry]]
  def markVerified(project: ProjectId, discOrdinal: Int, sessionOrdinal: Int): IO[CatalogError, Unit]
```

- [ ] **Step 5: Implement both in `CatalogStoreLive.scala`**

Add `IllegalStatusTransition` is already imported via `import diskrement.shell.model.CatalogError._`. Add:

```scala
  def fileHistory(project: ProjectId, relPath: RelPath): IO[CatalogError, List[FileHistoryEntry]] =
    tx {
      sql"""select fv.content_hash, fv.size_bytes, d.ordinal, s.ordinal, fvk.kind,
                   case when fv.id = cur.fvid then 1 else 0 end as is_current
            from file_version fv
            join placement p on p.file_version_id = fv.id
            join session s   on s.id = p.session_id
            join disc d      on d.id = s.disc_id
            join (
              select cr.file_version_id as fvid, cr.kind as kind,
                     row_number() over (partition by cr.file_version_id order by cr.id desc) as rn
              from change_record cr
            ) fvk on fvk.fvid = fv.id and fvk.rn = 1
            left join (
              select fvid from (
                select cr.file_version_id as fvid, cr.kind as kind,
                       row_number() over (partition by fv2.rel_path order by cr.id desc) as rn
                from change_record cr
                join file_version fv2 on fv2.id = cr.file_version_id
                where fv2.project_id = ${project.value} and fv2.rel_path = ${relPath.value}
              ) c where c.rn = 1 and c.kind <> 'DELETED'
            ) cur on cur.fvid = fv.id
            where fv.project_id = ${project.value} and fv.rel_path = ${relPath.value}
            order by fv.id"""
        .query[(String, Long, Int, Int, String, Int)].selectAll
    }.map(_.toList.map { case (ch, sz, dOrd, sOrd, kind, isCurrent) =>
      val status =
        if isCurrent == 1 then VersionStatus.CURRENT
        else if kind == "DELETED" then VersionStatus.DELETED
        else VersionStatus.SUPERSEDED
      FileHistoryEntry(ContentHash(ch), sz, dOrd, sOrd, status)
    })

  def markVerified(project: ProjectId, discOrdinal: Int, sessionOrdinal: Int): IO[CatalogError, Unit] =
    tx {
      sql"""update session set state = 'VERIFIED'
            where state = 'BURNED' and ordinal = $sessionOrdinal
              and disc_id = (select id from disc where project_id = ${project.value} and ordinal = $discOrdinal)"""
        .update.flatMap(n =>
          ZIO.when(n == 0L)(
            ZIO.fail(IllegalStatusTransition(
              s"no BURNED session $sessionOrdinal on disc $discOrdinal"))).unit)
    }
```

- [ ] **Step 6: Run them to verify they pass**

Run: `sbt "shell/testOnly diskrement.shell.catalog.FileHistorySpec diskrement.shell.catalog.MarkVerifiedSpec"`
Expected: `3 tests passed`.

- [ ] **Step 7: Commit**

```bash
git add shell/src/main/scala/diskrement/shell/model/FileHistory.scala shell/src/main/scala/diskrement/shell/catalog/CatalogStore.scala shell/src/main/scala/diskrement/shell/catalog/CatalogStoreLive.scala shell/src/test/scala/diskrement/shell/catalog/FileHistorySpec.scala shell/src/test/scala/diskrement/shell/catalog/MarkVerifiedSpec.scala
git commit -m "$(cat <<'EOF'
feat(shell): add CatalogStore.fileHistory and markVerified

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: `SourceSnapshot.checkCowCapable`

**Files:**
- Modify: `shell/src/main/scala/diskrement/shell/fs/SourceSnapshot.scala`
- Test: `shell/src/test/scala/diskrement/shell/fs/CheckCowCapableSpec.scala`

- [ ] **Step 1: Write the failing test**

`shell/src/test/scala/diskrement/shell/fs/CheckCowCapableSpec.scala`:

```scala
package diskrement.shell.fs

import zio._
import zio.test._
import java.nio.file.{Files, Path}

object CheckCowCapableSpec extends ZIOSpecDefault:
  private final case class FakeCloner(fail: Option[SnapshotError]) extends Cloner:
    def reflinkCopy(src: Path, dst: Path): IO[SnapshotError, Unit] =
      fail match
        case Some(e) => ZIO.fail(e)
        case None    => ZIO.attempt(Files.copy(src, dst)).mapError(SnapshotError.SnapshotIoError(_))

  private val makeWork: ZIO[Scope, Throwable, Path] =
    ZIO.acquireRelease(ZIO.attempt(Files.createTempDirectory("cowcheck")))(
      d => ZIO.attempt {
        val s = Files.walk(d); try s.sorted(java.util.Comparator.reverseOrder())
          .forEach(p => Files.deleteIfExists(p)) finally s.close()
      }.orDie)

  def spec = suite("SourceSnapshot.checkCowCapable")(
    test("succeeds when the cloner can reflink"):
      ZIO.scoped {
        for
          work <- makeWork
          r    <- SourceSnapshotLive(FakeCloner(None)).checkCowCapable(work).either
        yield assertTrue(r == Right(()))
      },
    test("fails NotCowCapable when the cloner cannot reflink"):
      ZIO.scoped {
        for
          work <- makeWork
          err  <- SourceSnapshotLive(FakeCloner(Some(
                    SnapshotError.NotCowCapable(work, "operation not supported")))).checkCowCapable(work).flip
        yield assertTrue(err.isInstanceOf[SnapshotError.NotCowCapable])
      }
  )
```

- [ ] **Step 2: Run it to verify it fails**

Run: `sbt "shell/testOnly diskrement.shell.fs.CheckCowCapableSpec"`
Expected: FAIL — `checkCowCapable` not a member.

- [ ] **Step 3: Add `checkCowCapable` to the trait and impl**

In `SourceSnapshot.scala`, add to `trait SourceSnapshot`:

```scala
  def checkCowCapable(workDir: Path): IO[SnapshotError, Unit]
```

and to `SourceSnapshotLive` (it reflink-probes a throwaway temp file in `workDir`; cleans up both files regardless of outcome):

```scala
  def checkCowCapable(workDir: Path): IO[SnapshotError, Unit] =
    ZIO.acquireReleaseWith(
      ZIO.attemptBlockingIO {
        Files.createDirectories(workDir)
        val src = Files.createTempFile(workDir, "cow-probe", ".src")
        Files.write(src, Array[Byte](0))
        src
      }.mapError(SnapshotError.SnapshotIoError(_))
    )(src =>
      ZIO.attemptBlockingIO {
        Files.deleteIfExists(src)
        Files.deleteIfExists(Path.of(src.toString + ".clone"))
      }.ignoreLogged
    )(src => cloner.reflinkCopy(src, Path.of(src.toString + ".clone")))
```

- [ ] **Step 4: Run it to verify it passes**

Run: `sbt "shell/testOnly diskrement.shell.fs.CheckCowCapableSpec"`
Expected: `2 tests passed`.

- [ ] **Step 5: Commit**

```bash
git add shell/src/main/scala/diskrement/shell/fs/SourceSnapshot.scala shell/src/test/scala/diskrement/shell/fs/CheckCowCapableSpec.scala
git commit -m "$(cat <<'EOF'
feat(shell): add SourceSnapshot.checkCowCapable reflink probe

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: `StagedStore` (work-dir scan/plan persistence)

**Files:**
- Create: `shell/src/main/scala/diskrement/shell/pipeline/PipelineError.scala`
- Create: `shell/src/main/scala/diskrement/shell/pipeline/StagedStore.scala`
- Test: `shell/src/test/scala/diskrement/shell/pipeline/StagedStoreSpec.scala`

- [ ] **Step 1: Write `PipelineError.scala`** (needed as the error type of `StagedStore`)

`shell/src/main/scala/diskrement/shell/pipeline/PipelineError.scala`:

```scala
package diskrement.shell.pipeline

import diskrement.core.RelPath
import java.nio.file.Path

/** Expected pipeline failures. Mirrors the other shell error ADTs. */
sealed abstract class PipelineError(msg: String, cause: Throwable = null)
    extends Exception(msg, cause)
object PipelineError:
  final case class ProjectNotFound(name: String)        extends PipelineError(s"project not found: $name")
  final case class NoStagedScan(name: String)           extends PipelineError(s"no staged scan for $name (run scan)")
  final case class NoStagedPlan(name: String)           extends PipelineError(s"no staged plan for $name (run plan)")
  final case class SnapshotMissing(name: String)        extends PipelineError(s"source snapshot missing for $name (run scan)")
  final case class NotSameFilesystem(src: Path, work: Path)
      extends PipelineError(s"work dir $work is not on the same filesystem as source $src")
  final case class NotCowCapable(workDir: Path, detail: String)
      extends PipelineError(s"work dir is not CoW-capable: $workDir [$detail]")
  final case class OversizedFiles(paths: List[RelPath])
      extends PipelineError(s"oversized files cannot be placed: ${paths.map(_.value).mkString(", ")}")
  final case class Failed(cause: Throwable) extends PipelineError("pipeline failure", cause)
```

- [ ] **Step 2: Write the failing test**

`shell/src/test/scala/diskrement/shell/pipeline/StagedStoreSpec.scala`:

```scala
package diskrement.shell.pipeline

import diskrement.core._
import zio._
import zio.test._
import java.nio.file.{Files, Path}
import java.time.Instant

object StagedStoreSpec extends ZIOSpecDefault:
  private val ts = Instant.parse("2026-06-01T00:00:00Z")
  private val scanned = List(
    ScannedFile(RelPath("a"), ContentHash("h1"), 10, ts),
    ScannedFile(RelPath("sub/b"), ContentHash("h2"), 20, ts))
  private val plan = BurnPlan(
    discs = List(PlannedDisc(1, isNew = true, willFinalize = false, scanned, 0L, 1_000L)),
    tombstones = List(RelPath("c")),
    oversized = Nil)

  private val makeWork: ZIO[Scope, Throwable, Path] =
    ZIO.acquireRelease(ZIO.attempt(Files.createTempDirectory("staged")))(
      d => ZIO.attempt {
        val s = Files.walk(d); try s.sorted(java.util.Comparator.reverseOrder())
          .forEach(p => Files.deleteIfExists(p)) finally s.close()
      }.orDie)

  def spec = suite("StagedStore")(
    test("round-trips a staged scan"):
      ZIO.scoped {
        for
          work <- makeWork
          ss    = StagedStoreLive()
          _    <- ss.writeScan(work, scanned)
          got  <- ss.readScan(work)
        yield assertTrue(got == Some(scanned))
      },
    test("round-trips a staged plan with its disc type"):
      ZIO.scoped {
        for
          work <- makeWork
          ss    = StagedStoreLive()
          _    <- ss.writePlan(work, plan, DiscType.BD_R_DL)
          got  <- ss.readPlan(work)
        yield assertTrue(got == Some((plan, DiscType.BD_R_DL)))
      },
    test("read returns None when nothing is staged; clear removes staged files"):
      ZIO.scoped {
        for
          work  <- makeWork
          ss     = StagedStoreLive()
          empty <- ss.readScan(work)
          _     <- ss.writeScan(work, scanned)
          _     <- ss.clear(work)
          after <- ss.readScan(work)
        yield assertTrue(empty.isEmpty, after.isEmpty)
      }
  )
```

- [ ] **Step 3: Run it to verify it fails**

Run: `sbt "shell/testOnly diskrement.shell.pipeline.StagedStoreSpec"`
Expected: FAIL — `StagedStoreLive` not found.

- [ ] **Step 4: Write `StagedStore.scala`**

`shell/src/main/scala/diskrement/shell/pipeline/StagedStore.scala`:

```scala
package diskrement.shell.pipeline

import diskrement.core._
import zio._
import zio.json._
import java.nio.file.{Files, Path}
import java.time.Instant

/** Persists the staged scan + plan in the work dir between pipeline stages. */
trait StagedStore:
  def writeScan(workDir: Path, scanned: List[ScannedFile]): IO[PipelineError, Unit]
  def readScan(workDir: Path): IO[PipelineError, Option[List[ScannedFile]]]
  def writePlan(workDir: Path, plan: BurnPlan, discType: DiscType): IO[PipelineError, Unit]
  def readPlan(workDir: Path): IO[PipelineError, Option[(BurnPlan, DiscType)]]
  def clear(workDir: Path): IO[PipelineError, Unit]

private[pipeline] final case class ScannedFileJson(
    relPath: String, contentHash: String, sizeBytes: Long, mtime: String)
private[pipeline] object ScannedFileJson:
  given JsonCodec[ScannedFileJson] = DeriveJsonCodec.gen[ScannedFileJson]
  def from(f: ScannedFile): ScannedFileJson =
    ScannedFileJson(f.relPath.value, f.contentHash.value, f.sizeBytes, f.mtime.toString)
  def to(j: ScannedFileJson): ScannedFile =
    ScannedFile(RelPath(j.relPath), ContentHash(j.contentHash), j.sizeBytes, Instant.parse(j.mtime))

private[pipeline] final case class PlannedDiscJson(
    ordinal: Int, isNew: Boolean, willFinalize: Boolean,
    files: List[ScannedFileJson], priorUsedBytes: Long, usableBytes: Long)
private[pipeline] object PlannedDiscJson:
  given JsonCodec[PlannedDiscJson] = DeriveJsonCodec.gen[PlannedDiscJson]
  def from(d: PlannedDisc): PlannedDiscJson =
    PlannedDiscJson(d.ordinal, d.isNew, d.willFinalize,
      d.files.map(ScannedFileJson.from), d.priorUsedBytes, d.usableBytes)
  def to(j: PlannedDiscJson): PlannedDisc =
    PlannedDisc(j.ordinal, j.isNew, j.willFinalize,
      j.files.map(ScannedFileJson.to), j.priorUsedBytes, j.usableBytes)

private[pipeline] final case class BurnPlanJson(
    discs: List[PlannedDiscJson], tombstones: List[String],
    oversized: List[ScannedFileJson], discType: String)
private[pipeline] object BurnPlanJson:
  given JsonCodec[BurnPlanJson] = DeriveJsonCodec.gen[BurnPlanJson]
  def from(p: BurnPlan, discType: DiscType): BurnPlanJson =
    BurnPlanJson(p.discs.map(PlannedDiscJson.from), p.tombstones.map(_.value),
      p.oversized.map(ScannedFileJson.from), discType.toString)
  def to(j: BurnPlanJson): (BurnPlan, DiscType) =
    (BurnPlan(j.discs.map(PlannedDiscJson.to), j.tombstones.map(RelPath(_)),
       j.oversized.map(ScannedFileJson.to)), DiscType.valueOf(j.discType))

final case class StagedStoreLive() extends StagedStore:
  private def stagedDir(workDir: Path): Path = workDir.resolve("staged")
  private def scanFile(workDir: Path): Path  = stagedDir(workDir).resolve("scan.json")
  private def planFile(workDir: Path): Path  = stagedDir(workDir).resolve("plan.json")

  def writeScan(workDir: Path, scanned: List[ScannedFile]): IO[PipelineError, Unit] =
    write(scanFile(workDir), scanned.map(ScannedFileJson.from).toJson)

  def readScan(workDir: Path): IO[PipelineError, Option[List[ScannedFile]]] =
    read(scanFile(workDir)).map(_.map(s =>
      s.fromJson[List[ScannedFileJson]].fold(
        e => throw new RuntimeException(s"corrupt scan.json: $e"),
        _.map(ScannedFileJson.to))))

  def writePlan(workDir: Path, plan: BurnPlan, discType: DiscType): IO[PipelineError, Unit] =
    write(planFile(workDir), BurnPlanJson.from(plan, discType).toJson)

  def readPlan(workDir: Path): IO[PipelineError, Option[(BurnPlan, DiscType)]] =
    read(planFile(workDir)).map(_.map(s =>
      s.fromJson[BurnPlanJson].fold(
        e => throw new RuntimeException(s"corrupt plan.json: $e"),
        BurnPlanJson.to)))

  def clear(workDir: Path): IO[PipelineError, Unit] =
    ZIO.attemptBlockingIO {
      Files.deleteIfExists(scanFile(workDir))
      Files.deleteIfExists(planFile(workDir))
      Files.deleteIfExists(stagedDir(workDir))
      ()
    }.mapError(PipelineError.Failed(_))

  private def write(file: Path, json: String): IO[PipelineError, Unit] =
    ZIO.attemptBlockingIO {
      Files.createDirectories(file.getParent)
      Files.writeString(file, json)
      ()
    }.mapError(PipelineError.Failed(_))

  private def read(file: Path): IO[PipelineError, Option[String]] =
    ZIO.attemptBlockingIO(
      if Files.exists(file) then Some(Files.readString(file)) else None
    ).mapError(PipelineError.Failed(_))

object StagedStore:
  val live: ZLayer[Any, Nothing, StagedStore] = ZLayer.succeed(StagedStoreLive())
```

- [ ] **Step 5: Run it to verify it passes**

Run: `sbt "shell/testOnly diskrement.shell.pipeline.StagedStoreSpec"`
Expected: `3 tests passed`.

- [ ] **Step 6: Commit**

```bash
git add shell/src/main/scala/diskrement/shell/pipeline/PipelineError.scala shell/src/main/scala/diskrement/shell/pipeline/StagedStore.scala shell/src/test/scala/diskrement/shell/pipeline/StagedStoreSpec.scala
git commit -m "$(cat <<'EOF'
feat(shell): add StagedStore for work-dir scan/plan persistence

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: `Pipeline` scaffolding + `init` / `status` / `history`

**Files:**
- Create: `shell/src/main/scala/diskrement/shell/pipeline/Pipeline.scala`
- Create: `shell/src/main/scala/diskrement/shell/pipeline/PipelineLive.scala`
- Create: `shell/src/test/scala/diskrement/shell/pipeline/PipelineTestKit.scala`
- Test: `shell/src/test/scala/diskrement/shell/pipeline/PipelineInitStatusHistorySpec.scala`

Depends on Tasks 1–3.

- [ ] **Step 1: Write `Pipeline.scala`** (trait starts with the three operations this task implements; later tasks add more)

`shell/src/main/scala/diskrement/shell/pipeline/Pipeline.scala`:

```scala
package diskrement.shell.pipeline

import diskrement.core.*
import diskrement.shell.catalog.CatalogStore
import diskrement.shell.fs.{FileScanner, Hasher, SimulatedBurner, SourceSnapshot}
import diskrement.shell.model.*
import zio.*
import java.nio.file.Path
import java.time.Instant

// Imports use package wildcards (matching the codebase style) so the trait can
// grow new methods across tasks without churning a selective import list and
// tripping -Wunused:all. The fs import is selective on purpose: only these four
// adapters are direct dependencies (FileSystem/SnapshotWriter are transitive).

/** Where a project's catalog DB and work dir live (resolved by the CLI in 3b). */
final case class ProjectPaths(catalogPath: Path, workDir: Path)

final case class ScanResult(added: Int, modified: Int, deleted: Int, unchanged: Int)
final case class DiscBurn(ordinal: Int, stagingDir: Path, imageBytes: Long)
final case class BurnReport(discs: List[DiscBurn], oversized: List[RelPath])
final case class StatusReport(
    discs: List[DiscState], openDiscOrdinal: Option[Int],
    totalCurrentFiles: Int, lastBurnAt: Option[Instant])
final case class VerifyReport(discOrdinal: Int, filesChecked: Int, mismatches: List[RelPath])

/** Staged orchestration of the whole workflow. Methods return data; the CLI (3b)
  * renders it. Grows across Plan 3a tasks. */
trait Pipeline:
  def init(p: ProjectInit): IO[PipelineError, ProjectId]
  def status(name: String): IO[PipelineError, StatusReport]
  def history(name: String, relPath: RelPath): IO[PipelineError, List[FileHistoryEntry]]

object Pipeline:
  val live: ZLayer[
    ProjectPaths & CatalogStore & FileScanner & SourceSnapshot & SimulatedBurner & StagedStore & Hasher,
    Nothing, Pipeline] =
    ZLayer {
      for
        paths    <- ZIO.service[ProjectPaths]
        catalog  <- ZIO.service[CatalogStore]
        scanner  <- ZIO.service[FileScanner]
        snapshot <- ZIO.service[SourceSnapshot]
        burner   <- ZIO.service[SimulatedBurner]
        staged   <- ZIO.service[StagedStore]
        hasher   <- ZIO.service[Hasher]
      yield PipelineLive(catalog, scanner, snapshot, burner, staged, hasher, paths)
    }
```

(Note: `ProjectId`/`ProjectInit` are in `diskrement.shell.model`. Confirm the exact import path when implementing — `Project.scala` defines them in `diskrement.shell.model`.)

- [ ] **Step 2: Write `PipelineLive.scala`** with `init`/`status`/`history` (the trait only declares these three so far)

`shell/src/main/scala/diskrement/shell/pipeline/PipelineLive.scala`:

```scala
package diskrement.shell.pipeline

import diskrement.core._
import diskrement.shell.catalog.CatalogStore
import diskrement.shell.fs.{FileScanner, Hasher, SimulatedBurner, SourceSnapshot}
import diskrement.shell.model._
import zio._
import java.nio.file.{Files, Path}

private[pipeline] final case class PipelineLive(
    catalog: CatalogStore,
    scanner: FileScanner,
    snapshot: SourceSnapshot,
    burner: SimulatedBurner,
    staged: StagedStore,
    hasher: Hasher,
    paths: ProjectPaths
) extends Pipeline:

  /** Resolve a project by name or fail ProjectNotFound. */
  private def project(name: String): IO[PipelineError, Project] =
    catalog.findProject(name).mapError(PipelineError.Failed(_))
      .flatMap {
        case Some(p) => ZIO.succeed(p)
        case None    => ZIO.fail(PipelineError.ProjectNotFound(name))
      }

  def init(p: ProjectInit): IO[PipelineError, ProjectId] =
    for
      _  <- sameFilesystem(Path.of(p.sourceRoot), paths.workDir)
      _  <- snapshot.checkCowCapable(paths.workDir)
              .mapError(e => PipelineError.NotCowCapable(paths.workDir, e.getMessage))
      id <- catalog.createProject(p).mapError(PipelineError.Failed(_))
    yield id

  private def sameFilesystem(src: Path, work: Path): IO[PipelineError, Unit] =
    ZIO.attemptBlockingIO(Files.getFileStore(src) == Files.getFileStore(work))
      .mapError(PipelineError.Failed(_))
      .flatMap(same => ZIO.unless(same)(ZIO.fail(PipelineError.NotSameFilesystem(src, work))).unit)

  def status(name: String): IO[PipelineError, StatusReport] =
    for
      p      <- project(name)
      discs  <- catalog.readDiscStates(p.id).mapError(PipelineError.Failed(_))
      view   <- catalog.readView(p.id).mapError(PipelineError.Failed(_))
      burned <- catalog.sessionsByState(p.id, SessionState.BURNED).mapError(PipelineError.Failed(_))
      verified <- catalog.sessionsByState(p.id, SessionState.VERIFIED).mapError(PipelineError.Failed(_))
    yield StatusReport(
      discs = discs,
      openDiscOrdinal = discs.find(_.status == DiscStatus.OPEN).map(_.ordinal),
      totalCurrentFiles = view.current.size,
      lastBurnAt = (burned ++ verified).flatMap(_.burnedAt).maxOption)

  def history(name: String, relPath: RelPath): IO[PipelineError, List[FileHistoryEntry]] =
    project(name).flatMap(p =>
      catalog.fileHistory(p.id, relPath).mapError(PipelineError.Failed(_)))
```

- [ ] **Step 3: Write the shared test kit**

`shell/src/test/scala/diskrement/shell/pipeline/PipelineTestKit.scala`:

```scala
package diskrement.shell.pipeline

import diskrement.shell.catalog.{CatalogStore, Db, DbPath}
import diskrement.shell.fs._
import zio._
import java.nio.file.{Files, Path}

/** A fake SourceSnapshot that plain-copies the source tree (no CoW needed), so
  * pipeline tests run anywhere. `cowOk` controls the init CoW check. */
final case class FakeSnapshot(cowOk: Boolean = true) extends SourceSnapshot:
  def create(sourceRoot: Path, workDir: Path): IO[SnapshotError, SnapshotHandle] =
    ZIO.attemptBlockingIO {
      val dst = workDir.resolve("snapshot")
      if Files.exists(dst) then
        val s = Files.walk(dst)
        try s.sorted(java.util.Comparator.reverseOrder()).forEach(p => Files.deleteIfExists(p))
        finally s.close()
      Files.createDirectories(dst)
      val s = Files.walk(sourceRoot)
      try s.forEach { p =>
        val rel = sourceRoot.relativize(p)
        val target = dst.resolve(rel.toString)
        if Files.isDirectory(p) then Files.createDirectories(target)
        else { Files.createDirectories(target.getParent); Files.copy(p, target) }
      } finally s.close()
      SnapshotHandle(dst)
    }.mapError(SnapshotError.SnapshotIoError(_))
  def release(handle: SnapshotHandle): UIO[Unit] =
    ZIO.attemptBlockingIO {
      if Files.exists(handle.root) then
        val s = Files.walk(handle.root)
        try s.sorted(java.util.Comparator.reverseOrder()).forEach(p => Files.deleteIfExists(p))
        finally s.close()
    }.ignoreLogged
  def checkCowCapable(workDir: Path): IO[SnapshotError, Unit] =
    if cowOk then ZIO.unit else ZIO.fail(SnapshotError.NotCowCapable(workDir, "fake: not cow"))

object PipelineTestKit:
  /** Recursively delete a directory tree (test cleanup). */
  def deleteRecursive(dir: Path): Unit =
    if Files.exists(dir) then
      val s = Files.walk(dir)
      try s.sorted(java.util.Comparator.reverseOrder()).forEach(p => Files.deleteIfExists(p))
      finally s.close()

  /** Fresh temp catalog + work dir per test, plus all real adapters except
    * SourceSnapshot (tests supply a FakeSnapshot via `mkPipeline`). */
  val env: ZLayer[Any, Throwable,
      ProjectPaths & CatalogStore & FileScanner & SimulatedBurner & StagedStore & Hasher] =
    val paths: ZLayer[Any, Throwable, ProjectPaths] =
      ZLayer.scoped {
        ZIO.acquireRelease(
          ZIO.attempt {
            val base = Files.createTempDirectory("pipeline-test")
            ProjectPaths(base.resolve("catalog.sqlite"), Files.createDirectories(base.resolve("work")))
          })(p => ZIO.attempt(deleteRecursive(p.workDir.getParent)).orDie)
      }
    ZLayer.make[
      ProjectPaths & CatalogStore & FileScanner & SimulatedBurner & StagedStore & Hasher](
      paths,
      ZLayer.fromFunction((p: ProjectPaths) => DbPath(p.catalogPath)),
      Db.poolLayer,
      FileSystem.live, Hasher.live, FileScanner.live,
      SnapshotWriter.live, SimulatedBurner.live,
      StagedStore.live, CatalogStore.live
    )

  /** Build a PipelineLive over the env services + a chosen snapshot. */
  def mkPipeline(snapshot: SourceSnapshot): URIO[
      ProjectPaths & CatalogStore & FileScanner & SimulatedBurner & StagedStore & Hasher, Pipeline] =
    for
      paths   <- ZIO.service[ProjectPaths]
      catalog <- ZIO.service[CatalogStore]
      scanner <- ZIO.service[FileScanner]
      burner  <- ZIO.service[SimulatedBurner]
      staged  <- ZIO.service[StagedStore]
      hasher  <- ZIO.service[Hasher]
    yield PipelineLive(catalog, scanner, snapshot, burner, staged, hasher, paths)

  /** A temp source tree on the same filesystem as the work dir (a sibling temp dir). */
  def makeSource(files: Map[String, String]): ZIO[Scope, Throwable, Path] =
    ZIO.acquireRelease(ZIO.attempt {
      val root = Files.createTempDirectory("pipeline-src")
      files.foreach { case (rel, content) =>
        val p = root.resolve(rel)
        Files.createDirectories(p.getParent)
        Files.writeString(p, content)
      }
      root
    })(root => ZIO.attempt(deleteRecursive(root)).orDie)
```

- [ ] **Step 4: Write the failing test**

`shell/src/test/scala/diskrement/shell/pipeline/PipelineInitStatusHistorySpec.scala`:

```scala
package diskrement.shell.pipeline

import diskrement.core._
import diskrement.shell.model._
import zio._
import zio.test._
import java.time.Instant

object PipelineInitStatusHistorySpec extends ZIOSpecDefault:
  private def initOf(name: String, src: java.nio.file.Path, work: java.nio.file.Path) =
    ProjectInit(name, src.toString, work.toString, DiscType.BD_R, 2.0, 1,
      Instant.parse("2026-06-01T00:00:00Z"))

  def spec = suite("Pipeline init/status/history")(
    test("init creates a project; status reports zero discs"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.txt" -> "aaa"))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("photos", src, paths.workDir))
          st    <- pipe.status("photos")
        yield assertTrue(st.discs.isEmpty, st.totalCurrentFiles == 0, st.openDiscOrdinal.isEmpty)
      },
    test("init fails NotCowCapable when the work dir is not CoW-capable"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.txt" -> "aaa"))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot(cowOk = false))
          err   <- pipe.init(initOf("photos", src, paths.workDir)).flip
        yield assertTrue(err.isInstanceOf[PipelineError.NotCowCapable])
      },
    test("status on an unknown project fails ProjectNotFound"):
      ZIO.scoped {
        for
          pipe <- PipelineTestKit.mkPipeline(FakeSnapshot())
          err  <- pipe.status("nope").flip
        yield assertTrue(err.isInstanceOf[PipelineError.ProjectNotFound])
      }
  ).provideLayer(PipelineTestKit.env.orDie)
```

- [ ] **Step 5: Run it (fails, then passes)**

Run: `sbt "shell/testOnly diskrement.shell.pipeline.PipelineInitStatusHistorySpec"`
First fails to compile (Pipeline not found); after Steps 1–3, expect `3 tests passed`. (`init`'s same-filesystem check passes because the source and work temp dirs are both under the system temp dir.)

- [ ] **Step 6: Commit**

```bash
git add shell/src/main/scala/diskrement/shell/pipeline/Pipeline.scala shell/src/main/scala/diskrement/shell/pipeline/PipelineLive.scala shell/src/test/scala/diskrement/shell/pipeline/PipelineTestKit.scala shell/src/test/scala/diskrement/shell/pipeline/PipelineInitStatusHistorySpec.scala
git commit -m "$(cat <<'EOF'
feat(shell): add Pipeline scaffolding with init/status/history

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: `Pipeline.scan` + `reset`

**Files:**
- Modify: `shell/src/main/scala/diskrement/shell/pipeline/Pipeline.scala` (add to trait)
- Modify: `shell/src/main/scala/diskrement/shell/pipeline/PipelineLive.scala`
- Test: `shell/src/test/scala/diskrement/shell/pipeline/PipelineScanResetSpec.scala`

- [ ] **Step 1: Write the failing test**

`shell/src/test/scala/diskrement/shell/pipeline/PipelineScanResetSpec.scala`:

```scala
package diskrement.shell.pipeline

import diskrement.core._
import diskrement.shell.model._
import zio._
import zio.test._
import java.nio.file.Files
import java.time.Instant

object PipelineScanResetSpec extends ZIOSpecDefault:
  private def initOf(name: String, src: java.nio.file.Path, work: java.nio.file.Path) =
    ProjectInit(name, src.toString, work.toString, DiscType.BD_R, 2.0, 1,
      Instant.parse("2026-06-01T00:00:00Z"))

  def spec = suite("Pipeline scan/reset")(
    test("scan on a fresh project reports all files as added and stages them"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.txt" -> "aaa", "sub/b.txt" -> "bbbb"))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          res   <- pipe.scan("p", verify = false, force = false)
          staged<- ZIO.serviceWithZIO[StagedStore](_.readScan(paths.workDir))
        yield assertTrue(
          res == ScanResult(added = 2, modified = 0, deleted = 0, unchanged = 0),
          staged.map(_.size) == Some(2))
      },
    test("reset clears staged scan and the staging dir"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.txt" -> "aaa"))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          _     <- pipe.scan("p", verify = false, force = false)
          _     <- pipe.reset("p")
          staged<- ZIO.serviceWithZIO[StagedStore](_.readScan(paths.workDir))
          snapGone <- ZIO.attempt(!Files.exists(paths.workDir.resolve("snapshot")))
        yield assertTrue(staged.isEmpty, snapGone)
      }
  ).provideLayer(PipelineTestKit.env.orDie)
```

- [ ] **Step 2: Run it to verify it fails**

Run: `sbt "shell/testOnly diskrement.shell.pipeline.PipelineScanResetSpec"`
Expected: FAIL — `scan`/`reset` not members.

- [ ] **Step 3: Add `scan` + `reset` to the trait**

In `Pipeline.scala`, add to `trait Pipeline`:

```scala
  def scan(name: String, verify: Boolean, force: Boolean): IO[PipelineError, ScanResult]
  def reset(name: String): IO[PipelineError, Unit]
```

- [ ] **Step 4: Implement them in `PipelineLive.scala`**

```scala
  def scan(name: String, verify: Boolean, force: Boolean): IO[PipelineError, ScanResult] =
    for
      p      <- project(name)
      reuse  <- staged.readScan(paths.workDir)
      result <- reuse match
                  case Some(s) if !force => countOf(p.id, s)
                  case _                 => freshScan(p, verify)
    yield result

  private def freshScan(p: Project, verify: Boolean): IO[PipelineError, ScanResult] =
    for
      handle  <- snapshot.create(Path.of(p.sourceRoot), paths.workDir)
                   .mapError(e => PipelineError.Failed(e))
      prior   <- catalog.fingerprints(p.id).mapError(PipelineError.Failed(_))
      scanned <- scanner.scan(handle.root, prior, verify).mapError(PipelineError.Failed(_))
      _       <- staged.writeScan(paths.workDir, scanned)
      res     <- countOf(p.id, scanned)
    yield res

  private def countOf(pid: ProjectId, scanned: List[ScannedFile]): IO[PipelineError, ScanResult] =
    catalog.readView(pid).mapError(PipelineError.Failed(_)).map { view =>
      val cs = Diff.diff(view, scanned)
      ScanResult(cs.added.size, cs.modified.size, cs.deleted.size, cs.unchanged.size)
    }

  def reset(name: String): IO[PipelineError, Unit] =
    for
      _ <- project(name)
      _ <- staged.clear(paths.workDir)
      _ <- ZIO.attemptBlockingIO {
             val staging = paths.workDir.resolve("staging")
             if Files.exists(staging) then
               val s = Files.walk(staging)
               try s.sorted(java.util.Comparator.reverseOrder()).forEach(x => Files.deleteIfExists(x))
               finally s.close()
           }.mapError(PipelineError.Failed(_))
      _ <- snapshot.release(SnapshotHandle(paths.workDir.resolve("snapshot")))
    yield ()
```

Add the needed imports to `PipelineLive.scala` if not present: `diskrement.shell.fs.SnapshotHandle` (for `reset`), `diskrement.core.Diff`. (`ScannedFile`, `Project`, `ProjectId` are already in scope via `diskrement.core._` / `diskrement.shell.model._`.)

- [ ] **Step 5: Run it to verify it passes**

Run: `sbt "shell/testOnly diskrement.shell.pipeline.PipelineScanResetSpec"`
Expected: `2 tests passed`.

- [ ] **Step 6: Commit**

```bash
git add shell/src/main/scala/diskrement/shell/pipeline/Pipeline.scala shell/src/main/scala/diskrement/shell/pipeline/PipelineLive.scala shell/src/test/scala/diskrement/shell/pipeline/PipelineScanResetSpec.scala
git commit -m "$(cat <<'EOF'
feat(shell): add Pipeline scan and reset

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 6: `Pipeline.plan`

**Files:**
- Modify: `shell/src/main/scala/diskrement/shell/pipeline/Pipeline.scala`
- Modify: `shell/src/main/scala/diskrement/shell/pipeline/PipelineLive.scala`
- Test: `shell/src/test/scala/diskrement/shell/pipeline/PipelinePlanSpec.scala`

- [ ] **Step 1: Write the failing test**

`shell/src/test/scala/diskrement/shell/pipeline/PipelinePlanSpec.scala`:

```scala
package diskrement.shell.pipeline

import diskrement.core._
import diskrement.shell.model._
import zio._
import zio.test._
import java.time.Instant

object PipelinePlanSpec extends ZIOSpecDefault:
  private def initOf(name: String, src: java.nio.file.Path, work: java.nio.file.Path) =
    ProjectInit(name, src.toString, work.toString, DiscType.BD_R, 2.0, 1,
      Instant.parse("2026-06-01T00:00:00Z"))

  def spec = suite("Pipeline plan")(
    test("plan from a staged scan places all files on one new disc and stages the plan"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.txt" -> "aaa", "b.txt" -> "bb"))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          _     <- pipe.scan("p", verify = false, force = false)
          plan  <- pipe.plan("p", discTypeOverride = None)
          staged<- ZIO.serviceWithZIO[StagedStore](_.readPlan(paths.workDir))
        yield assertTrue(
          plan.discs.size == 1,
          plan.discs.head.isNew,
          plan.discs.head.files.map(_.relPath.value).sorted == List("a.txt", "b.txt"),
          plan.oversized.isEmpty,
          staged.map(_._2) == Some(DiscType.BD_R))
      },
    test("plan fails NoStagedScan when scan has not run"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.txt" -> "aaa"))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          err   <- pipe.plan("p", None).flip
        yield assertTrue(err.isInstanceOf[PipelineError.NoStagedScan])
      }
  ).provideLayer(PipelineTestKit.env.orDie)
```

- [ ] **Step 2: Run it to verify it fails**

Run: `sbt "shell/testOnly diskrement.shell.pipeline.PipelinePlanSpec"`
Expected: FAIL — `plan` not a member.

- [ ] **Step 3: Add `plan` to the trait**

In `Pipeline.scala`, add to `trait Pipeline`:

```scala
  def plan(name: String, discTypeOverride: Option[DiscType]): IO[PipelineError, BurnPlan]
```

(`BurnPlan`/`DiscType` come from `diskrement.core` — ensure the import in `Pipeline.scala` includes them.)

- [ ] **Step 4: Implement `plan` in `PipelineLive.scala`**

```scala
  def plan(name: String, discTypeOverride: Option[DiscType]): IO[PipelineError, BurnPlan] =
    for
      p       <- project(name)
      scanned <- staged.readScan(paths.workDir).flatMap {
                   case Some(s) => ZIO.succeed(s)
                   case None    => ZIO.fail(PipelineError.NoStagedScan(name))
                 }
      view    <- catalog.readView(p.id).mapError(PipelineError.Failed(_))
      states  <- catalog.readDiscStates(p.id).mapError(PipelineError.Failed(_))
      discType = discTypeOverride.getOrElse(p.defaultDiscType)
      cap      = Capacity.fromDiscType(discType, p.marginPercent)
      cs       = Diff.diff(view, scanned)
      burnPlan = Planner.plan(cs, states, cap)
      _       <- staged.writePlan(paths.workDir, burnPlan, discType)
    yield burnPlan
```

- [ ] **Step 5: Run it to verify it passes**

Run: `sbt "shell/testOnly diskrement.shell.pipeline.PipelinePlanSpec"`
Expected: `2 tests passed`.

- [ ] **Step 6: Commit**

```bash
git add shell/src/main/scala/diskrement/shell/pipeline/Pipeline.scala shell/src/main/scala/diskrement/shell/pipeline/PipelineLive.scala shell/src/test/scala/diskrement/shell/pipeline/PipelinePlanSpec.scala
git commit -m "$(cat <<'EOF'
feat(shell): add Pipeline plan

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 7: `Pipeline.burn`

**Files:**
- Modify: `shell/src/main/scala/diskrement/shell/pipeline/Pipeline.scala`
- Modify: `shell/src/main/scala/diskrement/shell/pipeline/PipelineLive.scala`
- Test: `shell/src/test/scala/diskrement/shell/pipeline/PipelineBurnSpec.scala`

- [ ] **Step 1: Write the failing test**

`shell/src/test/scala/diskrement/shell/pipeline/PipelineBurnSpec.scala`:

```scala
package diskrement.shell.pipeline

import diskrement.core._
import diskrement.shell.catalog.CatalogStore
import diskrement.shell.model._
import zio._
import zio.test._
import java.nio.file.Files
import java.time.Instant

object PipelineBurnSpec extends ZIOSpecDefault:
  private def initOf(name: String, src: java.nio.file.Path, work: java.nio.file.Path) =
    ProjectInit(name, src.toString, work.toString, DiscType.BD_R, 2.0, 1,
      Instant.parse("2026-06-01T00:00:00Z"))

  def spec = suite("Pipeline burn")(
    test("burn stages discs, writes /.diskrement, and commits the catalog"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.txt" -> "aaa", "sub/b.txt" -> "bbbb"))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          _     <- pipe.scan("p", verify = false, force = false)
          _     <- pipe.plan("p", None)
          rep   <- pipe.burn("p")
          d0     = rep.discs.head
          dataA <- ZIO.attempt(Files.readString(d0.stagingDir.resolve("data/a.txt")))
          hasManifest <- ZIO.attempt(Files.exists(d0.stagingDir.resolve(".diskrement/manifest.json")))
          hasCatalog  <- ZIO.attempt(Files.exists(d0.stagingDir.resolve(".diskrement/catalog.sqlite")))
          proj  <- ZIO.serviceWithZIO[CatalogStore](_.findProject("p"))
          view  <- ZIO.serviceWithZIO[CatalogStore](_.readView(proj.get.id))
          states<- ZIO.serviceWithZIO[CatalogStore](_.readDiscStates(proj.get.id))
        yield assertTrue(
          rep.discs.size == 1, rep.oversized.isEmpty,
          dataA == "aaa", hasManifest, hasCatalog,
          view.current.keySet.map(_.value) == Set("a.txt", "sub/b.txt"),
          states.size == 1, states.head.usedBytes == 7L)
      },
    test("burn fails NoStagedPlan when plan has not run"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.txt" -> "aaa"))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          _     <- pipe.scan("p", verify = false, force = false)
          err   <- pipe.burn("p").flip
        yield assertTrue(err.isInstanceOf[PipelineError.NoStagedPlan])
      }
  ).provideLayer(PipelineTestKit.env.orDie)
```

- [ ] **Step 2: Run it to verify it fails**

Run: `sbt "shell/testOnly diskrement.shell.pipeline.PipelineBurnSpec"`
Expected: FAIL — `burn` not a member.

- [ ] **Step 3: Add `burn` to the trait**

In `Pipeline.scala`, add to `trait Pipeline`:

```scala
  def burn(name: String): IO[PipelineError, BurnReport]
```

- [ ] **Step 4: Implement `burn` in `PipelineLive.scala`**

Add imports: `diskrement.shell.burn.{DiscIdentity, DiscImage, DiscMetadata}`, `diskrement.shell.fs.SnapshotHandle`, `zio.json._` (for the manifest hash), `diskrement.core.ManifestBuilder`. Then:

```scala
  def burn(name: String): IO[PipelineError, BurnReport] =
    for
      p           <- project(name)
      scanned     <- staged.readScan(paths.workDir).flatMap {
                       case Some(s) => ZIO.succeed(s); case None => ZIO.fail(PipelineError.NoStagedScan(name)) }
      planAndType <- staged.readPlan(paths.workDir).flatMap {
                       case Some(pt) => ZIO.succeed(pt); case None => ZIO.fail(PipelineError.NoStagedPlan(name)) }
      (burnPlan, discType) = planAndType
      _           <- ZIO.when(burnPlan.oversized.nonEmpty)(
                       ZIO.fail(PipelineError.OversizedFiles(burnPlan.oversized.map(_.relPath))))
      snapRoot     = paths.workDir.resolve("snapshot")
      _           <- ZIO.attemptBlockingIO(Files.exists(snapRoot)).mapError(PipelineError.Failed(_))
                       .flatMap(e => ZIO.unless(e)(ZIO.fail(PipelineError.SnapshotMissing(name))).unit)
      view        <- catalog.readView(p.id).mapError(PipelineError.Failed(_))
      changeSet    = Diff.diff(view, scanned)
      now         <- Clock.instant
      tmpCatalog   = paths.workDir.resolve("staged").resolve("burn-catalog.sqlite")
      _           <- ZIO.attemptBlockingIO {
                       Files.createDirectories(tmpCatalog.getParent); Files.deleteIfExists(tmpCatalog) }
                       .mapError(PipelineError.Failed(_))
      _           <- catalog.snapshotTo(tmpCatalog).mapError(PipelineError.Failed(_))
      pointers    <- catalog.archivePointers(p.id).mapError(PipelineError.Failed(_))
      existing    <- existingSessions(p.id)
      discCount    = burnPlan.discs.map(_.ordinal).maxOption.getOrElse(0)
      lastIdx      = burnPlan.discs.size - 1
      // stage each disc
      reports     <- ZIO.foreach(burnPlan.discs.zipWithIndex) { case (d, i) =>
                       val tombs   = if i == lastIdx then burnPlan.tombstones else Nil
                       val sessOrd = existing.getOrElse(d.ordinal, 0) + 1
                       val manifest = ManifestBuilder.build(name, sessOrd, d, changeSet, tombs)
                       val identity = DiscIdentity(name, d.ordinal, discCount, sessOrd, now)
                       val meta  = DiscMetadata(identity, manifest, tmpCatalog, pointers)
                       val image = DiscImage(
                         paths.workDir.resolve("staging").resolve(s"disc-${d.ordinal}"), snapRoot, d.files, meta)
                       burner.burn(image).mapError(PipelineError.Failed(_))
                         .map(r => DiscBurn(d.ordinal, r.stagingDir, r.imageBytes))
                     }
      // commit once
      update       = CatalogUpdate(p.id, Nil, burnPlan.discs.zipWithIndex.map { case (d, i) =>
                       val tombs   = if i == lastIdx then burnPlan.tombstones else Nil
                       val sessOrd = existing.getOrElse(d.ordinal, 0) + 1
                       val manifest = ManifestBuilder.build(name, sessOrd, d, changeSet, tombs)
                       toDiscWrite(name, d, discType, sessOrd, now, changeSet, tombs, manifest)
                     })
      _           <- catalog.applyUpdate(update).mapError(PipelineError.Failed(_))
      _           <- snapshot.release(SnapshotHandle(snapRoot))
      _           <- staged.clear(paths.workDir)
    yield BurnReport(reports, burnPlan.oversized.map(_.relPath))

  /** Highest existing session ordinal per disc ordinal (for the next ordinal). */
  private def existingSessions(pid: ProjectId): IO[PipelineError, Map[Int, Int]] =
    for
      b <- catalog.sessionsByState(pid, SessionState.BURNED).mapError(PipelineError.Failed(_))
      v <- catalog.sessionsByState(pid, SessionState.VERIFIED).mapError(PipelineError.Failed(_))
    yield (b ++ v).groupBy(_.discOrdinal).view.mapValues(_.map(_.ordinal).max).toMap

  private def toDiscWrite(
      name: String, d: PlannedDisc, discType: DiscType, sessOrd: Int, now: java.time.Instant,
      cs: ChangeSet, tombs: List[RelPath], manifest: SessionManifest): DiscWrite =
    val addedPaths = cs.added.iterator.map(_.relPath).toSet
    val (addedFiles, modifiedFiles) = d.files.partition(f => addedPaths.contains(f.relPath))
    def fw(f: ScannedFile) =
      FileWrite(f.relPath, f.contentHash, f.sizeBytes, f.mtime, "/data/" + f.relPath.value, 0)
    DiscWrite(
      ordinal = d.ordinal, isNew = d.isNew, discType = discType, usableBytes = d.usableBytes,
      status = if d.willFinalize then DiscStatus.FINALIZED else DiscStatus.OPEN,
      label = s"$name disc ${d.ordinal}",
      session = SessionWrite(sessOrd, now, manifestHash(manifest),
        addedFiles.map(fw), modifiedFiles.map(fw), tombs))

  private def manifestHash(m: SessionManifest): String =
    import diskrement.shell.burn.ManifestJson
    val json = ManifestJson.from(m).toJson
    java.security.MessageDigest.getInstance("SHA-256")
      .digest(json.getBytes("UTF-8")).map(b => f"${b & 0xff}%02x").mkString
```

> The added/modified split and `fw` mapping use the scanned files' `mtime` (the core `ManifestEntry` carries no mtime, but `PlannedDisc.files` are `ScannedFile`s, which do). Tombstones go only on the last disc.

- [ ] **Step 5: Run it to verify it passes**

Run: `sbt "shell/testOnly diskrement.shell.pipeline.PipelineBurnSpec"`
Expected: `2 tests passed`.

- [ ] **Step 6: Commit**

```bash
git add shell/src/main/scala/diskrement/shell/pipeline/Pipeline.scala shell/src/main/scala/diskrement/shell/pipeline/PipelineLive.scala shell/src/test/scala/diskrement/shell/pipeline/PipelineBurnSpec.scala
git commit -m "$(cat <<'EOF'
feat(shell): add Pipeline burn (stage discs + commit catalog)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 8: `Pipeline.verify`

**Files:**
- Modify: `shell/src/main/scala/diskrement/shell/pipeline/Pipeline.scala`
- Modify: `shell/src/main/scala/diskrement/shell/pipeline/PipelineLive.scala`
- Test: `shell/src/test/scala/diskrement/shell/pipeline/PipelineVerifySpec.scala`

- [ ] **Step 1: Write the failing test**

`shell/src/test/scala/diskrement/shell/pipeline/PipelineVerifySpec.scala`:

```scala
package diskrement.shell.pipeline

import diskrement.core._
import diskrement.shell.model._
import zio._
import zio.test._
import java.nio.file.Files
import java.time.Instant

object PipelineVerifySpec extends ZIOSpecDefault:
  private def initOf(name: String, src: java.nio.file.Path, work: java.nio.file.Path) =
    ProjectInit(name, src.toString, work.toString, DiscType.BD_R, 2.0, 1,
      Instant.parse("2026-06-01T00:00:00Z"))

  def spec = suite("Pipeline verify")(
    test("verify of a clean burned disc reports no mismatches and marks it VERIFIED"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.txt" -> "aaa"))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          _     <- pipe.scan("p", verify = false, force = false)
          _     <- pipe.plan("p", None)
          _     <- pipe.burn("p")
          reps  <- pipe.verify("p", None)
          proj  <- ZIO.serviceWithZIO[CatalogStore](_.findProject("p"))
          ver   <- ZIO.serviceWithZIO[CatalogStore](_.sessionsByState(proj.get.id, SessionState.VERIFIED))
        yield assertTrue(
          reps.size == 1, reps.head.mismatches.isEmpty, reps.head.filesChecked == 1,
          ver.map(_.discOrdinal) == List(1))
      },
    test("verify detects a corrupted staged file"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.txt" -> "aaa"))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          _     <- pipe.scan("p", verify = false, force = false)
          _     <- pipe.plan("p", None)
          rep   <- pipe.burn("p")
          _     <- ZIO.attempt(Files.writeString(rep.discs.head.stagingDir.resolve("data/a.txt"), "CORRUPT"))
          reps  <- pipe.verify("p", None)
        yield assertTrue(reps.head.mismatches.map(_.value) == List("a.txt"))
      }
  ).provideLayer(PipelineTestKit.env.orDie)
```

- [ ] **Step 2: Run it to verify it fails**

Run: `sbt "shell/testOnly diskrement.shell.pipeline.PipelineVerifySpec"`
Expected: FAIL — `verify` not a member.

- [ ] **Step 3: Add `verify` to the trait**

In `Pipeline.scala`, add to `trait Pipeline`:

```scala
  def verify(name: String, discOrdinal: Option[Int]): IO[PipelineError, List[VerifyReport]]
```

- [ ] **Step 4: Implement `verify` in `PipelineLive.scala`**

`verify` re-reads each burned disc's staging `/data` tree, re-hashes each file, compares against the catalog's current content hash for that path, and marks a clean disc VERIFIED. Uses `scala.jdk.StreamConverters.*` (add the import) for the directory walk.

```scala
  def verify(name: String, discOrdinal: Option[Int]): IO[PipelineError, List[VerifyReport]] =
    for
      p      <- project(name)
      view   <- catalog.readView(p.id).mapError(PipelineError.Failed(_))
      states <- catalog.readDiscStates(p.id).mapError(PipelineError.Failed(_))
      sessions <- catalog.sessionsByState(p.id, SessionState.BURNED).mapError(PipelineError.Failed(_))
      targets  = states.map(_.ordinal).filter(o => discOrdinal.forall(_ == o))
      reps   <- ZIO.foreach(targets)(o => verifyDisc(p.id, o, view, sessions))
    yield reps

  private def verifyDisc(
      pid: ProjectId, ordinal: Int, view: CatalogView, sessions: List[Session]
  ): IO[PipelineError, VerifyReport] =
    val dataDir = paths.workDir.resolve("staging").resolve(s"disc-$ordinal").resolve("data")
    for
      files <- ZIO.attemptBlockingIO {
                 import scala.jdk.StreamConverters.*
                 if Files.exists(dataDir) then
                   val s = Files.walk(dataDir)
                   try s.toScala(List).filter(p => Files.isRegularFile(p))
                   finally s.close()
                 else Nil
               }.mapError(PipelineError.Failed(_))
      mismatches <- ZIO.foreach(files) { f =>
                      val rel = RelPath(dataDir.relativize(f).toString)
                      hasher.hash(f).mapError(PipelineError.Failed(_)).map { h =>
                        Option.when(!view.current.get(rel).exists(_.contentHash == h))(rel)
                      }
                    }.map(_.flatten)
      _ <- ZIO.when(mismatches.isEmpty)(
             sessions.filter(_.discOrdinal == ordinal).headOption match
               case Some(s) => catalog.markVerified(pid, ordinal, s.ordinal).mapError(PipelineError.Failed(_))
               case None    => ZIO.unit)
    yield VerifyReport(ordinal, files.size, mismatches)
```

- [ ] **Step 5: Run it to verify it passes**

Run: `sbt "shell/testOnly diskrement.shell.pipeline.PipelineVerifySpec"`
Expected: `2 tests passed`.

- [ ] **Step 6: Commit**

```bash
git add shell/src/main/scala/diskrement/shell/pipeline/Pipeline.scala shell/src/main/scala/diskrement/shell/pipeline/PipelineLive.scala shell/src/test/scala/diskrement/shell/pipeline/PipelineVerifySpec.scala
git commit -m "$(cat <<'EOF'
feat(shell): add Pipeline verify (re-hash staged discs, mark VERIFIED)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 9: Full-loop integration test + final green sweep

**Files:**
- Test: `shell/src/test/scala/diskrement/shell/pipeline/PipelineFullLoopSpec.scala`

- [ ] **Step 1: Write the full-loop test**

`shell/src/test/scala/diskrement/shell/pipeline/PipelineFullLoopSpec.scala`:

```scala
package diskrement.shell.pipeline

import diskrement.core._
import diskrement.shell.model._
import zio._
import zio.test._
import java.nio.file.Files
import java.time.Instant

object PipelineFullLoopSpec extends ZIOSpecDefault:
  private def initOf(name: String, src: java.nio.file.Path, work: java.nio.file.Path) =
    ProjectInit(name, src.toString, work.toString, DiscType.BD_R, 2.0, 1,
      Instant.parse("2026-06-01T00:00:00Z"))

  def spec = suite("Pipeline full loop")(
    test("init -> scan -> plan -> burn -> modify -> scan -> plan -> burn -> verify -> history"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.txt" -> "aaa", "b.txt" -> "bbb"))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          // first burn
          s1    <- pipe.scan("p", verify = false, force = false)
          _     <- pipe.plan("p", None)
          _     <- pipe.burn("p")
          // modify source: change a.txt, delete b.txt, add c.txt
          _     <- ZIO.attempt {
                     Files.writeString(src.resolve("a.txt"), "aaaa")
                     Files.deleteIfExists(src.resolve("b.txt"))
                     Files.writeString(src.resolve("c.txt"), "cc")
                   }
          // second burn (force a fresh scan since a staged scan exists)
          s2    <- pipe.scan("p", verify = false, force = true)
          _     <- pipe.plan("p", Some(DiscType.BD_R_DL))
          _     <- pipe.burn("p")
          reps  <- pipe.verify("p", None)
          hA    <- pipe.history("p", RelPath("a.txt"))
          hB    <- pipe.history("p", RelPath("b.txt"))
          proj  <- ZIO.serviceWithZIO[CatalogStore](_.findProject("p"))
          view  <- ZIO.serviceWithZIO[CatalogStore](_.readView(proj.get.id))
        yield assertTrue(
          s1 == ScanResult(2, 0, 0, 0),
          s2 == ScanResult(added = 1, modified = 1, deleted = 1, unchanged = 0),
          view.current.keySet.map(_.value) == Set("a.txt", "c.txt"),
          reps.forall(_.mismatches.isEmpty),
          hA.map(_.status) == List(VersionStatus.SUPERSEDED, VersionStatus.CURRENT),
          hB.map(_.status) == List(VersionStatus.DELETED))
      }
  ).provideLayer(PipelineTestKit.env.orDie)
```

- [ ] **Step 2: Run the full-loop test**

Run: `sbt "shell/testOnly diskrement.shell.pipeline.PipelineFullLoopSpec"`
Expected: `1 tests passed`. If it fails, the bug is in the orchestration wiring (most likely the burn assembly or the second scan/diff) — debug with superpowers:systematic-debugging; do not weaken the assertions.

- [ ] **Step 3: Run the entire suite green (both modules)**

Run: `sbt clean core/test shell/test`
Expected: core `Passed: Total 50, Failed 0`; shell all specs pass (Plan 2c left 41; this plan adds: FileHistory `1` + MarkVerified `2` + CheckCowCapable `2` + StagedStore `3` + InitStatusHistory `3` + ScanReset `2` + Plan `2` + Burn `2` + Verify `2` + FullLoop `1` = `+20` → `61`). Fix any `-Wunused:all` warnings before finishing.

- [ ] **Step 4: Commit**

```bash
git add shell/src/test/scala/diskrement/shell/pipeline/PipelineFullLoopSpec.scala
git commit -m "$(cat <<'EOF'
test(shell): add full-loop pipeline integration test

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review (completed by plan author)

**Spec coverage (Plan 3a design):**
- §3 `Pipeline` interface + result types → Tasks 4–8 (trait grows; `ProjectPaths`/result types in Task 4).
- §4 operations: `init` (fs checks) → Task 4 + Task 2 (`checkCowCapable`); `scan`/`reset` → Task 5; `plan` → Task 6; `burn` (commit-after-success, DiscImage + CatalogUpdate assembly, snapshot + staging lifecycle) → Task 7; `status`/`history` → Task 4; `verify` → Task 8.
- §5 `StagedStore` (zio-json scan/plan, restage policy) → Task 3 (+ disc-type refinement).
- §6 `fileHistory`/`markVerified`/`checkCowCapable` → Tasks 1 + 2.
- §7 `init` filesystem checks → Task 4 (`sameFilesystem`) + Task 2.
- §8 `PipelineError` ADT → Task 3.
- §9 testing incl. the full-loop test → per-task tests + Task 9.

**Correctly deferred (Plan 3b):** CLI arg parsing/dispatch, interactive prompts, output formatting + exit codes, XDG/systemd path resolution.

**Type consistency:** `ProjectPaths`, the result types, and `PipelineLive`'s constructor (`catalog, scanner, snapshot, burner, staged, hasher, paths`) match between `Pipeline.scala`, `PipelineLive.scala`, and `PipelineTestKit.mkPipeline`. `StagedStore.writePlan(workDir, BurnPlan, DiscType)` / `readPlan → Option[(BurnPlan, DiscType)]` are used consistently in Tasks 5–7. `fileHistory`/`markVerified`/`checkCowCapable` signatures match trait↔impl↔tests. The `burn` assembly uses `ManifestBuilder.build`, `CatalogUpdate`/`DiscWrite`/`SessionWrite`/`FileWrite` (Plan 2a), and `DiscImage`/`DiscMetadata`/`DiscIdentity` (Plan 2c) exactly as those types are defined.

**No placeholders:** every step has concrete code + commands. The one illustrative shorthand in Task 7 Step 1 is explicitly flagged and immediately followed by the concrete test body to use. The disc-type-in-staged-plan and model-location refinements are called out at the top.
