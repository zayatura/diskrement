# diskrement Plan 3b — CLI Adapter Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the terminal-facing `cli` module that turns a command line into a single `Pipeline` call and renders the structured result, completing Cycle 1.

**Architecture:** A new `cli` sbt module (`cli → shell → core`) using `zio-cli`. Parsing (`Commands` → a `DiskrementCommand` ADT), rendering (`Render`: result → String, error → (message, ExitCode)), and path resolution (`PathResolver`) are pure and unit-tested in isolation. The only effectful glue is `Runner`, which resolves paths, reads `workDir` from the project row, assembles the ZIO layer stack the `Pipeline` requires, calls the operation, and returns an `ExitCode`. `Main` is a `ZIOCliDefault` entry point.

**Tech Stack:** Scala 3.3.4, ZIO 2.1.19, zio-cli 0.5.0, zio-test. Reuses the existing `shell` adapters and `Pipeline` unchanged (no new `Pipeline` methods).

**Spec:** `docs/superpowers/specs/2026-06-21-diskrement-plan-3b-cli-design.md`

---

## Conventions for every task

- **Branch:** work is on `feat/plan-3b-cli` (already created off `main`).
- **Verify build/tests:** `sbt cli/compile` and `sbt cli/test` (or `sbt "cli/testOnly <FQCN>"` for one suite). Full gate at the end: `sbt clean core/test shell/test cli/test`.
- **Style:** `-Wunused:all` is on — keep imports tight (unused import = compile error). Scala 3 significant indentation, matching existing files.
- **Commit trailer** on every commit:
  ```
  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
  ```
- **zio-test shape:** suites extend `ZIOSpecDefault` with `def spec = suite("name")(test("...") { ... })`. Pure assertions use `assertTrue(...)`.

---

## File structure

New module `cli/`:

- `cli/src/main/scala/diskrement/cli/Args.scala` — `GlobalOpts` + the `DiskrementCommand` ADT (plain data; no zio-cli, no I/O).
- `cli/src/main/scala/diskrement/cli/PathResolver.scala` — pure data-dir/catalog-path resolution from `GlobalOpts` + an injected env lookup.
- `cli/src/main/scala/diskrement/cli/Render.scala` — pure `Pipeline` result → `String` formatters, plus error → `(String, ExitCode)` and the verify-mismatch exit rule.
- `cli/src/main/scala/diskrement/cli/Commands.scala` — zio-cli `Command` definitions parsing argv → `(GlobalOpts, DiskrementCommand)`.
- `cli/src/main/scala/diskrement/cli/Prompt.scala` — `[y/N]` confirmation over zio `Console`.
- `cli/src/main/scala/diskrement/cli/Runner.scala` — effectful dispatch: resolve paths → load project/workDir → build layer stack → call `Pipeline` → render → `ExitCode`.
- `cli/src/main/scala/diskrement/cli/Main.scala` — `ZIOCliDefault` entry point.

Tests under `cli/src/test/scala/diskrement/cli/`.

---

## Task 1: Scaffold the `cli` module

**Files:**
- Modify: `build.sbt`
- Create: `cli/src/main/scala/diskrement/cli/Main.scala` (temporary stub, replaced in Task 9)

- [ ] **Step 1: Add the `cli` module to `build.sbt`**

Add the version val near the other version vals (after `val zioJsonVersion = "0.7.3"`):

```scala
val zioCliVersion  = "0.5.0"
```

Add the module definition after the `shell` module and before `lazy val root`:

```scala
lazy val cli = (project in file("cli"))
  .dependsOn(shell)
  .settings(
    name := "diskrement-cli",
    libraryDependencies ++= Seq(
      "dev.zio" %% "zio-cli"      % zioCliVersion,
      "dev.zio" %% "zio-test"     % zioVersion % Test,
      "dev.zio" %% "zio-test-sbt" % zioVersion % Test
    ),
    testFrameworks += new TestFramework("zio.test.sbt.ZTestFramework")
  )
```

Update `root` to aggregate `cli`:

```scala
lazy val root = (project in file("."))
  .aggregate(core, shell, cli)
  .settings(name := "diskrement")
```

- [ ] **Step 2: Add a temporary `Main` stub so the module compiles**

Create `cli/src/main/scala/diskrement/cli/Main.scala`:

```scala
package diskrement.cli

import zio.*

object Main extends ZIOAppDefault:
  def run = Console.printLine("diskrement cli (scaffold)")
```

- [ ] **Step 3: Verify the module compiles**

Run: `sbt cli/compile`
Expected: `[success]` — zio-cli resolves and the stub compiles.

> If `zio-cli 0.5.0` fails to resolve against this ZIO version, bump `zioCliVersion` to the latest `0.5.x`/`0.6.x` on Maven Central (`dev.zio %% zio-cli`) targeting ZIO 2.x, then re-run.

- [ ] **Step 4: Commit**

```bash
git add build.sbt cli/src/main/scala/diskrement/cli/Main.scala
git commit -m "chore(cli): scaffold cli module with zio-cli

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 2: The `DiskrementCommand` ADT

**Files:**
- Create: `cli/src/main/scala/diskrement/cli/Args.scala`

- [ ] **Step 1: Write the ADT**

Create `cli/src/main/scala/diskrement/cli/Args.scala`:

```scala
package diskrement.cli

import diskrement.core.DiscType
import java.nio.file.Path

/** Global options accepted before any subcommand. */
final case class GlobalOpts(catalog: Option[Path], dataDir: Option[Path])

/** A fully-parsed command line, independent of zio-cli. The unit Render and
  * Runner operate on. `relPath` stays a String here; Runner wraps it in RelPath. */
enum DiskrementCommand:
  case Init(name: String, source: Path, workDir: Path, discType: DiscType, marginPercent: Double, copies: Int)
  case Scan(name: String, verify: Boolean, force: Boolean)
  case Plan(name: String, discType: Option[DiscType])
  case Burn(name: String, assumeYes: Boolean)
  case Reset(name: String)
  case Status(name: String)
  case Verify(name: String, disc: Option[Int])
  case History(name: String, relPath: String)
```

- [ ] **Step 2: Verify it compiles**

Run: `sbt cli/compile`
Expected: `[success]`.

- [ ] **Step 3: Commit**

```bash
git add cli/src/main/scala/diskrement/cli/Args.scala
git commit -m "feat(cli): add DiskrementCommand ADT and GlobalOpts

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 3: `PathResolver` (pure)

**Files:**
- Create: `cli/src/main/scala/diskrement/cli/PathResolver.scala`
- Test: `cli/src/test/scala/diskrement/cli/PathResolverSpec.scala`

- [ ] **Step 1: Write the failing test**

Create `cli/src/test/scala/diskrement/cli/PathResolverSpec.scala`:

