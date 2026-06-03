package diskrement.shell.fs

import zio._
import zio.test._
import java.nio.file.{Files, Path}

object CheckCowCapableSpec extends ZIOSpecDefault:
  private final case class FakeCloner(fail: Option[SnapshotError]) extends Cloner:
    def reflinkCopy(src: Path, dst: Path): IO[SnapshotError, Unit] =
      fail match
        case Some(e) => ZIO.fail(e)
        case None    => ZIO.attempt(Files.copy(src, dst)).unit.mapError(SnapshotError.SnapshotIoError(_))

  private val makeWork: ZIO[Scope, Throwable, Path] =
    ZIO.acquireRelease(ZIO.attempt(Files.createTempDirectory("cowcheck")))(
      d => ZIO.attempt {
        val s = Files.walk(d); try s.sorted(java.util.Comparator.reverseOrder())
          .forEach(p => Files.deleteIfExists(p)) finally s.close()
      }.orDie)

  def spec = suite("SourceSnapshot.checkCowCapable")(
    test("succeeds when the cloner can reflink"):
      ZIO.scoped {
        for
          work <- makeWork
          r    <- SourceSnapshotLive(FakeCloner(None)).checkCowCapable(work).either
        yield assertTrue(r == Right(()))
      },
    test("fails NotCowCapable when the cloner cannot reflink"):
      ZIO.scoped {
        for
          work <- makeWork
          err  <- SourceSnapshotLive(FakeCloner(Some(
                    SnapshotError.NotCowCapable(work, "operation not supported")))).checkCowCapable(work).flip
        yield assertTrue(err.isInstanceOf[SnapshotError.NotCowCapable])
      }
  )
