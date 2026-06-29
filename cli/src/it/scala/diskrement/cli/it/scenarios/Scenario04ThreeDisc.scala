package diskrement.cli.it
package scenarios

import zio.ZIO
import zio.test.*

/** Three cycles, each producing one disc under a 1000-byte capacity override.
  * Cycle 3 modifies a and deletes b, landing the modified a on disc 3. */
object Scenario04ThreeDisc:
  val suite = zio.test.suite("scenario 4: three-disc incremental")(
    test("a -> b -> (modify a, delete b) span discs 1,2,3 with correct history"):
      ZIO.scoped {
        for
          s   <- Support.scratch
          cli  = new Cli(s, discBytes = Some(1000L))
          _   <- ZIO.attempt(Tree.write(s.source, "a.bin", 800))
          _   <- Support.run(cli, "init", "p",
                   "--source", s.source.toString, "--work-dir", s.work.toString)
          // cycle 1 -> disc 1 [a]
          _   <- Support.run(cli, "scan", "p")
          _   <- Support.run(cli, "plan", "p")
          _   <- Support.run(cli, "burn", "p", "--yes")
          // cycle 2 -> disc 2 [b]
          _    = Tree.write(s.source, "b.bin", 800)
          _   <- Support.run(cli, "scan", "p")
          pl2 <- Support.run(cli, "plan", "p")
          _   <- Support.run(cli, "burn", "p", "--yes")
          // cycle 3 -> disc 3 [a'] (+ delete b)
          _    = Tree.modify(s.source, "a.bin", 800)
          _    = Tree.delete(s.source, "b.bin")
          sc3 <- Support.run(cli, "scan", "p")
          pl3 <- Support.run(cli, "plan", "p")
          _   <- Support.run(cli, "burn", "p", "--yes")
          d1   = DiscOracle.dataFiles(s.discDir(1))
          d2   = DiscOracle.dataFiles(s.discDir(2))
          d3   = DiscOracle.dataFiles(s.discDir(3))
          m3   = DiscOracle.manifest(s.discDir(3))
          hA  <- Support.run(cli, "history", "p", "a.bin")
          hB  <- Support.run(cli, "history", "p", "b.bin")
          st  <- Support.run(cli, "status", "p")
          vf  <- Support.run(cli, "verify", "p")
        yield assertTrue(
          pl2.stdout.contains("new disc"),
          sc3.stdout.contains("Scanned: 0 added, 1 modified, 1 deleted, 0 unchanged."),
          pl3.stdout.contains("new disc"),
          d1 == Set("a.bin"),
          d2 == Set("b.bin"),
          d3 == Set("a.bin"),
          m3.discOrdinal == 3,
          m3.modified.map(_.relPath).toSet == Set("a.bin"),
          m3.deleted.toSet == Set("b.bin"),
          hA.stdout.contains("SUPERSEDED") && hA.stdout.contains("CURRENT"),
          hB.stdout.contains("DELETED"),
          st.stdout.contains("Current files: 1"),
          vf.exitCode == 0,
          vf.stdout.contains("files OK"),
          !vf.stdout.contains("MISMATCH"))
      }
  )
