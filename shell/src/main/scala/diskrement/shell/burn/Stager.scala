package diskrement.shell.burn

import diskrement.core.ScannedFile
import diskrement.shell.fs.Hasher
import zio._
import java.nio.file.{Files, Path}
import scala.jdk.StreamConverters.*

/** Stages a full disc tree (/data + /.diskrement) to image.stagingDir, re-hashing
  * each file as it copies it (§6.2). Shared by the simulated and real burners:
  * for the real burner the staged tree is exactly what xorriso masters. */
object Stager:
  def stage(image: DiscImage, hasher: Hasher, writer: SnapshotWriter): IO[BurnError, BurnResult] =
    for
      _     <- clearAndCreate(image.stagingDir)
      _     <- ZIO.foreachDiscard(image.files)(copyOne(image, hasher, _))
      _     <- writer.write(image.stagingDir.resolve(".diskrement"), image.metadata)
      bytes <- totalBytes(image.stagingDir)
    yield BurnResult(image.stagingDir, bytes)

  private def clearAndCreate(dir: Path): IO[BurnError, Unit] =
    ZIO.attemptBlockingIO {
      if Files.exists(dir) then
        val s = Files.walk(dir)
        try s.sorted(java.util.Comparator.reverseOrder()).forEach(p => Files.deleteIfExists(p))
        finally s.close()
      Files.createDirectories(dir)
      ()
    }.mapError(BurnError.BurnIoError(_))

  private def copyOne(image: DiscImage, hasher: Hasher, f: ScannedFile): IO[BurnError, Unit] =
    val src = image.snapshotRoot.resolve(f.relPath.value)
    val dst = image.stagingDir.resolve("data").resolve(f.relPath.value)
    for
      exists <- ZIO.attemptBlockingIO(Files.exists(src)).mapError(BurnError.BurnIoError(_))
      _      <- ZIO.unless(exists)(ZIO.fail(BurnError.SourceFileMissing(f.relPath)))
      _      <- ZIO.attemptBlockingIO {
                  Files.createDirectories(dst.getParent)
                  Files.copy(src, dst)
                  ()
                }.mapError(BurnError.BurnIoError(_))
      actual <- hasher.hash(dst).mapError(BurnError.BurnIoError(_))
      _      <- ZIO.unless(actual == f.contentHash)(
                  ZIO.fail(BurnError.HashMismatch(f.relPath, f.contentHash, actual)))
    yield ()

  private def totalBytes(dir: Path): IO[BurnError, Long] =
    ZIO.attemptBlockingIO {
      val s = Files.walk(dir)
      try s.toScala(List).filter(p => Files.isRegularFile(p)).map(p => Files.size(p)).sum
      finally s.close()
    }.mapError(BurnError.BurnIoError(_))
