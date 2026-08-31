package diskrement.cli.it
package scenarios

import zio.ZIO
import zio.test.*
import java.nio.file.Files

/** Proves the warm fingerprint cache end-to-end: a first scan hashes every
  * file, a rescan after burn reuses every fingerprint (hashing nothing), and
  * a rescan after touching one file hashes only that file. */
object Scenario10WarmRescan:
  val suite = zio.test.suite("scenario 10: warm-cache rescan hashes only changes")(
    test("scan hashes all; post-burn rescan hashes none; a touched file hashes one"):
      ZIO.scoped {
        for
          s     <- Support.scratch
          cli    = new Cli(s, discBytes = None)
          _     <- ZIO.attempt(Tree.write(s.source, "a.txt", 100))
          _     <- ZIO.attempt(Tree.write(s.source, "b.txt", 200))
          _     <- ZIO.attempt(Tree.write(s.source, "c.txt", 300))
          _     <- Support.run(cli, "init", "p",
                     "--source", s.source.toString, "--work-dir", s.work.toString)
          scan1 <- Support.run(cli, "scan", "p")
          _     <- Support.run(cli, "plan", "p")
          _     <- Support.run(cli, "burn", "p", "--yes")
          scan2 <- Support.run(cli, "scan", "p")
          _     <- ZIO.attempt(Tree.write(s.source, "b.txt", 999))   // changes size -> fingerprint miss
          scan3 <- Support.run(cli, "scan", "p", "--force")
          cacheExists <- ZIO.attempt(
                           Files.exists(s.catalog.resolveSibling("cat.p.scancache.sqlite")))
        yield assertTrue(
          scan1.exitCode == 0,
          scan1.stdout.contains("Hashed 3 of 3 files (0 fingerprints reused)."),
          scan2.exitCode == 0,
          scan2.stdout.contains("Scanned: 0 added, 0 modified, 0 deleted, 3 unchanged."),
          scan2.stdout.contains("Hashed 0 of 3 files (3 fingerprints reused)."),
          scan3.exitCode == 0,
          scan3.stdout.contains("Scanned: 0 added, 1 modified, 0 deleted, 2 unchanged."),
          scan3.stdout.contains("Hashed 1 of 3 files (2 fingerprints reused)."),
          cacheExists)
      }
  )
