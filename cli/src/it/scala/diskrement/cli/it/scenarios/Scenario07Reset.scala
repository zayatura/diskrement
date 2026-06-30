package diskrement.cli.it
package scenarios

import zio.ZIO
import zio.test.*

/** `reset` discards the staged scan; a following `plan` then errors with exit 2
  * (NoStagedScan). Exit-code propagation (Task 7b) makes this assertion real. */
object Scenario07Reset:
  val suite = zio.test.suite("scenario 7: reset")(
    test("reset clears the staged scan; a following plan errors"):
      ZIO.scoped {
        for
          s   <- Support.scratch
          cli  = new Cli(s, discBytes = None)
          _   <- ZIO.attempt(Tree.write(s.source, "a.txt", 64))
          ini <- Support.run(cli, "init", "p",
                   "--source", s.source.toString, "--work-dir", s.work.toString)
          sc  <- Support.run(cli, "scan", "p")
          rs  <- Support.run(cli, "reset", "p")
          pl  <- Support.run(cli, "plan", "p")
        yield assertTrue(
          ini.exitCode == 0,
          sc.exitCode == 0,
          rs.exitCode == 0,
          rs.stdout.contains("Reset p."),
          pl.exitCode == 2,
          pl.stderr.nonEmpty)
      }
  )
