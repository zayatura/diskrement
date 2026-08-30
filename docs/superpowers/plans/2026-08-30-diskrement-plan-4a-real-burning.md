# Plan 4a — Real Burning + Read-Back Verify Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the simulated burn with real BD-R burning via xorriso, and make `verify` read back from the physical disc (mount + re-hash, plus extraction of superseded versions from earlier sessions).

**Architecture:** The `SimulatedBurner` trait splits into a `Burner` interface with two implementations (the existing stage-to-directory one, kept for tests/dev, and a new `XorrisoBurner`). A `MediaInspector` port interrogates the inserted disc before any write; a `DiscVerifier` port abstracts per-disc verification so the pipeline stays backend-agnostic. All external processes (xorriso, udisksctl) go through one `CmdRunner` port so unit tests fake process output and integration tests run the real tool against `stdio:` image files.

**Tech Stack:** Scala 3.3.4, ZIO 2.1.19, zio-json, zio-cli, xorriso ≥ 1.5 (external binary), udisks2 (external, verify-mount only). No new library dependencies.

**Spec:** `docs/superpowers/specs/2026-08-30-diskrement-cycle-2-real-burning-design.md`

**Branch:** `feat/real-burning-4a` off `main`.

---

## Validated xorriso facts (already tested on this machine, xorriso 1.5.8)

These command shapes were verified against `stdio:` image files before this plan was written. Do not re-derive them; use them as given.

- First session: `xorriso -abort_on FAILURE -outdev stdio:/path/disc.iso -compliance iso_9660_level=3 -volid VOLID -map <stagedDir> / -close off -commit` → exit 0, creates the image. `-iso_level` is mkisofs-emulation syntax and does NOT work in native mode; `-compliance iso_9660_level=3` is the native equivalent. Rock Ridge is on by default in native mode.
- Append: same but `-dev` instead of `-outdev` → merges the new tree as session 2; a file mapped at an existing path **hides** the old version in the merged view (validated: re-mapped `/data/a.txt` and `/.diskrement/project.json` read back as the new content).
- Inspect: `xorriso -abort_on FAILURE -outdev <target> -toc -tell_media_space` prints (among others):
  - `Media status : is blank` (blank/empty/nonexistent stdio file — a **nonexistent** stdio path counts as blank media, exit 0)
  - `Media status : is written , is appendable`
  - `Media summary: 2 sessions, 57 data blocks,  114k data,  660g free`
  - `ISO session  :   1 ,        32 ,        29s , ISOIMAGE` (one line per session)
  - `Media space  : 346016430s` (free space in 2048-byte blocks, from `-tell_media_space`)
- Extract from merged view: `xorriso -abort_on FAILURE -indev <target> -osirrox on -extract /.diskrement/project.json <dest>` → exit 0, writes the file.
- Extract from an earlier session (Tier-2 verify): insert `-load session 1` between `-indev <t>` and `-osirrox on` → reads that session's version (validated: got the session-1 content of a path hidden by session 2).
- Exit codes: genuine failures (bad path for stdio, permission denied on `/dev/sr0`) exit **5** with `FAILURE`/`aborting` lines in the output. Success exits 0.
- `stdio:` quirks: `-close on` has **no observable effect** on stdio files (they stay "appendable"), and free space on images is a fake huge number. Closing and capacity are only assertable on real media (smoke test).
- Permission reality on this dev machine: the user is **not** in the `optical` group yet, so `/dev/sr0` fails with `Permission denied` (exit 5). This is exactly the `DevicePermissionDenied` case.

Unknowns to be confirmed during the real-media smoke test (parsers are written tolerantly for them): the exact `Media status` line for a **closed** real disc (assumed: `is written` without `is appendable`), and the exact failure text for an **empty tray** (candidate substrings: `not present`, `no medium`, `not ready`, `No disc`).

---

## File structure

**Modified (shell):**
- `shell/src/main/scala/diskrement/shell/burn/Burn.scala` — `BurnDevice` enum, `DiscImage.closeDisc`, new `BurnError` cases.
- `shell/src/main/scala/diskrement/shell/burn/SimulatedBurner.scala` — trait moves out; `SimulatedBurnerLive` delegates staging to `Stager`.
- `shell/src/main/scala/diskrement/shell/burn/Json.scala` — `ProjectJson.toIdentity`.
- `shell/src/main/scala/diskrement/shell/catalog/CatalogStore.scala` / `CatalogStoreLive.scala` — `discRecordedContents`.
- `shell/src/main/scala/diskrement/shell/pipeline/Pipeline.scala` / `PipelineLive.scala` / `PipelineError.scala` — `Burner` + `DiscVerifier` deps, verify rework, `PipelineError.Burn`.

**Created (shell):**
- `shell/src/main/scala/diskrement/shell/burn/Burner.scala` — the `Burner` trait.
- `shell/src/main/scala/diskrement/shell/burn/Stager.scala` — shared stage-with-rehash logic.
- `shell/src/main/scala/diskrement/shell/burn/CmdRunner.scala` — external-process port + live impl.
- `shell/src/main/scala/diskrement/shell/burn/Xorriso.scala` — pure command builders + version parse.
- `shell/src/main/scala/diskrement/shell/burn/MediaParse.scala` — pure output parsers.
- `shell/src/main/scala/diskrement/shell/burn/MediaInspector.scala` — port + xorriso impl.
- `shell/src/main/scala/diskrement/shell/burn/BurnOperator.scala` — operator-interaction port.
- `shell/src/main/scala/diskrement/shell/burn/XorrisoBurner.scala` — the real burner.
- `shell/src/main/scala/diskrement/shell/burn/DiscReader.scala` — disc content access (udisks mount / osirrox extract).
- `shell/src/main/scala/diskrement/shell/burn/DiscVerifier.scala` — port + `RealDiscVerifier`.
- `shell/src/main/scala/diskrement/shell/model/DiscRecordedContents.scala` — catalog result types.
- `shell/src/main/scala/diskrement/shell/pipeline/StagingDiscVerifier.scala` — simulated-backend verifier.

**Modified (cli):**
- `cli/src/main/scala/diskrement/cli/Args.scala` — `GlobalOpts` gains `device`/`burner`; `BurnerKind` enum.
- `cli/src/main/scala/diskrement/cli/Commands.scala` — `--device`, `--burner` global options.
- `cli/src/main/scala/diskrement/cli/Runner.scala` — backend-dependent layer wiring, `ConsoleBurnOperator`.
- `cli/src/main/scala/diskrement/cli/Render.scala` — `PipelineError.Burn` rendering, exit code 6, skipped-verify line.
- `cli/src/it/scala/diskrement/cli/it/Cli.scala` — pin `DISKREMENT_BURNER=simulated`.

**Created (cli):**
- `cli/src/main/scala/diskrement/cli/BackendResolver.scala` — pure flag/env → backend resolution.

**Docs:** `docs/smoke-test-real-media.md` (new), `README.md` (status table).

Test files accompany each unit (named `<Unit>Spec.scala` in the mirrored test tree).

---

### Task 0: Branch

- [ ] **Step 1: Create the feature branch**

```bash
cd ~/work/diskrement && git checkout main && git pull && git checkout -b feat/real-burning-4a
```

- [ ] **Step 2: Confirm a clean baseline**

Run: `sbt clean core/test shell/test cli/test`
Expected: all green (core ~50 munit tests, shell + cli ZIO specs). Do not proceed on a red baseline.

---

### Task 1: `Burner` trait split, `Stager` extraction, `DiscImage.closeDisc`, `BurnDevice`

Pure refactor + additive model changes. No behavior change; existing tests must stay green.

**Files:**
- Create: `shell/src/main/scala/diskrement/shell/burn/Burner.scala`
- Create: `shell/src/main/scala/diskrement/shell/burn/Stager.scala`
- Modify: `shell/src/main/scala/diskrement/shell/burn/Burn.scala`
- Modify: `shell/src/main/scala/diskrement/shell/burn/SimulatedBurner.scala`
- Modify: `shell/src/main/scala/diskrement/shell/pipeline/Pipeline.scala`, `PipelineLive.scala`
- Modify: `cli/src/main/scala/diskrement/cli/Runner.scala`
- Modify: any test file referencing the old trait (find with `grep -rl SimulatedBurner shell/src cli/src`)

- [ ] **Step 1: Create `Burner.scala`**

```scala
package diskrement.shell.burn

import zio._

/** Writes one disc image (one session) to its destination. Implementations:
  * SimulatedBurnerLive (stage to a work directory) and XorrisoBurnerLive
  * (write real or image-file media via xorriso). */
trait Burner:
  def burn(image: DiscImage): IO[BurnError, BurnResult]
```

- [ ] **Step 2: Extend `Burn.scala`**

Add `closeDisc: Boolean` to `DiscImage` (NOT `finalize` — that name collides with `AnyRef.finalize` and will not compile), add the `BurnDevice` enum, and the new `BurnError` cases. The full new content of `Burn.scala`:

```scala
package diskrement.shell.burn

import diskrement.core.{ContentHash, RelPath, ScannedFile, SessionManifest}
import diskrement.shell.model.ArchivePointer
import java.nio.file.Path
import java.time.Instant

final case class DiscIdentity(
    projectName: String,
    discOrdinal: Int,
    sessionOrdinal: Int,
    createdAt: Instant
)

final case class DiscMetadata(
    identity: DiscIdentity,
    manifest: SessionManifest,
    catalogSnapshot: Path,
    pointers: List[ArchivePointer]
)

final case class DiscImage(
    stagingDir: Path,
    snapshotRoot: Path,
    files: List[ScannedFile],
    metadata: DiscMetadata,
    closeDisc: Boolean
)

final case class BurnResult(stagingDir: Path, imageBytes: Long)

/** Where a burn physically goes: a real optical drive, or (tests only) a plain
  * file addressed through xorriso's stdio: pseudo-drive. */
enum BurnDevice:
  case RealDevice(path: String)
  case ImageFile(path: Path)

  def xorrisoAddress: String = this match
    case RealDevice(p) => p
    case ImageFile(p)  => s"stdio:$p"

sealed abstract class BurnError(msg: String, cause: Throwable = null)
    extends Exception(msg, cause)
object BurnError:
  final case class HashMismatch(relPath: RelPath, expected: ContentHash, actual: ContentHash)
      extends BurnError(
        s"content hash mismatch for ${relPath.value}: expected ${expected.value}, got ${actual.value}")
  final case class SourceFileMissing(relPath: RelPath)
      extends BurnError(s"source file missing from snapshot: ${relPath.value}")
  final case class BurnIoError(cause: Throwable)
      extends BurnError("burn I/O error", cause)
  final case class ToolMissing(detail: String)
      extends BurnError(s"xorriso is not available: $detail (install the libisoburn/xorriso package)")
  final case class ToolTooOld(version: String)
      extends BurnError(s"xorriso $version is too old (need 1.5 or newer)")
  final case class DeviceNotFound(device: String)
      extends BurnError(s"burner device not found: $device")
  final case class DevicePermissionDenied(device: String)
      extends BurnError(
        s"no permission to open $device — add your user to the 'optical' group " +
          "(sudo usermod -aG optical <user>) and log in again")
  final case class NoMedia(device: String)
      extends BurnError(s"no disc in $device")
  final case class NotBlank(device: String)
      extends BurnError(s"expected a blank disc in $device but found written media")
  final case class DiscClosed(device: String)
      extends BurnError(s"the disc in $device is finalized (closed) and cannot be appended")
  final case class ForeignDisc(device: String)
      extends BurnError(s"the disc in $device is not a diskrement disc for this project")
  final case class WrongDisc(expected: String, found: String)
      extends BurnError(s"wrong disc inserted: expected $expected, found $found")
  final case class InsufficientCapacity(neededBytes: Long, freeBytes: Long)
      extends BurnError(s"insufficient media capacity: need $neededBytes bytes, only $freeBytes free")
  final case class BurnFailed(exitCode: Int, outputTail: List[String])
      extends BurnError(s"xorriso failed (exit $exitCode): ${outputTail.mkString(" | ")}")
  final case class PostBurnCheckFailed(detail: String)
      extends BurnError(s"post-burn sanity check failed: $detail")
  final case class MountFailed(detail: String)
      extends BurnError(s"could not mount the disc for verification: $detail")
  case object Aborted extends BurnError("aborted by operator")
```

- [ ] **Step 3: Create `Stager.scala`** (the body is moved verbatim from `SimulatedBurnerLive`)

```scala
package diskrement.shell.burn

import diskrement.core.ScannedFile
import diskrement.shell.fs.Hasher
import zio._
import java.nio.file.{Files, Path}
import scala.jdk.StreamConverters.*

/** Stages a full disc tree (/data + /.diskrement) to image.stagingDir, re-hashing
  * each file as it copies it (§6.2). Shared by the simulated and real burners:
  * for the real burner the staged tree is exactly what xorriso masters. */
object Stager:
  def stage(image: DiscImage, hasher: Hasher, writer: SnapshotWriter): IO[BurnError, BurnResult] =
    for
      _     <- clearAndCreate(image.stagingDir)
      _     <- ZIO.foreachDiscard(image.files)(copyOne(image, hasher, _))
      _     <- writer.write(image.stagingDir.resolve(".diskrement"), image.metadata)
      bytes <- totalBytes(image.stagingDir)
    yield BurnResult(image.stagingDir, bytes)

  private def clearAndCreate(dir: Path): IO[BurnError, Unit] =
    ZIO.attemptBlockingIO {
      if Files.exists(dir) then
        val s = Files.walk(dir)
        try s.sorted(java.util.Comparator.reverseOrder()).forEach(p => Files.deleteIfExists(p))
        finally s.close()
      Files.createDirectories(dir)
      ()
    }.mapError(BurnError.BurnIoError(_))

  private def copyOne(image: DiscImage, hasher: Hasher, f: ScannedFile): IO[BurnError, Unit] =
    val src = image.snapshotRoot.resolve(f.relPath.value)
    val dst = image.stagingDir.resolve("data").resolve(f.relPath.value)
    for
      exists <- ZIO.attemptBlockingIO(Files.exists(src)).mapError(BurnError.BurnIoError(_))
      _      <- ZIO.unless(exists)(ZIO.fail(BurnError.SourceFileMissing(f.relPath)))
      _      <- ZIO.attemptBlockingIO {
                  Files.createDirectories(dst.getParent)
                  Files.copy(src, dst)
                  ()
                }.mapError(BurnError.BurnIoError(_))
      actual <- hasher.hash(dst).mapError(BurnError.BurnIoError(_))
      _      <- ZIO.unless(actual == f.contentHash)(
                  ZIO.fail(BurnError.HashMismatch(f.relPath, f.contentHash, actual)))
    yield ()

  private def totalBytes(dir: Path): IO[BurnError, Long] =
    ZIO.attemptBlockingIO {
      val s = Files.walk(dir)
      try s.toScala(List).filter(p => Files.isRegularFile(p)).map(p => Files.size(p)).sum
      finally s.close()
    }.mapError(BurnError.BurnIoError(_))
```

