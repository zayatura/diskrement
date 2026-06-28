# Incremental Per-Disc Burn Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `plan`/`burn` operate one disc per `scan → plan → burn` cycle (catalog as cursor), with finalize-on-spill, a `plan --finalize` override, and operator label guidance at burn time.

**Architecture:** The pure `core/Planner` is unchanged — its existing `willFinalize = (i != lastIdx)` already means "the first disc finalizes iff the plan spilled." `Pipeline.plan` runs the full planner but **consumes only `discs.head`**, estimates the rest from `discs.tail`, and stages a single-disc `PlanReport`. `Pipeline.burn` writes that one disc and commits exactly one session. The CLI gains a `--finalize` flag and renders the single-disc plan + operator guidance.

**Tech Stack:** Scala 3.3.4, ZIO 2.1.19, zio-json, zio-cli, zio-test, SQLite (zio-jdbc). sbt modules `core` → `shell` → `cli`.

**Design spec:** `docs/superpowers/specs/2026-06-27-diskrement-incremental-per-disc-burn-design.md`

---

## File structure

**Modified — `shell`:**
- `shell/.../burn/Burn.scala` — drop `discCount` from `DiscIdentity`.
- `shell/.../burn/Json.scala` — drop `discCount` from `ProjectJson`.
- `shell/.../pipeline/Pipeline.scala` — new result types `RemainingEstimate`, `PlanReport`, reshaped `DiscBurn`/`BurnReport`; `plan` gains a `finalize` param.
- `shell/.../pipeline/PipelineLive.scala` — single-disc `plan` + single-session `burn`; remove the multi-disc loop and `DiscPlanData`/`discCount`.
- `shell/.../pipeline/StagedStore.scala` — stage a `PlanReport` instead of `(BurnPlan, DiscType)`.

**Modified — `cli`:**
- `cli/.../Args.scala` — `Plan` gains `finalize: Boolean`.
- `cli/.../Commands.scala` — `plan` adds `--finalize`.
- `cli/.../Render.scala` — `plan(PlanReport)` and `burn(BurnReport)` reworked.
- `cli/.../Runner.scala` — thread `finalize`; new `Render.plan` arity.

**Modified — tests:** `JsonSpec`, `StagedStoreSpec`, `PipelineBurnSpec`, `PipelineSpillSpec`, `PipelineFullLoopSpec`, `RenderSpec`, `CommandsSpec`, `CliSmokeSpec`.

**Unchanged:** `core/Planner.scala` and its tests.

---

## Task 1: Drop `discCount` from on-disc metadata

`discCount` ("disc M of N") is meaningless once the set grows one disc per cycle — total N is unknowable at burn time. Remove it from the embedded metadata. This task is self-contained and keeps the existing multi-disc burn loop compiling.

**Files:**
- Modify: `shell/src/main/scala/diskrement/shell/burn/Burn.scala:8-14`
- Modify: `shell/src/main/scala/diskrement/shell/burn/Json.scala:7-17`
- Modify: `shell/src/main/scala/diskrement/shell/pipeline/PipelineLive.scala:126,149`
- Test: `shell/src/test/scala/diskrement/shell/burn/JsonSpec.scala:13-17`

- [ ] **Step 1: Update the `JsonSpec` test to the 4-field shape (write the failing test)**

Replace the first test case (`ProjectJson maps from DiscIdentity and round-trips`) in `JsonSpec.scala`:

```scala
    test("ProjectJson maps from DiscIdentity and round-trips"):
      val pj = ProjectJson.from(DiscIdentity("photos", 2, 1, ts))
      assertTrue(
        pj == ProjectJson("photos", 2, 1, "2026-06-01T00:00:00Z"),
        pj.toJson.fromJson[ProjectJson] == Right(pj)),
```

- [ ] **Step 2: Run the test to verify it fails to compile**

Run: `sbt "shell/testOnly diskrement.shell.burn.JsonSpec"`
Expected: FAIL — compile error, `DiscIdentity`/`ProjectJson` still take 5 args.

- [ ] **Step 3: Drop `discCount` from `DiscIdentity`**

In `Burn.scala`, change the case class:

