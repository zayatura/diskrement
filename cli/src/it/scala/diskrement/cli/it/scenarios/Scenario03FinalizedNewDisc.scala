package diskrement.cli.it
package scenarios

import zio.ZIO
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
          _   <- ZIO.attempt(Tree.write(s.source, "a.txt", 128))
          ini <- Support.run(cli, "init", "p",
                   "--source", s.source.toString, "--work-dir", s.work.toString)
          // cycle 1: finalize disc 1
          _   <- Support.run(cli, "scan", "p")
          pl1 <- Support.run(cli, "plan", "p", "--finalize")
          b1  <- Support.run(cli, "burn", "p", "--yes")
          d1data <- ZIO.attemptBlocking(DiscOracle.dataFiles(s.discDir(1)))
          d1man  <- ZIO.attemptBlocking(DiscOracle.manifest(s.discDir(1)))
          // cycle 2: add b -> must be a new disc
          _   <- ZIO.attempt(Tree.write(s.source, "b.txt", 128))
          _   <- Support.run(cli, "scan", "p")
          pl2 <- Support.run(cli, "plan", "p")
          b2  <- Support.run(cli, "burn", "p", "--yes")
          d2data <- ZIO.attemptBlocking(DiscOracle.dataFiles(s.discDir(2)))
          d2man  <- ZIO.attemptBlocking(DiscOracle.manifest(s.discDir(2)))
          hA  <- Support.run(cli, "history", "p", "a.txt")
        yield assertTrue(
          ini.exitCode == 0,
          pl1.exitCode == 0,
          pl1.stdout.contains("will FINALIZE"),
          b1.exitCode == 0,
          b1.stdout.contains("Wrote disc 1") && b1.stdout.contains("FINALIZED"),
          d1data == Set("a.txt"),
          d1man.discOrdinal == 1,
          pl2.exitCode == 0,
          pl2.stdout.contains("new disc"),
          b2.exitCode == 0,
          b2.stdout.contains("Wrote disc 2"),
          d2data == Set("b.txt"),
          d2man.discOrdinal == 2,
          d2man.sessionOrdinal == 1,
          hA.exitCode == 0,
          hA.stdout.contains("CURRENT"))
      }
  )