- [ ] **Step 4: Rewrite `SimulatedBurner.scala`**

```scala
package diskrement.shell.burn

import diskrement.shell.fs.Hasher
import zio._

/** The stage-to-directory burner: the disc "image" is a work-dir tree. Kept as
  * the test/dev backend; select with --burner simulated / DISKREMENT_BURNER. */
final case class SimulatedBurnerLive(hasher: Hasher, writer: SnapshotWriter) extends Burner:
  def burn(image: DiscImage): IO[BurnError, BurnResult] =
    Stager.stage(image, hasher, writer)

object SimulatedBurner:
  val live: ZLayer[Hasher & SnapshotWriter, Nothing, Burner] =
    ZLayer {
      for
        hasher <- ZIO.service[Hasher]
        writer <- ZIO.service[SnapshotWriter]
      yield SimulatedBurnerLive(hasher, writer)
    }
```

- [ ] **Step 5: Ripple the rename**

`grep -rn SimulatedBurner shell/src cli/src` and update every reference so the *service type* is `Burner` while the layer stays `SimulatedBurner.live`:
- `Pipeline.scala`: import `diskrement.shell.burn.Burner` (drop `SimulatedBurner` from the import); layer environment type becomes `... & Burner & ...`; `ZIO.service[Burner]`.
- `PipelineLive.scala`: constructor field becomes `burner: Burner`; import adjusts. Also add `closeDisc = disc.willFinalize` when constructing `DiscImage` in `burn` (the `DiscImage(...)` call gains the fifth argument).
- `Runner.scala`: import stays `SimulatedBurner` (for `.live`) — only the import list may need `Burner` if referenced.
- Test files constructing `DiscImage(...)` gain `closeDisc = false`; test files referencing the trait switch to `Burner`.

- [ ] **Step 6: Compile and run all tests**

Run: `sbt shell/test cli/test`
Expected: PASS, identical test counts to baseline (`-Wunused:all` means unused imports fail the build — clean them).

- [ ] **Step 7: Commit**

```bash
git add -A && git commit -m "refactor(burn): split Burner trait from SimulatedBurner; extract Stager; add BurnDevice + closeDisc + real-burn error cases"
```

---

### Task 2: `CmdRunner` (external process port) and `Xorriso` command builders

**Files:**
- Create: `shell/src/main/scala/diskrement/shell/burn/CmdRunner.scala`
- Create: `shell/src/main/scala/diskrement/shell/burn/Xorriso.scala`
- Test: `shell/src/test/scala/diskrement/shell/burn/XorrisoSpec.scala`

- [ ] **Step 1: Write the failing test** (pure builders + version parsing)

```scala
package diskrement.shell.burn

import zio.test._
import java.nio.file.Path

object XorrisoSpec extends ZIOSpecDefault:
  private val img = BurnDevice.ImageFile(Path.of("/tmp/d.iso"))

  def spec = suite("XorrisoSpec")(
    test("burnArgs first session uses -outdev, close off") {
      assertTrue(Xorriso.burnArgs(img, Path.of("/w/staging/disc-1"), firstSession = true,
        closeDisc = false, volid = "PHOTOS_DISC_1") ==
        List("xorriso", "-abort_on", "FAILURE", "-outdev", "stdio:/tmp/d.iso",
          "-compliance", "iso_9660_level=3", "-volid", "PHOTOS_DISC_1",
          "-map", "/w/staging/disc-1", "/", "-close", "off", "-commit"))
    },
    test("burnArgs append uses -dev, close on") {
      val args = Xorriso.burnArgs(BurnDevice.RealDevice("/dev/sr0"), Path.of("/w/s"),
        firstSession = false, closeDisc = true, volid = "V")
      assertTrue(args.contains("-dev"), args.contains("/dev/sr0"),
        !args.contains("-outdev"), args.containsSlice(List("-close", "on")))
    },
    test("inspectArgs reads toc and media space") {
      assertTrue(Xorriso.inspectArgs(img) ==
        List("xorriso", "-abort_on", "FAILURE", "-outdev", "stdio:/tmp/d.iso",
          "-toc", "-tell_media_space"))
    },
    test("extractArgs without session reads the merged view") {
      assertTrue(Xorriso.extractArgs(img, "/.diskrement/project.json", Path.of("/tmp/p.json"), None) ==
        List("xorriso", "-abort_on", "FAILURE", "-indev", "stdio:/tmp/d.iso",
          "-osirrox", "on", "-extract", "/.diskrement/project.json", "/tmp/p.json"))
    },
    test("extractArgs with session loads that session first") {
      assertTrue(Xorriso.extractArgs(img, "/data/a.txt", Path.of("/tmp/a"), Some(1)).containsSlice(
        List("-indev", "stdio:/tmp/d.iso", "-load", "session", "1", "-osirrox", "on")))
    },
    test("volid sanitizes to uppercase alnum/underscore, max 32") {
      assertTrue(
        Xorriso.volid("my photos!", 3) == "MY_PHOTOS__DISC_3",
        Xorriso.volid("x" * 40, 1).length == 32)
    },
    test("parseVersion accepts 1.5.8.pl02 and rejects 1.4") {
      assertTrue(
        Xorriso.parseVersion(List("xorriso 1.5.8.pl02", "ISO 9660 ...")) == Some((1, 5, "1.5.8.pl02")),
        Xorriso.parseVersion(List("xorriso 1.4.2")) == Some((1, 4, "1.4.2")),
        Xorriso.parseVersion(List("garbage")) == None)
    }
  )
```

- [ ] **Step 2: Run it to verify it fails**

Run: `sbt 'shell/testOnly diskrement.shell.burn.XorrisoSpec'`
Expected: compile FAILURE (`Xorriso` not found).

- [ ] **Step 3: Implement `Xorriso.scala`**

```scala
package diskrement.shell.burn

import java.nio.file.Path

/** Pure xorriso command construction and version parsing. Every list starts with
  * the binary name; CmdRunner executes it. Shapes validated against xorriso 1.5.8
  * (see the plan's "Validated xorriso facts"). */
object Xorriso:

  def burnArgs(device: BurnDevice, stagedDir: Path, firstSession: Boolean,
               closeDisc: Boolean, volid: String): List[String] =
    List("xorriso", "-abort_on", "FAILURE",
      if firstSession then "-outdev" else "-dev", device.xorrisoAddress,
      "-compliance", "iso_9660_level=3",
      "-volid", volid,
      "-map", stagedDir.toString, "/",
      "-close", if closeDisc then "on" else "off",
      "-commit")

  def inspectArgs(device: BurnDevice): List[String] =
    List("xorriso", "-abort_on", "FAILURE", "-outdev", device.xorrisoAddress,
      "-toc", "-tell_media_space")

  def extractArgs(device: BurnDevice, discPath: String, dest: Path,
                  session: Option[Int]): List[String] =
    List("xorriso", "-abort_on", "FAILURE", "-indev", device.xorrisoAddress) ++
      session.toList.flatMap(n => List("-load", "session", n.toString)) ++
      List("-osirrox", "on", "-extract", discPath, dest.toString)

  def ejectArgs(device: BurnDevice): List[String] =
    List("xorriso", "-outdev", device.xorrisoAddress, "-eject", "all")

  val versionArgs: List[String] = List("xorriso", "--version")

  /** ISO volume id: uppercase, [A-Z0-9_], at most 32 chars. */
  def volid(projectName: String, discOrdinal: Int): String =
    s"${projectName}_DISC_$discOrdinal".toUpperCase
      .map(c => if c.isLetterOrDigit then c else '_')
      .take(32)

  private val VersionRe = """xorriso\s+((\d+)\.(\d+)\S*).*""".r

  /** (major, minor, raw version string) from `xorriso --version` output. */
  def parseVersion(lines: List[String]): Option[(Int, Int, String)] =
    lines.collectFirst { case VersionRe(raw, maj, min) => (maj.toInt, min.toInt, raw) }
```

- [ ] **Step 4: Implement `CmdRunner.scala`** (no unit test of the live impl; it is exercised by the integration spec in Task 6 and faked everywhere else)

```scala
package diskrement.shell.burn

import zio._
import java.io.{BufferedReader, InputStreamReader}

final case class CmdResult(exitCode: Int, output: List[String]):
  def tail(n: Int = 10): List[String] = output.takeRight(n)

/** Runs one external command, streaming each combined stdout/stderr line through
  * `onLine` as it appears (xorriso progress reaches the operator live). Fails only
  * when the process cannot be launched; a nonzero exit is a normal CmdResult. */
trait CmdRunner:
  def run(command: List[String], onLine: String => UIO[Unit] = _ => ZIO.unit): IO[BurnError, CmdResult]

final case class CmdRunnerLive() extends CmdRunner:
  def run(command: List[String], onLine: String => UIO[Unit]): IO[BurnError, CmdResult] =
    ZIO.attemptBlocking {
      val pb = new ProcessBuilder(command*)
      pb.redirectErrorStream(true)
      pb.start()
    }.mapError {
      case e: java.io.IOException
          if Option(e.getMessage).exists(_.toLowerCase.contains("no such file")) =>
        BurnError.ToolMissing(s"cannot launch ${command.headOption.getOrElse("?")}: ${e.getMessage}")
      case t => BurnError.BurnIoError(t)
    }.flatMap { proc =>
      val reader = new BufferedReader(new InputStreamReader(proc.getInputStream))
      def loop(acc: List[String]): IO[BurnError, List[String]] =
        ZIO.attemptBlockingIO(Option(reader.readLine())).mapError(BurnError.BurnIoError(_)).flatMap {
          case None       => ZIO.succeed(acc.reverse)
          case Some(line) => onLine(line) *> loop(line :: acc)
        }
      for
        lines <- loop(Nil)
        code  <- ZIO.attemptBlocking(proc.waitFor()).mapError(BurnError.BurnIoError(_))
      yield CmdResult(code, lines)
    }

object CmdRunner:
  val live: ZLayer[Any, Nothing, CmdRunner] = ZLayer.succeed(CmdRunnerLive())
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `sbt 'shell/testOnly diskrement.shell.burn.XorrisoSpec'`
Expected: PASS (7 tests).

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "feat(burn): CmdRunner process port + pure xorriso command builders"
```

---

### Task 3: `MediaParse` — parse xorriso inspection output

**Files:**
- Create: `shell/src/main/scala/diskrement/shell/burn/MediaParse.scala`
- Test: `shell/src/test/scala/diskrement/shell/burn/MediaParseSpec.scala`

- [ ] **Step 1: Write the failing test** (fixtures are real captured output; the "closed" fixture is the assumed shape, revisited in the smoke test)

```scala
package diskrement.shell.burn

import zio.test._

object MediaParseSpec extends ZIOSpecDefault:

  private val blankFixture = List(
    "xorriso 1.5.8.pl02 : RockRidge filesystem manipulator, libburnia project.",
    "Drive current: -outdev 'stdio:empty.iso'",
    "Media current: stdio file, overwriteable",
    "Media status : is blank",
    "Media summary: 0 sessions, 0 data blocks, 0 data,  660g free",
    "Media space  : 346016270s",
    "After commit : 346016270s")

  private val appendableFixture = List(
    "Media current: stdio file, overwriteable",
    "Media status : is written , is appendable",
    "ISO session  :   1 ,        32 ,        29s , ISOIMAGE",
    "ISO session  :   2 ,        64 ,        28s , ISOIMAGE",
    "Media summary: 2 sessions, 57 data blocks,  114k data,  660g free",
    "Media space  : 346016430s")

  private val closedFixture = List(
    "Media current: BD-R sequential recording",
    "Media status : is written , is closed",
    "Media summary: 3 sessions, 57 data blocks,  114k data, 0 free",
    "Media space  : 0s")

  def spec = suite("MediaParseSpec")(
    test("blank media") {
      assertTrue(MediaParse.parse(blankFixture) ==
        Right(RawMediaInfo(RawStatus.Blank, 0, 346016270L * 2048)))
    },
    test("appendable media with session count") {
      assertTrue(MediaParse.parse(appendableFixture) ==
        Right(RawMediaInfo(RawStatus.Appendable, 2, 346016430L * 2048)))
    },
    test("written but not appendable parses as Closed") {
      assertTrue(MediaParse.parse(closedFixture) == Right(RawMediaInfo(RawStatus.Closed, 3, 0L)))
    },
    test("no recognizable status is an error") {
      assertTrue(MediaParse.parse(List("Drive current: ...")).isLeft)
    },
    test("failure classification recognizes no-media, permission, missing device") {
      assertTrue(
        MediaParse.classifyFailure(List("libburn : SORRY : No disc loaded, not ready")) == FailureKind.NoMediaKind,
        MediaParse.classifyFailure(List("libburn : NOTE : Failed to open device '/dev/sr0' : Permission denied")) == FailureKind.PermissionKind,
        MediaParse.classifyFailure(List("stdio: could not open, No such file or directory")) == FailureKind.NotFoundKind,
        MediaParse.classifyFailure(List("xorriso : FAILURE : something else")) == FailureKind.OtherKind)
    }
  )
```

- [ ] **Step 2: Run it to verify it fails**

Run: `sbt 'shell/testOnly diskrement.shell.burn.MediaParseSpec'`
Expected: compile FAILURE.