```scala
final case class DiscIdentity(
    projectName: String,
    discOrdinal: Int,
    sessionOrdinal: Int,
    createdAt: Instant
)
```

- [ ] **Step 4: Drop `discCount` from `ProjectJson`**

In `Json.scala`, change the DTO and its mapper:

```scala
final case class ProjectJson(
    projectName: String,
    discOrdinal: Int,
    sessionOrdinal: Int,
    createdAt: String
)
object ProjectJson:
  given JsonCodec[ProjectJson] = DeriveJsonCodec.gen[ProjectJson]
  def from(id: DiscIdentity): ProjectJson =
    ProjectJson(id.projectName, id.discOrdinal, id.sessionOrdinal, id.createdAt.toString)
```

- [ ] **Step 5: Remove `discCount` from `PipelineLive`**

In `PipelineLive.scala`, delete the `discCount` binding (currently line 126):

```scala
      discCount    = burnPlan.discs.map(_.ordinal).maxOption.getOrElse(0)
```

and update the `DiscIdentity` construction (currently line 149) to drop the `discCount` argument:

```scala
                       val identity = DiscIdentity(name, dd.disc.ordinal, dd.sessOrd, now)
```

- [ ] **Step 6: Run the JSON test to verify it passes**

Run: `sbt "shell/testOnly diskrement.shell.burn.JsonSpec"`
Expected: PASS (3 tests).

- [ ] **Step 7: Compile the shell module to confirm nothing else broke**

Run: `sbt shell/compile`
Expected: success.

- [ ] **Step 8: Commit**

```bash
git add shell/src/main/scala/diskrement/shell/burn/Burn.scala \
        shell/src/main/scala/diskrement/shell/burn/Json.scala \
        shell/src/main/scala/diskrement/shell/pipeline/PipelineLive.scala \
        shell/src/test/scala/diskrement/shell/burn/JsonSpec.scala
git commit -m "refactor(burn): drop discCount from on-disc metadata

Total set size is unknowable in an incremental per-disc model.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 2: Single-disc `plan` + single-session `burn` (shell)

The core behavioural change. New report types, a single-disc `PlanReport` staged by `plan`, and a `burn` that writes exactly one disc and commits one session. The pure `Planner` is **not** touched. Because this changes types across `Pipeline`, `PipelineLive`, and `StagedStore`, the source files move together and the shell tests are updated in the same task; compilation only goes green once all of them are in place.

**Files:**
- Modify: `shell/src/main/scala/diskrement/shell/pipeline/Pipeline.scala:19-37`
- Modify: `shell/src/main/scala/diskrement/shell/pipeline/StagedStore.scala`
- Modify: `shell/src/main/scala/diskrement/shell/pipeline/PipelineLive.scala`
- Test: `shell/src/test/scala/diskrement/shell/pipeline/StagedStoreSpec.scala:14-44`
- Test: `shell/src/test/scala/diskrement/shell/pipeline/PipelineBurnSpec.scala`
- Test: `shell/src/test/scala/diskrement/shell/pipeline/PipelineSpillSpec.scala`
- Test: `shell/src/test/scala/diskrement/shell/pipeline/PipelineFullLoopSpec.scala`

- [ ] **Step 1: Add the new result types and `plan`/`burn` signatures in `Pipeline.scala`**

Replace the result-type block (currently lines 19-25) with:

```scala
final case class ScanResult(added: Int, modified: Int, deleted: Int, unchanged: Int)

/** How much outstanding change did NOT fit on the disc planned this cycle. */
final case class RemainingEstimate(moreDiscs: Int, moreFiles: Int, moreBytes: Long)

/** The single disc this cycle will write, plus what spills to later cycles. */
final case class PlanReport(
    disc: PlannedDisc,
    discType: DiscType,
    tombstones: List[RelPath],
    remaining: RemainingEstimate,
    oversized: List[ScannedFile])

final case class DiscBurn(ordinal: Int, label: String, finalized: Boolean, stagingDir: Path, imageBytes: Long)
final case class BurnReport(disc: DiscBurn, remaining: RemainingEstimate)

final case class StatusReport(
    discs: List[DiscState], openDiscOrdinal: Option[Int],
    totalCurrentFiles: Int, lastBurnAt: Option[Instant])