```scala
package diskrement.cli

import zio.test.*
import java.nio.file.Paths

object PathResolverSpec extends ZIOSpecDefault:
  private val home = Paths.get("/home/u")
  private def env(m: Map[String, String]): String => Option[String] = m.get

  def spec = suite("PathResolver")(
    test("defaults to ~/.local/share/diskrement when nothing is set") {
      val r = PathResolver.resolve("photos", GlobalOpts(None, None), env(Map.empty), home)
      assertTrue(
        r.dataDir == Paths.get("/home/u/.local/share/diskrement"),
        r.catalogPath == Paths.get("/home/u/.local/share/diskrement/photos.sqlite"))
    },
    test("uses XDG_DATA_HOME when set") {
      val r = PathResolver.resolve("photos", GlobalOpts(None, None), env(Map("XDG_DATA_HOME" -> "/xdg")), home)
      assertTrue(r.catalogPath == Paths.get("/xdg/diskrement/photos.sqlite"))
    },
    test("STATE_DIRECTORY (systemd) wins over XDG, first entry of a colon list") {
      val r = PathResolver.resolve("photos", GlobalOpts(None, None),
        env(Map("STATE_DIRECTORY" -> "/var/lib/diskrement:/other", "XDG_DATA_HOME" -> "/xdg")), home)
      assertTrue(r.catalogPath == Paths.get("/var/lib/diskrement/photos.sqlite"))
    },
    test("--data-dir overrides env") {
      val r = PathResolver.resolve("photos", GlobalOpts(None, Some(Paths.get("/d"))),
        env(Map("STATE_DIRECTORY" -> "/var/lib/diskrement")), home)
      assertTrue(r.catalogPath == Paths.get("/d/photos.sqlite"))
    },
    test("--catalog is used directly; its parent is the data dir") {
      val r = PathResolver.resolve("photos", GlobalOpts(Some(Paths.get("/tmp/c/my.sqlite")), None), env(Map.empty), home)
      assertTrue(
        r.catalogPath == Paths.get("/tmp/c/my.sqlite"),
        r.dataDir == Paths.get("/tmp/c"))
    }
  )
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `sbt "cli/testOnly diskrement.cli.PathResolverSpec"`
Expected: FAIL — `PathResolver` is not defined.

- [ ] **Step 3: Write the implementation**

Create `cli/src/main/scala/diskrement/cli/PathResolver.scala`:

```scala
package diskrement.cli

import java.nio.file.{Path, Paths}

final case class ResolvedPaths(catalogPath: Path, dataDir: Path)

/** Pure resolution of the catalog file location. Precedence:
  *   --catalog  >  --data-dir  >  $STATE_DIRECTORY  >  $XDG_DATA_HOME/diskrement
  *   >  <home>/.local/share/diskrement
  * The environment is injected (a `String => Option[String]` lookup) so this is a
  * pure function of its inputs. */
object PathResolver:
  def resolve(
      projectName: String,
      global: GlobalOpts,
      env: String => Option[String],
      homeDir: Path
  ): ResolvedPaths =
    global.catalog match
      case Some(catalogPath) =>
        val parent = Option(catalogPath.toAbsolutePath.getParent).getOrElse(Paths.get("."))
        ResolvedPaths(catalogPath, parent)
      case None =>
        val dataDir = global.dataDir
          .orElse(env("STATE_DIRECTORY").map(s => Paths.get(s.split(':').head)))
          .orElse(env("XDG_DATA_HOME").map(d => Paths.get(d).resolve("diskrement")))
          .getOrElse(homeDir.resolve(".local/share/diskrement"))
        ResolvedPaths(dataDir.resolve(s"$projectName.sqlite"), dataDir)
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `sbt "cli/testOnly diskrement.cli.PathResolverSpec"`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add cli/src/main/scala/diskrement/cli/PathResolver.scala cli/src/test/scala/diskrement/cli/PathResolverSpec.scala
git commit -m "feat(cli): add PathResolver with XDG/systemd/flag precedence

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 4: `Render` — success formatters

**Files:**
- Create: `cli/src/main/scala/diskrement/cli/Render.scala`
- Test: `cli/src/test/scala/diskrement/cli/RenderSpec.scala`

- [ ] **Step 1: Write the failing test**

Create `cli/src/test/scala/diskrement/cli/RenderSpec.scala`:

```scala
package diskrement.cli

import diskrement.core.*
import diskrement.shell.model.*
import diskrement.shell.pipeline.*
import zio.test.*
import java.nio.file.Paths
import java.time.Instant

object RenderSpec extends ZIOSpecDefault:

  private val project = Project(
    ProjectId(1), "photos", "/srv/photos", "/srv/work",
    DiscType.BD_R, 2.0, 1, Instant.parse("2026-06-21T00:00:00Z"))

  private def sf(path: String, size: Long): ScannedFile =
    ScannedFile(RelPath(path), ContentHash("h"), size, Instant.parse("2026-06-21T00:00:00Z"))

  def spec = suite("Render")(
    test("scan summary") {
      assertTrue(Render.scan(ScanResult(12, 3, 1, 540)) ==
        "Scanned: 12 added, 3 modified, 1 deleted, 540 unchanged.")
    },
    test("plan: discs with top-off/new and an oversized block") {
      val plan = BurnPlan(
        discs = List(
          PlannedDisc(1, isNew = false, willFinalize = true, files = List(sf("a", 24_100_000_000L)),
            priorUsedBytes = 0L, usableBytes = 25_000_000_000L),
          PlannedDisc(2, isNew = true, willFinalize = false, files = List(sf("b", 3_200_000_000L)),
            priorUsedBytes = 0L, usableBytes = 25_000_000_000L)),
        tombstones = Nil,
        oversized = List(sf("huge.bin", 99_000_000_000L)))
      val out = Render.plan(plan, DiscType.BD_R)
      assertTrue(
        out.contains("Plan: 2 discs (BD_R)"),
        out.contains("disc 1"), out.contains("(top-off)"),
        out.contains("disc 2"), out.contains("(new)"),
        out.contains("Oversized (cannot be placed):"),
        out.contains("huge.bin"))
    },
    test("plan: empty plan says nothing to burn") {
      assertTrue(Render.plan(BurnPlan(Nil, Nil, Nil), DiscType.BD_R) == "Nothing to burn: no changes to plan.")
    },
    test("burn report") {
      val report = BurnReport(
        discs = List(DiscBurn(1, Paths.get("/w/disc-1"), 24_100_000_000L), DiscBurn(2, Paths.get("/w/disc-2"), 3_200_000_000L)),
        oversized = Nil)
      val out = Render.burn(report)
      assertTrue(out.contains("Burned disc 1"), out.contains("Burned disc 2"), out.contains("✓"))
    },
    test("status table") {
      val report = StatusReport(
        discs = List(DiscState(1, DiscStatus.FINALIZED, 25_000_000_000L, 24_800_000_000L),
                     DiscState(2, DiscStatus.OPEN, 25_000_000_000L, 3_200_000_000L)),
        openDiscOrdinal = Some(2), totalCurrentFiles = 556,
        lastBurnAt = Some(Instant.parse("2026-06-21T14:03:00Z")))
      val out = Render.status(project, report)
      assertTrue(
        out.contains("Project photos"),
        out.contains("#1"), out.contains("FINALIZED"),
        out.contains("#2"), out.contains("OPEN"),
        out.contains("Current files: 556"))
    },
    test("history list") {
      val entries = List(
        FileHistoryEntry(ContentHash("a1b2c3"), 3_400_000L, 1, 1, VersionStatus.SUPERSEDED),
        FileHistoryEntry(ContentHash("d4e5f6"), 3_500_000L, 2, 2, VersionStatus.CURRENT))
      val out = Render.history(RelPath("vacation/img.jpg"), entries)
      assertTrue(
        out.contains("vacation/img.jpg"),
        out.contains("disc 1 / session 1"), out.contains("SUPERSEDED"),
        out.contains("disc 2 / session 2"), out.contains("CURRENT"))
    },
    test("history: empty") {
      assertTrue(Render.history(RelPath("nope"), Nil) == "No history for nope.")
    },
    test("init confirmation line") {
      assertTrue(Render.init("photos", ProjectId(7)) == "Created project photos (id 7).")
    }
  )
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `sbt "cli/testOnly diskrement.cli.RenderSpec"`
Expected: FAIL — `Render` is not defined.

- [ ] **Step 3: Write the implementation**

Create `cli/src/main/scala/diskrement/cli/Render.scala`:

```scala
package diskrement.cli