- [ ] **Step 3: Implement `MediaParse.scala`**

```scala
package diskrement.shell.burn

enum RawStatus { case Blank, Appendable, Closed }
final case class RawMediaInfo(status: RawStatus, sessions: Int, freeBytes: Long)

enum FailureKind { case NoMediaKind, PermissionKind, NotFoundKind, OtherKind }

/** Pure parsers for `xorriso -toc -tell_media_space` output and for classifying
  * xorriso failure output. Line shapes validated on xorriso 1.5.8; the "closed"
  * and empty-tray shapes are tolerant assumptions confirmed by the smoke test. */
object MediaParse:

  private val SessionsRe = """.*Media summary:\s*(\d+)\s+session.*""".r
  private val SpaceRe    = """.*Media space\s*:\s*(\d+)s.*""".r

  def parse(lines: List[String]): Either[String, RawMediaInfo] =
    val status = lines.collectFirst {
      case l if l.contains("Media status") && l.contains("is blank")      => RawStatus.Blank
      case l if l.contains("Media status") && l.contains("is appendable") => RawStatus.Appendable
      case l if l.contains("Media status") && l.contains("is written")    => RawStatus.Closed
    }
    val sessions = lines.collectFirst { case SessionsRe(n) => n.toInt }.getOrElse(0)
    val free     = lines.collectFirst { case SpaceRe(blocks) => blocks.toLong * 2048 }.getOrElse(0L)
    status.toRight(s"no Media status line in xorriso output: ${lines.take(5).mkString(" | ")}")
      .map(RawMediaInfo(_, sessions, free))

  /** Why did an inspection/burn invocation fail? Substrings chosen from observed
    * and documented libburn messages; unknown text falls through to OtherKind. */
  def classifyFailure(lines: List[String]): FailureKind =
    val text = lines.mkString("\n").toLowerCase
    if text.contains("permission denied") then FailureKind.PermissionKind
    else if text.contains("no such file") || text.contains("cannot find device") then FailureKind.NotFoundKind
    else if List("no disc", "not ready", "no medium", "not present").exists(text.contains) then
      FailureKind.NoMediaKind
    else FailureKind.OtherKind
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `sbt 'shell/testOnly diskrement.shell.burn.MediaParseSpec'`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(burn): MediaParse for xorriso toc/media-space output and failure classification"
```

---

### Task 4: `MediaInspector`

**Files:**
- Create: `shell/src/main/scala/diskrement/shell/burn/MediaInspector.scala`
- Modify: `shell/src/main/scala/diskrement/shell/burn/Json.scala` (add `ProjectJson.toIdentity`)
- Test: `shell/src/test/scala/diskrement/shell/burn/MediaInspectorSpec.scala`

- [ ] **Step 1: Add `toIdentity` to `ProjectJson`'s companion in `Json.scala`**

```scala
  def toIdentity(p: ProjectJson): Either[String, DiscIdentity] =
    try Right(DiscIdentity(p.projectName, p.discOrdinal, p.sessionOrdinal,
      java.time.Instant.parse(p.createdAt)))
    catch case e: Exception => Left(s"bad createdAt in project.json: ${e.getMessage}")
```

- [ ] **Step 2: Write the failing test** — a `FakeCmdRunner` scripts responses per command shape:

```scala
package diskrement.shell.burn

import zio._
import zio.test._
import java.nio.file.{Files, Path}

/** Scripted CmdRunner: matches each command by a predicate on its args and
  * replies with a canned result; `-extract` commands may also write the dest file. */
final case class FakeCmdRunner(script: List[String] => UIO[CmdResult]) extends CmdRunner:
  def run(command: List[String], onLine: String => UIO[Unit]): IO[BurnError, CmdResult] =
    script(command)

object MediaInspectorSpec extends ZIOSpecDefault:
  private val dev = BurnDevice.RealDevice("/dev/sr0")

  private val blankToc = CmdResult(0, List(
    "Media status : is blank",
    "Media summary: 0 sessions, 0 data blocks, 0 data, 23g free",
    "Media space  : 11826176s"))

  private val appendableToc = CmdResult(0, List(
    "Media status : is written , is appendable",
    "Media summary: 2 sessions, 57 data blocks, 114k data, 23g free",
    "Media space  : 11826000s"))

  private def identityJson = """{"projectName":"photos","discOrdinal":1,"sessionOrdinal":2,"createdAt":"2026-08-30T10:00:00Z"}"""

  def spec = suite("MediaInspectorSpec")(
    test("blank media") {
      val insp = XorrisoMediaInspectorLive(FakeCmdRunner(_ => ZIO.succeed(blankToc)))
      insp.inspect(dev).map(s => assertTrue(s == MediaStatus.Blank(11826176L * 2048)))
    },
    test("appendable diskrement disc yields identity") {
      val runner = FakeCmdRunner { cmd =>
        if cmd.contains("-toc") then ZIO.succeed(appendableToc)
        else if cmd.contains("-extract") then
          ZIO.succeed { Files.writeString(Path.of(cmd.last), identityJson); CmdResult(0, Nil) }
        else ZIO.succeed(CmdResult(1, List("unexpected")))
      }
      XorrisoMediaInspectorLive(runner).inspect(dev).map {
        case MediaStatus.Appendable(free, sessions, Some(id)) =>
          assertTrue(free == 11826000L * 2048, sessions == 2,
            id.projectName == "photos", id.discOrdinal == 1, id.sessionOrdinal == 2)
        case other => assertNever(s"unexpected: $other")
      }
    },
    test("appendable non-diskrement disc yields None identity") {
      val runner = FakeCmdRunner { cmd =>
        if cmd.contains("-toc") then ZIO.succeed(appendableToc)
        else ZIO.succeed(CmdResult(5, List("FAILURE : cannot find path")))
      }
      XorrisoMediaInspectorLive(runner).inspect(dev).map {
        case MediaStatus.Appendable(_, _, None) => assertCompletes
        case other                              => assertNever(s"unexpected: $other")
      }
    },
    test("empty tray maps to NoMedia") {
      val runner = FakeCmdRunner(_ => ZIO.succeed(CmdResult(5, List("libburn : SORRY : No disc loaded, not ready"))))
      XorrisoMediaInspectorLive(runner).inspect(dev).map(s => assertTrue(s == MediaStatus.NoMedia))
    },
    test("permission denied fails with DevicePermissionDenied") {
      val runner = FakeCmdRunner(_ => ZIO.succeed(CmdResult(5, List("Failed to open device '/dev/sr0' : Permission denied"))))
      XorrisoMediaInspectorLive(runner).inspect(dev).exit.map(e =>
        assertTrue(e.isFailure) && assertTrue(e.causeOption.flatMap(_.failureOption).exists(_.isInstanceOf[BurnError.DevicePermissionDenied]))
      )
    }
  )
```

- [ ] **Step 3: Run it to verify it fails**

Run: `sbt 'shell/testOnly diskrement.shell.burn.MediaInspectorSpec'`
Expected: compile FAILURE.

- [ ] **Step 4: Implement `MediaInspector.scala`**

```scala
package diskrement.shell.burn

import zio._
import zio.json._
import java.nio.file.{Files, Path}

enum MediaStatus:
  case NoMedia
  case Blank(freeBytes: Long)
  case Appendable(freeBytes: Long, sessions: Int, identity: Option[DiscIdentity])
  case Closed(identity: Option[DiscIdentity])

/** Interrogates the medium in a device: state, free space, and — for written
  * diskrement discs — the identity embedded in the last session. */
trait MediaInspector:
  def inspect(device: BurnDevice): IO[BurnError, MediaStatus]
  /** The /.diskrement/project.json identity of the disc's merged view, if any. */
  def readIdentity(device: BurnDevice): IO[BurnError, Option[DiscIdentity]]

final case class XorrisoMediaInspectorLive(runner: CmdRunner) extends MediaInspector:

  def inspect(device: BurnDevice): IO[BurnError, MediaStatus] =
    runner.run(Xorriso.inspectArgs(device)).flatMap { res =>
      if res.exitCode != 0 then
        MediaParse.classifyFailure(res.output) match
          case FailureKind.NoMediaKind    => ZIO.succeed(MediaStatus.NoMedia)
          case FailureKind.PermissionKind => ZIO.fail(BurnError.DevicePermissionDenied(device.xorrisoAddress))
          case FailureKind.NotFoundKind   => ZIO.fail(BurnError.DeviceNotFound(device.xorrisoAddress))
          case FailureKind.OtherKind      => ZIO.fail(BurnError.BurnFailed(res.exitCode, res.tail()))
      else
        MediaParse.parse(res.output) match
          case Left(msg) => ZIO.fail(BurnError.BurnFailed(res.exitCode, List(msg)))
          case Right(RawMediaInfo(RawStatus.Blank, _, free)) =>
            ZIO.succeed(MediaStatus.Blank(free))
          case Right(RawMediaInfo(RawStatus.Appendable, sessions, free)) =>
            readIdentity(device).map(MediaStatus.Appendable(free, sessions, _))
          case Right(RawMediaInfo(RawStatus.Closed, _, _)) =>
            readIdentity(device).map(MediaStatus.Closed(_))
    }

  def readIdentity(device: BurnDevice): IO[BurnError, Option[DiscIdentity]] =
    ZIO.attemptBlockingIO(Files.createTempFile("diskrement-identity", ".json"))
      .mapError(BurnError.BurnIoError(_))
      .flatMap { tmp =>
        runner.run(Xorriso.extractArgs(device, "/.diskrement/project.json", tmp, None))
          .flatMap { res =>
            if res.exitCode != 0 then ZIO.succeed(None)
            else
              ZIO.attemptBlockingIO(Files.readString(tmp)).mapError(BurnError.BurnIoError(_))
                .map(_.fromJson[ProjectJson].flatMap(ProjectJson.toIdentity).toOption)
          }
          .ensuring(ZIO.attemptBlockingIO(Files.deleteIfExists(tmp)).ignore)
      }

object MediaInspector:
  val live: ZLayer[CmdRunner, Nothing, MediaInspector] =
    ZLayer(ZIO.service[CmdRunner].map(XorrisoMediaInspectorLive(_)))
```

Note: `-osirrox on -extract` refuses to overwrite an existing file in some configurations; if the integration spec (Task 6) hits that, delete the temp file before extraction (`Files.delete(tmp)` right after creation — keep the unique name) — the same applies to `DiscReader`.

- [ ] **Step 5: Run the test to verify it passes**

Run: `sbt 'shell/testOnly diskrement.shell.burn.MediaInspectorSpec'`
Expected: PASS (5 tests).

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "feat(burn): MediaInspector port with xorriso implementation"
```

---

### Task 5: `BurnOperator` + `XorrisoBurner` (unit-tested decision flow)

**Files:**
- Create: `shell/src/main/scala/diskrement/shell/burn/BurnOperator.scala`
- Create: `shell/src/main/scala/diskrement/shell/burn/XorrisoBurner.scala`
- Test: `shell/src/test/scala/diskrement/shell/burn/XorrisoBurnerSpec.scala`

- [ ] **Step 1: Create `BurnOperator.scala`**

```scala
package diskrement.shell.burn

import zio._

/** Operator interaction during real burns and verifies: insert-disc
  * confirmations and live progress lines. The CLI supplies a console
  * implementation; tests script one. */
trait BurnOperator:
  /** Show `message`, wait for the operator. false = abort/skip. */
  def confirmInsert(message: String): UIO[Boolean]
  def progress(line: String): UIO[Unit]
```

- [ ] **Step 2: Write the failing test.** The fakes: `FakeCmdRunner` from Task 4's spec (move it to its own file `shell/src/test/scala/diskrement/shell/burn/FakeCmdRunner.scala` now, since two specs use it), a `FakeInspector` returning a scripted sequence of `MediaStatus`, and a `ScriptedOperator` recording messages and answering from a queue.

```scala
package diskrement.shell.burn

import diskrement.core.{ContentHash, RelPath, ScannedFile, SessionManifest}
import zio._
import zio.test._
import java.nio.file.{Files, Path}
import java.time.Instant

final case class FakeInspector(statuses: Ref[List[MediaStatus]], identity: Option[DiscIdentity])
    extends MediaInspector:
  def inspect(device: BurnDevice): IO[BurnError, MediaStatus] =
    statuses.modify { case h :: t => (h, t); case Nil => (MediaStatus.NoMedia, Nil) }
  def readIdentity(device: BurnDevice): IO[BurnError, Option[DiscIdentity]] =
    ZIO.succeed(identity)

final case class ScriptedOperator(answers: Ref[List[Boolean]], log: Ref[List[String]])
    extends BurnOperator:
  def confirmInsert(message: String): UIO[Boolean] =
    log.update(message :: _) *> answers.modify { case h :: t => (h, t); case Nil => (false, Nil) }
  def progress(line: String): UIO[Unit] = log.update(line :: _)

