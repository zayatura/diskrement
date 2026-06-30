package diskrement.cli.it
package scenarios

import zio.ZIO
import zio.test.*

/** Disc 1 is left OPEN after cycle 1; cycle 2's changes (add/modify/delete) are
  * written as session 2 to the SAME disc. Staging holds only session 2's files;
  * the cumulative current set is checked via status/history. */
object Scenario02AppendOpen:
  val suite = zio.test.suite("scenario 2: append to open disc")(
    test("second session lands on disc 1 (ordinal 1, session 2)"):
      ZIO.scoped {
        for
          s   <- Support.scratch
          cli  = new Cli(s, discBytes = None)
          _   <- ZIO.attempt(Tree.write(s.source, "a.txt", 128))
          _   <- ZIO.attempt(Tree.write(s.source, "b.txt", 128))
          ini <- Support.run(cli, "init", "p",
                   "--source", s.source.toString, "--work-dir", s.work.toString)
          // cycle 1
          _   <- Support.run(cli, "scan", "p")
          _   <- Support.run(cli, "plan", "p")
          b1  <- Support.run(cli, "burn", "p", "--yes")
          d1man1  <- ZIO.attemptBlocking(DiscOracle.manifest(s.discDir(1)))
          d1data1 <- ZIO.attemptBlocking(DiscOracle.dataFiles(s.discDir(1)))
          // cycle 2: modify a, add c, delete b
          _   <- ZIO.attempt(Tree.modify(s.source, "a.txt", 128))
          _   <- ZIO.attempt(Tree.write(s.source, "c.txt", 64))
          _   <- ZIO.attempt(Tree.delete(s.source, "b.txt"))
          sc2 <- Support.run(cli, "scan", "p")
          pl2 <- Support.run(cli, "plan", "p")
          b2  <- Support.run(cli, "burn", "p", "--yes")
          d1man2  <- ZIO.attemptBlocking(DiscOracle.manifest(s.discDir(1)))
          d1data2 <- ZIO.attemptBlocking(DiscOracle.dataFiles(s.discDir(1)))
          hA  <- Support.run(cli, "history", "p", "a.txt")
          hB  <- Support.run(cli, "history", "p", "b.txt")
          st  <- Support.run(cli, "status", "p")
        yield assertTrue(
          ini.exitCode == 0,
          // cycle 1: disc 1 open, session 1, both files
          b1.exitCode == 0,
          b1.stdout.contains("Wrote disc 1") && b1.stdout.contains("left OPEN"),
          d1man1.discOrdinal == 1,
          d1man1.sessionOrdinal == 1,
          d1data1 == Set("a.txt", "b.txt"),
          // cycle 2 scan/plan
          sc2.exitCode == 0,
          sc2.stdout.contains("Scanned: 1 added, 1 modified, 1 deleted, 0 unchanged."),
          pl2.exitCode == 0,
          pl2.stdout.contains("appending to open disc 1"),
          // cycle 2 staged disc 1 now holds only session 2's files
          b2.exitCode == 0,
          b2.stdout.contains("Wrote disc 1") && b2.stdout.contains("left OPEN"),
          d1man2.discOrdinal == 1,
          d1man2.sessionOrdinal == 2,
          d1data2 == Set("a.txt", "c.txt"),
          d1man2.added.map(_.relPath).toSet == Set("c.txt"),
          d1man2.modified.map(_.relPath).toSet == Set("a.txt"),
          d1man2.deleted.toSet == Set("b.txt"),
          // cumulative view via history + status
          hA.exitCode == 0,
          hA.stdout.contains("SUPERSEDED") && hA.stdout.contains("CURRENT"),
          hB.exitCode == 0,
          hB.stdout.contains("DELETED"),
          st.exitCode == 0,
          st.stdout.contains("Current files: 2"))
      }
  )