final case class VerifyReport(discOrdinal: Int, filesChecked: Int, mismatches: List[RelPath])
```

Then change the two trait methods (currently lines 35-36):

```scala
  def plan(name: String, discTypeOverride: Option[DiscType], finalize: Boolean): IO[PipelineError, PlanReport]
  def burn(name: String): IO[PipelineError, BurnReport]
```

- [ ] **Step 2: Stage a `PlanReport` in `StagedStore`**

In `StagedStore.scala`, change the trait methods (currently lines 13-14):

```scala
  def writePlan(workDir: Path, plan: PlanReport): IO[PipelineError, Unit]
  def readPlan(workDir: Path): IO[PipelineError, Option[PlanReport]]
```

Delete the `BurnPlanJson` DTO + object (currently lines 41-48) and replace with `RemainingJson` + `PlanReportJson` (reusing the existing `PlannedDiscJson` and `ScannedFileJson`):

```scala
private[pipeline] final case class RemainingJson(moreDiscs: Int, moreFiles: Int, moreBytes: Long)
private[pipeline] object RemainingJson:
  given JsonCodec[RemainingJson] = DeriveJsonCodec.gen[RemainingJson]

private[pipeline] final case class PlanReportJson(
    disc: PlannedDiscJson, discType: String, tombstones: List[String],
    remaining: RemainingJson, oversized: List[ScannedFileJson])
private[pipeline] object PlanReportJson:
  given JsonCodec[PlanReportJson] = DeriveJsonCodec.gen[PlanReportJson]
  def from(p: PlanReport): PlanReportJson =
    PlanReportJson(
      PlannedDiscJson.from(p.disc), p.discType.toString, p.tombstones.map(_.value),
      RemainingJson(p.remaining.moreDiscs, p.remaining.moreFiles, p.remaining.moreBytes),
      p.oversized.map(ScannedFileJson.from))
  def to(j: PlanReportJson): PlanReport =
    PlanReport(
      PlannedDiscJson.to(j.disc), DiscType.valueOf(j.discType), j.tombstones.map(RelPath(_)),
      RemainingEstimate(j.remaining.moreDiscs, j.remaining.moreFiles, j.remaining.moreBytes),
      j.oversized.map(ScannedFileJson.to))
```

In `StagedStoreLive`, replace `writePlan`/`readPlan` (currently lines 66-72):

```scala
  def writePlan(workDir: Path, plan: PlanReport): IO[PipelineError, Unit] =
    write(planFile(workDir), PlanReportJson.from(plan).toJson)

  def readPlan(workDir: Path): IO[PipelineError, Option[PlanReport]] =
    read(planFile(workDir)).map(_.map(s =>
      s.fromJson[PlanReportJson].fold(
        e => throw new RuntimeException(s"corrupt plan.json: $e"),
        PlanReportJson.to)))
```

- [ ] **Step 3: Rewrite `plan` in `PipelineLive` to stage one disc**

In `PipelineLive.scala`, replace the whole `plan` method (currently lines 93-107):

```scala
  def plan(name: String, discTypeOverride: Option[DiscType], finalize: Boolean): IO[PipelineError, PlanReport] =
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
      full     = Planner.plan(cs, states, cap)
      // The planner already finalizes the first disc iff the plan spilled; the
      // --finalize flag forces a seal even when everything fit.
      head       = full.discs.head
      sealedDisc = head.copy(willFinalize = head.willFinalize || finalize)
      tail       = full.discs.tail
      remaining  = RemainingEstimate(
                     tail.size, tail.map(_.files.size).sum, tail.flatMap(_.files).map(_.sizeBytes).sum)
      report     = PlanReport(sealedDisc, discType, full.tombstones, remaining, full.oversized)
      _       <- staged.writePlan(paths.workDir, report)
    yield report
