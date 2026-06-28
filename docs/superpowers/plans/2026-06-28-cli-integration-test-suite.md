# CLI Integration Test Suite Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a hermetic, black-box integration test suite that drives the real `diskrement` CLI binary through full scan→plan→burn→verify cycles and asserts on the contents of the discs it stages.

**Architecture:** A new opt-in sbt `IntegrationTest` config on the `cli` subproject runs ZIO-Test specs that exec the staged launcher via `ProcessBuilder`. Each test creates a unique scratch dir under the working directory (assumed btrfs), runs CLI commands against an isolated catalog/work-dir, asserts on the freshly-staged `disc-N/` tree immediately after each burn, and deletes its scratch in a finalizer. A small test-only env var (`DISKREMENT_DISC_BYTES`) shrinks disc capacity so multi-disc scenarios run at KB scale.

**Tech Stack:** Scala 3.3, ZIO 2.1 + zio-test, sbt-native-packager (`stage`), zio-json (production codecs for on-disc metadata), sqlite-jdbc (read the on-disc catalog snapshot).

**Spec:** `docs/superpowers/specs/2026-06-28-cli-integration-test-suite-design.md`

---

## File Structure

**Production change (one file):**
- Modify `shell/src/main/scala/diskrement/shell/pipeline/PipelineLive.scala` — read `DISKREMENT_DISC_BYTES` in `plan` to override disc capacity.
- Test `shell/src/test/scala/diskrement/shell/pipeline/PipelineCapacityOverrideSpec.scala`.

**Build:**
- Modify `build.sbt` — add `IntegrationTest` config, deps, `stage` wiring, and a `-Ddiskrement.bin=...` system property to the `cli` project.

**Integration test harness (all under `cli/src/it/scala/diskrement/cli/it/`):**
- `Scratch.scala` — per-test scratch directory layout + create/delete.
- `CoW.scala` — reflink-capability probe.
- `Cli.scala` — exec the staged binary with injected env; capture exit/stdout/stderr.
- `Tree.scala` — generate/modify/delete files with unique random content.
- `DiscOracle.scala` — read a staged `disc-N/` and expose facts (data tree, manifest, project/pointers JSON, catalog table count, byte-equality vs source).
- `Support.scala` — shared ZIO helpers: scratch resource, `run`, CoW skip aspect.
- `CliIntegrationSpec.scala` — composes all scenario suites; applies sequential + CoW-skip aspects; final no-leftovers check.
- `scenarios/Scenario01Happy.scala` … `Scenario08Idempotent.scala` — one suite each.

---

## Task 1: sbt IntegrationTest config + build smoke test

**Files:**
- Modify: `build.sbt` (the `cli` project block, lines 36-49)
- Test: `cli/src/it/scala/diskrement/cli/it/BuildSmokeSpec.scala`

- [ ] **Step 1: Write the failing build-smoke test**

Create `cli/src/it/scala/diskrement/cli/it/BuildSmokeSpec.scala`:

```scala
package diskrement.cli.it

import zio.*
import zio.test.*
import java.nio.file.{Files, Paths}

/** Proves the IntegrationTest config is wired: the staged binary path is passed
  * via -Ddiskrement.bin and the launcher runs. */
object BuildSmokeSpec extends ZIOSpecDefault:
  def spec = suite("build smoke")(
    test("diskrement.bin system property points at an existing executable"):
      ZIO.attempt {
        val bin = sys.props.getOrElse("diskrement.bin", "")
        (bin, Files.isRegularFile(Paths.get(bin)))
      }.map { case (bin, exists) =>
        assertTrue(bin.nonEmpty, exists)
      }
    ,
    test("`diskrement --help` exits 0"):
      ZIO.attemptBlocking {
        val p = new ProcessBuilder(sys.props("diskrement.bin"), "--help")
          .redirectOutput(ProcessBuilder.Redirect.DISCARD)
          .redirectError(ProcessBuilder.Redirect.DISCARD)
          .start()
        p.waitFor()
      }.map(code => assertTrue(code == 0))
  )
```

- [ ] **Step 2: Run it to verify it fails**

Run: `sbt "cli/IntegrationTest/test"`
Expected: FAIL — sbt errors that the `IntegrationTest` config/task does not exist on `cli` (no `it` sources are compiled yet).

- [ ] **Step 3: Wire the build**

Replace the `cli` project block in `build.sbt` (currently lines 36-49) with:

```scala
lazy val cli = (project in file("cli"))
  .dependsOn(shell)
  .enablePlugins(JavaAppPackaging)
  .configs(IntegrationTest)
  .settings(
    name := "diskrement",
    executableScriptName := "diskrement",
    Compile / mainClass := Some("diskrement.cli.Main"),
    libraryDependencies ++= Seq(
      "dev.zio" %% "zio-cli"      % zioCliVersion,
      "dev.zio" %% "zio-test"     % zioVersion % Test,
      "dev.zio" %% "zio-test-sbt" % zioVersion % Test,
      "dev.zio" %% "zio-test"     % zioVersion % IntegrationTest,
      "dev.zio" %% "zio-test-sbt" % zioVersion % IntegrationTest
    ),
    testFrameworks += new TestFramework("zio.test.sbt.ZTestFramework"),
    Defaults.itSettings,
    IntegrationTest / fork := true,
    // Build the launcher before IT runs, and hand its path to the tests.
    IntegrationTest / javaOptions += {
      val stagedDir = (Universal / stage).value
      s"-Ddiskrement.bin=${(stagedDir / "bin" / "diskrement").getAbsolutePath}"
    }
  )
```

Note: `testFrameworks` is project-scoped (unscoped), so the IntegrationTest config inherits the ZIO test framework. Referencing `(Universal / stage).value` inside `javaOptions` makes `stage` a build dependency of the IT run, so the binary is always current.

- [ ] **Step 4: Run it to verify it passes**

Run: `sbt "cli/IntegrationTest/test"`
Expected: PASS — both build-smoke tests green (sbt stages the binary first).

- [ ] **Step 5: Commit**

```bash
git add build.sbt cli/src/it/scala/diskrement/cli/it/BuildSmokeSpec.scala
git commit -m "test(cli): add opt-in IntegrationTest config and build smoke test"
```

---

## Task 2: Test-only disc-capacity override

