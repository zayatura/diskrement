package diskrement.cli.it
package scenarios

import zio.ZIO
import zio.test.*
import java.nio.file.Files

/** A single file larger than disc capacity: plan reports it oversized (exit 0),
  * burn refuses (exit 3), and no disc is staged. */
object Scenario06Oversized:
  val suite = zio.test.suite("scenario 6: oversized file")(
    test("a file larger than a disc is reported and burn refuses"):
      ZIO.scoped {
        for
          s   <- Support.scratch
          cli  = new Cli(s, discBytes = Some(1000L))
          _   <- ZIO.attempt(Tree.write(s.source, "huge.bin", 5000))
          ini <- Support.run(cli, "init", "p",
                   "--source", s.source.toString, "--work-dir", s.work.toString)
          _   <- Support.run(cli, "scan", "p")
          pl  <- Support.run(cli, "plan", "p")
          bn  <- Support.run(cli, "burn", "p", "--yes")
          staged <- ZIO.attempt(Files.exists(s.discDir(1)))
        yield assertTrue(
          ini.exitCode == 0,
          pl.exitCode == 0,
          pl.stdout.contains("Oversized (cannot be placed):"),
          pl.stdout.contains("huge.bin"),
          bn.exitCode == 3,
          bn.stderr.contains("oversized files cannot be placed"),
          !staged)
      }
  )