```

- [ ] **Step 4: Rewrite `burn` in `PipelineLive` to write one disc / commit one session**

Replace the whole `burn` method (currently lines 109-165) and delete the `DiscPlanData` case class above the class (currently lines 13-16):

```scala
  def burn(name: String): IO[PipelineError, BurnReport] =
    for
      p         <- project(name)
      scanned   <- staged.readScan(paths.workDir).flatMap {
                     case Some(s) => ZIO.succeed(s); case None => ZIO.fail(PipelineError.NoStagedScan(name)) }
      report    <- staged.readPlan(paths.workDir).flatMap {
                     case Some(r) => ZIO.succeed(r); case None => ZIO.fail(PipelineError.NoStagedPlan(name)) }
      _         <- ZIO.when(report.oversized.nonEmpty)(
                     ZIO.fail(PipelineError.OversizedFiles(report.oversized.map(_.relPath))))
      snapRoot   = paths.workDir.resolve("snapshot")
      _         <- ZIO.attemptBlockingIO(Files.exists(snapRoot)).mapError(PipelineError.Failed(_))
                     .flatMap(e => ZIO.unless(e)(ZIO.fail(PipelineError.SnapshotMissing(name))).unit)
      view      <- catalog.readView(p.id).mapError(PipelineError.Failed(_))
      changeSet  = Diff.diff(view, scanned)
      now       <- Clock.instant
      existing  <- existingSessions(p.id)
      disc       = report.disc
      discType   = report.discType
      tombs      = report.tombstones
      sessOrd    = existing.getOrElse(disc.ordinal, 0) + 1
      manifest   = ManifestBuilder.build(name, sessOrd, disc, changeSet, tombs)
      label      = s"$name disc ${disc.ordinal}"
      update     = CatalogUpdate(p.id, Nil,
                     List(toDiscWrite(name, disc, discType, sessOrd, now, changeSet, tombs, manifest)))
      tmpCatalog = paths.workDir.resolve("burn-catalog.sqlite")
      _         <- ZIO.attemptBlockingIO {
                     Files.createDirectories(tmpCatalog.getParent); Files.deleteIfExists(tmpCatalog) }
                     .mapError(PipelineError.Failed(_))
      _         <- catalog.snapshotTo(tmpCatalog).mapError(PipelineError.Failed(_))
      // Materialize the post-burn state into the snapshot this disc embeds, so a
      // single disc fully describes the archive including its own session. Applies
      // to the copy only; the live catalog is committed after the disc is staged.
      _         <- CatalogStore.applyUpdateTo(tmpCatalog, update).mapError(PipelineError.Failed(_))
      pointers  <- catalog.archivePointers(p.id).mapError(PipelineError.Failed(_))
      identity   = DiscIdentity(name, disc.ordinal, sessOrd, now)
      meta       = DiscMetadata(identity, manifest, tmpCatalog, pointers)
      image      = DiscImage(
                     paths.workDir.resolve("staging").resolve(s"disc-${disc.ordinal}"),
                     snapRoot, disc.files, meta)
      burned    <- burner.burn(image).mapError(PipelineError.Failed(_))
      // Commit the single session only after the disc is staged — the crash-safe
      // commit point. The committed state equals the embedded snapshot.
      _         <- catalog.applyUpdate(update).mapError(PipelineError.Failed(_))
      _         <- snapshot.release(SnapshotHandle(snapRoot))
      _         <- staged.clear(paths.workDir)
      _         <- ZIO.attemptBlockingIO(Files.deleteIfExists(tmpCatalog)).mapError(PipelineError.Failed(_))
    yield BurnReport(
      DiscBurn(disc.ordinal, label, disc.willFinalize, burned.stagingDir, burned.imageBytes),
      report.remaining)
```

Note: the `import diskrement.shell.burn.{DiscIdentity, DiscImage, DiscMetadata, ManifestJson, SimulatedBurner}` line stays — all five are still used (`ManifestJson` in `manifestHash`).

- [ ] **Step 5: Update `StagedStoreSpec` to the `PlanReport` round-trip**

In `StagedStoreSpec.scala`, replace the `plan` value (currently lines 14-17) and the round-trip test (currently lines 36-44):

```scala
  private val plan = PlanReport(
    disc = PlannedDisc(1, isNew = true, willFinalize = false, scanned, 0L, 1_000L),
    discType = DiscType.BD_R_DL,
    tombstones = List(RelPath("c")),
    remaining = RemainingEstimate(moreDiscs = 1, moreFiles = 2, moreBytes = 30L),
    oversized = Nil)