**Files:**
- Modify: `shell/src/main/scala/diskrement/shell/pipeline/PipelineLive.scala:97-98`
- Test: `shell/src/test/scala/diskrement/shell/pipeline/PipelineCapacityOverrideSpec.scala`

- [ ] **Step 1: Write the failing test**

Create `shell/src/test/scala/diskrement/shell/pipeline/PipelineCapacityOverrideSpec.scala`:

```scala
package diskrement.shell.pipeline

import diskrement.core.*
import zio.*
import zio.test.*
import java.time.Instant

object PipelineCapacityOverrideSpec extends ZIOSpecDefault:
  private def initOf(name: String, src: java.nio.file.Path, work: java.nio.file.Path) =
    ProjectInit(name, src.toString, work.toString, DiscType.BD_R, 2.0, 1,
      Instant.parse("2026-06-01T00:00:00Z"))

  // Two 60-byte files: trivially fit a real BD-R together, but with a 100-byte
  // override the second spills to another disc.
  private val files = Map("a.bin" -> ("a" * 60), "b.bin" -> ("b" * 60))

  def spec = suite("DISKREMENT_DISC_BYTES override")(
    test("absent: everything fits one disc (no remaining)"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(files)
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          _     <- pipe.scan("p", verify = false, force = false)
          plan  <- pipe.plan("p", None, finalize = false)
        yield assertTrue(plan.remaining.moreDiscs == 0)
      }
    ,
    test("set to 100: the plan spills to more discs"):
      ZIO.scoped {
        for
          _     <- TestSystem.putEnv("DISKREMENT_DISC_BYTES", "100")
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(files)
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("q", src, paths.workDir))
          _     <- pipe.scan("q", verify = false, force = false)
          plan  <- pipe.plan("q", None, finalize = false)
        yield assertTrue(plan.remaining.moreDiscs >= 1)
      }
  ).provideLayer(PipelineTestKit.env.orDie)
```

Notes:
- `TestSystem.putEnv` is scoped to its own test fiber, so the first test (no
  `putEnv`) sees an empty env and exercises the "absent" path. ZIO Test supplies
  `TestSystem` as a default service, so `zio.System.env` in `PipelineLive` reads
  the injected value even though `provideLayer` only supplies the pipeline
  services.

- [ ] **Step 2: Run test to verify it fails**

Run: `sbt "shell/testOnly diskrement.shell.pipeline.PipelineCapacityOverrideSpec"`
Expected: FAIL — the "set to 100" case still reports `moreDiscs == 0` because the override is not read yet.

- [ ] **Step 3: Implement the override**

In `shell/src/main/scala/diskrement/shell/pipeline/PipelineLive.scala`, replace these two lines inside `plan` (currently lines 97-98):

```scala
      discType = discTypeOverride.getOrElse(p.defaultDiscType)
      cap      = Capacity.fromDiscType(discType, p.marginPercent)
```

with:

```scala
      // Test-only: DISKREMENT_DISC_BYTES overrides usable capacity directly
      // (bypassing margin) so multi-disc scenarios are cheap and deterministic.
      // Not documented in --help; read via zio.System so TestSystem can inject it.
      capBytes <- zio.System.env("DISKREMENT_DISC_BYTES").mapError(PipelineError.Failed(_))
                    .map(_.flatMap(_.toLongOption).filter(_ > 0L))
      discType  = discTypeOverride.getOrElse(p.defaultDiscType)
      cap       = capBytes.fold(Capacity.fromDiscType(discType, p.marginPercent))(Capacity(_))
```

(`zio.System` is fully qualified to avoid clashing with `java.lang.System`. `Capacity` is already imported via `import diskrement.core.*`.)

- [ ] **Step 4: Run test to verify it passes**

Run: `sbt "shell/testOnly diskrement.shell.pipeline.PipelineCapacityOverrideSpec"`
Expected: PASS — both cases green.

- [ ] **Step 5: Run the full shell suite to confirm no regression**

Run: `sbt shell/test`
Expected: PASS — existing pipeline specs unaffected (they set no env var, so they take the `Capacity.fromDiscType` path).

- [ ] **Step 6: Commit**

```bash
git add shell/src/main/scala/diskrement/shell/pipeline/PipelineLive.scala \
        shell/src/test/scala/diskrement/shell/pipeline/PipelineCapacityOverrideSpec.scala
git commit -m "feat(pipeline): test-only DISKREMENT_DISC_BYTES capacity override"
```

---

## Task 3: Harness — Scratch, CoW, Cli, Support

**Files:**
- Create: `cli/src/it/scala/diskrement/cli/it/Scratch.scala`
- Create: `cli/src/it/scala/diskrement/cli/it/CoW.scala`
- Create: `cli/src/it/scala/diskrement/cli/it/Cli.scala`
- Create: `cli/src/it/scala/diskrement/cli/it/Support.scala`
- Test: `cli/src/it/scala/diskrement/cli/it/HarnessSpec.scala`

- [ ] **Step 1: Write the failing harness test**

Create `cli/src/it/scala/diskrement/cli/it/HarnessSpec.scala`:

```scala
package diskrement.cli.it

import zio.*
import zio.test.*
import java.nio.file.Files

/** Exercises Scratch + Cli end-to-end with a real `init`, and proves the
  * scratch directory is removed afterward. */
object HarnessSpec extends ZIOSpecDefault:
  def spec = suite("harness")(
    test("init in an isolated scratch succeeds and leaves nothing behind"):
      ZIO.scoped {
        for
          s   <- Support.scratch
          cli  = new Cli(s, discBytes = None)
          r   <- Support.run(cli, "init", "proj",
                   "--source", s.source.toString, "--work-dir", s.work.toString)
          dirExistsDuring <- ZIO.attempt(Files.isDirectory(s.dir))
        yield assertTrue(
          r.exitCode == 0,
          r.stdout.contains("Created project proj"),
          dirExistsDuring)
      }
  ) @@ Support.cowOrIgnore @@ TestAspect.sequential
```

- [ ] **Step 2: Run it to verify it fails**

Run: `sbt "cli/IntegrationTest/testOnly diskrement.cli.it.HarnessSpec"`
Expected: FAIL — `Scratch`, `Cli`, `Support` are not defined (compile error).

- [ ] **Step 3: Implement `Scratch`**

Create `cli/src/it/scala/diskrement/cli/it/Scratch.scala`:

```scala
package diskrement.cli.it

import java.nio.file.{Files, Path, Paths}
import scala.jdk.StreamConverters.*

/** Per-test scratch layout. Everything lives under one unique dir so cleanup is
  * a single recursive delete. The catalog is a fixed file we always pass via
  * --catalog, so the system-wide default location is never touched. */
final case class Scratch(dir: Path):
  val source: Path  = dir.resolve("source")
  val work: Path    = dir.resolve("work")
  val catalog: Path = dir.resolve("catalog/cat.sqlite")
  val home: Path    = dir.resolve("home")
  val xdg: Path     = dir.resolve("home/.local/share")
  val state: Path   = dir.resolve("home/state")
  def discDir(ordinal: Int): Path = work.resolve(s"staging/disc-$ordinal")

object Scratch:
  /** Base under the working directory, which the suite assumes is on btrfs. */
  val base: Path = Paths.get("target", "it-scratch")

  def make(): Scratch =
    Files.createDirectories(base)
    val s = Scratch(Files.createTempDirectory(base, "it-"))
    List(s.source, s.work, s.catalog.getParent, s.home, s.xdg, s.state)
      .foreach(Files.createDirectories(_))
    s

  def delete(s: Scratch): Unit = deleteRecursive(s.dir)

  def deleteRecursive(dir: Path): Unit =
    if Files.exists(dir) then
      val stream = Files.walk(dir)
      try stream.sorted(java.util.Comparator.reverseOrder()).forEach(p => Files.deleteIfExists(p))
      finally stream.close()
```

- [ ] **Step 4: Implement `CoW`**

Create `cli/src/it/scala/diskrement/cli/it/CoW.scala`:

```scala
package diskrement.cli.it

import java.nio.file.{Files, Path}

/** Probes whether `dir`'s filesystem supports reflink copies (btrfs/xfs/bcachefs).
  * diskrement's real SourceSnapshot requires CoW; without it the suite skips. */
object CoW:
  def capable(dir: Path): Boolean =
    Files.createDirectories(dir)
    val a = dir.resolve(".cow-probe-src")
    val b = dir.resolve(".cow-probe-dst")
    try
      Files.write(a, Array[Byte](1, 2, 3, 4))
      Files.deleteIfExists(b)
      val proc = new ProcessBuilder("cp", "--reflink=always", a.toString, b.toString)
        .redirectOutput(ProcessBuilder.Redirect.DISCARD)
        .redirectError(ProcessBuilder.Redirect.DISCARD)
        .start()
      proc.waitFor() == 0
    catch case _: Throwable => false
    finally
      Files.deleteIfExists(a)
      Files.deleteIfExists(b)
```

- [ ] **Step 5: Implement `Cli`**

Create `cli/src/it/scala/diskrement/cli/it/Cli.scala`:

```scala
package diskrement.cli.it

import java.nio.charset.StandardCharsets.UTF_8
import java.nio.file.Files

final case class CliResult(exitCode: Int, stdout: String, stderr: String)

/** Execs the staged launcher with an isolated environment. The catalog is always
  * forced via --catalog; HOME/XDG_DATA_HOME/STATE_DIRECTORY are redirected into
  * the scratch dir so even a path-resolution bug cannot touch real locations. */
final class Cli(scratch: Scratch, discBytes: Option[Long]):
  private val bin: String =
    sys.props.getOrElse("diskrement.bin",
      sys.error("diskrement.bin not set — run via `sbt cli/IntegrationTest/test`"))

  def run(args: String*): CliResult =
    val cmd = (Seq(bin, "--catalog", scratch.catalog.toString) ++ args)
    val pb  = new ProcessBuilder(cmd*)
    val env = pb.environment()
    env.put("HOME", scratch.home.toString)
    env.put("XDG_DATA_HOME", scratch.xdg.toString)
    env.put("STATE_DIRECTORY", scratch.state.toString)
    discBytes.foreach(b => env.put("DISKREMENT_DISC_BYTES", b.toString))

    val outF = Files.createTempFile(scratch.dir, "out", ".txt")
    val errF = Files.createTempFile(scratch.dir, "err", ".txt")
    pb.redirectOutput(outF.toFile)
    pb.redirectError(errF.toFile)
    val code = pb.start().waitFor()
    val out  = new String(Files.readAllBytes(outF), UTF_8)
    val err  = new String(Files.readAllBytes(errF), UTF_8)
    Files.deleteIfExists(outF)
    Files.deleteIfExists(errF)
    CliResult(code, out, err)
```

- [ ] **Step 6: Implement `Support`**

Create `cli/src/it/scala/diskrement/cli/it/Support.scala`:

```scala
package diskrement.cli.it

import zio.*
import zio.test.*

/** Shared ZIO helpers for the integration scenarios. */
object Support:
  /** A scratch dir scoped to the test: created on acquire, deleted on release
    * (runs even if the test fails). */
  val scratch: ZIO[Scope, Throwable, Scratch] =
    ZIO.acquireRelease(ZIO.attempt(Scratch.make()))(s => ZIO.succeed(Scratch.delete(s)))

  /** Run one CLI command on the blocking pool. */
  def run(cli: Cli, args: String*): Task[CliResult] =
    ZIO.attemptBlocking(cli.run(args*))

  /** Skip the whole suite cleanly when the working directory is not CoW-capable. */
  val cowOrIgnore: TestAspectPoly =
    if CoW.capable(Scratch.base.resolve(".cow-check")) then TestAspect.identity
    else TestAspect.ignore
```

- [ ] **Step 7: Run the harness test to verify it passes**

Run: `sbt "cli/IntegrationTest/testOnly diskrement.cli.it.HarnessSpec"`
Expected: PASS on a btrfs working dir (or IGNORED with a skip indication elsewhere).

- [ ] **Step 8: Commit**

```bash
git add cli/src/it/scala/diskrement/cli/it/Scratch.scala \
        cli/src/it/scala/diskrement/cli/it/CoW.scala \
        cli/src/it/scala/diskrement/cli/it/Cli.scala \
        cli/src/it/scala/diskrement/cli/it/Support.scala \
        cli/src/it/scala/diskrement/cli/it/HarnessSpec.scala
git commit -m "test(cli): integration harness (scratch, cow probe, cli runner)"
```

---

## Task 4: Tree, DiscOracle, and Scenario 1 (single-disc happy path)