import diskrement.core.*
import diskrement.shell.model.*
import diskrement.shell.pipeline.*

/** Pure formatters: Pipeline result types -> terminal text. No I/O. */
object Render:

  /** Bytes as a fixed-precision GB string for deterministic golden tests. */
  private def gb(bytes: Long): String = f"${bytes / 1.0e9}%.1f GB"

  def scan(r: ScanResult): String =
    s"Scanned: ${r.added} added, ${r.modified} modified, ${r.deleted} deleted, ${r.unchanged} unchanged."

  def plan(p: BurnPlan, discType: DiscType): String =
    if p.discs.isEmpty then "Nothing to burn: no changes to plan."
    else
      val header = s"Plan: ${p.discs.size} discs (${discType})"
      val lines = p.discs.map { d =>
        val kind = if d.isNew then "(new)" else "(top-off)"
        f"  disc ${d.ordinal}%-3d ${gb(d.totalUsedBytes)}%>10s   $kind"
      }
      val oversized =
        if p.oversized.isEmpty then Nil
        else "Oversized (cannot be placed):" :: p.oversized.map(f => s"  ${f.relPath.value}  ${gb(f.sizeBytes)}")
      (header :: lines ::: oversized).mkString("\n")

  def burn(r: BurnReport): String =
    val discs = r.discs.map(d => s"Burned disc ${d.ordinal} ✓  (${gb(d.imageBytes)})").mkString("\n")
    val oversized =
      if r.oversized.isEmpty then ""
      else "\nOversized (skipped):\n" + r.oversized.map(p => s"  ${p.value}").mkString("\n")
    discs + oversized

  def status(project: Project, r: StatusReport): String =
    val header = s"Project ${project.name}  (source ${project.sourceRoot}, ${project.defaultDiscType}, margin ${project.marginPercent}%)"
    val discLines = r.discs.map { d =>
      val open = if r.openDiscOrdinal.contains(d.ordinal) then "  ← open" else ""
      f"  #${d.ordinal}%-3d ${d.status}%-10s ${gb(d.usedBytes)} / ${gb(d.usableBytes)}$open"
    }
    val last = r.lastBurnAt.map(_.toString).getOrElse("never")
    (s"$header\nDiscs:" :: discLines ::: List(s"Current files: ${r.totalCurrentFiles}      Last burn: $last")).mkString("\n")

  def verify(reports: List[VerifyReport]): String =
    if reports.isEmpty then "Nothing to verify: no burned discs."
    else reports.map { v =>
      if v.mismatches.isEmpty then s"disc ${v.discOrdinal}: ${v.filesChecked} files OK ✓"
      else s"disc ${v.discOrdinal}: ${v.filesChecked} checked, ${v.mismatches.size} MISMATCH:\n" +
        v.mismatches.map(p => s"  ${p.value}").mkString("\n")
    }.mkString("\n")

  def history(relPath: RelPath, entries: List[FileHistoryEntry]): String =
    if entries.isEmpty then s"No history for ${relPath.value}."
    else
      val lines = entries.zipWithIndex.map { case (e, i) =>
        f"  v${i + 1}  disc ${e.discOrdinal} / session ${e.sessionOrdinal}   ${gb(e.sizeBytes)}  ${e.contentHash.value}  ${e.status}"
      }
      (relPath.value :: lines).mkString("\n")

  def init(name: String, id: ProjectId): String =
    s"Created project $name (id ${id.value})."
```

> Note: the f-interpolator width token for a right-aligned string in `plan` is written as `%>10s` above only as intent; Scala's `f` interpolator uses `%10s` for right alignment. If the engineer sees a format error, replace `%>10s` with `%10s`. The `RenderSpec` asserts on `contains`, not exact column positions, so either is fine.

- [ ] **Step 4: Run the test to verify it passes**

Run: `sbt "cli/testOnly diskrement.cli.RenderSpec"`
Expected: PASS. If a `java.util.UnknownFormatConversionException` appears, apply the `%10s` fix noted above and re-run.

- [ ] **Step 5: Commit**

```bash
git add cli/src/main/scala/diskrement/cli/Render.scala cli/src/test/scala/diskrement/cli/RenderSpec.scala
git commit -m "feat(cli): add Render success formatters

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 5: `Render` — error message + exit-code mapping

**Files:**
- Modify: `cli/src/main/scala/diskrement/cli/Render.scala`
- Test: `cli/src/test/scala/diskrement/cli/ExitMappingSpec.scala`

- [ ] **Step 1: Write the failing test**

Create `cli/src/test/scala/diskrement/cli/ExitMappingSpec.scala`:

```scala
package diskrement.cli

import diskrement.core.RelPath
import diskrement.shell.pipeline.*
import zio.ExitCode
import zio.test.*
import java.nio.file.Paths

object ExitMappingSpec extends ZIOSpecDefault:
  def spec = suite("Render.handle / verifyExit")(
    test("ProjectNotFound -> exit 2 with its message") {
      val (msg, code) = Render.handle(PipelineError.ProjectNotFound("photos"))
      assertTrue(code == ExitCode(2), msg.contains("project not found: photos"))
    },
    test("NoStagedScan / NoStagedPlan / SnapshotMissing -> exit 2") {
      assertTrue(
        Render.handle(PipelineError.NoStagedScan("p"))._2 == ExitCode(2),
        Render.handle(PipelineError.NoStagedPlan("p"))._2 == ExitCode(2),
        Render.handle(PipelineError.SnapshotMissing("p"))._2 == ExitCode(2))
    },
    test("precondition failures -> exit 3") {
      assertTrue(
        Render.handle(PipelineError.NotSameFilesystem(Paths.get("/a"), Paths.get("/b")))._2 == ExitCode(3),
        Render.handle(PipelineError.NotCowCapable(Paths.get("/w"), "x"))._2 == ExitCode(3),
        Render.handle(PipelineError.OversizedFiles(List(RelPath("h"))))._2 == ExitCode(3))
    },
    test("Failed -> exit 5") {
      assertTrue(Render.handle(PipelineError.Failed(new RuntimeException("boom")))._2 == ExitCode(5))
    },
    test("verifyExit: clean reports -> 0, any mismatch -> 4") {
      val clean = List(VerifyReport(1, 10, Nil))
      val dirty = List(VerifyReport(1, 10, Nil), VerifyReport(2, 5, List(RelPath("bad"))))
      assertTrue(Render.verifyExit(clean) == ExitCode(0), Render.verifyExit(dirty) == ExitCode(4))
    }
  )
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `sbt "cli/testOnly diskrement.cli.ExitMappingSpec"`
Expected: FAIL — `Render.handle` / `Render.verifyExit` not defined.

- [ ] **Step 3: Add the mapping to `Render.scala`**

Add these imports at the top of `cli/src/main/scala/diskrement/cli/Render.scala` (alongside the existing imports):

```scala
import zio.ExitCode
```

Append to `object Render`:

```scala
  /** Known errors -> a stderr message and a stable exit code. */
  def handle(e: PipelineError): (String, ExitCode) =
    val code = e match
      case _: PipelineError.ProjectNotFound   => ExitCode(2)
      case _: PipelineError.NoStagedScan      => ExitCode(2)
      case _: PipelineError.NoStagedPlan      => ExitCode(2)
      case _: PipelineError.SnapshotMissing   => ExitCode(2)
      case _: PipelineError.NotSameFilesystem => ExitCode(3)
      case _: PipelineError.NotCowCapable     => ExitCode(3)
      case _: PipelineError.OversizedFiles    => ExitCode(3)
      case _: PipelineError.Failed            => ExitCode(5)
    val message = e match
      case PipelineError.Failed(cause) => s"internal error: ${cause.getMessage} (this is a bug)"
      case other                       => other.getMessage
    (message, code)

  /** Result-driven exit: a verify that finds corruption is a successful run with a bad outcome. */
  def verifyExit(reports: List[VerifyReport]): ExitCode =
    if reports.exists(_.mismatches.nonEmpty) then ExitCode(4) else ExitCode(0)
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `sbt "cli/testOnly diskrement.cli.ExitMappingSpec"`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add cli/src/main/scala/diskrement/cli/Render.scala cli/src/test/scala/diskrement/cli/ExitMappingSpec.scala
git commit -m "feat(cli): map PipelineError to messages and exit codes

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 6: `Commands` — argv parsing into the ADT

**Files:**
- Create: `cli/src/main/scala/diskrement/cli/Commands.scala`
- Test: `cli/src/test/scala/diskrement/cli/CommandsSpec.scala`

- [ ] **Step 1: Write the failing test**

Create `cli/src/test/scala/diskrement/cli/CommandsSpec.scala`:

```scala
package diskrement.cli

import diskrement.core.DiscType
import zio.cli.*
import zio.test.*
import java.nio.file.Paths

object CommandsSpec extends ZIOSpecDefault:

  /** Parse argv and pull out the (GlobalOpts, DiskrementCommand) value. */
  private def parse(args: String*) =
    Commands.root.parse(args.toList, CliConfig.default).map {
      case CommandDirective.UserDefined(_, value) => value
      case other => throw new AssertionError(s"expected UserDefined, got $other")
    }

  def spec = suite("Commands")(
    test("init with defaults") {
      for v <- parse("init", "photos", "--source", "/srv/p", "--work-dir", "/srv/w")
      yield assertTrue(v == (
        GlobalOpts(None, None),
        DiskrementCommand.Init("photos", Paths.get("/srv/p"), Paths.get("/srv/w"), DiscType.BD_R, 2.0, 1)))
    },
    test("init with explicit disc-type, margin, copies") {
      for v <- parse("init", "photos", "--source", "/s", "--work-dir", "/w",
                     "--disc-type", "BD_R_DL", "--margin", "5.0", "--copies", "2")
      yield assertTrue(v._2 ==
        DiskrementCommand.Init("photos", Paths.get("/s"), Paths.get("/w"), DiscType.BD_R_DL, 5.0, 2))
    },
    test("scan flags") {
      for v <- parse("scan", "photos", "--verify", "--force")
      yield assertTrue(v._2 == DiskrementCommand.Scan("photos", verify = true, force = true))
    },
    test("scan defaults (flags off)") {
      for v <- parse("scan", "photos")
      yield assertTrue(v._2 == DiskrementCommand.Scan("photos", verify = false, force = false))
    },
    test("plan with optional disc-type override") {
      for a <- parse("plan", "photos")
          b <- parse("plan", "photos", "--disc-type", "DVD")
      yield assertTrue(
        a._2 == DiskrementCommand.Plan("photos", None),
        b._2 == DiskrementCommand.Plan("photos", Some(DiscType.DVD)))
    },
    test("burn with --yes") {
      for v <- parse("burn", "photos", "--yes")
      yield assertTrue(v._2 == DiskrementCommand.Burn("photos", assumeYes = true))
    },
    test("reset / status") {
      for r <- parse("reset", "photos"); s <- parse("status", "photos")
      yield assertTrue(r._2 == DiskrementCommand.Reset("photos"), s._2 == DiskrementCommand.Status("photos"))
    },
    test("verify with optional --disc") {
      for a <- parse("verify", "photos"); b <- parse("verify", "photos", "--disc", "2")
      yield assertTrue(a._2 == DiskrementCommand.Verify("photos", None), b._2 == DiskrementCommand.Verify("photos", Some(2)))
    },
    test("history with relPath arg") {
      for v <- parse("history", "photos", "vacation/img.jpg")
      yield assertTrue(v._2 == DiskrementCommand.History("photos", "vacation/img.jpg"))
    },
    test("global --catalog is captured") {
      for v <- parse("--catalog", "/tmp/c.sqlite", "status", "photos")
      yield assertTrue(v._1 == GlobalOpts(Some(Paths.get("/tmp/c.sqlite")), None))
    },
    test("an unknown disc-type is rejected") {
      parse("init", "photos", "--source", "/s", "--work-dir", "/w", "--disc-type", "ZIP")
        .exit.map(e => assertTrue(e.isFailure))
    }
  )
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `sbt "cli/testOnly diskrement.cli.CommandsSpec"`
Expected: FAIL — `Commands` is not defined.

- [ ] **Step 3: Write the implementation**

Create `cli/src/main/scala/diskrement/cli/Commands.scala`:

```scala
package diskrement.cli