```

```scala
    test("round-trips a staged plan report"):
      ZIO.scoped {
        for
          work <- makeWork
          ss    = StagedStoreLive()
          _    <- ss.writePlan(work, plan)
          got  <- ss.readPlan(work)
        yield assertTrue(got == Some(plan))
      },
```

- [ ] **Step 6: Update `PipelineBurnSpec` to the single-disc report shape**

In `PipelineBurnSpec.scala`, every `pipe.plan("p", None)` becomes `pipe.plan("p", None, finalize = false)`. Replace the first test's assertions block and the disc handles:

In test 1 (`burn stages discs...`), change `d0 = rep.discs.head` to `d0 = rep.disc`, and replace the final `assertTrue(...)`:

```scala
        yield assertTrue(
          dataA == "aaa", hasManifest, hasCatalog,
          rep.disc.ordinal == 1, rep.disc.finalized == false,
          view.current.keySet.map(_.value) == Set("a.txt", "sub/b.txt"),
          states.size == 1, states.head.usedBytes == 7L)
```

In test 2 (`burn embeds the post-burn catalog state...`), change `rep.discs.head.stagingDir` to `rep.disc.stagingDir`.

- [ ] **Step 7: Rewrite `PipelineSpillSpec` first test as a two-cycle burn**

In `PipelineSpillSpec.scala`, replace the first test (`two files larger than half capacity spill across two discs and both commit`) with a two-cycle version, and update the oversized test's `plan` call. Full new `spec`:

```scala
  def spec = suite("Pipeline spill / oversized")(
    test("a cycle plans one disc, finalizes on spill, and the next cycle burns the rest"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.txt" -> ("a" * 40), "b.txt" -> ("b" * 40)))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          // cycle 1: only the first file fits; the disc finalizes on spill
          _     <- pipe.scan("p", verify = false, force = false)
          plan1 <- pipe.plan("p", None, finalize = false)
          rep1  <- pipe.burn("p")
          proj  <- ZIO.serviceWithZIO[CatalogStore](_.findProject("p"))
          st1   <- ZIO.serviceWithZIO[CatalogStore](_.readDiscStates(proj.get.id))
          // cycle 2: the remaining file lands on a fresh disc
          _     <- pipe.scan("p", verify = false, force = true)
          plan2 <- pipe.plan("p", None, finalize = false)
          rep2  <- pipe.burn("p")
          st2   <- ZIO.serviceWithZIO[CatalogStore](_.readDiscStates(proj.get.id))
          view  <- ZIO.serviceWithZIO[CatalogStore](_.readView(proj.get.id))
        yield assertTrue(
          plan1.disc.files.size == 1,
          plan1.disc.willFinalize,
          plan1.remaining.moreDiscs == 1,
          rep1.disc.ordinal == 1, rep1.disc.finalized,
          st1.size == 1, st1.head.status == DiscStatus.FINALIZED,
          plan2.disc.ordinal == 2,
          plan2.remaining.moreDiscs == 0,
          rep2.disc.ordinal == 2, rep2.disc.finalized == false,
          st2.size == 2,
          view.current.keySet.map(_.value) == Set("a.txt", "b.txt"))
      },
    test("a file larger than any disc is surfaced by plan and rejected by burn"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("big.txt" -> ("x" * 100)))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          _     <- pipe.scan("p", verify = false, force = false)
          plan  <- pipe.plan("p", None, finalize = false)
          err   <- pipe.burn("p").flip
        yield assertTrue(
          plan.oversized.map(_.relPath.value) == List("big.txt"),
          err.isInstanceOf[PipelineError.OversizedFiles])
      }
  ).provideLayer(PipelineTestKit.env.orDie)