**Files:**
- Create: `cli/src/it/scala/diskrement/cli/it/Tree.scala`
- Create: `cli/src/it/scala/diskrement/cli/it/DiscOracle.scala`
- Create: `cli/src/it/scala/diskrement/cli/it/scenarios/Scenario01Happy.scala`
- Create: `cli/src/it/scala/diskrement/cli/it/CliIntegrationSpec.scala`

- [ ] **Step 1: Write the failing scenario + runner**

Create `cli/src/it/scala/diskrement/cli/it/scenarios/Scenario01Happy.scala`:

```scala
package diskrement.cli.it
package scenarios

import zio.*
import zio.test.*

object Scenario01Happy:
  val suite = zio.test.suite("scenario 1: single-disc happy path")(
    test("init -> scan -> plan -> burn -> verify; disc contents == source"):
      ZIO.scoped {
        for
          s   <- Support.scratch
          cli  = new Cli(s, discBytes = None)
          _    = Tree.write(s.source, "docs/a.txt", 256)
          _    = Tree.write(s.source, "img/b.bin", 512)
          _   <- Support.run(cli, "init", "p",
                   "--source", s.source.toString, "--work-dir", s.work.toString)
          scan <- Support.run(cli, "scan", "p")
          plan <- Support.run(cli, "plan", "p")
          burn <- Support.run(cli, "burn", "p", "--yes")
          // Oracle: read the freshly-staged disc 1 right now.
          d1   = s.discDir(1)
          data = DiscOracle.dataFiles(d1)
          man  = DiscOracle.manifest(d1)
          proj = DiscOracle.projectJson(d1)
          pts  = DiscOracle.pointers(d1)
          aOk  = DiscOracle.contentMatches(d1, s.source, "docs/a.txt")
          bOk  = DiscOracle.contentMatches(d1, s.source, "img/b.bin")
          tabs = DiscOracle.catalogTableCount(d1)
          vrfy <- Support.run(cli, "verify", "p")
          stat <- Support.run(cli, "status", "p")
        yield assertTrue(
          scan.exitCode == 0,
          scan.stdout.contains("Scanned: 2 added, 0 modified, 0 deleted, 0 unchanged."),
          plan.exitCode == 0,
          plan.stdout.contains("Plan: disc 1") && plan.stdout.contains("new disc"),
          burn.exitCode == 0,
          burn.stdout.contains("Wrote disc 1"),
          data == Set("docs/a.txt", "img/b.bin"),
          DiscOracle.manifestFiles(man) == data,
          man.discOrdinal == 1,
          man.sessionOrdinal == 1,
          man.added.map(_.relPath).toSet == Set("docs/a.txt", "img/b.bin"),
          man.deleted.isEmpty,
          proj.projectName == "p",
          pts.pointers.nonEmpty,
          aOk, bOk,
          tabs > 0,
          vrfy.exitCode == 0,
          vrfy.stdout.contains("files OK"),
          stat.stdout.contains("Current files: 2"))
      }
  )
```

Create `cli/src/it/scala/diskrement/cli/it/CliIntegrationSpec.scala`:

```scala
package diskrement.cli.it

import zio.*
import zio.test.*
import scala.jdk.StreamConverters.*
import java.nio.file.Files

/** Entry point: composes every scenario suite, runs them sequentially, and
  * skips cleanly when the working directory lacks CoW support. */
object CliIntegrationSpec extends ZIOSpecDefault:
  private val noLeftovers =
    test("leaves no scratch directories behind"):
      ZIO.attempt {
        if !Files.exists(Scratch.base) then Nil
        else
          val stream = Files.list(Scratch.base)
          try stream.toScala(List)
            .filter(p => Files.isDirectory(p) && p.getFileName.toString.startsWith("it-"))
          finally stream.close()
      }.map(leftover => assertTrue(leftover.isEmpty))

  def spec = suite("diskrement CLI integration")(
    scenarios.Scenario01Happy.suite,
    noLeftovers
  ) @@ Support.cowOrIgnore @@ TestAspect.sequential
```

- [ ] **Step 2: Run it to verify it fails**

Run: `sbt "cli/IntegrationTest/testOnly diskrement.cli.it.CliIntegrationSpec"`
Expected: FAIL — `Tree` and `DiscOracle` are undefined (compile error).

- [ ] **Step 3: Implement `Tree`**

Create `cli/src/it/scala/diskrement/cli/it/Tree.scala`:

```scala
package diskrement.cli.it

import java.nio.file.{Files, Path}

/** Generates files with deterministic, per-(path,version) unique random content,
  * so two same-size files never collide and runs are reproducible. */
object Tree:
  private def bytes(relPath: String, version: Int, size: Int): Array[Byte] =
    val rnd = new java.util.Random(s"$relPath@$version".hashCode.toLong)
    val buf = new Array[Byte](size)
    rnd.nextBytes(buf)
    buf

  /** Create (or overwrite) `relPath` under `root` with version-0 content. */
  def write(root: Path, relPath: String, size: Int): Unit =
    val p = root.resolve(relPath)
    Files.createDirectories(p.getParent)
    Files.write(p, bytes(relPath, 0, size))

  /** Overwrite `relPath` with version-1 content (distinct from `write`). */
  def modify(root: Path, relPath: String, size: Int): Unit =
    val p = root.resolve(relPath)
    Files.createDirectories(p.getParent)
    Files.write(p, bytes(relPath, 1, size))

  def delete(root: Path, relPath: String): Unit =
    Files.deleteIfExists(root.resolve(relPath))
```

- [ ] **Step 4: Implement `DiscOracle`**

Create `cli/src/it/scala/diskrement/cli/it/DiscOracle.scala`:

```scala
package diskrement.cli.it

import diskrement.shell.burn.{ManifestJson, PointersJson, ProjectJson}
import zio.json.*
import scala.jdk.StreamConverters.*
import java.nio.file.{Files, Path}

/** Reads a staged `disc-N/` directory and exposes facts for assertions. Uses the
  * production zio-json codecs so the on-disc format stays honest. */
object DiscOracle:
  /** Relative paths of all regular files under `disc-N/data`. */
  def dataFiles(discDir: Path): Set[String] =
    val data = discDir.resolve("data")
    if !Files.exists(data) then Set.empty
    else
      val stream = Files.walk(data)
      try stream.toScala(List)
        .filter(Files.isRegularFile(_))
        .map(p => data.relativize(p).toString)
        .toSet
      finally stream.close()

  def manifest(discDir: Path): ManifestJson =
    decode[ManifestJson](discDir.resolve(".diskrement/manifest.json"))

  def projectJson(discDir: Path): ProjectJson =
    decode[ProjectJson](discDir.resolve(".diskrement/project.json"))

  def pointers(discDir: Path): PointersJson =
    decode[PointersJson](discDir.resolve(".diskrement/pointers.json"))

  /** Files staged on this disc, per the manifest (added ++ modified). */
  def manifestFiles(m: ManifestJson): Set[String] =
    (m.added.map(_.relPath) ++ m.modified.map(_.relPath)).toSet

  /** True if the staged copy of `relPath` is byte-identical to the source. */
  def contentMatches(discDir: Path, source: Path, relPath: String): Boolean =
    val staged = discDir.resolve("data").resolve(relPath)
    val src    = source.resolve(relPath)
    Files.exists(staged) && Files.exists(src) && Files.mismatch(staged, src) == -1L

  /** Number of tables in the embedded catalog snapshot (proves it opens). */
  def catalogTableCount(discDir: Path): Int =
    val url  = s"jdbc:sqlite:${discDir.resolve(".diskrement/catalog.sqlite")}"
    val conn = java.sql.DriverManager.getConnection(url)
    try
      val rs = conn.createStatement()
        .executeQuery("select count(*) from sqlite_master where type='table'")
      rs.next()
      rs.getInt(1)
    finally conn.close()

  private def decode[A: JsonDecoder](path: Path): A =
    Files.readString(path).fromJson[A].fold(err => sys.error(s"$path: $err"), identity)
```

- [ ] **Step 5: Run it to verify it passes**

Run: `sbt "cli/IntegrationTest/testOnly diskrement.cli.it.CliIntegrationSpec"`
Expected: PASS — scenario 1 green and the no-leftovers check passes.

- [ ] **Step 6: Commit**

```bash
git add cli/src/it/scala/diskrement/cli/it/Tree.scala \
        cli/src/it/scala/diskrement/cli/it/DiscOracle.scala \
        cli/src/it/scala/diskrement/cli/it/scenarios/Scenario01Happy.scala \
        cli/src/it/scala/diskrement/cli/it/CliIntegrationSpec.scala
git commit -m "test(cli): tree + disc oracle + scenario 1 (happy path)"
```

---

## Task 5: Scenario 2 — append to an open disc

**Files:**
- Create: `cli/src/it/scala/diskrement/cli/it/scenarios/Scenario02AppendOpen.scala`
- Modify: `cli/src/it/scala/diskrement/cli/it/CliIntegrationSpec.scala` (add to suite list)

- [ ] **Step 1: Write the scenario**

Create `cli/src/it/scala/diskrement/cli/it/scenarios/Scenario02AppendOpen.scala`:

```scala
package diskrement.cli.it
package scenarios

import zio.*
import zio.test.*

/** Disc 1 is left OPEN after cycle 1; cycle 2's changes (add/modify/delete) are
  * written as session 2 to the SAME disc. Staging holds only session 2's files;
  * the cumulative current set is checked via status/history. */
object Scenario02AppendOpen:
  val suite = zio.test.suite("scenario 2: append to open disc")(
    test("second session lands on disc 1 (ordinal 1, session 2)"):
      ZIO.scoped {
        for
          s   <- Support.scratch
          cli  = new Cli(s, discBytes = None)
          _    = Tree.write(s.source, "a.txt", 128)
          _    = Tree.write(s.source, "b.txt", 128)
          _   <- Support.run(cli, "init", "p",
                   "--source", s.source.toString, "--work-dir", s.work.toString)
          // cycle 1
          _   <- Support.run(cli, "scan", "p")
          _   <- Support.run(cli, "plan", "p")
          b1  <- Support.run(cli, "burn", "p", "--yes")
          d1man1 = DiscOracle.manifest(s.discDir(1))
          d1data1 = DiscOracle.dataFiles(s.discDir(1))
          // cycle 2: modify a, add c, delete b
          _    = Tree.modify(s.source, "a.txt", 128)
          _    = Tree.write(s.source, "c.txt", 64)
          _    = Tree.delete(s.source, "b.txt")
          sc2 <- Support.run(cli, "scan", "p")
          pl2 <- Support.run(cli, "plan", "p")
          b2  <- Support.run(cli, "burn", "p", "--yes")
          d1man2  = DiscOracle.manifest(s.discDir(1))
          d1data2 = DiscOracle.dataFiles(s.discDir(1))
          hA  <- Support.run(cli, "history", "p", "a.txt")
          hB  <- Support.run(cli, "history", "p", "b.txt")
          st  <- Support.run(cli, "status", "p")
        yield assertTrue(
          // cycle 1: disc 1 open, session 1, both files
          b1.stdout.contains("Wrote disc 1") && b1.stdout.contains("left OPEN"),
          d1man1.sessionOrdinal == 1,
          d1data1 == Set("a.txt", "b.txt"),
          // cycle 2 scan/plan
          sc2.stdout.contains("Scanned: 1 added, 1 modified, 1 deleted, 0 unchanged."),
          pl2.stdout.contains("appending to open disc 1"),
          // cycle 2 staged disc 1 now holds only session 2's files
          b2.stdout.contains("Wrote disc 1") && b2.stdout.contains("left OPEN"),
          d1man2.discOrdinal == 1,
          d1man2.sessionOrdinal == 2,
          d1data2 == Set("a.txt", "c.txt"),
          d1man2.added.map(_.relPath).toSet == Set("c.txt"),
          d1man2.modified.map(_.relPath).toSet == Set("a.txt"),
          d1man2.deleted.toSet == Set("b.txt"),
          // cumulative view via history + status
          hA.stdout.contains("SUPERSEDED") && hA.stdout.contains("CURRENT"),
          hB.stdout.contains("DELETED"),
          st.stdout.contains("Current files: 2"))
      }
  )
```

- [ ] **Step 2: Register it**

In `cli/src/it/scala/diskrement/cli/it/CliIntegrationSpec.scala`, change the suite list from:

```scala
    scenarios.Scenario01Happy.suite,
    noLeftovers
```

to:

```scala
    scenarios.Scenario01Happy.suite,
    scenarios.Scenario02AppendOpen.suite,
    noLeftovers
```

- [ ] **Step 3: Run it to verify it passes**