object XorrisoBurnerSpec extends ZIOSpecDefault:

  private def mkImage(base: Path, sessionOrdinal: Int, closeDisc: Boolean): DiscImage =
    val snap = base.resolve("snapshot"); Files.createDirectories(snap)
    Files.writeString(snap.resolve("a.txt"), "hello")
    val catSnap = base.resolve("burn-catalog.sqlite"); Files.writeString(catSnap, "not-a-real-db")
    val hash = ContentHash( // SHA-256 of "hello"
      "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824")
    val f = ScannedFile(RelPath("a.txt"), 5L, Instant.parse("2026-08-30T10:00:00Z"), hash)
    val id = DiscIdentity("photos", 1, sessionOrdinal, Instant.parse("2026-08-30T10:00:00Z"))
    val manifest = SessionManifest("photos", 1, sessionOrdinal, Nil, Nil, Nil)
    DiscImage(base.resolve("staging"), snap, List(f),
      DiscMetadata(id, manifest, catSnap, Nil), closeDisc)

  private val okBurn = CmdResult(0, List("xorriso : UPDATE : 100% done"))
  private def identityFor(sessionOrdinal: Int) =
    Some(DiscIdentity("photos", 1, sessionOrdinal, Instant.parse("2026-08-30T10:00:00Z")))

  private def burner(statuses: List[MediaStatus], answers: List[Boolean],
                     postBurnIdentity: Option[DiscIdentity],
                     runnerScript: List[String] => UIO[CmdResult]) =
    for
      st  <- Ref.make(statuses)
      ans <- Ref.make(answers)
      log <- Ref.make(List.empty[String])
      b    = XorrisoBurnerLive(BurnDevice.RealDevice("/dev/sr0"), FakeCmdRunner(runnerScript),
               FakeInspector(st, postBurnIdentity), ScriptedOperator(ans, log),
               diskrement.shell.fs.HasherLive(), SnapshotWriterLive())
    yield (b, log)

  private val versionOk: List[String] => UIO[CmdResult] = cmd =>
    if cmd.contains("--version") then ZIO.succeed(CmdResult(0, List("xorriso 1.5.8.pl02")))
    else ZIO.succeed(okBurn)

  def spec = suite("XorrisoBurnerSpec")(
    test("session 1 on blank media burns and post-checks") {
      ZIO.attemptBlocking(Files.createTempDirectory("xb")).orDie.flatMap { base =>
        for
          (b, _) <- burner(List(MediaStatus.Blank(1_000_000_000L)), List(true),
                      identityFor(1), versionOk)
          res    <- b.burn(mkImage(base, 1, closeDisc = false))
        yield assertTrue(res.imageBytes > 0L)
      }
    },
    test("session 1 refuses written media, re-prompts, succeeds on swap") {
      ZIO.attemptBlocking(Files.createTempDirectory("xb")).orDie.flatMap { base =>
        for
          (b, log) <- burner(
                        List(MediaStatus.Appendable(1_000_000_000L, 1, identityFor(1)),
                             MediaStatus.Blank(1_000_000_000L)),
                        List(true, true), identityFor(1), versionOk)
          res      <- b.burn(mkImage(base, 1, closeDisc = false))
          lines    <- log.get
        yield assertTrue(res.imageBytes > 0L, lines.exists(_.contains("Not the expected disc")))
      }
    },
    test("top-off requires matching identity at previous session ordinal") {
      ZIO.attemptBlocking(Files.createTempDirectory("xb")).orDie.flatMap { base =>
        for // disc says session 1; we burn session 2 — match.
          (b, _) <- burner(List(MediaStatus.Appendable(1_000_000_000L, 1, identityFor(1))),
                      List(true), identityFor(2), versionOk)
          res    <- b.burn(mkImage(base, 2, closeDisc = true))
        yield assertTrue(res.imageBytes > 0L)
      }
    },
    test("operator abort fails with Aborted") {
      ZIO.attemptBlocking(Files.createTempDirectory("xb")).orDie.flatMap { base =>
        for
          (b, _) <- burner(List(MediaStatus.Blank(1_000_000_000L)), List(false),
                      identityFor(1), versionOk)
          exit   <- b.burn(mkImage(base, 1, closeDisc = false)).exit
        yield assertTrue(exit == Exit.fail(BurnError.Aborted))
      }
    },
    test("insufficient capacity refuses before burning") {
      ZIO.attemptBlocking(Files.createTempDirectory("xb")).orDie.flatMap { base =>
        for // 1000 bytes free < 32 MiB fixed overhead
          (b, _) <- burner(List(MediaStatus.Blank(1000L)), List(true), identityFor(1), versionOk)
          exit   <- b.burn(mkImage(base, 1, closeDisc = false)).exit
        yield assertTrue(exit.isFailure) &&
          assertTrue(exit.causeOption.flatMap(_.failureOption)
            .exists(_.isInstanceOf[BurnError.InsufficientCapacity]))
      }
    },
    test("xorriso burn failure surfaces BurnFailed with output tail") {
      ZIO.attemptBlocking(Files.createTempDirectory("xb")).orDie.flatMap { base =>
        val failingRunner: List[String] => UIO[CmdResult] = cmd =>
          if cmd.contains("--version") then ZIO.succeed(CmdResult(0, List("xorriso 1.5.8.pl02")))
          else if cmd.contains("-commit") then ZIO.succeed(CmdResult(5, List("FAILURE : write error")))
          else ZIO.succeed(okBurn)
        for
          (b, _) <- burner(List(MediaStatus.Blank(1_000_000_000L)), List(true),
                      identityFor(1), failingRunner)
          exit   <- b.burn(mkImage(base, 1, closeDisc = false)).exit
        yield assertTrue(exit.causeOption.flatMap(_.failureOption)
          .exists(_.isInstanceOf[BurnError.BurnFailed]))
      }
    },
    test("post-burn identity mismatch fails PostBurnCheckFailed") {
      ZIO.attemptBlocking(Files.createTempDirectory("xb")).orDie.flatMap { base =>
        for // disc still reports session 1 identity after burning session 2 header? -> expected 1, got stale
          (b, _) <- burner(List(MediaStatus.Blank(1_000_000_000L)), List(true),
                      identityFor(7), versionOk)
          exit   <- b.burn(mkImage(base, 1, closeDisc = false)).exit
        yield assertTrue(exit.causeOption.flatMap(_.failureOption)
          .exists(_.isInstanceOf[BurnError.PostBurnCheckFailed]))
      }
    },
    test("old xorriso fails ToolTooOld") {
      ZIO.attemptBlocking(Files.createTempDirectory("xb")).orDie.flatMap { base =>
        val oldRunner: List[String] => UIO[CmdResult] = cmd =>
          if cmd.contains("--version") then ZIO.succeed(CmdResult(0, List("xorriso 1.4.2")))
          else ZIO.succeed(okBurn)
        for
          (b, _) <- burner(List(MediaStatus.Blank(1_000_000_000L)), List(true), identityFor(1), oldRunner)
          exit   <- b.burn(mkImage(base, 1, closeDisc = false)).exit
        yield assertTrue(exit.causeOption.flatMap(_.failureOption)
          .exists(_.isInstanceOf[BurnError.ToolTooOld]))
      }
    }
  )
```

Notes for the implementer: `SessionManifest`'s constructor shape lives in `core` — check it (`grep -n "case class SessionManifest" core/src`) and adjust the `mkImage` call if the field list differs. Same for `ScannedFile`. `HasherLive`/`SnapshotWriterLive` are the real implementations — staging is real file I/O in these tests, which keeps the hash check honest.

- [ ] **Step 3: Run it to verify it fails**

Run: `sbt 'shell/testOnly diskrement.shell.burn.XorrisoBurnerSpec'`
Expected: compile FAILURE (`XorrisoBurnerLive` not found).

- [ ] **Step 4: Implement `XorrisoBurner.scala`**

```scala
package diskrement.shell.burn

import diskrement.shell.fs.Hasher
import zio._

/** The real burner: prompts for the disc, inspects it (strict identity check),
  * stages with re-hash, checks capacity, burns via xorriso, sanity-checks the
  * result, ejects. Spec §4. Any failure leaves the catalog uncommitted (the
  * pipeline commits only after this returns). */
final case class XorrisoBurnerLive(
    device: BurnDevice,
    runner: CmdRunner,
    inspector: MediaInspector,
    operator: BurnOperator,
    hasher: Hasher,
    writer: SnapshotWriter
) extends Burner:

  private val fixedOverheadBytes = 32L * 1024 * 1024

  def burn(image: DiscImage): IO[BurnError, BurnResult] =
    val id = image.metadata.identity
    val firstSession = id.sessionOrdinal == 1
    for
      _      <- preflight
      free   <- awaitCorrectMedia(image)
      staged <- Stager.stage(image, hasher, writer)
      _      <- checkCapacity(staged.imageBytes, free)
      _      <- doBurn(image, firstSession)
      _      <- postBurnCheck(id)
      _      <- eject
    yield staged

  private val preflight: IO[BurnError, Unit] =
    runner.run(Xorriso.versionArgs).flatMap { r =>
      Xorriso.parseVersion(r.output) match
        case None => ZIO.fail(BurnError.ToolMissing("cannot parse `xorriso --version` output"))
        case Some((maj, min, raw)) =>
          ZIO.unless(maj > 1 || (maj == 1 && min >= 5))(
            ZIO.fail(BurnError.ToolTooOld(raw))).unit
    }

  /** Prompt-inspect loop: returns the media's free bytes once the right disc is
    * in, fails Aborted when the operator declines. Wrong media re-prompts. */
  private def awaitCorrectMedia(image: DiscImage): IO[BurnError, Long] =
    val id = image.metadata.identity
    val expected =
      if id.sessionOrdinal == 1 then s"a blank disc for '${id.projectName}' disc ${id.discOrdinal}"
      else s"disc ${id.discOrdinal} of '${id.projectName}' (last burned session ${id.sessionOrdinal - 1})"
    val sealNote = if image.closeDisc then " This burn will FINALIZE the disc." else ""
    val prompt = s"Insert $expected into ${device.xorrisoAddress} and confirm.$sealNote"

    def check: IO[BurnError, Either[String, Long]] =
      inspector.inspect(device).map { status =>
        (status, id.sessionOrdinal) match
          case (MediaStatus.NoMedia, _)             => Left("no disc in the drive")
          case (MediaStatus.Blank(free), 1)         => Right(free)
          case (MediaStatus.Blank(_), _)            => Left("a blank disc (expected written media to append to)")
          case (MediaStatus.Appendable(_, _, _), 1) => Left("written media (expected a blank disc)")
          case (MediaStatus.Appendable(free, _, Some(found)), n)
              if found.projectName == id.projectName
                && found.discOrdinal == id.discOrdinal
                && found.sessionOrdinal == n - 1    => Right(free)
          case (MediaStatus.Appendable(_, _, Some(found)), _) =>
            Left(s"disc '${found.projectName}' #${found.discOrdinal} at session ${found.sessionOrdinal}")
          case (MediaStatus.Appendable(_, _, None), _) => Left("a written non-diskrement disc")
          case (MediaStatus.Closed(_), _)              => Left("a finalized (closed) disc")
      }

    def loop: IO[BurnError, Long] =
      operator.confirmInsert(prompt).flatMap {
        case false => ZIO.fail(BurnError.Aborted)
        case true =>
          check.flatMap {
            case Right(free)  => ZIO.succeed(free)
            case Left(found) =>
              operator.progress(s"Not the expected disc: found $found. Swap discs and confirm again.") *> loop
          }
      }
    loop

  private def checkCapacity(stagedBytes: Long, freeBytes: Long): IO[BurnError, Unit] =
    val needed = stagedBytes + stagedBytes / 100 + fixedOverheadBytes
    ZIO.unless(needed <= freeBytes)(
      ZIO.fail(BurnError.InsufficientCapacity(needed, freeBytes))).unit

  private def doBurn(image: DiscImage, firstSession: Boolean): IO[BurnError, Unit] =
    val id    = image.metadata.identity
    val volid = Xorriso.volid(id.projectName, id.discOrdinal)
    runner.run(
      Xorriso.burnArgs(device, image.stagingDir, firstSession, image.closeDisc, volid),
      line => ZIO.when(line.contains("UPDATE"))(operator.progress(line)).unit
    ).flatMap { r =>
      ZIO.unless(r.exitCode == 0)(
        ZIO.fail(BurnError.BurnFailed(r.exitCode, r.tail()))).unit
    }

  private def postBurnCheck(id: DiscIdentity): IO[BurnError, Unit] =
    inspector.readIdentity(device).flatMap {
      case Some(found)
          if found.projectName == id.projectName
            && found.discOrdinal == id.discOrdinal
            && found.sessionOrdinal == id.sessionOrdinal => ZIO.unit
      case other =>
        ZIO.fail(BurnError.PostBurnCheckFailed(
          s"expected identity (${id.projectName}, disc ${id.discOrdinal}, session ${id.sessionOrdinal}) " +
            s"readable from the disc, found: $other"))
    }

  /** Best-effort: the burn already succeeded, an eject hiccup must not fail it. */
  private val eject: IO[BurnError, Unit] = device match
    case BurnDevice.RealDevice(_) =>
      runner.run(Xorriso.ejectArgs(device))
        .flatMap(r => ZIO.when(r.exitCode != 0)(operator.progress("note: eject failed"))).unit
        .catchAll(_ => operator.progress("note: eject failed"))
    case BurnDevice.ImageFile(_) => ZIO.unit

object XorrisoBurner:
  val live: ZLayer[
    BurnDevice & CmdRunner & MediaInspector & BurnOperator & Hasher & SnapshotWriter,
    Nothing, Burner] =
    ZLayer {
      for
        dev  <- ZIO.service[BurnDevice]
        run  <- ZIO.service[CmdRunner]
        insp <- ZIO.service[MediaInspector]
        op   <- ZIO.service[BurnOperator]
        h    <- ZIO.service[Hasher]
        w    <- ZIO.service[SnapshotWriter]
      yield XorrisoBurnerLive(dev, run, insp, op, h, w)
    }
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `sbt 'shell/testOnly diskrement.shell.burn.XorrisoBurnerSpec'`
Expected: PASS (8 tests). Also run `sbt 'shell/testOnly diskrement.shell.burn.MediaInspectorSpec'` again — it now imports `FakeCmdRunner` from its own file.

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "feat(burn): XorrisoBurner with strict media checks, capacity guard, post-burn check"
```

---

### Task 6: Image-file integration spec for the real burner

The real xorriso binary against `stdio:` images: this exercises the exact production code path minus physical media.

**Files:**
- Test: `shell/src/test/scala/diskrement/shell/burn/XorrisoImageITSpec.scala`

- [ ] **Step 1: Write the spec** (it can be written directly against the now-existing implementation; it is "failing" only if the implementation is wrong — which is the point)

```scala
package diskrement.shell.burn

import diskrement.core.{ContentHash, RelPath, ScannedFile, SessionManifest}
import diskrement.shell.fs.HasherLive
import zio._
import zio.test._
import java.nio.file.{Files, Path}
import java.time.Instant

/** Runs the REAL xorriso against stdio: image files — the production code path
  * end to end, no hardware. Skipped (with a visible marker test) when xorriso
  * is not installed. */
