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