Run: `sbt "cli/IntegrationTest/testOnly diskrement.cli.it.CliIntegrationSpec"`
Expected: PASS — scenarios 1 and 2 green.

- [ ] **Step 4: Commit**

```bash
git add cli/src/it/scala/diskrement/cli/it/scenarios/Scenario02AppendOpen.scala \
        cli/src/it/scala/diskrement/cli/it/CliIntegrationSpec.scala
git commit -m "test(cli): scenario 2 (append to open disc)"
```

---

## Task 6: Scenario 3 — finalized disc forces a new disc

**Files:**
- Create: `cli/src/it/scala/diskrement/cli/it/scenarios/Scenario03FinalizedNewDisc.scala`
- Modify: `cli/src/it/scala/diskrement/cli/it/CliIntegrationSpec.scala`

- [ ] **Step 1: Write the scenario**

Create `cli/src/it/scala/diskrement/cli/it/scenarios/Scenario03FinalizedNewDisc.scala`:

```scala
package diskrement.cli.it
package scenarios

import zio.*
import zio.test.*

/** `plan --finalize` seals disc 1 even though it has room; the next cycle's
  * change must open a fresh disc 2. */
object Scenario03FinalizedNewDisc:
  val suite = zio.test.suite("scenario 3: finalized -> new disc")(
    test("after finalize, new changes go to disc 2"):
      ZIO.scoped {
        for
          s   <- Support.scratch
          cli  = new Cli(s, discBytes = None)
          _    = Tree.write(s.source, "a.txt", 128)
          _   <- Support.run(cli, "init", "p",
                   "--source", s.source.toString, "--work-dir", s.work.toString)
          // cycle 1: finalize disc 1
          _   <- Support.run(cli, "scan", "p")
          pl1 <- Support.run(cli, "plan", "p", "--finalize")
          b1  <- Support.run(cli, "burn", "p", "--yes")
          d1data = DiscOracle.dataFiles(s.discDir(1))
          d1man  = DiscOracle.manifest(s.discDir(1))
          // cycle 2: add b -> must be a new disc
          _    = Tree.write(s.source, "b.txt", 128)
          _   <- Support.run(cli, "scan", "p")
          pl2 <- Support.run(cli, "plan", "p")
          b2  <- Support.run(cli, "burn", "p", "--yes")
          d2data = DiscOracle.dataFiles(s.discDir(2))
          d2man  = DiscOracle.manifest(s.discDir(2))
          hA  <- Support.run(cli, "history", "p", "a.txt")
        yield assertTrue(
          pl1.stdout.contains("will FINALIZE"),
          b1.stdout.contains("FINALIZED"),
          d1data == Set("a.txt"),
          d1man.discOrdinal == 1,
          pl2.stdout.contains("new disc"),
          b2.stdout.contains("Wrote disc 2"),
          d2data == Set("b.txt"),
          d2man.discOrdinal == 2,
          d2man.sessionOrdinal == 1,
          hA.stdout.contains("CURRENT"))
      }
  )
```

- [ ] **Step 2: Register it**

In `CliIntegrationSpec.scala`, add `scenarios.Scenario03FinalizedNewDisc.suite,` immediately before `noLeftovers`.

- [ ] **Step 3: Run it to verify it passes**

Run: `sbt "cli/IntegrationTest/testOnly diskrement.cli.it.CliIntegrationSpec"`
Expected: PASS — scenarios 1-3 green.

- [ ] **Step 4: Commit**

```bash
git add cli/src/it/scala/diskrement/cli/it/scenarios/Scenario03FinalizedNewDisc.scala \
        cli/src/it/scala/diskrement/cli/it/CliIntegrationSpec.scala
git commit -m "test(cli): scenario 3 (finalized disc forces new disc)"
```

---

## Task 7: Scenario 4 — three-disc incremental with add/modify/delete

**Files:**
- Create: `cli/src/it/scala/diskrement/cli/it/scenarios/Scenario04ThreeDisc.scala`
- Modify: `cli/src/it/scala/diskrement/cli/it/CliIntegrationSpec.scala`

- [ ] **Step 1: Write the scenario**

Uses `discBytes = Some(1000)`. Each file is 800 bytes, so only one fits per disc (200 free is never enough for the next 800-byte file), giving one disc per cycle deterministically.

Create `cli/src/it/scala/diskrement/cli/it/scenarios/Scenario04ThreeDisc.scala`:

```scala
package diskrement.cli.it
package scenarios

import zio.*
import zio.test.*

/** Three cycles, each producing one disc under a 1000-byte capacity override.
  * Cycle 3 modifies a and deletes b, landing the modified a on disc 3. */
object Scenario04ThreeDisc:
  val suite = zio.test.suite("scenario 4: three-disc incremental")(
    test("a -> b -> (modify a, delete b) span discs 1,2,3 with correct history"):
      ZIO.scoped {
        for
          s   <- Support.scratch
          cli  = new Cli(s, discBytes = Some(1000L))
          _   <- ZIO.attempt(Tree.write(s.source, "a.bin", 800))
          _   <- Support.run(cli, "init", "p",
                   "--source", s.source.toString, "--work-dir", s.work.toString)
          // cycle 1 -> disc 1 [a]
          _   <- Support.run(cli, "scan", "p")
          _   <- Support.run(cli, "plan", "p")
          _   <- Support.run(cli, "burn", "p", "--yes")
          // cycle 2 -> disc 2 [b]
          _    = Tree.write(s.source, "b.bin", 800)
          _   <- Support.run(cli, "scan", "p")
          pl2 <- Support.run(cli, "plan", "p")
          _   <- Support.run(cli, "burn", "p", "--yes")
          // cycle 3 -> disc 3 [a'] (+ delete b)
          _    = Tree.modify(s.source, "a.bin", 800)
          _    = Tree.delete(s.source, "b.bin")
          sc3 <- Support.run(cli, "scan", "p")
          pl3 <- Support.run(cli, "plan", "p")
          _   <- Support.run(cli, "burn", "p", "--yes")
          d1   = DiscOracle.dataFiles(s.discDir(1))
          d2   = DiscOracle.dataFiles(s.discDir(2))
          d3   = DiscOracle.dataFiles(s.discDir(3))
          m3   = DiscOracle.manifest(s.discDir(3))
          hA  <- Support.run(cli, "history", "p", "a.bin")
          hB  <- Support.run(cli, "history", "p", "b.bin")
          st  <- Support.run(cli, "status", "p")
          vf  <- Support.run(cli, "verify", "p")
        yield assertTrue(
          pl2.stdout.contains("new disc"),
          sc3.stdout.contains("Scanned: 0 added, 1 modified, 1 deleted, 0 unchanged."),
          pl3.stdout.contains("new disc"),
          d1 == Set("a.bin"),
          d2 == Set("b.bin"),
          d3 == Set("a.bin"),
          m3.discOrdinal == 3,
          m3.modified.map(_.relPath).toSet == Set("a.bin"),
          m3.deleted.toSet == Set("b.bin"),
          hA.stdout.contains("SUPERSEDED") && hA.stdout.contains("CURRENT"),
          hB.stdout.contains("DELETED"),
          st.stdout.contains("Current files: 1"),
          vf.exitCode == 0)
      }
  )
```