object XorrisoImageITSpec extends ZIOSpecDefault:

  private val hasXorriso: Boolean =
    try new ProcessBuilder("xorriso", "--version").start().waitFor() == 0
    catch case _: Exception => false

  private val hasher = HasherLive()

  private def sha256(s: String): ContentHash =
    ContentHash(java.security.MessageDigest.getInstance("SHA-256")
      .digest(s.getBytes("UTF-8")).map(b => f"${b & 0xff}%02x").mkString)

  private def scanned(rel: String, content: String): ScannedFile =
    ScannedFile(RelPath(rel), content.getBytes("UTF-8").length.toLong,
      Instant.parse("2026-08-30T10:00:00Z"), sha256(content))

  /** Build a real snapshot dir + DiscImage for one session. */
  private def mkImage(base: Path, sessionOrdinal: Int, files: Map[String, String],
                      closeDisc: Boolean): DiscImage =
    val snap = base.resolve(s"snapshot-$sessionOrdinal")
    files.foreach { case (rel, content) =>
      val p = snap.resolve(rel); Files.createDirectories(p.getParent); Files.writeString(p, content)
    }
    val catSnap = base.resolve(s"cat-$sessionOrdinal.sqlite")
    Files.writeString(catSnap, s"fake-catalog-session-$sessionOrdinal")
    val id = DiscIdentity("photos", 1, sessionOrdinal, Instant.parse("2026-08-30T10:00:00Z"))
    DiscImage(base.resolve(s"staging-$sessionOrdinal"), snap,
      files.map((r, c) => scanned(r, c)).toList,
      DiscMetadata(id, SessionManifest("photos", 1, sessionOrdinal, Nil, Nil, Nil), catSnap, Nil),
      closeDisc)

  private def mkBurner(iso: Path, answers: List[Boolean]) =
    for
      ans <- Ref.make(answers)
      log <- Ref.make(List.empty[String])
      dev  = BurnDevice.ImageFile(iso)
      run  = CmdRunnerLive()
      insp = XorrisoMediaInspectorLive(run)
      b    = XorrisoBurnerLive(dev, run, insp, ScriptedOperator(ans, log), hasher, SnapshotWriterLive())
    yield (b, dev, run, insp)

  private def realSuite = suite("XorrisoImageITSpec")(
    test("burn session 1, append session 2; merged view hides v1; session 1 still readable") {
      ZIO.attemptBlocking(Files.createTempDirectory("xit")).orDie.flatMap { base =>
        val iso = base.resolve("disc.iso")
        for
          (b1, dev, run, insp) <- mkBurner(iso, List(true))
          // session 1: a.txt v1 + b.txt
          r1   <- b1.burn(mkImage(base, 1, Map("a.txt" -> "hello v1", "b.txt" -> "stable"), closeDisc = false))
          s1   <- insp.inspect(dev)
          // session 2: a.txt v2 (hides v1)
          (b2, _, _, _) <- mkBurner(iso, List(true))
          r2   <- b2.burn(mkImage(base, 2, Map("a.txt" -> "hello v2"), closeDisc = false))
          s2   <- insp.inspect(dev)
          // merged view reads v2
          mergedDest = base.resolve("merged-a.txt")
          _    <- run.run(Xorriso.extractArgs(dev, "/data/a.txt", mergedDest, None))
          merged <- ZIO.attemptBlocking(Files.readString(mergedDest)).orDie
          // session 1 still holds v1 (Tier-2 mechanics)
          oldDest = base.resolve("old-a.txt")
          _    <- run.run(Xorriso.extractArgs(dev, "/data/a.txt", oldDest, Some(1)))
          old  <- ZIO.attemptBlocking(Files.readString(oldDest)).orDie
        yield assertTrue(
          r1.imageBytes > 0L, r2.imageBytes > 0L,
          s1 match { case MediaStatus.Appendable(_, 1, Some(id)) => id.sessionOrdinal == 1; case _ => false },
          s2 match { case MediaStatus.Appendable(_, 2, Some(id)) => id.sessionOrdinal == 2; case _ => false },
          merged == "hello v2", old == "hello v1")
      }
    },
    test("top-off of a foreign image is refused until abort") {
      ZIO.attemptBlocking(Files.createTempDirectory("xit")).orDie.flatMap { base =>
        val iso = base.resolve("foreign.iso")
        for
          (b1, _, _, _) <- mkBurner(iso, List(true))
          _    <- b1.burn(mkImage(base, 1, Map("x.txt" -> "other"), closeDisc = false))
          // now pretend it's disc 1 session 1 of project 'other' by burning to it from 'photos' session 3:
          (b2, _, _, _) <- mkBurner(iso, List(true, false)) // one retry then give up
          img   = mkImage(base, 3, Map("y.txt" -> "new"), closeDisc = false)
          exit <- b2.burn(img).exit
        yield assertTrue(exit == Exit.fail(BurnError.Aborted))
      }
    }
  ) @@ TestAspect.sequential @@ TestAspect.withLiveClock

  def spec =
    if hasXorriso then realSuite
    else suite("XorrisoImageITSpec")(
      test("SKIPPED: xorriso not installed")(assertCompletes))
```

(The second test's foreign disc is "wrong session ordinal" — the image identifies as photos/1/1 but session 3 expects previous ordinal 2 → refusal loop → scripted abort.)

- [ ] **Step 2: Run it**

Run: `sbt 'shell/testOnly diskrement.shell.burn.XorrisoImageITSpec'`
Expected: PASS (2 tests) on this machine (xorriso installed). Debug the burner against the real tool here until green — this test is the authority on whether the command shapes are right.

- [ ] **Step 3: Run the whole shell suite**

Run: `sbt shell/test`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "test(burn): image-file integration spec driving XorrisoBurner through real xorriso"
```

---

### Task 7: Catalog — `discRecordedContents`

**Files:**
- Create: `shell/src/main/scala/diskrement/shell/model/DiscRecordedContents.scala`
- Modify: `shell/src/main/scala/diskrement/shell/catalog/CatalogStore.scala`, `CatalogStoreLive.scala`
- Test: `shell/src/test/scala/diskrement/shell/catalog/DiscRecordedContentsSpec.scala` (the catalog suite is one spec file per method; `TestDb.layer` provides a per-test temp SQLite store)

- [ ] **Step 1: Create the model types**

```scala
package diskrement.shell.model

import diskrement.core.{ContentHash, RelPath}

/** A file version on a disc hidden by a later session of the SAME disc. */
final case class SupersededVersion(sessionOrdinal: Int, relPath: RelPath, contentHash: ContentHash)

/** Everything the catalog records as physically on one disc: the merged view a
  * mount shows (`visible`), and versions hidden within the disc (`superseded`). */
final case class DiscRecordedContents(
    visible: Map[RelPath, ContentHash],
    superseded: List[SupersededVersion])
```

- [ ] **Step 2: Write the failing test** — new file `DiscRecordedContentsSpec.scala`, same helper style as `ArchivePointersSpec.scala`:

```scala
package diskrement.shell.catalog

import diskrement.core._
import diskrement.shell.model._
import zio._
import zio.test._
import java.time.Instant

object DiscRecordedContentsSpec extends ZIOSpecDefault:
  private val init = ProjectInit("p", "/src", "/work", DiscType.BD_R, 2.0, 1,
    Instant.parse("2026-08-30T00:00:00Z"))
  private val ts = Instant.parse("2026-08-30T10:00:00Z")
  private def fw(path: String, hash: String, size: Long) =
    FileWrite(RelPath(path), ContentHash(hash), size, ts, "/data/" + path, 0)
  private def session(discOrd: Int, ord: Int, added: List[FileWrite] = Nil,
                      modified: List[FileWrite] = Nil) =
    CatalogUpdate(ProjectId(0), Nil, List(DiscWrite(
      discOrd, isNew = ord == 1, DiscType.BD_R, 1_000_000L, DiscStatus.OPEN, s"d$discOrd",
      SessionWrite(ord, ts, "m" + ord, added, modified, Nil))))

  def spec = suite("CatalogStore.discRecordedContents")(
    test("merged view hides same-path earlier versions; superseded lists them with their session"):
      for
        pid <- ZIO.serviceWithZIO[CatalogStore](_.createProject(init))
        st   = ZIO.serviceWithZIO[CatalogStore]
        _   <- st(_.applyUpdate(session(1, 1,
                 added = List(fw("a.txt", "aa", 10), fw("b.txt", "bb", 20))).copy(projectId = pid)))
        _   <- st(_.applyUpdate(session(1, 2,
                 modified = List(fw("a.txt", "a2", 12))).copy(projectId = pid)))
        rec <- st(_.discRecordedContents(pid, 1))
      yield assertTrue(
        rec.visible == Map(RelPath("a.txt") -> ContentHash("a2"), RelPath("b.txt") -> ContentHash("bb")),
        rec.superseded == List(SupersededVersion(1, RelPath("a.txt"), ContentHash("aa")))),
    test("versions on other discs do not leak in; a fresh disc has no superseded"):
      for
        pid <- ZIO.serviceWithZIO[CatalogStore](_.createProject(init))
        st   = ZIO.serviceWithZIO[CatalogStore]
        _   <- st(_.applyUpdate(session(1, 1, added = List(fw("a.txt", "aa", 10))).copy(projectId = pid)))
        _   <- st(_.applyUpdate(session(2, 1, added = List(fw("z.txt", "zz", 30))).copy(projectId = pid)))
        r1  <- st(_.discRecordedContents(pid, 1))
        r2  <- st(_.discRecordedContents(pid, 2))
      yield assertTrue(
        r1 == DiscRecordedContents(Map(RelPath("a.txt") -> ContentHash("aa")), Nil),
        r2 == DiscRecordedContents(Map(RelPath("z.txt") -> ContentHash("zz")), Nil))
  ).provideLayer(TestDb.layer)
```

Note: a tombstone does NOT remove a file from the disc's recorded contents (deleted files stay physically on the disc and must still verify) — the query reads `placement` rows only, never `change_record`.

- [ ] **Step 3: Run it to verify it fails**

Run: `sbt 'shell/testOnly diskrement.shell.catalog.*'`
Expected: compile FAILURE (`discRecordedContents` not a member).

- [ ] **Step 4: Implement.** Trait method (in `CatalogStore.scala`, after `placementsForDisc`):

```scala
  /** Everything recorded as physically on `discOrdinal`: the merged view a mount
    * of the disc shows (later sessions hide same-path versions from earlier
    * ones), plus those hidden ("superseded within disc") versions with the
    * session that recorded them. Powers two-tier read-back verify. */
  def discRecordedContents(project: ProjectId, discOrdinal: Int): IO[CatalogError, DiscRecordedContents]
```

Implementation (in `CatalogStoreLive.scala`):

```scala
  def discRecordedContents(project: ProjectId, discOrdinal: Int): IO[CatalogError, DiscRecordedContents] =
    tx {
      sql"""select rp, ch, sess_ord, rn from (
              select fv.rel_path as rp, fv.content_hash as ch, s.ordinal as sess_ord,
                     row_number() over (partition by fv.rel_path order by s.ordinal desc, p.id desc) as rn
              from placement p
              join session s on s.id = p.session_id
              join disc d on d.id = s.disc_id
              join file_version fv on fv.id = p.file_version_id
              where d.project_id = ${project.value} and d.ordinal = $discOrdinal
            ) order by rp, rn"""
        .query[(String, String, Int, Int)].selectAll
    }.map { rows =>
      val (vis, sup) = rows.toList.partition(_._4 == 1)
      DiscRecordedContents(
        visible = vis.map { case (rp, ch, _, _) => RelPath(rp) -> ContentHash(ch) }.toMap,
        superseded = sup.map { case (rp, ch, sOrd, _) => SupersededVersion(sOrd, RelPath(rp), ContentHash(ch)) })
    }
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `sbt 'shell/testOnly diskrement.shell.catalog.*'`
Expected: PASS (existing tests + the new one).

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "feat(catalog): discRecordedContents (merged view + superseded-within-disc)"
```

---

### Task 8: `DiscReader`

**Files:**
- Create: `shell/src/main/scala/diskrement/shell/burn/DiscReader.scala`
- Test: `shell/src/test/scala/diskrement/shell/burn/DiscReaderSpec.scala`

- [ ] **Step 1: Write the failing test** (image-backed, guarded like Task 6; plus a pure test of the udisks mount-point parser)

