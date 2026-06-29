package diskrement.cli.it
package scenarios

import zio.ZIO
import zio.test.*

/** After a full burn, re-scanning an unchanged source reports all-unchanged and
  * plans nothing to burn. */
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
          ini <- Support.run(cli, "init", "p",
                   "--source", s.source.toString, "--work-dir", s.work.toString)
          _   <- Support.run(cli, "scan", "p")
          _   <- Support.run(cli, "plan", "p")
          _   <- Support.run(cli, "burn", "p", "--yes")
          // source unchanged; staged state was cleared by burn, so this re-scans
          sc2 <- Support.run(cli, "scan", "p")
          pl2 <- Support.run(cli, "plan", "p")
        yield assertTrue(
          ini.exitCode == 0,
          sc2.exitCode == 0,
          sc2.stdout.contains("Scanned: 0 added, 0 modified, 0 deleted, 2 unchanged."),
          pl2.exitCode == 0,
          pl2.stdout.contains("Nothing to burn: no changes to plan."))
      }
  )
