package diskrement.cli.it
package scenarios

import zio.ZIO
import zio.test.*
import java.nio.file.Files

/** Proves the warm fingerprint cache end-to-end. `PipelineLive.freshScan`
  * fast-paths a file's hash by merging two sources: `prior` (fingerprints read
  * back from the catalog, populated once a session is burned) and `cached`
  * (the sidecar `FingerprintCache`, written after every scan regardless of
  * burn state). A rescan taken AFTER a burn is explained by either source, so
  * it alone cannot show the sidecar is doing anything. This suite therefore
  * pins three rescans: one BEFORE any burn (where the catalog is provably
  * empty, so a fast-path hit can only come from the sidecar), one after a
  * burn (where the catalog also has fingerprints), and one after touching a
  * file (which must miss on size regardless of source). It also pins that
  * `--force` bypasses staged-scan reuse rather than the fingerprint cache. */
object Scenario10WarmRescan:
  val suite = zio.test.suite("scenario 10: warm-cache rescan hashes only changes")(
    test("scan hashes all; pre- and post-burn rescans reuse every fingerprint; a touched file hashes one"):
      ZIO.scoped {
        for
          s      <- Support.scratch
          cli     = new Cli(s, discBytes = None)
          _      <- ZIO.attempt(Tree.write(s.source, "a.txt", 100))
          _      <- ZIO.attempt(Tree.write(s.source, "b.txt", 200))
          _      <- ZIO.attempt(Tree.write(s.source, "c.txt", 300))
          _      <- Support.run(cli, "init", "p",
                      "--source", s.source.toString, "--work-dir", s.work.toString)
          scan1  <- Support.run(cli, "scan", "p")
          // Discriminating step: catalog.fingerprints is still empty here (nothing
          // has been burned), so a fast-path hit can only be explained by the
          // sidecar FingerprintCache. --force is required so this rescan actually
          // re-fingerprints instead of reusing the staged scan from scan1 (which
          // would omit the "Hashed" line entirely and prove nothing). Do not
          // "simplify" this away or move it after the burn — that is exactly the
          // change that made the old version of this scenario unable to tell the
          // sidecar cache and the catalog fast path apart.
          scan1b <- Support.run(cli, "scan", "p", "--force")
          _      <- Support.run(cli, "plan", "p")
          _      <- Support.run(cli, "burn", "p", "--yes")
          scan2  <- Support.run(cli, "scan", "p")
          _      <- ZIO.attempt(Tree.modify(s.source, "b.txt", 999))   // changes size -> fingerprint miss
          scan3  <- Support.run(cli, "scan", "p", "--force")
          // The sidecar file is created (via openConn's create-on-demand) by any
          // cache call, including a plain readAll, so mere existence would not
          // show anything was ever persisted. This only pins the naming
          // convention the CLI resolves the sidecar path with.
          cachePathUsed <- ZIO.attempt(
                             Files.exists(s.catalog.resolveSibling("cat.p.scancache.sqlite")))
        yield assertTrue(
          scan1.exitCode == 0,
          scan1.stdout.contains("Hashed 3 of 3 files (0 fingerprints reused)."),
          scan1b.exitCode == 0,
          scan1b.stdout.contains("Hashed 0 of 3 files (3 fingerprints reused)."),
          scan2.exitCode == 0,
          scan2.stdout.contains("Scanned: 0 added, 0 modified, 0 deleted, 3 unchanged."),
          scan2.stdout.contains("Hashed 0 of 3 files (3 fingerprints reused)."),
          scan3.exitCode == 0,
          scan3.stdout.contains("Scanned: 0 added, 1 modified, 0 deleted, 2 unchanged."),
          scan3.stdout.contains("Hashed 1 of 3 files (2 fingerprints reused)."),
          cachePathUsed)
      }
  )