```

`DiscStatus` is already available via `import diskrement.core._` at the top of the file.

- [ ] **Step 8: Update `PipelineFullLoopSpec` and add a `--finalize` test**

In `PipelineFullLoopSpec.scala`, update the two `plan` calls in the existing test: `pipe.plan("p", None)` → `pipe.plan("p", None, finalize = false)` and `pipe.plan("p", Some(DiscType.BD_R_DL))` → `pipe.plan("p", Some(DiscType.BD_R_DL), finalize = false)`.

Then add this second test to the suite (after the existing one, before the closing `).provideLayer`):

```scala
    test("plan --finalize seals a non-full disc; the next cycle starts a fresh disc"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.txt" -> "aaa"))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          // cycle 1: everything fits, but --finalize seals disc 1 anyway
          _     <- pipe.scan("p", verify = false, force = false)
          plan1 <- pipe.plan("p", None, finalize = true)
          rep1  <- pipe.burn("p")
          // cycle 2: a new file must start a fresh disc, since disc 1 is sealed
          _     <- ZIO.attempt(Files.writeString(src.resolve("b.txt"), "bb"))
          _     <- pipe.scan("p", verify = false, force = true)
          plan2 <- pipe.plan("p", None, finalize = false)
          rep2  <- pipe.burn("p")
          proj  <- ZIO.serviceWithZIO[CatalogStore](_.findProject("p"))
          st    <- ZIO.serviceWithZIO[CatalogStore](_.readDiscStates(proj.get.id))
        yield assertTrue(
          plan1.disc.willFinalize,
          rep1.disc.finalized,
          plan2.disc.ordinal == 2,
          rep2.disc.ordinal == 2,
          st.map(_.ordinal).sorted == List(1, 2),
          st.find(_.ordinal == 1).get.status == DiscStatus.FINALIZED)
      }
```

`DiscStatus` and `Files` are already imported in this file.

- [ ] **Step 9: Run the full shell test suite**

Run: `sbt shell/test`
Expected: PASS — all shell suites green (`PipelineBurnSpec`, `PipelineSpillSpec`, `PipelineFullLoopSpec`, `StagedStoreSpec`, `JsonSpec`, and the rest).

- [ ] **Step 10: Commit**

```bash
git add shell/src/main/scala/diskrement/shell/pipeline/ \
        shell/src/test/scala/diskrement/shell/pipeline/
git commit -m "feat(pipeline): one disc per scan/plan/burn cycle

plan stages a single disc (finalize-on-spill, --finalize override) and
estimates the remainder; burn writes that disc and commits one session.
Planner is unchanged. The catalog is the cursor across cycles.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 3: CLI `--finalize` flag, rendering, and dispatch

Expose the new behaviour: a `plan --finalize` flag, single-disc plan rendering, and operator guidance at burn time.

**Files:**
- Modify: `cli/src/main/scala/diskrement/cli/Args.scala:14`
- Modify: `cli/src/main/scala/diskrement/cli/Commands.scala:50-55`
- Modify: `cli/src/main/scala/diskrement/cli/Render.scala:17-35`
- Modify: `cli/src/main/scala/diskrement/cli/Runner.scala:62-66`
- Test: `cli/src/test/scala/diskrement/cli/CommandsSpec.scala:39-45`
- Test: `cli/src/test/scala/diskrement/cli/RenderSpec.scala:24-57`
- Test: `cli/src/test/scala/diskrement/cli/CliSmokeSpec.scala:52,63`

- [ ] **Step 1: Update `CommandsSpec` to expect the `finalize` field (write the failing test)**

In `CommandsSpec.scala`, replace the `plan with optional disc-type override` test:

```scala
    test("plan with optional disc-type override and finalize flag") {
      for a <- parse("plan", "photos")
          b <- parse("plan", "photos", "--disc-type", "DVD")
          c <- parse("plan", "photos", "--finalize")
      yield assertTrue(
        a._2 == DiskrementCommand.Plan("photos", None, finalize = false),
        b._2 == DiskrementCommand.Plan("photos", Some(DiscType.DVD), finalize = false),
        c._2 == DiskrementCommand.Plan("photos", None, finalize = true))
    },
```

- [ ] **Step 2: Run the parser test to verify it fails to compile**

Run: `sbt "cli/testOnly diskrement.cli.CommandsSpec"`
Expected: FAIL — `DiskrementCommand.Plan` still takes 2 args.

- [ ] **Step 3: Add `finalize` to the `Plan` command ADT**

In `Args.scala`, change the `Plan` case:

```scala
  case Plan(name: String, discType: Option[DiscType], finalize: Boolean)
```

- [ ] **Step 4: Add the `--finalize` flag to the `plan` command**

In `Commands.scala`, replace the `plan` definition:

```scala
  // --- plan ---
  private val plan: Command[DiskrementCommand] =
    val discType = Options.enumeration("disc-type")(discTypeChoices*).optional
    val finalize = Options.boolean("finalize")
    Command("plan", discType ++ finalize, nameArg)
      .withHelp(HelpDoc.p("Plan the next disc from the staged scan."))
      .map { case ((dt, fin), name) => DiskrementCommand.Plan(name, dt, fin) }
```

- [ ] **Step 5: Run the parser test to verify it passes**

Run: `sbt "cli/testOnly diskrement.cli.CommandsSpec"`
Expected: PASS.

- [ ] **Step 6: Update `RenderSpec` plan/burn tests (write the failing tests)**

In `RenderSpec.scala`, replace the four tests `plan: discs with top-off/new...`, `plan: empty plan...`, `burn report`, and `burn report: oversized...` with:

```scala
    test("plan: single disc with remaining estimate") {
      val rep = PlanReport(
        disc = PlannedDisc(1, isNew = true, willFinalize = true, files = List(sf("a", 24_100_000_000L)),
          priorUsedBytes = 0L, usableBytes = 25_000_000_000L),
        discType = DiscType.BD_R, tombstones = Nil,
        remaining = RemainingEstimate(moreDiscs = 1, moreFiles = 1, moreBytes = 3_200_000_000L),
        oversized = Nil)
      val out = Render.plan(rep)
      assertTrue(
        out.contains("disc 1"), out.contains("BD_R"),
        out.contains("FINALIZE"), out.contains("more discs"))
    },
    test("plan: empty plan says nothing to burn") {
      val rep = PlanReport(
        PlannedDisc(1, isNew = false, willFinalize = false, Nil, 0L, 25_000_000_000L),
        DiscType.BD_R, Nil, RemainingEstimate(0, 0, 0), Nil)
      assertTrue(Render.plan(rep) == "Nothing to burn: no changes to plan.")
    },
    test("plan: oversized files are listed") {
      val rep = PlanReport(
        PlannedDisc(1, isNew = true, willFinalize = false, List(sf("a", 10L)), 0L, 25_000_000_000L),
        DiscType.BD_R, Nil, RemainingEstimate(0, 0, 0), List(sf("huge.bin", 99_000_000_000L)))
      val out = Render.plan(rep)
      assertTrue(out.contains("Oversized (cannot be placed):"), out.contains("huge.bin"))
    },
    test("burn: finalized disc, with more discs remaining") {
      val rep = BurnReport(
        DiscBurn(1, "photos disc 1", finalized = true, Paths.get("/w/disc-1"), 24_100_000_000L),
        RemainingEstimate(moreDiscs = 2, moreFiles = 50, moreBytes = 40_000_000_000L))
      val out = Render.burn(rep)
      assertTrue(
        out.contains("Wrote disc 1"), out.contains("photos disc 1"),
        out.contains("FINALIZED"), out.contains("more discs"))
    },
    test("burn: open disc, set up to date") {
      val rep = BurnReport(
        DiscBurn(1, "photos disc 1", finalized = false, Paths.get("/w/disc-1"), 1_000L),
        RemainingEstimate(0, 0, 0))
      val out = Render.burn(rep)
      assertTrue(out.contains("Wrote disc 1"), out.contains("OPEN"), out.contains("up to date"))
    },
```

- [ ] **Step 7: Run the render tests to verify they fail to compile**

Run: `sbt "cli/testOnly diskrement.cli.RenderSpec"`
Expected: FAIL — `Render.plan`/`Render.burn` still have the old shapes.

- [ ] **Step 8: Rework `Render.plan` and `Render.burn`**

In `Render.scala`, replace `plan` (lines 17-28) and `burn` (lines 30-35):

