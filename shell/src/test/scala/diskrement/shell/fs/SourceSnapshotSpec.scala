package diskrement.shell.fs

import zio._
import zio.test._
import java.nio.file.{Files, Path}

object SourceSnapshotSpec extends ZIOSpecDefault:

  /** Fake cloner: plain recursive copy (so the snapshot really appears), or a
    * configured failure for the error-mapping tests. */
  private final case class FakeCloner(fail: Option[SnapshotError]) extends Cloner:
    def reflinkCopy(src: Path, dst: Path): IO[SnapshotError, Unit] =
      fail match
        case Some(e) => ZIO.fail(e)
        case None    =>
          ZIO.attempt {
            Files.createDirectories(dst)
            val list = Files.list(src)
            try list.forEach(p => Files.copy(p, dst.resolve(p.getFileName)))
            finally list.close()
          }.mapError(SnapshotError.SnapshotIoError(_))

  /** base/{src/f.txt, work} ; deleted recursively on release. */
  private val makeDirs: ZIO[Scope, Throwable, (Path, Path)] =
    ZIO.acquireRelease(
      ZIO.attempt {
        val base = Files.createTempDirectory("snapspec")
        val src  = Files.createDirectory(base.resolve("src"))
        Files.writeString(src.resolve("f.txt"), "data")
        val work = Files.createDirectory(base.resolve("work"))
        (src, work)
      })((dirs) =>
        ZIO.attempt {
          val base = dirs._1.getParent
          val stream = Files.walk(base)
          try stream.sorted(java.util.Comparator.reverseOrder())
                    .forEach(p => Files.deleteIfExists(p))
          finally stream.close()
        }.orDie)

  def spec = suite("SourceSnapshot")(
    test("create reflinks the source into workDir/snapshot and returns the handle"):
      ZIO.scoped {
        for
          dirs <- makeDirs
          (src, work) = dirs
          snap = SourceSnapshotLive(FakeCloner(None))
          h    <- snap.create(src, work)
          ok   <- ZIO.attempt(Files.exists(h.root.resolve("f.txt")))
        yield assertTrue(h.root == work.resolve("snapshot"), ok)
      },
    test("create clears a stale snapshot dir first"):
      ZIO.scoped {
        for
          dirs <- makeDirs
          (src, work) = dirs
          stale = work.resolve("snapshot")
          _    <- ZIO.attempt {
                    Files.createDirectories(stale)
                    Files.writeString(stale.resolve("old.txt"), "stale")
                  }
          snap = SourceSnapshotLive(FakeCloner(None))
          h    <- snap.create(src, work)
          oldGone  <- ZIO.attempt(!Files.exists(h.root.resolve("old.txt")))
          newThere <- ZIO.attempt(Files.exists(h.root.resolve("f.txt")))
        yield assertTrue(oldGone, newThere)
      },
    test("a missing source fails with SourceMissing"):
      ZIO.scoped {
        for
          dirs <- makeDirs
          (src, work) = dirs
          snap = SourceSnapshotLive(FakeCloner(None))
          err  <- snap.create(work.resolve("does-not-exist"), work).flip
        yield assertTrue(err.isInstanceOf[SnapshotError.SourceMissing])
      },
    test("a NotCowCapable clone error propagates"):
      ZIO.scoped {
        for
          dirs <- makeDirs
          (src, work) = dirs
          snap = SourceSnapshotLive(FakeCloner(Some(
                   SnapshotError.NotCowCapable(work, "operation not supported"))))
          err  <- snap.create(src, work).flip
        yield assertTrue(err.isInstanceOf[SnapshotError.NotCowCapable])
      },
    test("a SnapshotIoError from the cloner propagates"):
      ZIO.scoped {
        for
          dirs <- makeDirs
          (src, work) = dirs
          snap = SourceSnapshotLive(FakeCloner(Some(
                   SnapshotError.SnapshotIoError(new RuntimeException("boom")))))
          err  <- snap.create(src, work).flip
        yield assertTrue(err.isInstanceOf[SnapshotError.SnapshotIoError])
      },
    test("release removes the snapshot dir"):
      ZIO.scoped {
        for
          dirs <- makeDirs
          (src, work) = dirs
          snap = SourceSnapshotLive(FakeCloner(None))
          h    <- snap.create(src, work)
          _    <- snap.release(h)
          gone <- ZIO.attempt(!Files.exists(h.root))
        yield assertTrue(gone)
      },
    test("real cp --reflink reflinks on a CoW filesystem (auto-skips otherwise)"):
      ZIO.scoped {
        for
          dirs <- makeDirs
          (src, work) = dirs
          snap = SourceSnapshotLive(CpCloner())
          res  <- snap.create(src, work).either
          result <- res match
            case Right(h) =>
              ZIO.attempt(Files.exists(h.root.resolve("f.txt")))
                .map(exists => assertTrue(exists))
            case Left(_: SnapshotError.NotCowCapable) =>
              ZIO.succeed(assertCompletes) // not a CoW fs: skip
            case Left(other) =>
              ZIO.succeed(assertTrue(false)) // unexpected real failure
        yield result
      }
  )
