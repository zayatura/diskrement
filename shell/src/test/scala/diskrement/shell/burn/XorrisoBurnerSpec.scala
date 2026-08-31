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
    val f = ScannedFile(RelPath("a.txt"), hash, 5L, Instant.parse("2026-08-30T10:00:00Z"))
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
               diskrement.shell.fs.HasherLive(), SnapshotWriterLive(),
               diskrement.shell.ProgressSink.Noop, 2)
    yield (b, log)

  private val versionOk: List[String] => UIO[CmdResult] = cmd =>
    if cmd.contains("--version") then ZIO.succeed(CmdResult(0, List("xorriso 1.5.8.pl02")))
    else ZIO.succeed(okBurn)

  def spec = suite("XorrisoBurnerSpec")(
    test("session 1 on blank media burns and post-checks") {
      ZIO.attemptBlocking(Files.createTempDirectory("xb")).orDie.flatMap { base =>
        for
          made   <- burner(List(MediaStatus.Blank(1_000_000_000L)), List(true),
                      identityFor(1), versionOk)
          (b, _) = made
          res    <- b.burn(mkImage(base, 1, closeDisc = false))
        yield assertTrue(res.imageBytes > 0L)
      }
    },
    test("session 1 refuses written media, re-prompts, succeeds on swap") {
      ZIO.attemptBlocking(Files.createTempDirectory("xb")).orDie.flatMap { base =>
        for
          made   <- burner(
                        List(MediaStatus.Appendable(1_000_000_000L, 1, identityFor(1)),
                             MediaStatus.Blank(1_000_000_000L)),
                        List(true, true), identityFor(1), versionOk)
          (b, log) = made
          res      <- b.burn(mkImage(base, 1, closeDisc = false))
          lines    <- log.get
        yield assertTrue(res.imageBytes > 0L, lines.exists(_.contains("Not the expected disc")))
      }
    },
    test("top-off requires matching identity at previous session ordinal") {
      ZIO.attemptBlocking(Files.createTempDirectory("xb")).orDie.flatMap { base =>
        for // disc says session 1; we burn session 2 — match.
          made   <- burner(List(MediaStatus.Appendable(1_000_000_000L, 1, identityFor(1))),
                      List(true), identityFor(2), versionOk)
          (b, _) = made
          res    <- b.burn(mkImage(base, 2, closeDisc = true))
        yield assertTrue(res.imageBytes > 0L)
      }
    },
    test("operator abort fails with Aborted") {
      ZIO.attemptBlocking(Files.createTempDirectory("xb")).orDie.flatMap { base =>
        for
          made   <- burner(List(MediaStatus.Blank(1_000_000_000L)), List(false),
                      identityFor(1), versionOk)
          (b, _) = made
          exit   <- b.burn(mkImage(base, 1, closeDisc = false)).exit
        yield assertTrue(exit == Exit.fail(BurnError.Aborted))
      }
    },
    test("insufficient capacity refuses before burning") {
      ZIO.attemptBlocking(Files.createTempDirectory("xb")).orDie.flatMap { base =>
        for // 1000 bytes free < 32 MiB fixed overhead
          made   <- burner(List(MediaStatus.Blank(1000L)), List(true), identityFor(1), versionOk)
          (b, _) = made
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
          made   <- burner(List(MediaStatus.Blank(1_000_000_000L)), List(true),
                      identityFor(1), failingRunner)
          (b, _) = made
          exit   <- b.burn(mkImage(base, 1, closeDisc = false)).exit
        yield assertTrue(exit.causeOption.flatMap(_.failureOption)
          .exists(_.isInstanceOf[BurnError.BurnFailed]))
      }
    },
    test("post-burn identity mismatch fails PostBurnCheckFailed") {
      ZIO.attemptBlocking(Files.createTempDirectory("xb")).orDie.flatMap { base =>
        for
          made   <- burner(List(MediaStatus.Blank(1_000_000_000L)), List(true),
                      identityFor(7), versionOk)
          (b, _) = made
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
          made   <- burner(List(MediaStatus.Blank(1_000_000_000L)), List(true), identityFor(1), oldRunner)
          (b, _) = made
          exit   <- b.burn(mkImage(base, 1, closeDisc = false)).exit
        yield assertTrue(exit.causeOption.flatMap(_.failureOption)
          .exists(_.isInstanceOf[BurnError.ToolTooOld]))
      }
    }
  )