- [ ] **Step 2: Register it**

In `CliIntegrationSpec.scala`, add `scenarios.Scenario04ThreeDisc.suite,` immediately before `noLeftovers`.

- [ ] **Step 3: Run it to verify it passes**

Run: `sbt "cli/IntegrationTest/testOnly diskrement.cli.it.CliIntegrationSpec"`
Expected: PASS — scenarios 1-4 green.

- [ ] **Step 4: Commit**

```bash
git add cli/src/it/scala/diskrement/cli/it/scenarios/Scenario04ThreeDisc.scala \
        cli/src/it/scala/diskrement/cli/it/CliIntegrationSpec.scala
git commit -m "test(cli): scenario 4 (three-disc incremental)"
```

---

## Task 8: Scenario 5 — spill across cycles partitions the file set

**Files:**
- Create: `cli/src/it/scala/diskrement/cli/it/scenarios/Scenario05Spill.scala`
- Modify: `cli/src/it/scala/diskrement/cli/it/CliIntegrationSpec.scala`

- [ ] **Step 1: Write the scenario**

Three 400-byte files with a 1000-byte cap: two fit disc 1, the third spills to disc 2 in the next cycle. Asserts the discs *partition* the file set without knowing the planner's packing order.

Create `cli/src/it/scala/diskrement/cli/it/scenarios/Scenario05Spill.scala`:

```scala
package diskrement.cli.it
package scenarios

import zio.*
import zio.test.*

object Scenario05Spill:
  val suite = zio.test.suite("scenario 5: spill across cycles")(
    test("three files over two discs: disjoint union == all files"):
      ZIO.scoped {
        for
          s   <- Support.scratch
          cli  = new Cli(s, discBytes = Some(1000L))
          _   <- ZIO.attempt {
                   Tree.write(s.source, "f1.bin", 400)
                   Tree.write(s.source, "f2.bin", 400)
                   Tree.write(s.source, "f3.bin", 400)
                 }
          _   <- Support.run(cli, "init", "p",
                   "--source", s.source.toString, "--work-dir", s.work.toString)
          // cycle 1: disc 1 fills with two of the three files
          sc1 <- Support.run(cli, "scan", "p")
          pl1 <- Support.run(cli, "plan", "p")
          _   <- Support.run(cli, "burn", "p", "--yes")
          d1   = DiscOracle.dataFiles(s.discDir(1))
          // cycle 2: the remaining file goes to disc 2
          sc2 <- Support.run(cli, "scan", "p")
          pl2 <- Support.run(cli, "plan", "p")
          _   <- Support.run(cli, "burn", "p", "--yes")
          d2   = DiscOracle.dataFiles(s.discDir(2))
          st  <- Support.run(cli, "status", "p")
          vf  <- Support.run(cli, "verify", "p")
          all  = Set("f1.bin", "f2.bin", "f3.bin")
        yield assertTrue(
          sc1.stdout.contains("Scanned: 3 added"),
          pl1.stdout.contains("more discs) remain"),
          d1.size == 2,
          pl2.stdout.contains("This disc completes the set."),
          d1.intersect(d2).isEmpty,
          (d1 union d2) == all,
          st.stdout.contains("Current files: 3"),
          vf.exitCode == 0)
      }
  )
```

- [ ] **Step 2: Register it**

In `CliIntegrationSpec.scala`, add `scenarios.Scenario05Spill.suite,` immediately before `noLeftovers`.

- [ ] **Step 3: Run it to verify it passes**

Run: `sbt "cli/IntegrationTest/testOnly diskrement.cli.it.CliIntegrationSpec"`
Expected: PASS — scenarios 1-5 green.

- [ ] **Step 4: Commit**

```bash
git add cli/src/it/scala/diskrement/cli/it/scenarios/Scenario05Spill.scala \
        cli/src/it/scala/diskrement/cli/it/CliIntegrationSpec.scala
git commit -m "test(cli): scenario 5 (spill across cycles)"
```

---

## Task 9: Scenario 6 — oversized file is refused

**Files:**
- Create: `cli/src/it/scala/diskrement/cli/it/scenarios/Scenario06Oversized.scala`
- Modify: `cli/src/it/scala/diskrement/cli/it/CliIntegrationSpec.scala`

- [ ] **Step 1: Write the scenario**

A single 5000-byte file with a 1000-byte cap can never be placed. `plan` reports it as oversized (exit 0); `burn` refuses with exit 3 and stages no disc.

Create `cli/src/it/scala/diskrement/cli/it/scenarios/Scenario06Oversized.scala`:

```scala
package diskrement.cli.it
package scenarios

import zio.*
import zio.test.*
import java.nio.file.Files

object Scenario06Oversized:
  val suite = zio.test.suite("scenario 6: oversized file")(
    test("a file larger than a disc is reported and burn refuses"):
      ZIO.scoped {
        for
          s   <- Support.scratch
          cli  = new Cli(s, discBytes = Some(1000L))
          _   <- ZIO.attempt(Tree.write(s.source, "huge.bin", 5000))
          _   <- Support.run(cli, "init", "p",
                   "--source", s.source.toString, "--work-dir", s.work.toString)
          _   <- Support.run(cli, "scan", "p")
          pl  <- Support.run(cli, "plan", "p")
          bn  <- Support.run(cli, "burn", "p", "--yes")
          staged <- ZIO.attempt(Files.exists(s.discDir(1)))
        yield assertTrue(
          pl.exitCode == 0,
          pl.stdout.contains("Oversized (cannot be placed):"),
          pl.stdout.contains("huge.bin"),
          bn.exitCode == 3,
          bn.stderr.nonEmpty,
          !staged)
      }
  )
```

