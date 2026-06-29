package diskrement.cli.it
package scenarios

import zio.ZIO
import zio.test.*

/** The CLI must propagate non-zero exit codes for application errors
  * (Render.handle / verifyExit), not collapse everything to 0. */
object ScenarioExitCodes:
  val suite = zio.test.suite("scenario: CLI exit codes propagate")(
    test("verify of a nonexistent project exits 2; plan with no staged scan exits 2"):
      ZIO.scoped {
        for
          s   <- Support.scratch
          cli  = new Cli(s, discBytes = None)
          _    = Tree.write(s.source, "a.txt", 64)
          _   <- Support.run(cli, "init", "p",
                   "--source", s.source.toString, "--work-dir", s.work.toString)
          vNo <- Support.run(cli, "verify", "does-not-exist")
          pNo <- Support.run(cli, "plan", "p")
        yield assertTrue(
          vNo.exitCode == 2,
          pNo.exitCode == 2)
      }
  )