```scala
package diskrement.shell.burn

import diskrement.core.{ContentHash, RelPath, ScannedFile, SessionManifest}
import diskrement.shell.fs.HasherLive
import zio._
import zio.test._
import java.nio.file.{Files, Path}
import java.time.Instant

object DiscReaderSpec extends ZIOSpecDefault:

  private val hasXorriso: Boolean =
    try new ProcessBuilder("xorriso", "--version").start().waitFor() == 0
    catch case _: Exception => false

  private val parserSuite = suite("mount-point parsing")(
    test("parses udisksctl mount output, with and without trailing dot") {
      assertTrue(
        DiscReaderLive.parseMountPoint(List("Mounted /dev/sr0 at /run/media/z/PHOTOS_DISC_1")) ==
          Some(Path.of("/run/media/z/PHOTOS_DISC_1")),
        DiscReaderLive.parseMountPoint(List("Mounted /dev/sr0 at /run/media/z/D.")) ==
          Some(Path.of("/run/media/z/D")),
        DiscReaderLive.parseMountPoint(
          List("Error mounting: GDBus.Error:org.freedesktop.UDisks2.Error.AlreadyMounted: " +
               "Device /dev/sr0 is already mounted at `/run/media/z/OLD'.")) ==
          Some(Path.of("/run/media/z/OLD")),
        DiscReaderLive.parseMountPoint(List("garbage")) == None)
    })

  private def imageSuite = suite("image-backed access")(
    test("withDataTree exposes the merged /data tree; extractFromSession reads an old version") {
      ZIO.attemptBlocking(Files.createTempDirectory("dr")).orDie.flatMap { base =>
        val iso = base.resolve("disc.iso")
        val run = CmdRunnerLive()
        val dev = BurnDevice.ImageFile(iso)
        // Build a 2-session image with the same helper approach as XorrisoImageITSpec:
        // session 1: a.txt "v1"; session 2: a.txt "v2".
        def burnSession(n: Int, content: String) =
          for
            ans <- Ref.make(List(true)); log <- Ref.make(List.empty[String])
            b    = XorrisoBurnerLive(dev, run, XorrisoMediaInspectorLive(run),
                     ScriptedOperator(ans, log), HasherLive(), SnapshotWriterLive())
            snap = base.resolve(s"snap$n"); _ = Files.createDirectories(snap)
            _    = Files.writeString(snap.resolve("a.txt"), content)
            cat  = base.resolve(s"cat$n"); _ = Files.writeString(cat, "x")
            hash = ContentHash(java.security.MessageDigest.getInstance("SHA-256")
                     .digest(content.getBytes("UTF-8")).map(x => f"${x & 0xff}%02x").mkString)
            img  = DiscImage(base.resolve(s"stage$n"), snap,
                     List(ScannedFile(RelPath("a.txt"), content.length.toLong,
                       Instant.parse("2026-08-30T10:00:00Z"), hash)),
                     DiscMetadata(DiscIdentity("p", 1, n, Instant.parse("2026-08-30T10:00:00Z")),
                       SessionManifest("p", 1, n, Nil, Nil, Nil), cat, Nil), false)
            _   <- b.burn(img)
          yield ()
        val reader = DiscReaderLive(run)
        for
          _   <- burnSession(1, "v1")
          _   <- burnSession(2, "v2")
          v2  <- reader.withDataTree(dev)(data =>
                   ZIO.attemptBlocking(Files.readString(data.resolve("a.txt")))
                     .mapError(BurnError.BurnIoError(_)))
          old  = base.resolve("old.txt")
          _   <- reader.extractFromSession(dev, 1, "/data/a.txt", old)
          v1  <- ZIO.attemptBlocking(Files.readString(old)).orDie
        yield assertTrue(v2 == "v2", v1 == "v1")
      }
    }
  ) @@ TestAspect.sequential @@ TestAspect.withLiveClock

  def spec =
    if hasXorriso then suite("DiscReaderSpec")(parserSuite, imageSuite)
    else suite("DiscReaderSpec")(parserSuite,
      test("SKIPPED image suite: xorriso not installed")(assertCompletes))
```

- [ ] **Step 2: Run it to verify it fails**

Run: `sbt 'shell/testOnly diskrement.shell.burn.DiscReaderSpec'`
Expected: compile FAILURE.

- [ ] **Step 3: Implement `DiscReader.scala`**

```scala
package diskrement.shell.burn

import zio._
import java.nio.file.{Files, Path}
import java.util.Comparator

/** Read access to a burned disc's content for verification. Real devices are
  * mounted via udisks (polkit lets a desktop session mount removable media
  * without root); image files are extracted via xorriso's osirrox. */
trait DiscReader:
  /** Expose the disc's merged /data tree as a local directory for `use`. */
  def withDataTree[A](device: BurnDevice)(use: Path => IO[BurnError, A]): IO[BurnError, A]
  /** Extract one file as recorded by a specific session (Tier-2 verify). */
  def extractFromSession(device: BurnDevice, sessionOrdinal: Int,
                         discPath: String, dest: Path): IO[BurnError, Unit]

final case class DiscReaderLive(runner: CmdRunner) extends DiscReader:

  def withDataTree[A](device: BurnDevice)(use: Path => IO[BurnError, A]): IO[BurnError, A] =
    device match
      case BurnDevice.RealDevice(path) =>
        ZIO.acquireReleaseWith(mount(path))(_ => unmount(path)) { mountPoint =>
          use(mountPoint.resolve("data"))
        }
      case BurnDevice.ImageFile(_) =>
        ZIO.acquireReleaseWith(
          ZIO.attemptBlockingIO(Files.createTempDirectory("diskrement-verify"))
            .mapError(BurnError.BurnIoError(_)))(dir =>
          ZIO.attemptBlockingIO {
            val s = Files.walk(dir)
            try s.sorted(Comparator.reverseOrder()).forEach(p => Files.deleteIfExists(p))
            finally s.close()
          }.ignore) { dir =>
          val dataDir = dir.resolve("data")
          runner.run(Xorriso.extractArgs(device, "/data", dataDir, None)).flatMap { r =>
            if r.exitCode != 0 then ZIO.fail(BurnError.MountFailed(
              s"osirrox extraction failed: ${r.tail().mkString(" | ")}"))
            else use(dataDir)
          }
        }

  def extractFromSession(device: BurnDevice, sessionOrdinal: Int,
                         discPath: String, dest: Path): IO[BurnError, Unit] =
    runner.run(Xorriso.extractArgs(device, discPath, dest, Some(sessionOrdinal))).flatMap { r =>
      ZIO.unless(r.exitCode == 0)(ZIO.fail(BurnError.MountFailed(
        s"session $sessionOrdinal extraction of $discPath failed: ${r.tail().mkString(" | ")}"))).unit
    }

  private def mount(devicePath: String): IO[BurnError, Path] =
    runner.run(List("udisksctl", "mount", "-b", devicePath)).flatMap { r =>
      DiscReaderLive.parseMountPoint(r.output) match
        case Some(p) => ZIO.succeed(p)
        case None => ZIO.fail(BurnError.MountFailed(
          s"udisksctl gave no mount point: ${r.tail().mkString(" | ")}"))
    }.mapError {
      case BurnError.ToolMissing(d) => BurnError.MountFailed(s"udisksctl not available: $d")
      case other                    => other
    }

  private def unmount(devicePath: String): UIO[Unit] =
    runner.run(List("udisksctl", "unmount", "-b", devicePath)).ignore

object DiscReaderLive:
  private val MountedRe = """.*Mounted \S+ at (\S+?)\.?$""".r
  private val AlreadyRe = """.*already mounted at `([^']+)'.*""".r

  def parseMountPoint(lines: List[String]): Option[Path] =
    lines.collectFirst {
      case MountedRe(p) => Path.of(p)
      case AlreadyRe(p) => Path.of(p)
    }

object DiscReader:
  val live: ZLayer[CmdRunner, Nothing, DiscReader] =
    ZLayer(ZIO.service[CmdRunner].map(DiscReaderLive(_)))
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `sbt 'shell/testOnly diskrement.shell.burn.DiscReaderSpec'`
Expected: PASS. If osirrox refuses to extract into the fresh `dataDir` (directory must not pre-exist for a dir extract, or vice versa), adjust the temp-dir handling in `withDataTree` until the image test is green — the test is the authority.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(burn): DiscReader (udisks mount / osirrox extraction)"
```

---

### Task 9: `DiscVerifier` port; `StagingDiscVerifier`; pipeline verify rework

**Files:**
- Create: `shell/src/main/scala/diskrement/shell/burn/DiscVerifier.scala` (port + expectation types only)
- Create: `shell/src/main/scala/diskrement/shell/pipeline/StagingDiscVerifier.scala`
- Modify: `shell/src/main/scala/diskrement/shell/pipeline/Pipeline.scala` (VerifyReport gains `skipped`; layer gains `DiscVerifier`)
- Modify: `shell/src/main/scala/diskrement/shell/pipeline/PipelineLive.scala` (verify via the port)
- Modify: `shell/src/main/scala/diskrement/shell/pipeline/PipelineError.scala` (add `Burn`)
- Modify: `cli/src/main/scala/diskrement/cli/Render.scala` + `Runner.scala` (compile only; full CLI work is Task 11)
- Test: `shell/src/test/scala/diskrement/shell/pipeline/StagingDiscVerifierSpec.scala`; existing pipeline specs updated

- [ ] **Step 1: Create `DiscVerifier.scala`**

```scala
package diskrement.shell.burn

import diskrement.core.{ContentHash, RelPath}
import diskrement.shell.model.SupersededVersion
import zio._

/** What the catalog says one disc should hold. `latestSession` is what the
  * simulated backend's staging dir contains; `visible`/`superseded` describe the
  * physical disc (merged mount view + versions hidden by later sessions). Each
  * verifier consumes the fields that match its substrate. */
final case class DiscExpectation(
    projectName: String,
    discOrdinal: Int,
    latestSession: Map[RelPath, ContentHash],
    visible: Map[RelPath, ContentHash],
    superseded: List[SupersededVersion])

/** skipped = nothing was checked (no staged data / operator declined the disc);
  * a skipped disc is never marked verified. */
final case class VerifyOutcome(filesChecked: Int, mismatches: List[RelPath], skipped: Boolean)

trait DiscVerifier:
  def verify(expectation: DiscExpectation): IO[BurnError, VerifyOutcome]
```

- [ ] **Step 2: Write the failing test for `StagingDiscVerifier`**

```scala
package diskrement.shell.pipeline

import diskrement.core.{ContentHash, RelPath}
import diskrement.shell.burn.{DiscExpectation, VerifyOutcome}
import diskrement.shell.fs.HasherLive
import zio._
import zio.test._
import java.nio.file.Files

object StagingDiscVerifierSpec extends ZIOSpecDefault:

  private def sha(s: String) = ContentHash(java.security.MessageDigest.getInstance("SHA-256")
    .digest(s.getBytes("UTF-8")).map(b => f"${b & 0xff}%02x").mkString)

  def spec = suite("StagingDiscVerifierSpec")(
    test("clean staging dir verifies; corrupt file mismatches; missing dir skips") {
      ZIO.attemptBlocking(Files.createTempDirectory("sdv")).orDie.flatMap { work =>
        val data = work.resolve("staging/disc-1/data")
        Files.createDirectories(data)
        Files.writeString(data.resolve("a.txt"), "hello")
        Files.writeString(data.resolve("bad.txt"), "corrupted")
        val v = StagingDiscVerifier(work.resolve("staging"), HasherLive())
        val exp = DiscExpectation("p", 1,
          latestSession = Map(RelPath("a.txt") -> sha("hello"), RelPath("bad.txt") -> sha("original")),
          visible = Map.empty, superseded = Nil)
        for
          out  <- v.verify(exp)
          out2 <- v.verify(exp.copy(discOrdinal = 9))
        yield assertTrue(
          out == VerifyOutcome(2, List(RelPath("bad.txt")), skipped = false),
          out2 == VerifyOutcome(0, Nil, skipped = true))
      }
    }
  )
```

- [ ] **Step 3: Run it to verify it fails**

Run: `sbt 'shell/testOnly diskrement.shell.pipeline.StagingDiscVerifierSpec'`
Expected: compile FAILURE.

- [ ] **Step 4: Implement `StagingDiscVerifier.scala`** (the logic moves out of `PipelineLive.verifyDisc`)

```scala
package diskrement.shell.pipeline

import diskrement.core.RelPath
import diskrement.shell.burn.{BurnError, DiscExpectation, DiscVerifier, VerifyOutcome}
import diskrement.shell.fs.Hasher
import zio._
import java.nio.file.{Files, Path}
import scala.jdk.StreamConverters.*

/** Verifies a disc against the work-dir staging tree (simulated backend). The
  * staging dir holds only the disc's LATEST session, so the expectation's
  * `latestSession` map is the reference; an absent staging dir is a skip. */
final case class StagingDiscVerifier(stagingRoot: Path, hasher: Hasher) extends DiscVerifier:

  def verify(exp: DiscExpectation): IO[BurnError, VerifyOutcome] =
    val dataDir = stagingRoot.resolve(s"disc-${exp.discOrdinal}").resolve("data")
    ZIO.attemptBlockingIO(Files.exists(dataDir)).mapError(BurnError.BurnIoError(_)).flatMap {
      case false => ZIO.succeed(VerifyOutcome(0, Nil, skipped = true))
      case true =>
        for
          files <- ZIO.attemptBlockingIO {
                     val s = Files.walk(dataDir)
                     try s.toScala(List).filter(p => Files.isRegularFile(p))
                     finally s.close()
                   }.mapError(BurnError.BurnIoError(_))
          mismatches <- ZIO.foreach(files) { f =>
                          val rel = RelPath(dataDir.relativize(f).toString)
                          hasher.hash(f).mapError(BurnError.BurnIoError(_)).map { h =>
                            Option.when(!exp.latestSession.get(rel).contains(h))(rel)
                          }
                        }.map(_.flatten)
        yield VerifyOutcome(files.size, mismatches, skipped = false)
    }

object StagingDiscVerifier:
  val live: ZLayer[ProjectPaths & Hasher, Nothing, DiscVerifier] =
    ZLayer {
      for
        paths  <- ZIO.service[ProjectPaths]
        hasher <- ZIO.service[Hasher]
      yield StagingDiscVerifier(paths.workDir.resolve("staging"), hasher)
    }
```

- [ ] **Step 5: Rework the pipeline.**

`PipelineError.scala` — add:

```scala
  final case class Burn(cause: BurnError) extends PipelineError(cause.getMessage, cause)
```

(and `import diskrement.shell.burn.BurnError` at the top.)

`Pipeline.scala`:
- `VerifyReport` becomes `final case class VerifyReport(discOrdinal: Int, filesChecked: Int, mismatches: List[RelPath], skipped: Boolean = false)`.
- The layer: environment gains `DiscVerifier`; `ZIO.service[DiscVerifier]` feeds a new `PipelineLive` field.

`PipelineLive.scala`:
- Constructor gains `verifier: DiscVerifier` (after `burner`).
- In `burn`, change `burner.burn(image).mapError(PipelineError.Failed(_))` to `.mapError(PipelineError.Burn(_))`.
- Replace `verifyDisc` entirely:

```scala
  private def verifyDisc(
      p: Project, ordinal: Int, sessions: List[Session]
  ): IO[PipelineError, VerifyReport] =
    for
      latest   <- catalog.placementsForDisc(p.id, ordinal).mapError(PipelineError.Failed(_))
      recorded <- catalog.discRecordedContents(p.id, ordinal).mapError(PipelineError.Failed(_))
      exp       = DiscExpectation(p.name, ordinal, latest, recorded.visible, recorded.superseded)
      out      <- verifier.verify(exp).mapError(PipelineError.Burn(_))
      _        <- ZIO.when(!out.skipped && out.mismatches.isEmpty)(
                    sessions.filter(_.discOrdinal == ordinal).lastOption match
                      case Some(s) => catalog.markVerified(p.id, ordinal, s.ordinal)
                                        .mapError(PipelineError.Failed(_))
                      case None    => ZIO.unit)
    yield VerifyReport(ordinal, out.filesChecked, out.mismatches, out.skipped)
