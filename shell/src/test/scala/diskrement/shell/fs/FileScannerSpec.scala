package diskrement.shell.fs

import diskrement.core.{ContentHash, RelPath}
import diskrement.shell.model.FileFingerprint
import zio._
import zio.test._
import java.io.IOException
import java.nio.file.{Path, Paths}
import java.time.Instant

object FileScannerSpec extends ZIOSpecDefault:
  private val mt1 = Instant.parse("2026-05-01T00:00:00Z")
  private val mt2 = Instant.parse("2026-05-02T00:00:00Z")
  private val root = Paths.get("/snap")

  /** In-memory FileSystem: a fixed relpath -> stat map under `root`. */
  private final case class FakeFs(files: Map[String, FileStat]) extends FileSystem:
    def walk(r: Path): IO[IOException, List[Path]] =
      ZIO.succeed(files.keys.toList.map(root.resolve))
    def stat(p: Path): IO[IOException, FileStat] =
      files.get(root.relativize(p).toString) match
        case Some(s) => ZIO.succeed(s)
        case None    => ZIO.fail(new IOException(s"no such file $p"))

  /** Hasher that returns "H:<filename>" and counts how many times it ran. */
  private final case class CountingHasher(ref: Ref[Int]) extends Hasher:
    def hash(p: Path): IO[IOException, ContentHash] =
      ref.update(_ + 1).as(ContentHash("H:" + p.getFileName.toString))

  private def run(files: Map[String, FileStat],
                  prior: Map[RelPath, FileFingerprint],
                  verify: Boolean): UIO[(List[diskrement.core.ScannedFile], Int)] =
    for
      counter <- Ref.make(0)
      scanner  = FileScannerLive(FakeFs(files), CountingHasher(counter))
      out     <- scanner.scan(root, prior, verify).orDie
      hashed  <- counter.get
    yield (out, hashed)

  def spec = suite("FileScanner")(
    test("fast path reuses the stored hash when size+mtime match (no hashing)"):
      val files = Map("a" -> FileStat(10, mt1), "b" -> FileStat(20, mt1))
      val prior = Map(
        RelPath("a") -> FileFingerprint(10, mt1, ContentHash("old-a")),
        RelPath("b") -> FileFingerprint(20, mt1, ContentHash("old-b")))
      run(files, prior, verify = false).map { case (out, hashed) =>
        assertTrue(
          hashed == 0,
          out.map(f => (f.relPath.value, f.contentHash.value)) ==
            List(("a", "old-a"), ("b", "old-b")))
      }
    ,
    test("a size or mtime change forces a re-hash"):
      val files = Map("a" -> FileStat(10, mt2), "b" -> FileStat(99, mt1))
      val prior = Map(
        RelPath("a") -> FileFingerprint(10, mt1, ContentHash("old-a")),
        RelPath("b") -> FileFingerprint(20, mt1, ContentHash("old-b")))
      run(files, prior, verify = false).map { case (out, hashed) =>
        assertTrue(hashed == 2, out.map(_.contentHash.value) == List("H:a", "H:b"))
      }
    ,
    test("verify forces a re-hash even when the fingerprint matches"):
      val files = Map("a" -> FileStat(10, mt1))
      val prior = Map(RelPath("a") -> FileFingerprint(10, mt1, ContentHash("old-a")))
      run(files, prior, verify = true).map { case (out, hashed) =>
        assertTrue(hashed == 1, out.head.contentHash.value == "H:a")
      }
    ,
    test("a file absent from prior is hashed; output is sorted by relPath"):
      val files = Map("z" -> FileStat(1, mt1), "a" -> FileStat(2, mt1))
      run(files, Map.empty, verify = false).map { case (out, hashed) =>
        assertTrue(hashed == 2, out.map(_.relPath.value) == List("a", "z"))
      }
  )