```scala
  def plan(p: PlanReport): String =
    val d = p.disc
    if d.files.isEmpty && p.tombstones.isEmpty && p.oversized.isEmpty then
      "Nothing to burn: no changes to plan."
    else
      val kind = if d.isNew then "new disc" else s"appending to open disc ${d.ordinal}"
      val fin  = if d.willFinalize then "will FINALIZE" else "left OPEN"
      val header =
        f"Plan: disc ${d.ordinal} ($kind, ${p.discType}) — ${d.files.size} files, ${gb(d.totalUsedBytes)}, $fin"
      val rem =
        if p.remaining.moreDiscs == 0 then "This disc completes the set."
        else s"~${p.remaining.moreFiles} more files (~${p.remaining.moreDiscs} more discs) remain after this one."
      val tomb = if p.tombstones.isEmpty then Nil else List(s"Deletions: ${p.tombstones.size}")
      val oversized =
        if p.oversized.isEmpty then Nil
        else "Oversized (cannot be placed):" :: p.oversized.map(f => s"  ${f.relPath.value}  ${gb(f.sizeBytes)}")
      (header :: rem :: tomb ::: oversized).mkString("\n")

  def burn(r: BurnReport): String =
    val d = r.disc
    val status =
      if d.finalized then "Status: FINALIZED — set this disc aside, it is sealed."
      else "Status: left OPEN — may receive more sessions later."
    val next =
      if r.remaining.moreDiscs == 0 then "No more changes — the set is up to date."
      else s"~${r.remaining.moreDiscs} more discs of changes remain. Run: scan → plan → burn"
    s"""✔ Wrote disc ${d.ordinal}  —  label: "${d.label}"
       |$status
       |$next""".stripMargin
```

- [ ] **Step 9: Run the render tests to verify they pass**

Run: `sbt "cli/testOnly diskrement.cli.RenderSpec"`
Expected: PASS.

- [ ] **Step 10: Thread `finalize` through `Runner` and drop the now-unused disc-type arg**

In `Runner.scala`, replace the `Plan` case (lines 62-66):

```scala
      case c: DiskrementCommand.Plan =>
        withProject(catalogPath, c.name) { (_, pipeline) =>
          pipeline.plan(c.name, c.discType, c.finalize).flatMap(r => printOut(Render.plan(r))).as(ExitCode.success)
        }
```

- [ ] **Step 11: Update `CliSmokeSpec` for the new `Plan` arity and burn output**

In `CliSmokeSpec.scala`, change the `Plan` construction (line 52):

```scala
            c3      <- Runner.run(global, DiskrementCommand.Plan("photos", None, finalize = false))
```

and the burn-output assertion (line 63):

```scala
            text.contains("Wrote disc 1"),
```

- [ ] **Step 12: Run the full CLI test suite**

Run: `sbt cli/test`
Expected: PASS — `CommandsSpec`, `RenderSpec`, `CliSmokeSpec`, all green.

- [ ] **Step 13: Run the whole build to confirm every module is green**

Run: `sbt test`
Expected: PASS — `core`, `shell`, and `cli` suites all pass.

- [ ] **Step 14: Commit**

```bash
git add cli/src/main/scala/diskrement/cli/ cli/src/test/scala/diskrement/cli/
git commit -m "feat(cli): plan --finalize and per-disc operator guidance

plan renders the single disc + remaining estimate; burn prints the disc
label, finalize status, and the next scan/plan/burn step.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Self-review notes

- **Spec coverage:** workflow/state (Task 2 §plan/burn + spill/full-loop tests); planner unchanged (no task touches `core`); finalize default + `--finalize` (Task 2 plan, Task 3 flag, finalize test); operator label display (Task 3 `Render.burn`); `discCount` dropped (Task 1); multi-cycle loop test (Task 2 Step 7) — all covered.
- **Deviation from spec §5.4:** `BurnReport` drops the `oversized` field. Rationale: `burn` hard-fails `OversizedFiles` before producing a report, so the field was always empty (dead in the old design too). Oversized is surfaced by `plan` only. YAGNI.
- **Type consistency:** `PlanReport(disc, discType, tombstones, remaining, oversized)`, `RemainingEstimate(moreDiscs, moreFiles, moreBytes)`, `DiscBurn(ordinal, label, finalized, stagingDir, imageBytes)`, `BurnReport(disc, remaining)`, `DiscIdentity(projectName, discOrdinal, sessionOrdinal, createdAt)` are used identically across `Pipeline.scala`, `PipelineLive.scala`, `StagedStore.scala`, `Render.scala`, and all tests. `plan(name, discTypeOverride, finalize)` arity matches in trait, impl, `Runner`, and every test call.