```

`verify` itself changes its call to `verifyDisc(p, o, sessions)` (passing the `Project`, not just the id) and its `targets` line stays as is. Remove the now-unused `hasher`-based walking imports if they become unused.

`Runner.scala` (compile-only for now): add `StagingDiscVerifier.live` to the `ZLayer.make[Pipeline]` list.

`Render.scala` (compile-only for now): in `handle`, add `case PipelineError.Burn(_) => ExitCode(6)` to the code match (before the message match; `Burn`'s message passes through the default `other => other.getMessage` branch). In `verify`, render skipped discs: inside the `reports.map`, first branch on `v.skipped`:

```scala
      if v.skipped then s"disc ${v.discOrdinal}: skipped (not checked)"
      else if v.mismatches.isEmpty then s"disc ${v.discOrdinal}: ${v.filesChecked} files OK ✓"
      else ...
```

- [ ] **Step 6: Run everything**

Run: `sbt shell/test cli/test`
Expected: PASS. Existing pipeline specs may need `StagingDiscVerifier.live` added to their layer stacks and `VerifyReport` pattern matches extended — the `skipped` default keeps most construction sites compiling. `RenderSpec` may need a skipped-case expectation added.

- [ ] **Step 7: Commit**

```bash
git add -A && git commit -m "feat(pipeline): DiscVerifier port; staging verifier; PipelineError.Burn; skipped verify reports"
```

---

### Task 10: `RealDiscVerifier` (Tier 1 + Tier 2)

**Files:**
- Modify: `shell/src/main/scala/diskrement/shell/burn/DiscVerifier.scala` (add the real implementation + layer)
- Test: `shell/src/test/scala/diskrement/shell/burn/RealDiscVerifierSpec.scala`

- [ ] **Step 1: Write the failing test** — image-backed end-to-end (guarded like Task 6): burn 2 sessions, then verify with a hand-built expectation. Plus a scripted-operator skip case.

```scala
package diskrement.shell.burn

import diskrement.core.{ContentHash, RelPath, ScannedFile, SessionManifest}
import diskrement.shell.fs.HasherLive
import diskrement.shell.model.SupersededVersion
import zio._
import zio.test._
import java.nio.file.{Files, Path}
import java.time.Instant

object RealDiscVerifierSpec extends ZIOSpecDefault:

  private val hasXorriso: Boolean =
    try new ProcessBuilder("xorriso", "--version").start().waitFor() == 0
    catch case _: Exception => false

  private def sha(s: String) = ContentHash(java.security.MessageDigest.getInstance("SHA-256")
    .digest(s.getBytes("UTF-8")).map(b => f"${b & 0xff}%02x").mkString)

  private def burnSession(base: Path, iso: Path, n: Int, files: Map[String, String]) =
    val run = CmdRunnerLive()
    val dev = BurnDevice.ImageFile(iso)
    for
      ans <- Ref.make(List(true)); log <- Ref.make(List.empty[String])
      snap = base.resolve(s"snap$n")
      _    = files.foreach { (r, c) =>
               val p = snap.resolve(r); Files.createDirectories(p.getParent); Files.writeString(p, c) }
      cat  = base.resolve(s"cat$n"); _ = Files.writeString(cat, "x")
      img  = DiscImage(base.resolve(s"stage$n"), snap,
               files.map((r, c) => ScannedFile(RelPath(r), c.length.toLong,
                 Instant.parse("2026-08-30T10:00:00Z"), sha(c))).toList,
               DiscMetadata(DiscIdentity("photos", 1, n, Instant.parse("2026-08-30T10:00:00Z")),
                 SessionManifest("photos", 1, n, Nil, Nil, Nil), cat, Nil), false)
      _   <- XorrisoBurnerLive(dev, run, XorrisoMediaInspectorLive(run),
               ScriptedOperator(ans, log), HasherLive(), SnapshotWriterLive()).burn(img)
    yield ()

  private def mkVerifier(iso: Path, answers: List[Boolean]) =
    val run = CmdRunnerLive()
    val dev = BurnDevice.ImageFile(iso)
    for
      ans <- Ref.make(answers); log <- Ref.make(List.empty[String])
    yield RealDiscVerifierLive(dev, XorrisoMediaInspectorLive(run), ScriptedOperator(ans, log),
      DiscReaderLive(run), HasherLive())

  private def realSuite = suite("RealDiscVerifierSpec")(
    test("two-tier verify: clean disc passes, corrupted expectation mismatches, decline skips") {
      ZIO.attemptBlocking(Files.createTempDirectory("rdv")).orDie.flatMap { base =>
        val iso = base.resolve("disc.iso")
        val exp = DiscExpectation("photos", 1,
          latestSession = Map.empty,
          visible = Map(RelPath("a.txt") -> sha("v2"), RelPath("b.txt") -> sha("keep")),
          superseded = List(SupersededVersion(1, RelPath("a.txt"), sha("v1"))))
        for
          _     <- burnSession(base, iso, 1, Map("a.txt" -> "v1", "b.txt" -> "keep"))
          _     <- burnSession(base, iso, 2, Map("a.txt" -> "v2"))
          v     <- mkVerifier(iso, List(true))
          clean <- v.verify(exp)
          v2    <- mkVerifier(iso, List(true))
          bad   <- v2.verify(exp.copy(
                     visible = exp.visible.updated(RelPath("b.txt"), sha("TAMPERED")),
                     superseded = List(SupersededVersion(1, RelPath("a.txt"), sha("WRONG")))))
          v3    <- mkVerifier(iso, List(false))
          skip  <- v3.verify(exp)
        yield assertTrue(
          clean == VerifyOutcome(3, Nil, skipped = false), // 2 visible + 1 superseded
          bad.mismatches.toSet == Set(RelPath("b.txt"), RelPath("a.txt")),
          skip == VerifyOutcome(0, Nil, skipped = true))
      }
    },
    test("missing expected file is a mismatch") {
      ZIO.attemptBlocking(Files.createTempDirectory("rdv2")).orDie.flatMap { base =>
        val iso = base.resolve("disc.iso")
        for
          _   <- burnSession(base, iso, 1, Map("only.txt" -> "here"))
          v   <- mkVerifier(iso, List(true))
          out <- v.verify(DiscExpectation("photos", 1, Map.empty,
                   visible = Map(RelPath("only.txt") -> sha("here"),
                                 RelPath("ghost.txt") -> sha("missing")),
                   superseded = Nil))
        yield assertTrue(out.mismatches == List(RelPath("ghost.txt")))
      }
    }
  ) @@ TestAspect.sequential @@ TestAspect.withLiveClock

  def spec =
    if hasXorriso then realSuite
    else suite("RealDiscVerifierSpec")(test("SKIPPED: xorriso not installed")(assertCompletes))
```

- [ ] **Step 2: Run it to verify it fails**

Run: `sbt 'shell/testOnly diskrement.shell.burn.RealDiscVerifierSpec'`
Expected: compile FAILURE.

- [ ] **Step 3: Implement** — append to `DiscVerifier.scala`:

```scala
import diskrement.shell.fs.Hasher
import java.nio.file.{Files, Path}
import scala.jdk.StreamConverters.*

/** Read-back verification of physical (or image) media. Tier 1: the merged
  * /data tree via DiscReader, every file hashed against `visible`, expected
  * files missing from the disc are mismatches. Tier 2: versions superseded
  * within the disc, extracted from their recording session. An operator
  * decline skips the disc (it stays unverified). */
final case class RealDiscVerifierLive(
    device: BurnDevice,
    inspector: MediaInspector,
    operator: BurnOperator,
    reader: DiscReader,
    hasher: Hasher
) extends DiscVerifier:

  def verify(exp: DiscExpectation): IO[BurnError, VerifyOutcome] =
    awaitDisc(exp).flatMap {
      case false => ZIO.succeed(VerifyOutcome(0, Nil, skipped = true))
      case true =>
        for
          t1 <- tier1(exp)
          t2 <- tier2(exp)
        yield VerifyOutcome(t1._1 + t2._1, t1._2 ++ t2._2, skipped = false)
    }

  /** Insert/identity loop like the burner's, but any session ordinal of the
    * right disc is acceptable, and a decline is a skip, not an abort. */
  private def awaitDisc(exp: DiscExpectation): IO[BurnError, Boolean] =
    val prompt =
      s"Insert disc ${exp.discOrdinal} of '${exp.projectName}' into ${device.xorrisoAddress} " +
        "to verify it (or decline to skip this disc)."
    def matches(id: DiscIdentity) =
      id.projectName == exp.projectName && id.discOrdinal == exp.discOrdinal
    def loop: IO[BurnError, Boolean] =
      operator.confirmInsert(prompt).flatMap {
        case false => ZIO.succeed(false)
        case true =>
          inspector.inspect(device).flatMap {
            case MediaStatus.Appendable(_, _, Some(id)) if matches(id) => ZIO.succeed(true)
            case MediaStatus.Closed(Some(id)) if matches(id)           => ZIO.succeed(true)
            case found =>
              operator.progress(s"Not the expected disc (found: $found). Swap discs and confirm again.") *> loop
          }
      }
    loop

  private def tier1(exp: DiscExpectation): IO[BurnError, (Int, List[RelPath])] =
    reader.withDataTree(device) { dataDir =>
      for
        files <- ZIO.attemptBlockingIO {
                   val s = Files.walk(dataDir)
                   try s.toScala(List).filter(p => Files.isRegularFile(p))
                   finally s.close()
                 }.mapError(BurnError.BurnIoError(_))
        found <- ZIO.foreach(files) { f =>
                   val rel = RelPath(dataDir.relativize(f).toString)
                   hasher.hash(f).mapError(BurnError.BurnIoError(_)).map(h => rel -> h)
                 }
        foundMap   = found.toMap
        mismatched = found.collect {
                       case (rel, h) if !exp.visible.get(rel).contains(h) => rel }
        missing    = exp.visible.keys.filterNot(foundMap.contains).toList
      yield (files.size + missing.size, mismatched ++ missing)
    }

  private def tier2(exp: DiscExpectation): IO[BurnError, (Int, List[RelPath])] =
    ZIO.foreach(exp.superseded) { sv =>
      ZIO.attemptBlockingIO(Files.createTempFile("diskrement-tier2", ".bin"))
        .mapError(BurnError.BurnIoError(_))
        .flatMap { tmp =>
          val checkOne =
            ZIO.attemptBlockingIO { Files.deleteIfExists(tmp); () }
              .mapError(BurnError.BurnIoError(_)) *>
            reader.extractFromSession(device, sv.sessionOrdinal, "/data/" + sv.relPath.value, tmp) *>
            hasher.hash(tmp).mapError(BurnError.BurnIoError(_))
              .map(h => Option.when(h != sv.contentHash)(sv.relPath))
          checkOne
            .catchAll(_ => ZIO.succeed(Some(sv.relPath))) // unreadable = mismatch
            .ensuring(ZIO.attemptBlockingIO(Files.deleteIfExists(tmp)).ignore)
        }
    }.map(res => (res.size, res.flatten))

object RealDiscVerifier:
  val live: ZLayer[BurnDevice & MediaInspector & BurnOperator & DiscReader & Hasher, Nothing, DiscVerifier] =
    ZLayer {
      for
        dev  <- ZIO.service[BurnDevice]
        insp <- ZIO.service[MediaInspector]
        op   <- ZIO.service[BurnOperator]
        rd   <- ZIO.service[DiscReader]
        h    <- ZIO.service[Hasher]
      yield RealDiscVerifierLive(dev, insp, op, rd, h)
    }
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `sbt 'shell/testOnly diskrement.shell.burn.RealDiscVerifierSpec'`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(burn): RealDiscVerifier — two-tier read-back verification"
```

---

### Task 11: CLI — `--device`/`--burner`, backend wiring, console operator, error rendering

**Files:**
- Modify: `cli/src/main/scala/diskrement/cli/Args.scala`
- Modify: `cli/src/main/scala/diskrement/cli/Commands.scala`
- Create: `cli/src/main/scala/diskrement/cli/BackendResolver.scala`
- Modify: `cli/src/main/scala/diskrement/cli/Runner.scala`
- Modify: `cli/src/main/scala/diskrement/cli/Render.scala`
- Test: `cli/src/test/scala/diskrement/cli/BackendResolverSpec.scala`; modify `CliSmokeSpec.scala`, `RenderSpec.scala`

- [ ] **Step 1: Extend `Args.scala`**

```scala
package diskrement.cli

import diskrement.core.DiscType
import java.nio.file.Path

enum BurnerKind { case Real, Simulated }

/** Global options accepted before any subcommand. */
final case class GlobalOpts(
    catalog: Option[Path],
    dataDir: Option[Path],
    device: Option[String] = None,
    burner: Option[BurnerKind] = None)
```

(`DiskrementCommand` is unchanged.)

- [ ] **Step 2: Write the failing `BackendResolver` test**

```scala
package diskrement.cli

import diskrement.shell.burn.BurnDevice
import zio.test._

