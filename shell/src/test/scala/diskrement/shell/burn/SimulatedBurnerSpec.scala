package diskrement.shell.burn

import diskrement.core._
import diskrement.shell.fs.Hasher
import zio._
import zio.test._
import java.nio.file.{Files, Path}
import java.time.Instant

object SimulatedBurnerSpec extends ZIOSpecDefault:
  private val ts = Instant.parse("2026-06-01T00:00:00Z")

  private def sha256(s: String): String =
    java.security.MessageDigest.getInstance("SHA-256")
      .digest(s.getBytes("UTF-8")).map(b => f"${b & 0xff}%02x").mkString

  private def deleteRecursive(dir: Path): Unit =
    val st = Files.walk(dir)
    try st.sorted(java.util.Comparator.reverseOrder()).forEach(p => Files.deleteIfExists(p))
    finally st.close()

  /** base/{snapshot/{a.txt,sub/b.txt}, catalog-snapshot.sqlite} + a staging path. */
  private val makeWorld: ZIO[Scope, Throwable, (Path, Path, Path)] =
    ZIO.acquireRelease(ZIO.attempt {
      val base = Files.createTempDirectory("burnspec")
      val snap = Files.createDirectories(base.resolve("snapshot"))
      Files.writeString(snap.resolve("a.txt"), "aaa")
      Files.createDirectories(snap.resolve("sub"))
      Files.writeString(snap.resolve("sub/b.txt"), "bbbb")
      val cat = base.resolve("catalog-snapshot.sqlite")
      Files.write(cat, Array[Byte](9, 9))
      (snap, base.resolve("staging"), cat)
    })(t => ZIO.attempt(deleteRecursive(t._1.getParent)).orDie)

  private def meta(cat: Path, files: List[ScannedFile]) =
    DiscMetadata(
      DiscIdentity("p", 1, 1, ts),
      SessionManifest("p", 1, 1,
        files.map(f => ManifestEntry(f.relPath, f.contentHash, f.sizeBytes)), Nil, Nil),
      cat, Nil)

  def spec = suite("SimulatedBurner")(
    test("stages /data + /.diskrement and computes the image size"):
      ZIO.scoped {
        for
          world <- makeWorld
          (snap, staging, cat) = world
          files = List(
            ScannedFile(RelPath("a.txt"), ContentHash(sha256("aaa")), 3, ts),
            ScannedFile(RelPath("sub/b.txt"), ContentHash(sha256("bbbb")), 4, ts))
          image = DiscImage(staging, snap, files, meta(cat, files), closeDisc = false)
          res   <- ZIO.serviceWithZIO[Burner](_.burn(image))
          aTxt  <- ZIO.attempt(Files.readString(staging.resolve("data/a.txt")))
          bTxt  <- ZIO.attempt(Files.readString(staging.resolve("data/sub/b.txt")))
          hasManifest <- ZIO.attempt(Files.exists(staging.resolve(".diskrement/manifest.json")))
          hasCatalog  <- ZIO.attempt(Files.exists(staging.resolve(".diskrement/catalog.sqlite")))
        yield assertTrue(aTxt == "aaa", bTxt == "bbbb", hasManifest, hasCatalog, res.imageBytes > 7L)
      },
    test("aborts with HashMismatch when bytes don't match the expected hash"):
      ZIO.scoped {
        for
          world <- makeWorld
          (snap, staging, cat) = world
          files = List(ScannedFile(RelPath("a.txt"), ContentHash("deadbeef"), 3, ts))
          image = DiscImage(staging, snap, files, meta(cat, files), closeDisc = false)
          err   <- ZIO.serviceWithZIO[Burner](_.burn(image)).flip
        yield assertTrue(err.isInstanceOf[BurnError.HashMismatch])
      },
    test("aborts with SourceFileMissing when a planned file is absent"):
      ZIO.scoped {
        for
          world <- makeWorld
          (snap, staging, cat) = world
          files = List(ScannedFile(RelPath("ghost.txt"), ContentHash(sha256("x")), 1, ts))
          image = DiscImage(staging, snap, files, meta(cat, files), closeDisc = false)
          err   <- ZIO.serviceWithZIO[Burner](_.burn(image)).flip
        yield assertTrue(err.isInstanceOf[BurnError.SourceFileMissing])
      }
  ).provideLayer((Hasher.live ++ SnapshotWriter.live) >>> SimulatedBurner.live)