- [ ] **Step 2: Register it**

In `CliIntegrationSpec.scala`, add `scenarios.Scenario06Oversized.suite,` immediately before `noLeftovers`.

- [ ] **Step 3: Run it to verify it passes**

Run: `sbt "cli/IntegrationTest/testOnly diskrement.cli.it.CliIntegrationSpec"`
Expected: PASS — scenarios 1-6 green.

- [ ] **Step 4: Commit**

```bash
git add cli/src/it/scala/diskrement/cli/it/scenarios/Scenario06Oversized.scala \
        cli/src/it/scala/diskrement/cli/it/CliIntegrationSpec.scala
git commit -m "test(cli): scenario 6 (oversized file refused)"
```

---

## Task 10: Scenario 7 — reset discards staged state

**Files:**
- Create: `cli/src/it/scala/diskrement/cli/it/scenarios/Scenario07Reset.scala`
- Modify: `cli/src/it/scala/diskrement/cli/it/CliIntegrationSpec.scala`

- [ ] **Step 1: Write the scenario**

Create `cli/src/it/scala/diskrement/cli/it/scenarios/Scenario07Reset.scala`:

```scala
package diskrement.cli.it
package scenarios

import zio.*
import zio.test.*

object Scenario07Reset:
  val suite = zio.test.suite("scenario 7: reset")(
    test("reset clears the staged scan; a following plan errors"):
      ZIO.scoped {
        for
          s   <- Support.scratch
          cli  = new Cli(s, discBytes = None)
          _   <- ZIO.attempt(Tree.write(s.source, "a.txt", 64))
          _   <- Support.run(cli, "init", "p",
                   "--source", s.source.toString, "--work-dir", s.work.toString)
          sc  <- Support.run(cli, "scan", "p")
          rs  <- Support.run(cli, "reset", "p")
          pl  <- Support.run(cli, "plan", "p")
        yield assertTrue(
          sc.exitCode == 0,
          rs.exitCode == 0,
          rs.stdout.contains("Reset p."),
          pl.exitCode == 2,
          pl.stderr.nonEmpty)
      }
  )
```

- [ ] **Step 2: Register it**

In `CliIntegrationSpec.scala`, add `scenarios.Scenario07Reset.suite,` immediately before `noLeftovers`.

- [ ] **Step 3: Run it to verify it passes**

Run: `sbt "cli/IntegrationTest/testOnly diskrement.cli.it.CliIntegrationSpec"`
Expected: PASS — scenarios 1-7 green.

- [ ] **Step 4: Commit**

```bash
git add cli/src/it/scala/diskrement/cli/it/scenarios/Scenario07Reset.scala \
        cli/src/it/scala/diskrement/cli/it/CliIntegrationSpec.scala
git commit -m "test(cli): scenario 7 (reset discards staged state)"
```

---

## Task 11: Scenario 8 — idempotent re-scan

**Files:**
- Create: `cli/src/it/scala/diskrement/cli/it/scenarios/Scenario08Idempotent.scala`
- Modify: `cli/src/it/scala/diskrement/cli/it/CliIntegrationSpec.scala`

- [ ] **Step 1: Write the scenario**

Create `cli/src/it/scala/diskrement/cli/it/scenarios/Scenario08Idempotent.scala`:

```scala
package diskrement.cli.it
package scenarios

import zio.*
import zio.test.*

object Scenario08Idempotent:
  val suite = zio.test.suite("scenario 8: idempotent re-scan")(
    test("re-scanning an unchanged source reports all-unchanged and plans nothing"):
      ZIO.scoped {
        for
          s   <- Support.scratch
          cli  = new Cli(s, discBytes = None)
          _   <- ZIO.attempt {
                   Tree.write(s.source, "a.txt", 64)
                   Tree.write(s.source, "b.txt", 64)
                 }
          _   <- Support.run(cli, "init", "p",
                   "--source", s.source.toString, "--work-dir", s.work.toString)
          _   <- Support.run(cli, "scan", "p")
          _   <- Support.run(cli, "plan", "p")
          _   <- Support.run(cli, "burn", "p", "--yes")
          // source unchanged; staged state was cleared by burn, so this re-scans
          sc2 <- Support.run(cli, "scan", "p")
          pl2 <- Support.run(cli, "plan", "p")
        yield assertTrue(
          sc2.exitCode == 0,
          sc2.stdout.contains("Scanned: 0 added, 0 modified, 0 deleted, 2 unchanged."),
          pl2.exitCode == 0,
          pl2.stdout.contains("Nothing to burn: no changes to plan."))
      }
  )
```

- [ ] **Step 2: Register it**

In `CliIntegrationSpec.scala`, add `scenarios.Scenario08Idempotent.suite,` immediately before `noLeftovers`.

- [ ] **Step 3: Run the whole suite to verify everything passes**

Run: `sbt "cli/IntegrationTest/test"`
Expected: PASS — all eight scenarios plus build-smoke, harness, and no-leftovers checks green.

- [ ] **Step 4: Final full-build check**

Run: `sbt test` then `sbt "cli/IntegrationTest/test"`
Expected: unit suites unaffected; integration suite green (or cleanly ignored on a non-CoW filesystem).

- [ ] **Step 5: Commit**

```bash
git add cli/src/it/scala/diskrement/cli/it/scenarios/Scenario08Idempotent.scala \
        cli/src/it/scala/diskrement/cli/it/CliIntegrationSpec.scala
git commit -m "test(cli): scenario 8 (idempotent re-scan)"
```

---

## Notes for the implementer

- **Running the suite:** `sbt "cli/IntegrationTest/test"`. It is opt-in; plain `sbt test` does not run it. Add a `addCommandAlias("itTest", "cli/IntegrationTest/test")` to `build.sbt` if a shorter alias is desired (optional, not required).
- **CoW requirement:** the suite probes for reflink support under `target/it-scratch`. On a non-CoW filesystem every scenario is marked ignored rather than failing.
- **Determinism:** capacity-override scenarios use byte sizes chosen so each cycle's packing outcome is unambiguous. If you change a file size, re-check the per-disc expectations.
- **Output coupling:** assertions match substrings of `Render`'s output (`cli/src/main/scala/diskrement/cli/Render.scala`). If `Render` wording changes, update the `.contains(...)` checks.
```