object BackendResolverSpec extends ZIOSpecDefault:
  private val none: String => Option[String] = _ => None
  private def env(pairs: (String, String)*): String => Option[String] = pairs.toMap.get

  def spec = suite("BackendResolverSpec")(
    test("defaults to real burner on /dev/sr0") {
      assertTrue(BackendResolver.resolve(GlobalOpts(None, None), none) ==
        Backend.Real(BurnDevice.RealDevice("/dev/sr0")))
    },
    test("--burner simulated wins over env") {
      assertTrue(BackendResolver.resolve(
        GlobalOpts(None, None, burner = Some(BurnerKind.Simulated)),
        env("DISKREMENT_BURNER" -> "real")) == Backend.Simulated)
    },
    test("DISKREMENT_BURNER=simulated works without flags") {
      assertTrue(BackendResolver.resolve(GlobalOpts(None, None),
        env("DISKREMENT_BURNER" -> "simulated")) == Backend.Simulated)
    },
    test("device precedence: flag > env > default") {
      assertTrue(
        BackendResolver.resolve(GlobalOpts(None, None, device = Some("/dev/sr1")),
          env("DISKREMENT_DEVICE" -> "/dev/sr9")) == Backend.Real(BurnDevice.RealDevice("/dev/sr1")),
        BackendResolver.resolve(GlobalOpts(None, None),
          env("DISKREMENT_DEVICE" -> "/dev/sr9")) == Backend.Real(BurnDevice.RealDevice("/dev/sr9")))
    },
    test("unknown env burner value falls back to real") {
      assertTrue(BackendResolver.resolve(GlobalOpts(None, None),
        env("DISKREMENT_BURNER" -> "banana")) == Backend.Real(BurnDevice.RealDevice("/dev/sr0")))
    }
  )
```

- [ ] **Step 3: Run it to verify it fails**

Run: `sbt 'cli/testOnly diskrement.cli.BackendResolverSpec'`
Expected: compile FAILURE.

- [ ] **Step 4: Implement `BackendResolver.scala`**

```scala
package diskrement.cli

import diskrement.shell.burn.BurnDevice

enum Backend:
  case Simulated
  case Real(device: BurnDevice)

/** Pure flag/env resolution of the burn backend. Precedence: flag > env >
  * default (real burner on /dev/sr0). The environment is injected, matching
  * PathResolver's style. */
object BackendResolver:
  def resolve(global: GlobalOpts, env: String => Option[String]): Backend =
    val kind = global.burner.orElse {
      env("DISKREMENT_BURNER").map(_.trim.toLowerCase).collect {
        case "simulated" => BurnerKind.Simulated
        case "real"      => BurnerKind.Real
      }
    }.getOrElse(BurnerKind.Real)
    kind match
      case BurnerKind.Simulated => Backend.Simulated
      case BurnerKind.Real =>
        val dev = global.device
          .orElse(env("DISKREMENT_DEVICE").filter(_.nonEmpty))
          .getOrElse("/dev/sr0")
        Backend.Real(BurnDevice.RealDevice(dev))
```

- [ ] **Step 5: Wire `Commands.scala`** — extend the global options block:

```scala
  private val catalogOpt: Options[Option[Path]] = Options.text("catalog").map(path).optional
  private val dataDirOpt: Options[Option[Path]] = Options.text("data-dir").map(path).optional
  private val deviceOpt:  Options[Option[String]] = Options.text("device").optional
  private val burnerOpt:  Options[Option[BurnerKind]] =
    Options.enumeration("burner")("real" -> BurnerKind.Real, "simulated" -> BurnerKind.Simulated).optional
  private val globalOpts: Options[GlobalOpts] =
    (catalogOpt ++ dataDirOpt ++ deviceOpt ++ burnerOpt).map { case (c, d, dev, b) =>
      GlobalOpts(c, d, dev, b)
    }
```

- [ ] **Step 6: Wire `Runner.scala`.** Add a console operator and backend-dependent layers:

```scala
/** Console implementation of the operator port: insert prompts + progress lines. */
private object ConsoleBurnOperator:
  val live: ZLayer[Any, Nothing, BurnOperator] = ZLayer.succeed(new BurnOperator:
    def confirmInsert(message: String): UIO[Boolean] = Prompt.confirm(message, assumeYes = false)
    def progress(line: String): UIO[Unit]            = Console.printLine(line).orDie)
```

`program` resolves the backend right after `env` and threads it down:

```scala
      backend  = BackendResolver.resolve(global, env)
      ...
      code     <- dispatch(cmd, resolved.catalogPath, backend)
```

`dispatch`, `withProject`, `withPipeline` each gain the `backend: Backend` parameter (pass it through). `withPipeline` becomes:

```scala
  private def withPipeline[A](catalogPath: Path, workDir: Path, backend: Backend)(
      use: Pipeline => IO[PipelineError, A]): IO[PipelineError, A] =
    val layer = backend match
      case Backend.Simulated =>
        ZLayer.make[Pipeline](
          ZLayer.succeed(ProjectPaths(catalogPath, workDir)),
          ZLayer.fromFunction((p: ProjectPaths) => DbPath(p.catalogPath)),
          Db.poolLayer,
          FileSystem.live, Hasher.live, FileScanner.live,
          SnapshotWriter.live, SimulatedBurner.live, SourceSnapshot.live,
          StagedStore.live, CatalogStore.live, StagingDiscVerifier.live,
          Pipeline.live)
      case Backend.Real(device) =>
        ZLayer.make[Pipeline](
          ZLayer.succeed(ProjectPaths(catalogPath, workDir)),
          ZLayer.fromFunction((p: ProjectPaths) => DbPath(p.catalogPath)),
          Db.poolLayer,
          FileSystem.live, Hasher.live, FileScanner.live,
          SnapshotWriter.live, SourceSnapshot.live,
          StagedStore.live, CatalogStore.live,
          ZLayer.succeed(device: BurnDevice),
          CmdRunner.live, MediaInspector.live, DiscReader.live,
          ConsoleBurnOperator.live,
          XorrisoBurner.live, RealDiscVerifier.live,
          Pipeline.live)
    ZIO.serviceWithZIO[Pipeline](use)
      .provideLayer(layer)
      .mapError {
        case pe: PipelineError => pe
        case t: Throwable      => PipelineError.Failed(t)
      }
```

Imports: `diskrement.shell.burn.{BurnDevice, BurnOperator, CmdRunner, DiscReader, MediaInspector, RealDiscVerifier, SimulatedBurner, SnapshotWriter, XorrisoBurner}` plus `StagingDiscVerifier` from the pipeline package.

- [ ] **Step 7: Extend `Render.handle`** — full new `handle`:

```scala
  def handle(e: PipelineError): (String, ExitCode) =
    val code = e match
      case _: PipelineError.ProjectNotFound        => ExitCode(2)
      case _: PipelineError.NoStagedScan           => ExitCode(2)
      case _: PipelineError.NoStagedPlan           => ExitCode(2)
      case _: PipelineError.SnapshotMissing        => ExitCode(2)
      case _: PipelineError.NotSameFilesystem      => ExitCode(3)
      case _: PipelineError.NotCowCapable          => ExitCode(3)
      case _: PipelineError.OversizedFiles         => ExitCode(3)
      case PipelineError.Burn(BurnError.Aborted)   => ExitCode(0)
      case _: PipelineError.Burn                   => ExitCode(6)
      case _: PipelineError.Failed                 => ExitCode(5)
    val message = e match
      case PipelineError.Burn(BurnError.Aborted) => "Aborted."
      case PipelineError.Failed(cause) =>
        val detail = Option(cause.getMessage).getOrElse(cause.getClass.getSimpleName)
        s"internal error: $detail (this is a bug)"
      case other => other.getMessage
    (message, code)
```

(import `diskrement.shell.burn.BurnError`.) Exit-code contract after this task: 0 success/aborted, 2 state, 3 constraint, 4 verify mismatch, 5 internal, **6 media/burner environment**.

- [ ] **Step 8: Fix the smoke test.** In `CliSmokeSpec.scala`, both `GlobalOpts(...)` constructions gain `burner = Some(BurnerKind.Simulated)` — without this the burn step would try xorriso against `/dev/sr0`. Add a test asserting the resolver is actually consulted:

The existing first test's `global` becomes:

```scala
            global   = GlobalOpts(catalog = Some(base.resolve("data/photos.sqlite")), dataDir = None,
                         burner = Some(BurnerKind.Simulated))
```

(second test likewise).

- [ ] **Step 9: Update `RenderSpec`** if it asserts on `handle` exhaustively or constructs `VerifyReport` (add the `skipped` render expectation from Task 9 if not already done).

- [ ] **Step 10: Run the cli suite**

Run: `sbt cli/test`
Expected: PASS.

- [ ] **Step 11: Commit**

```bash
git add -A && git commit -m "feat(cli): --device/--burner backend selection, real-burner wiring, exit code 6"
```

---

### Task 12: Integration-suite pinning, README, smoke-test checklist

**Files:**
- Modify: `cli/src/it/scala/diskrement/cli/it/Cli.scala`
- Modify: `README.md`
- Create: `docs/smoke-test-real-media.md`

- [ ] **Step 1: Pin the IT suite to the simulated backend.** In `Cli.scala`, next to the other `env.put` lines:

```scala
    env.put("DISKREMENT_BURNER", "simulated")
```

- [ ] **Step 2: Run the full integration suite**

Run: `sbt cli/IntegrationTest/test`
Expected: PASS (all 8 scenarios + harness/smoke specs, unchanged).

- [ ] **Step 3: Update `README.md`** — in the status table, change the row
`| Real disc burning (`xorriso` / `growisofs`) | Planned (Cycle 2) |` to
`| Real disc burning (xorriso, BD-R multisession) | Done (burn + read-back verify) |`
and add a row `| Coaster workflow (mark-dead + re-plan) | Planned (Plan 4b) |`.
Add a short "Real burning" subsection near the usage docs:

```markdown
### Real burning

The default backend burns real media via `xorriso` (ISO 9660 level 3 + Rock Ridge,
multisession). Select the device and backend with `--device /dev/sr0` /
`--burner real|simulated` (env: `DISKREMENT_DEVICE`, `DISKREMENT_BURNER`).
Your user must be able to open the drive — on most distros that means membership
in the `optical` group. `verify` mounts the disc via `udisks2` and re-hashes every
file, including versions superseded by later sessions on the same disc.
```

- [ ] **Step 4: Write `docs/smoke-test-real-media.md`**

```markdown
# Real-media smoke test (Plan 4a)

Run on the dev machine (Pioneer BD-RW BDR-UD04, /dev/sr0) with one expendable
BD-R (or BD-RE for repeatable drills) before declaring cycle 2 done.
Prereq: `sudo usermod -aG optical $USER` + re-login (else expect the
DevicePermissionDenied message — which is itself checklist item 0).

- [ ] 0. Without group membership: `diskrement burn ...` prints the
      optical-group hint and exits 6.
- [ ] 1. `init` a scratch project (small source dir, a few MB), `scan`, `plan`,
      `burn` to a blank BD-R. Confirm: insert prompt appears, xorriso UPDATE
      progress streams, disc ejects, catalog commits (`status` shows the disc).
- [ ] 2. `verify` — confirm the disc mounts via udisks, all files OK, session
      marked VERIFIED.
- [ ] 3. Add/modify a file, `scan`/`plan`/`burn` again to the SAME disc
      (top-off). Confirm the identity check accepted it and `-toc` (or
      `xorriso -indev /dev/sr0 -toc`) shows 2 sessions.
- [ ] 4. Insert a WRONG disc (any other disc) for a top-off: confirm refusal +
      re-prompt, then decline: "Aborted.", exit 0, catalog unchanged.
- [ ] 5. `verify` again: with a version superseded in-disc (the modified file),
      confirm Tier 2 ran (superseded version extracted and OK).
- [ ] 6. `plan --finalize` + `burn`: confirm the disc closes. Record the exact
      `Media status` line of the closed disc here: ________________ and update
      `MediaParseSpec`'s closedFixture if it differs from `is written , is closed`.
- [ ] 7. Attempt a top-off of the finalized disc: confirm DiscClosed refusal.
- [ ] 8. Empty tray: run `burn` with no disc; record the exact failure text:
      ________________ and update `MediaParse.classifyFailure` candidates if it
      is not matched.
- [ ] 9. `verify` with the disc in another machine's drive or after remount —
      optional bit-rot drill.

Findings feed parser fixes; re-run `sbt shell/test` after any parser change.
```

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "docs+it: pin IT suite to simulated backend; README real-burning section; smoke-test checklist"
```

---

### Task 13: Full verification + wrap-up

- [ ] **Step 1: Full clean build and every suite**

Run: `sbt clean core/test shell/test cli/test cli/IntegrationTest/test`
Expected: all PASS. (`-Wunused:all` — the build fails on any leftover import.)

- [ ] **Step 2: Manual CLI sanity (simulated)**

```bash
sbt cli/stage
d=$(mktemp -d); mkdir -p $d/src $d/work $d/data; echo hi > $d/src/f.txt
cli/target/universal/stage/bin/diskrement --catalog $d/data/t.sqlite --burner simulated init --source $d/src --work-dir $d/work t
cli/target/universal/stage/bin/diskrement --catalog $d/data/t.sqlite --burner simulated scan t
cli/target/universal/stage/bin/diskrement --catalog $d/data/t.sqlite --burner simulated plan t
cli/target/universal/stage/bin/diskrement --catalog $d/data/t.sqlite --burner simulated burn --yes t
cli/target/universal/stage/bin/diskrement --catalog $d/data/t.sqlite --burner simulated verify t
```

Expected: the familiar cycle-1 output, `verify` reports `1 files OK ✓`.

- [ ] **Step 3: Run the real-media smoke test** (`docs/smoke-test-real-media.md`) if the operator (user) is available and the drive is accessible; otherwise flag it as the remaining manual step in the completion report.

- [ ] **Step 4: Finish the branch** — use superpowers:finishing-a-development-branch (merge into `main`, push, delete branch), per the project convention.

---

## Self-review notes (already applied)

- `finalize` as a field name collides with `AnyRef.finalize` — the field is `closeDisc` everywhere.
- The spec's "`DiscImage` gains `device`" is realized as the device being a constructor input of `XorrisoBurnerLive` (layer-provided `BurnDevice`) instead — same information, better boundary: the simulated burner never sees a device, and `DiscImage` stays a pure description of content. `closeDisc` did land on `DiscImage` as specced.
- Spec §5's `MediaStatus` shape is kept with `freeBytes`/`sessions` added to the cases that have them; `MediaError` is folded into `BurnError` (one error channel through burner + inspector + reader; fewer wrappers).
- Verify semantics: `latestSession` (staging) vs `visible` (merged disc) are deliberately different reference maps — see `DiscExpectation`'s doc comment.
- Exit code 0 for operator-aborted burns matches the existing "Aborted." behavior of the staged-plan confirmation decline.
```