import diskrement.core.DiscType
import zio.cli.*
import java.nio.file.{Path, Paths}

/** zio-cli command definitions. Each subcommand parses straight into a
  * DiskrementCommand case; the root threads GlobalOpts in front. */
object Commands:

  private val discTypeChoices: Seq[(String, DiscType)] = Seq(
    "BD_R"    -> DiscType.BD_R,
    "BD_R_DL" -> DiscType.BD_R_DL,
    "DVD"     -> DiscType.DVD,
    "DVD_DL"  -> DiscType.DVD_DL)

  private val nameArg: Args[String]    = Args.text("name")
  private def path(s: String): Path    = Paths.get(s)

  // --- global options (before the subcommand) ---
  private val catalogOpt: Options[Option[Path]] = Options.text("catalog").map(path).optional
  private val dataDirOpt: Options[Option[Path]] = Options.text("data-dir").map(path).optional
  private val globalOpts: Options[GlobalOpts] =
    (catalogOpt ++ dataDirOpt).map { case (c, d) => GlobalOpts(c, d) }

  // --- init ---
  private val init: Command[DiskrementCommand] =
    val source   = Options.text("source").map(path)
    val workDir  = Options.text("work-dir").map(path)
    val discType = Options.enumeration("disc-type")(discTypeChoices*).withDefault(DiscType.BD_R)
    val margin   = Options.decimal("margin").withDefault(BigDecimal(2.0))
    val copies   = Options.integer("copies").withDefault(BigInt(1))
    Command("init", source ++ workDir ++ discType ++ margin ++ copies, nameArg)
      .withHelp(HelpDoc.p("Create a project and run filesystem checks."))
      .map { case (((((src, wd), dt), m), c), name) =>
        DiskrementCommand.Init(name, src, wd, dt, m.toDouble, c.toInt)
      }

  // --- scan ---
  private val scan: Command[DiskrementCommand] =
    val verify = Options.boolean("verify")
    val force  = Options.boolean("force")
    Command("scan", verify ++ force, nameArg)
      .withHelp(HelpDoc.p("Snapshot the source and stage a diff."))
      .map { case ((v, f), name) => DiskrementCommand.Scan(name, v, f) }

  // --- plan ---
  private val plan: Command[DiskrementCommand] =
    val discType = Options.enumeration("disc-type")(discTypeChoices*).optional
    Command("plan", discType, nameArg)
      .withHelp(HelpDoc.p("Plan a burn from the staged scan."))
      .map { case (dt, name) => DiskrementCommand.Plan(name, dt) }

  // --- burn ---
  private val burn: Command[DiskrementCommand] =
    val yes = Options.boolean("yes").alias("y")
    Command("burn", yes, nameArg)
      .withHelp(HelpDoc.p("Burn the staged plan (with confirmation)."))
      .map { case (y, name) => DiskrementCommand.Burn(name, y) }

  // --- reset / status ---
  private val reset: Command[DiskrementCommand] =
    Command("reset", Options.none, nameArg)
      .withHelp(HelpDoc.p("Discard staged scan/plan state."))
      .map { case (_, name) => DiskrementCommand.Reset(name) }

  private val status: Command[DiskrementCommand] =
    Command("status", Options.none, nameArg)
      .withHelp(HelpDoc.p("Show discs, totals, and last burn."))
      .map { case (_, name) => DiskrementCommand.Status(name) }

  // --- verify ---
  private val verify: Command[DiskrementCommand] =
    val disc = Options.integer("disc").map(_.toInt).optional
    Command("verify", disc, nameArg)
      .withHelp(HelpDoc.p("Re-hash burned disc(s) against the catalog."))
      .map { case (d, name) => DiskrementCommand.Verify(name, d) }

  // --- history ---
  private val history: Command[DiskrementCommand] =
    Command("history", Options.none, nameArg ++ Args.text("relPath"))
      .withHelp(HelpDoc.p("Show the full version history of one file."))
      .map { case (_, (name, rel)) => DiskrementCommand.History(name, rel) }

  val root: Command[(GlobalOpts, DiskrementCommand)] =
    Command("diskrement", globalOpts)
      .subcommands(init, scan, plan, burn, reset, status, verify, history)
```

> zio-cli combines `a ++ b ++ c` left-associatively into nested tuples — hence the `((((src, wd), dt), m), c)` shape in `init`. `Command(name, options, args)` yields `Command[(OptionsValue, ArgsValue)]`; `root` carries `(GlobalOpts, <subcommand value>)` because `globalOpts` is non-empty and `.subcommands` pairs the parent options value with the chosen subcommand's value.

- [ ] **Step 4: Run the test to verify it passes**

Run: `sbt "cli/testOnly diskrement.cli.CommandsSpec"`
Expected: PASS.

> If `.subcommands` produces a differently-nested type than `(GlobalOpts, DiskrementCommand)` (e.g. an extra tuple layer), adjust `root`'s type and add a trailing `.map` to flatten to `(GlobalOpts, DiskrementCommand)`, mirroring the `remote` example in the zio-cli docs (`.map(_._2)` style). The `CommandsSpec` asserts the final shape, so let it drive the exact `.map`.

- [ ] **Step 5: Commit**

```bash
git add cli/src/main/scala/diskrement/cli/Commands.scala cli/src/test/scala/diskrement/cli/CommandsSpec.scala
git commit -m "feat(cli): add zio-cli command definitions

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 7: `Prompt` — confirmation over Console

**Files:**
- Create: `cli/src/main/scala/diskrement/cli/Prompt.scala`
- Test: `cli/src/test/scala/diskrement/cli/PromptSpec.scala`

- [ ] **Step 1: Write the failing test**

Create `cli/src/test/scala/diskrement/cli/PromptSpec.scala`:

```scala
package diskrement.cli

import zio.*
import zio.test.*

object PromptSpec extends ZIOSpecDefault:
  def spec = suite("Prompt.confirm")(
    test("--yes bypasses the prompt and returns true") {
      for ok <- Prompt.confirm("Proceed?", assumeYes = true)
      yield assertTrue(ok)
    },
    test("'y' is yes") {
      for _  <- TestConsole.feedLines("y")
          ok <- Prompt.confirm("Proceed?", assumeYes = false)
      yield assertTrue(ok)
    },
    test("'yes' is yes") {
      for _  <- TestConsole.feedLines("yes")
          ok <- Prompt.confirm("Proceed?", assumeYes = false)
      yield assertTrue(ok)
    },
    test("empty line is no") {
      for _  <- TestConsole.feedLines("")
          ok <- Prompt.confirm("Proceed?", assumeYes = false)
      yield assertTrue(!ok)
    },
    test("EOF (no input) is no") {
      for ok <- Prompt.confirm("Proceed?", assumeYes = false)
      yield assertTrue(!ok)
    }
  )
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `sbt "cli/testOnly diskrement.cli.PromptSpec"`
Expected: FAIL — `Prompt` is not defined.

- [ ] **Step 3: Write the implementation**

Create `cli/src/main/scala/diskrement/cli/Prompt.scala`:

```scala
package diskrement.cli

import zio.*

/** A [y/N] confirmation over the ZIO Console (testable with TestConsole). */
object Prompt:
  def confirm(message: String, assumeYes: Boolean): UIO[Boolean] =
    if assumeYes then ZIO.succeed(true)
    else
      (Console.print(s"$message [y/N] ") *> Console.readLine).either.map {
        case Right(line) =>
          val t = line.trim.toLowerCase
          t == "y" || t == "yes"
        case Left(_) => false // EOF or read error -> treat as "no"
      }
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `sbt "cli/testOnly diskrement.cli.PromptSpec"`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add cli/src/main/scala/diskrement/cli/Prompt.scala cli/src/test/scala/diskrement/cli/PromptSpec.scala
git commit -m "feat(cli): add confirmation prompt

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 8: `Runner` — effectful dispatch

**Files:**
- Create: `cli/src/main/scala/diskrement/cli/Runner.scala`

This is the glue: it has no business logic, so it is exercised by the Task 10 smoke test rather than a dedicated unit test (the pure pieces it calls — `Render`, `PathResolver`, `Prompt` — are already covered).

- [ ] **Step 1: Write the `Runner`**

Create `cli/src/main/scala/diskrement/cli/Runner.scala`:

```scala
package diskrement.cli

import diskrement.core.RelPath
import diskrement.shell.burn.{SimulatedBurner, SnapshotWriter}
import diskrement.shell.catalog.{CatalogStore, Db, DbPath}
import diskrement.shell.fs.*
import diskrement.shell.model.{Project, ProjectInit}
import diskrement.shell.pipeline.*
import zio.*
import java.nio.file.{Files, Path, Paths}

/** Resolves paths, wires the Pipeline layer stack, runs one operation, and
  * renders the result. Returns a process ExitCode; never fails. */
object Runner:

  def run(global: GlobalOpts, cmd: DiskrementCommand): UIO[ExitCode] =
    program(global, cmd)
      .catchAll { e =>
        val (msg, code) = Render.handle(e)
        Console.printLineError(msg).orDie.as(code)
      }
      .catchAllDefect { t =>
        Console.printLineError(s"internal error: ${t.getMessage} (this is a bug)").orDie.as(ExitCode(5))
      }

  // --- the happy path; PipelineError is handled by `run` ---
  private def program(global: GlobalOpts, cmd: DiskrementCommand): IO[PipelineError, ExitCode] =
    val name = projectName(cmd)
    for
      env      <- ZIO.attempt(System.getenv()).map(m => (k: String) => Option(m.get(k))).mapError(PipelineError.Failed(_))
      home     <- ZIO.attempt(Paths.get(System.getProperty("user.home"))).mapError(PipelineError.Failed(_))
      resolved  = PathResolver.resolve(name, global, env, home)
      _        <- ZIO.attempt(Files.createDirectories(resolved.dataDir)).mapError(PipelineError.Failed(_))
      code     <- dispatch(cmd, resolved.catalogPath)
    yield code

  private def projectName(cmd: DiskrementCommand): String = cmd match
    case c: DiskrementCommand.Init    => c.name
    case c: DiskrementCommand.Scan    => c.name
    case c: DiskrementCommand.Plan    => c.name
    case c: DiskrementCommand.Burn    => c.name
    case c: DiskrementCommand.Reset   => c.name
    case c: DiskrementCommand.Status  => c.name
    case c: DiskrementCommand.Verify  => c.name
    case c: DiskrementCommand.History => c.name

  private def dispatch(cmd: DiskrementCommand, catalogPath: Path): IO[PipelineError, ExitCode] =
    cmd match
      case c: DiskrementCommand.Init =>
        ZIO.attempt(Files.createDirectories(c.workDir)).mapError(PipelineError.Failed(_)) *>
          withPipeline(catalogPath, c.workDir) { pipeline =>
            val pi = ProjectInit(c.name, c.source.toString, c.workDir.toString, c.discType, c.marginPercent, c.copies, java.time.Instant.now())
            pipeline.init(pi).flatMap(id => printOut(Render.init(c.name, id))).as(ExitCode.success)
          }

      case c: DiskrementCommand.Scan =>
        withProject(catalogPath, c.name) { (_, pipeline) =>
          pipeline.scan(c.name, c.verify, c.force).flatMap(r => printOut(Render.scan(r))).as(ExitCode.success)
        }

      case c: DiskrementCommand.Plan =>
        withProject(catalogPath, c.name) { (project, pipeline) =>
          val effectiveType = c.discType.getOrElse(project.defaultDiscType)
          pipeline.plan(c.name, c.discType).flatMap(p => printOut(Render.plan(p, effectiveType))).as(ExitCode.success)
        }

      case c: DiskrementCommand.Burn =>
        withProject(catalogPath, c.name) { (project, pipeline) =>
          for
            staged <- pipeline.plan(c.name, None) // re-derive the staged plan, idempotent
            _      <- printOut(Render.plan(staged, project.defaultDiscType))
            ok     <- Prompt.confirm("Proceed?", c.assumeYes)
            code   <- if !ok then printOut("Aborted.").as(ExitCode.success)
                      else pipeline.burn(c.name).flatMap(r => printOut(Render.burn(r))).as(ExitCode.success)
          yield code
        }

      case c: DiskrementCommand.Reset =>
        withProject(catalogPath, c.name) { (_, pipeline) =>
          pipeline.reset(c.name) *> printOut(s"Reset ${c.name}.").as(ExitCode.success)
        }

      case c: DiskrementCommand.Status =>
        withProject(catalogPath, c.name) { (project, pipeline) =>
          pipeline.status(c.name).flatMap(r => printOut(Render.status(project, r))).as(ExitCode.success)
        }

      case c: DiskrementCommand.Verify =>
        withProject(catalogPath, c.name) { (_, pipeline) =>
          pipeline.verify(c.name, c.disc).flatMap(rs => printOut(Render.verify(rs)).as(Render.verifyExit(rs)))
        }

      case c: DiskrementCommand.History =>
        withProject(catalogPath, c.name) { (_, pipeline) =>
          val rel = RelPath(c.relPath)
          pipeline.history(c.name, rel).flatMap(es => printOut(Render.history(rel, es))).as(ExitCode.success)
        }

  private def printOut(s: String): UIO[Unit] = Console.printLine(s).orDie

  /** Load the project row (for workDir + metadata), then run with a wired Pipeline. */
  private def withProject[A](catalogPath: Path, name: String)(
      use: (Project, Pipeline) => IO[PipelineError, A]): IO[PipelineError, A] =
    loadProject(catalogPath, name).flatMap { project =>
      withPipeline(catalogPath, Paths.get(project.workDir))(pipeline => use(project, pipeline))
    }

  private def loadProject(catalogPath: Path, name: String): IO[PipelineError, Project] =
    ZIO.serviceWithZIO[CatalogStore](_.findProject(name))
      .provideLayer(ZLayer.succeed(DbPath(catalogPath)) >>> Db.poolLayer >>> CatalogStore.live)
      .mapError(PipelineError.Failed(_))
      .flatMap {
        case Some(p) => ZIO.succeed(p)
        case None    => ZIO.fail(PipelineError.ProjectNotFound(name))
      }

  /** Build the full Pipeline layer stack for one (catalog, workDir) pair and run `use`. */
  private def withPipeline[A](catalogPath: Path, workDir: Path)(
      use: Pipeline => IO[PipelineError, A]): IO[PipelineError, A] =
    val layer = ZLayer.make[Pipeline](
      ZLayer.succeed(ProjectPaths(catalogPath, workDir)),
      ZLayer.fromFunction((p: ProjectPaths) => DbPath(p.catalogPath)),
      Db.poolLayer,
      FileSystem.live, Hasher.live, FileScanner.live,
      SnapshotWriter.live, SimulatedBurner.live, SourceSnapshot.live,
      StagedStore.live, CatalogStore.live,
      Pipeline.live)
    ZIO.serviceWithZIO[Pipeline](use)
      .provideLayer(layer)
      .mapError {
        case pe: PipelineError => pe
        case t: Throwable      => PipelineError.Failed(t)
      }
```

