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

  /** A scoped temp tree: a two-file source, plus empty work/ and data/ dirs. */
  private def projectTree(prefix: String): ZIO[Scope, Nothing, Path] =
    ZIO.acquireRelease(
      ZIO.attempt {
        val b = Files.createTempDirectory(prefix)
        val src = b.resolve("src"); Files.createDirectories(src)
        Files.writeString(src.resolve("a.txt"), "hello")
        Files.writeString(src.resolve("b.txt"), "world")
        Files.createDirectories(b.resolve("work"))
        Files.createDirectories(b.resolve("data"))
        b
      })(b => ZIO.attempt(deleteRecursive(b)).orDie).orDie

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
            base    <- projectTree("cli-smoke")
            src      = base.resolve("src").toString
            work     = base.resolve("work").toString
            global   = GlobalOpts(catalog = Some(base.resolve("data/photos.sqlite")), dataDir = None,
                         burner = Some(BurnerKind.Simulated))

            c1      <- Runner.run(global, DiskrementCommand.Init("photos", java.nio.file.Paths.get(src),
                         java.nio.file.Paths.get(work), diskrement.core.DiscType.BD_R, 2.0, 1))
            c2      <- Runner.run(global, DiskrementCommand.Scan("photos", verify = false, force = false))
            c3      <- Runner.run(global, DiskrementCommand.Plan("photos", None, willFinalize = false))
            _       <- TestConsole.feedLines("y")
            c4      <- Runner.run(global, DiskrementCommand.Burn("photos", assumeYes = false))
            c5      <- Runner.run(global, DiskrementCommand.Status("photos"))
            out     <- TestConsole.output
            text     = out.mkString
          yield assertTrue(
            c1 == ExitCode.success, c2 == ExitCode.success, c3 == ExitCode.success,
            c4 == ExitCode.success, c5 == ExitCode.success,
            text.contains("Created project photos"),
            text.contains("Scanned: 2 added"),
            text.contains("Wrote disc 1"),
            text.contains("Project photos"))
        }
      }
    },
    test("a declined mark-dead prints Aborted., exits 0, and leaves the disc alive") {
      cowAvailable.flatMap { cow =>
        if !cow then ZIO.succeed(assertTrue(true)) // auto-skip: no CoW filesystem
        else ZIO.scoped {
          for
            base    <- projectTree("cli-decline")
            src      = base.resolve("src").toString
            work     = base.resolve("work").toString
            global   = GlobalOpts(catalog = Some(base.resolve("data/photos.sqlite")), dataDir = None,
                         burner = Some(BurnerKind.Simulated))

            _       <- Runner.run(global, DiskrementCommand.Init("photos", java.nio.file.Paths.get(src),
                         java.nio.file.Paths.get(work), diskrement.core.DiscType.BD_R, 2.0, 1))
            _       <- Runner.run(global, DiskrementCommand.Scan("photos", verify = false, force = false))
            _       <- Runner.run(global, DiskrementCommand.Plan("photos", None, willFinalize = false))
            _       <- TestConsole.feedLines("y")
            _       <- Runner.run(global, DiskrementCommand.Burn("photos", assumeYes = false))

            // Answering "n" must abort before the pipeline touches the catalog.
            _       <- TestConsole.feedLines("n")
            declined <- Runner.run(global, DiskrementCommand.MarkDead("photos", 1, assumeYes = false))
            after   <- Runner.run(global, DiskrementCommand.Status("photos"))
            out     <- TestConsole.output
            text     = out.mkString
          yield assertTrue(
            declined == ExitCode.success,
            after == ExitCode.success,
            text.contains("Aborted."),
            // The catalog is untouched: no mark-dead report was printed (the prompt text
            // itself says DEAD, so pin the report header), and disc 1 is still the live
            // open disc afterwards.
            !text.contains("marked DEAD"),
            text.contains("Wrote disc 1"),
            text.contains("#1"),
            text.contains("<- open"))
        }
      }
    },
    test("status on a missing project exits 2") {
      ZIO.scoped {
        for
          base  <- ZIO.acquireRelease(
                     ZIO.attempt(Files.createTempDirectory("cli-missing")))(p => ZIO.attempt(deleteRecursive(p)).orDie).orDie
          global = GlobalOpts(catalog = Some(base.resolve("nope.sqlite")), dataDir = None,
                     burner = Some(BurnerKind.Simulated))
          code  <- Runner.run(global, DiskrementCommand.Status("ghost"))
        yield assertTrue(code == ExitCode(2))
      }
    }
    // Runner opens/closes a real zio-jdbc ZConnectionPool per call; the pool's
    // lifecycle uses wall-clock time. Under the default TestClock those timers
    // never advance, which hangs the suite. Run on the live clock.
  ) @@ TestAspect.withLiveClock
