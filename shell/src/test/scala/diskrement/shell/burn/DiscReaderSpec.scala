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
        // 2-session image: session 1: a.txt "v1"; session 2: a.txt "v2".
        def burnSession(n: Int, content: String) =
          for
            ans <- Ref.make(List(true))
            log <- Ref.make(List.empty[String])
            b    = XorrisoBurnerLive(dev, run, XorrisoMediaInspectorLive(run),
                     ScriptedOperator(ans, log), HasherLive(), SnapshotWriterLive(),
                     diskrement.shell.ProgressSink.Noop, 2)
            snap = base.resolve(s"snap$n")
            _    = Files.createDirectories(snap)
            _    = Files.writeString(snap.resolve("a.txt"), content)
            cat  = base.resolve(s"cat$n")
            _    = Files.writeString(cat, "x")
            hash = ContentHash(java.security.MessageDigest.getInstance("SHA-256")
                     .digest(content.getBytes("UTF-8")).map(x => f"${x & 0xff}%02x").mkString)
            img  = DiscImage(base.resolve(s"stage$n"), snap,
                     List(ScannedFile(RelPath("a.txt"), hash, content.length.toLong,
                       Instant.parse("2026-08-30T10:00:00Z"))),
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