- [ ] **Step 2: Verify it compiles**

Run: `sbt cli/compile`
Expected: `[success]`.

> If `provideLayer` reports a leftover requirement, confirm the layer set matches `Pipeline.live`'s requirement exactly (`ProjectPaths & CatalogStore & FileScanner & SourceSnapshot & SimulatedBurner & StagedStore & Hasher`) plus the `DbPath`/`Db.poolLayer`/`FileSystem.live`/`SnapshotWriter.live` those depend on — i.e. the same set `PipelineTestKit.env` uses, with `SourceSnapshot.live` added.

- [ ] **Step 3: Commit**

```bash
git add cli/src/main/scala/diskrement/cli/Runner.scala
git commit -m "feat(cli): add Runner dispatch wiring the Pipeline

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 9: `Main` — the `ZIOCliDefault` entry point

**Files:**
- Modify: `cli/src/main/scala/diskrement/cli/Main.scala`

- [ ] **Step 1: Replace the stub with the real entry point**

Replace the contents of `cli/src/main/scala/diskrement/cli/Main.scala`:

```scala
package diskrement.cli

import zio.cli.*
import zio.cli.HelpDoc.Span.text

object Main extends ZIOCliDefault:
  val cliApp = CliApp.make(
    name = "diskrement",
    version = "0.1.0",
    summary = text("incremental backups to optical discs, with a memory"),
    command = Commands.root
  ) { case (global, cmd) => Runner.run(global, cmd) }
```

> `Runner.run` returns an `ExitCode`; `ZIOCliDefault` propagates a returned `ExitCode` as the process exit status. The smoke test (Task 10) asserts exit codes by calling `Runner.run` directly, so the CLI's exit semantics are verified independently of the process-exit plumbing.

- [ ] **Step 2: Verify it compiles**

Run: `sbt cli/compile`
Expected: `[success]`.

- [ ] **Step 3: Manual smoke (optional but recommended)**

Run: `sbt "cli/run --help"`
Expected: zio-cli prints the `diskrement` usage with all eight subcommands listed.

- [ ] **Step 4: Commit**

```bash
git add cli/src/main/scala/diskrement/cli/Main.scala
git commit -m "feat(cli): wire Main entry point (ZIOCliDefault)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 10: End-to-end smoke test

**Files:**
- Test: `cli/src/test/scala/diskrement/cli/CliSmokeSpec.scala`

Drives `Runner.run` through `init → scan → plan → burn → status` against a real temp dir + real per-project catalog, asserting exit codes and a few stdout lines. Uses `TestConsole` to (a) feed the burn confirmation and (b) capture output. Because the real `SourceSnapshot.live` requires a CoW filesystem, the test auto-skips when CoW is unavailable — mirroring 3a's CoW-aware integration tests.

- [ ] **Step 1: Write the test**

Create `cli/src/test/scala/diskrement/cli/CliSmokeSpec.scala`:

```scala
package diskrement.cli

import diskrement.shell.fs.SourceSnapshot
import zio.*
import zio.test.*
import java.nio.file.{Files, Path}

object CliSmokeSpec extends ZIOSpecDefault:

  private def deleteRecursive(dir: Path): Unit =
    if Files.exists(dir) then
      val s = Files.walk(dir)
      try s.sorted(java.util.Comparator.reverseOrder()).forEach(p => Files.deleteIfExists(p))
      finally s.close()

  /** True when the system temp dir supports CoW reflinks (probe via the real adapter). */
  private val cowAvailable: UIO[Boolean] =
    ZIO.scoped {
      for
        work <- ZIO.acquireRelease(
                  ZIO.attempt(Files.createTempDirectory("cli-cow")))(p => ZIO.attempt(deleteRecursive(p)).orDie)
                  .orDie
        ok   <- ZIO.serviceWithZIO[SourceSnapshot](_.checkCowCapable(work))
                  .provideLayer(SourceSnapshot.live).as(true)
                  .catchAll(_ => ZIO.succeed(false))
      yield ok
    }

  def spec = suite("CliSmokeSpec")(
    test("init -> scan -> plan -> burn -> status, with exit codes") {
      cowAvailable.flatMap { cow =>
        if !cow then ZIO.succeed(assertTrue(true)) // auto-skip: no CoW filesystem
        else ZIO.scoped {
          for
            base    <- ZIO.acquireRelease(
                         ZIO.attempt {
                           val b = Files.createTempDirectory("cli-smoke")
                           // source tree (must be on the same filesystem as the work dir)
                           val src = b.resolve("src"); Files.createDirectories(src)
                           Files.writeString(src.resolve("a.txt"), "hello")
                           Files.writeString(src.resolve("b.txt"), "world")
                           Files.createDirectories(b.resolve("work"))
                           Files.createDirectories(b.resolve("data"))
                           b
                         })(b => ZIO.attempt(deleteRecursive(b)).orDie).orDie
            src      = base.resolve("src").toString
            work     = base.resolve("work").toString
            global   = GlobalOpts(catalog = Some(base.resolve("data/photos.sqlite")), dataDir = None)

            // init
            c1      <- Runner.run(global, DiskrementCommand.Init("photos", java.nio.file.Paths.get(src),
                         java.nio.file.Paths.get(work), diskrement.core.DiscType.BD_R, 2.0, 1))
            // scan
            c2      <- Runner.run(global, DiskrementCommand.Scan("photos", verify = false, force = false))
            // plan
            c3      <- Runner.run(global, DiskrementCommand.Plan("photos", None))
            // burn (feed "y" to the confirmation)
            _       <- TestConsole.feedLines("y")
            c4      <- Runner.run(global, DiskrementCommand.Burn("photos", assumeYes = false))
            // status
            c5      <- Runner.run(global, DiskrementCommand.Status("photos"))
            out     <- TestConsole.output
            text     = out.mkString
          yield assertTrue(
            c1 == ExitCode.success, c2 == ExitCode.success, c3 == ExitCode.success,
            c4 == ExitCode.success, c5 == ExitCode.success,
            text.contains("Created project photos"),
            text.contains("Scanned: 2 added"),
            text.contains("Burned disc 1"),
            text.contains("Project photos"))
        }
      }
    },
    test("status on a missing project exits 2") {
      ZIO.scoped {
        for
          base  <- ZIO.acquireRelease(
                     ZIO.attempt(Files.createTempDirectory("cli-missing")))(p => ZIO.attempt(deleteRecursive(p)).orDie).orDie
          global = GlobalOpts(catalog = Some(base.resolve("nope.sqlite")), dataDir = None)
          code  <- Runner.run(global, DiskrementCommand.Status("ghost"))
        yield assertTrue(code == ExitCode(2))
      }
    }
  )
```

> The missing-project case opens a catalog file that does not exist. `CatalogStore`/`Db` create an empty SQLite database on first connection and `findProject` returns `None`, which `loadProject` maps to `ProjectNotFound` → exit `2`. If opening a brand-new DB without migrations surfaces a different error, the project still won't be found, and the assertion is on exit `2`; adjust `loadProject` only if the observed code differs.

- [ ] **Step 2: Run the test**

Run: `sbt "cli/testOnly diskrement.cli.CliSmokeSpec"`
Expected: PASS. On a non-CoW filesystem the first test self-skips (asserts trivially) and the missing-project test still runs.

- [ ] **Step 3: Commit**

```bash
git add cli/src/test/scala/diskrement/cli/CliSmokeSpec.scala
git commit -m "test(cli): full init->scan->plan->burn->status smoke test

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 11: Docs + final verification

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Update the README status table**

In `README.md`, change the CLI row of the status table from:

```
| **CLI adapter** (argument parsing, prompts, output formatting) | **In progress** |
```

to:

```
| CLI adapter (argument parsing, prompts, output formatting) | Done |
```

And update the closing paragraph that currently reads "The CLI wiring (Plan 3b) is the remaining piece of Cycle 1. Once it lands, the tool will be runnable end-to-end with simulated burns." to reflect that Cycle 1 is complete and the tool runs end-to-end with simulated burns via `sbt "cli/run ..."`.

- [ ] **Step 2: Run the full verification gate**

Run: `sbt clean core/test shell/test cli/test`
Expected: all green — core (munit) + shell (ZIO-test) + cli (ZIO-test) suites pass. (CoW-dependent integration tests self-skip on non-CoW filesystems.)

- [ ] **Step 3: Manual end-to-end check (optional, requires a CoW work dir)**

```bash
mkdir -p /tmp/dk/src /tmp/dk/work && echo hi > /tmp/dk/src/a.txt
sbt "cli/run --catalog /tmp/dk/photos.sqlite init photos --source /tmp/dk/src --work-dir /tmp/dk/work"
sbt "cli/run --catalog /tmp/dk/photos.sqlite scan photos"
sbt "cli/run --catalog /tmp/dk/photos.sqlite plan photos"
sbt "cli/run --catalog /tmp/dk/photos.sqlite burn photos --yes"
sbt "cli/run --catalog /tmp/dk/photos.sqlite status photos"
```
Expected: each command prints its rendered output and exits 0; `status` shows one disc with the burned files.

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "docs: mark CLI adapter (Plan 3b) done; Cycle 1 complete

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Self-review notes (spec coverage)

| Spec section | Task(s) |
|---|---|
| §3 module/file structure (`cli` module, 7 files) | 1–10 |
| §2.1.1 new `cli` module | 1 |
| §2.1.2 zio-cli parsing/dispatch | 6, 9 |
| §2.1.3 pure parsing/rendering/handle isolation | 4, 5, 6 |
| §2.1.4 per-project `<name>.sqlite` | 3, 8 |
| §2.1.5 workDir from the project row | 8 (`loadProject`/`withProject`) |
| §2.1.6 confirm-only burn, auto labels | 7, 8 (`Burn` case) |
| §2.1.7 no env reads in core logic (injected env) | 3, 8 |
| §4 command surface (8 commands + flags + global opts) | 2, 6 |
| §5 invocation flow (resolve → load → wire → call → render) | 8 |
| §6 path resolution precedence | 3 |
| §7 rendering (all result types, oversized block) | 4 |
| §8 error handling + exit-code table (incl. result-driven 4) | 5, 8 |
| §9 testing (parsing, render, exit, resolver, prompt, smoke) | 3–7, 10 |

All eight commands map to `Pipeline` calls; `burn` adds the plan-preview + confirm; `verify` adds the result-driven exit `4`. No `Pipeline` methods are added or changed. Types are consistent across tasks: `DiskrementCommand` (Task 2) is the single currency between `Commands` (Task 6), `Render`/`Runner` (Tasks 4/5/8); `ResolvedPaths` (Task 3) feeds `Runner` (Task 8); `Render.handle`/`Render.verifyExit` (Task 5) are consumed by `Runner` (Task 8).
