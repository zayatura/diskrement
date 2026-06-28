package diskrement.cli.it
package scenarios

import zio.ZIO
import zio.test.*

object Scenario01Happy:
  val suite = zio.test.suite("scenario 1: single-disc happy path")(
    test("init -> scan -> plan -> burn -> verify; disc contents == source"):
      ZIO.scoped {
        for
          s   <- Support.scratch
          cli  = new Cli(s, discBytes = None)
          _    = Tree.write(s.source, "docs/a.txt", 256)
          _    = Tree.write(s.source, "img/b.bin", 512)
          _   <- Support.run(cli, "init", "p",
                   "--source", s.source.toString, "--work-dir", s.work.toString)
          scan <- Support.run(cli, "scan", "p")
          plan <- Support.run(cli, "plan", "p")
          burn <- Support.run(cli, "burn", "p", "--yes")
          // Oracle: read the freshly-staged disc 1 right now.
          d1   = s.discDir(1)
          data = DiscOracle.dataFiles(d1)
          man  = DiscOracle.manifest(d1)
          proj = DiscOracle.projectJson(d1)
          pts  = DiscOracle.pointers(d1)
          aOk  = DiscOracle.contentMatches(d1, s.source, "docs/a.txt")
          bOk  = DiscOracle.contentMatches(d1, s.source, "img/b.bin")
          tabs = DiscOracle.catalogTableCount(d1)
          vrfy <- Support.run(cli, "verify", "p")
          stat <- Support.run(cli, "status", "p")
        yield assertTrue(
          scan.exitCode == 0,
          scan.stdout.contains("Scanned: 2 added, 0 modified, 0 deleted, 0 unchanged."),
          plan.exitCode == 0,
          plan.stdout.contains("Plan: disc 1") && plan.stdout.contains("new disc"),
          burn.exitCode == 0,
          burn.stdout.contains("Wrote disc 1"),
          data == Set("docs/a.txt", "img/b.bin"),
          DiscOracle.manifestFiles(man) == data,
          man.discOrdinal == 1,
          man.sessionOrdinal == 1,
          man.added.map(_.relPath).toSet == Set("docs/a.txt", "img/b.bin"),
          man.deleted.isEmpty,
          proj.projectName == "p",
          // pointers.json holds only superseded/deleted versions; empty on a first burn
          pts.pointers.isEmpty,
          aOk, bOk,
          tabs > 0,
          vrfy.exitCode == 0,
          vrfy.stdout.contains("files OK"),
          stat.stdout.contains("Current files: 2"))
      }
  )
