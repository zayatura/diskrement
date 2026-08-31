package diskrement.shell.burn

import diskrement.core._
import diskrement.shell.ProgressSink
import diskrement.shell.fs.{Hasher, HashParallelism}
import zio._
import zio.test._
import java.nio.file.{Files, Path}
import java.nio.file.attribute.PosixFilePermissions
import java.time.Instant

object SimulatedBurnerSpec extends ZIOSpecDefault:
  private val ts = Instant.parse("2026-06-01T00:00:00Z")

  /** True iff /dev/shm exists, is writable, and lives on a different
    * filesystem than java.io.tmpdir (where makeWorld's snapshot lands) — the
    * condition needed to force the plain-copy fallback via a real
    * "failed to clone"/"operation not supported" from cp. Evaluated eagerly
    * (not inside a ZIO effect) so it can gate a TestAspect. */
  private val shmCrossDevice: Boolean =
    try
      val tmp = Path.of(java.lang.System.getProperty("java.io.tmpdir", "/tmp"))
      val shm = Path.of("/dev/shm")
      Files.isDirectory(shm) && Files.isWritable(shm) &&
        Files.getFileStore(tmp) != Files.getFileStore(shm)
    catch case _: Throwable => false

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
      },
    test("staging preserves the source mtimes on the staged tree"):
      ZIO.scoped {
        for
          world <- makeWorld
          (snap, staging, cat) = world
          srcMtime <- ZIO.attempt(Files.getLastModifiedTime(snap.resolve("a.txt")))
          files = List(ScannedFile(RelPath("a.txt"), ContentHash(sha256("aaa")), 3, ts))
          image = DiscImage(staging, snap, files, meta(cat, files), closeDisc = false)
          _     <- ZIO.serviceWithZIO[Burner](_.burn(image))
          dstMtime <- ZIO.attempt(Files.getLastModifiedTime(staging.resolve("data/a.txt")))
        yield assertTrue(dstMtime == srcMtime)
      },
    test("stages files with awkward names (leading dash, space, asterisk)"):
      ZIO.scoped {
        for
          world <- makeWorld
          (snap, staging, cat) = world
          _     <- ZIO.attempt {
                     Files.writeString(snap.resolve("-dash.txt"), "d")
                     Files.writeString(snap.resolve("sp ace.txt"), "s")
                     Files.writeString(snap.resolve("star*.txt"), "t")
                   }
          files = List(
                    ScannedFile(RelPath("-dash.txt"), ContentHash(sha256("d")), 1, ts),
                    ScannedFile(RelPath("sp ace.txt"), ContentHash(sha256("s")), 1, ts),
                    ScannedFile(RelPath("star*.txt"), ContentHash(sha256("t")), 1, ts))
          image = DiscImage(staging, snap, files, meta(cat, files), closeDisc = false)
          _     <- ZIO.serviceWithZIO[Burner](_.burn(image))
          dash  <- ZIO.attempt(Files.readString(staging.resolve("data/-dash.txt")))
          space <- ZIO.attempt(Files.readString(staging.resolve("data/sp ace.txt")))
          star  <- ZIO.attempt(Files.readString(staging.resolve("data/star*.txt")))
        yield assertTrue(dash == "d", space == "s", star == "t")
      },
    test("stages more files than one batch, spanning a directory across the batch boundary"):
      ZIO.scoped {
        for
          world <- makeWorld
          (snap, staging, cat) = world
          manyDir = snap.resolve("many")
          _     <- ZIO.attempt {
                     Files.createDirectories(manyDir)
                     (1 to 600).foreach(i => Files.writeString(manyDir.resolve(f"f$i%04d.txt"), i.toString))
                   }
          files = (1 to 600).toList.map { i =>
                     val content = i.toString
                     ScannedFile(RelPath(f"many/f$i%04d.txt"), ContentHash(sha256(content)), content.length.toLong, ts)
                   }
          image = DiscImage(staging, snap, files, meta(cat, files), closeDisc = false)
          _     <- ZIO.serviceWithZIO[Burner](_.burn(image))
          allOk <- ZIO.attempt {
                     (1 to 600).forall { i =>
                       Files.readString(staging.resolve("data/many").resolve(f"f$i%04d.txt")) == i.toString
                     }
                   }
        yield assertTrue(allOk)
      },
    test("staged directory metadata is ours, not the source's (read-only source dir)"):
      ZIO.scoped {
        for
          world     <- makeWorld
          (snap, staging, cat) = world
          lockedDir = snap.resolve("locked")
          _         <- ZIO.attempt {
                         Files.createDirectories(lockedDir)
                         Files.writeString(lockedDir.resolve("one.txt"), "1")
                         Files.writeString(lockedDir.resolve("two.txt"), "2")
                       }
          _         <- ZIO.acquireRelease(
                         ZIO.attempt(Files.setPosixFilePermissions(
                           lockedDir, PosixFilePermissions.fromString("r-x------"))))(
                         _ => ZIO.attempt(Files.setPosixFilePermissions(
                           lockedDir, PosixFilePermissions.fromString("rwx------"))).orDie)
          files = List(
                    ScannedFile(RelPath("locked/one.txt"), ContentHash(sha256("1")), 1, ts),
                    ScannedFile(RelPath("locked/two.txt"), ContentHash(sha256("2")), 1, ts))
          image = DiscImage(staging, snap, files, meta(cat, files), closeDisc = false)
          _     <- ZIO.serviceWithZIO[Burner](_.burn(image))
          writable <- ZIO.attempt(Files.isWritable(staging.resolve("data/locked")))
        yield assertTrue(writable)
      },
    (test("falls back to plain copy across filesystems (/dev/shm) and preserves source mtime"):
      ZIO.scoped {
        for
          world    <- makeWorld
          (snap, _, cat) = world
          staging  <- ZIO.acquireRelease(
                        ZIO.attempt(Files.createTempDirectory(Path.of("/dev/shm"), "burnspec-shm")))(
                        d => ZIO.attempt(deleteRecursive(d)).orDie)
          srcMtime <- ZIO.attempt(Files.getLastModifiedTime(snap.resolve("a.txt")))
          files = List(ScannedFile(RelPath("a.txt"), ContentHash(sha256("aaa")), 3, ts))
          image = DiscImage(staging, snap, files, meta(cat, files), closeDisc = false)
          _        <- ZIO.serviceWithZIO[Burner](_.burn(image))
          aTxt     <- ZIO.attempt(Files.readString(staging.resolve("data/a.txt")))
          dstMtime <- ZIO.attempt(Files.getLastModifiedTime(staging.resolve("data/a.txt")))
          // Compared at millisecond granularity: Files.copy(COPY_ATTRIBUTES) on this
          // platform truncates sub-microsecond precision (verified independently of
          // this fix — a JDK/libc utimes() artifact, not something the fallback
          // controls), so exact FileTime equality is not a meaningful assertion here.
        yield assertTrue(aTxt == "aaa", dstMtime.toMillis == srcMtime.toMillis)
      }
    ) @@ (if shmCrossDevice then TestAspect.identity else TestAspect.ignore)
  ).provideLayer(
    (Hasher.live ++ SnapshotWriter.live ++ ProgressSink.noop ++
      ZLayer.succeed(HashParallelism(2))) >>> SimulatedBurner.live)
