package diskrement.cli.it
package scenarios

import zio.ZIO
import zio.test.*

/** Three 400-byte files under a 1000-byte cap: two fit disc 1, the third spills
  * to disc 2 in the next cycle. Asserts the discs PARTITION the file set without
  * assuming the planner's packing order. */
object Scenario05Spill:
  val suite = zio.test.suite("scenario 5: spill across cycles")(
    test("three files over two discs: disjoint union == all files"):
      ZIO.scoped {
        for
          s   <- Support.scratch
          cli  = new Cli(s, discBytes = Some(1000L))
          _   <- ZIO.attempt {
                   Tree.write(s.source, "f1.bin", 400)
                   Tree.write(s.source, "f2.bin", 400)
                   Tree.write(s.source, "f3.bin", 400)
                 }
          ini <- Support.run(cli, "init", "p",
                   "--source", s.source.toString, "--work-dir", s.work.toString)
          // cycle 1: disc 1 fills with two of the three files
          sc1 <- Support.run(cli, "scan", "p")
          pl1 <- Support.run(cli, "plan", "p")
          _   <- Support.run(cli, "burn", "p", "--yes")
          d1  <- ZIO.attemptBlocking(DiscOracle.dataFiles(s.discDir(1)))
          // cycle 2: the remaining file goes to disc 2
          sc2 <- Support.run(cli, "scan", "p")
          pl2 <- Support.run(cli, "plan", "p")
          _   <- Support.run(cli, "burn", "p", "--yes")
          d2  <- ZIO.attemptBlocking(DiscOracle.dataFiles(s.discDir(2)))
          st  <- Support.run(cli, "status", "p")
          vf  <- Support.run(cli, "verify", "p")
          all  = Set("f1.bin", "f2.bin", "f3.bin")
        yield assertTrue(
          ini.exitCode == 0,
          sc1.exitCode == 0,
          sc1.stdout.contains("Scanned: 3 added"),
          pl1.exitCode == 0,
          pl1.stdout.contains("more discs) remain"),
          d1.size == 2,
          sc2.exitCode == 0,
          pl2.exitCode == 0,
          pl2.stdout.contains("This disc completes the set."),
          d2.size == 1,
          d1.intersect(d2).isEmpty,
          (d1 union d2) == all,
          st.exitCode == 0,
          st.stdout.contains("Current files: 3"),
          vf.exitCode == 0)
      }
  )
