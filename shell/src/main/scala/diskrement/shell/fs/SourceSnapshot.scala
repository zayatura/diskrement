package diskrement.shell.fs

import zio._
import java.nio.file.{Files, Path}

/** The frozen source root the rest of the run reads from. */
final case class SnapshotHandle(root: Path)

/** Expected snapshot failures. Mirrors CatalogError (extends Exception). */
sealed abstract class SnapshotError(msg: String, cause: Throwable = null)
    extends Exception(msg, cause)
object SnapshotError:
  final case class NotCowCapable(workDir: Path, detail: String)
      extends SnapshotError(
        s"work dir is not on a CoW-capable filesystem (need btrfs or XFS-with-reflink): $workDir [$detail]")
  final case class SourceMissing(sourceRoot: Path)
      extends SnapshotError(s"source root does not exist: $sourceRoot")
  final case class SnapshotIoError(cause: Throwable)
      extends SnapshotError("snapshot I/O error", cause)

/** Internal seam over the actual reflink clone, so SourceSnapshot's logic is
  * unit-testable with a fake and the real `cp` runs only in an env-gated test. */
private[fs] trait Cloner:
  def reflinkCopy(src: Path, dst: Path): IO[SnapshotError, Unit]

/** Live cloner: `cp --reflink=always -a SRC DST`. A clone-unsupported failure
  * maps to NotCowCapable; everything else to SnapshotIoError. */
private[fs] final case class CpCloner() extends Cloner:
  def reflinkCopy(src: Path, dst: Path): IO[SnapshotError, Unit] =
    ZIO.attemptBlockingIO {
      val proc = new ProcessBuilder(
        "cp", "--reflink=always", "-a", src.toString, dst.toString).start()
      val stderr = new String(proc.getErrorStream.readAllBytes(), "UTF-8")
      val code   = proc.waitFor()
      (code, stderr)
    }.mapError(SnapshotError.SnapshotIoError(_))
      .flatMap { case (code, stderr) =>
        if code == 0 then ZIO.unit
        else if isCloneUnsupported(stderr) then
          ZIO.fail(SnapshotError.NotCowCapable(dst.getParent, stderr.trim))
        else
          ZIO.fail(SnapshotError.SnapshotIoError(
            new RuntimeException(s"cp exited $code: ${stderr.trim}")))
      }

  private def isCloneUnsupported(stderr: String): Boolean =
    val s = stderr.toLowerCase
    s.contains("failed to clone") || s.contains("operation not supported")

/** Freezes the source tree by reflinking it to a deterministic dir under the
  * work dir. Lifecycle is explicit (create/release) and on-disk so it can span
  * the staged CLI invocations (engine design §6; spec §2.1 deviation 3). */
trait SourceSnapshot:
  def create(sourceRoot: Path, workDir: Path): IO[SnapshotError, SnapshotHandle]
  def release(handle: SnapshotHandle): UIO[Unit]

private[fs] final case class SourceSnapshotLive(cloner: Cloner) extends SourceSnapshot:
  def create(sourceRoot: Path, workDir: Path): IO[SnapshotError, SnapshotHandle] =
    val dst = workDir.resolve("snapshot")
    for
      exists <- ZIO.attemptBlockingIO(Files.exists(sourceRoot))
                  .mapError(SnapshotError.SnapshotIoError(_))
      _      <- ZIO.unless(exists)(ZIO.fail(SnapshotError.SourceMissing(sourceRoot)))
      _      <- removeDir(dst)
      _      <- ZIO.attemptBlockingIO(Files.createDirectories(workDir))
                  .mapError(SnapshotError.SnapshotIoError(_))
      _      <- cloner.reflinkCopy(sourceRoot, dst)
    yield SnapshotHandle(dst)

  def release(handle: SnapshotHandle): UIO[Unit] = removeDir(handle.root)

  private def removeDir(dir: Path): UIO[Unit] =
    ZIO.attemptBlockingIO {
      if Files.exists(dir) then
        val stream = Files.walk(dir)
        try
          stream.sorted(java.util.Comparator.reverseOrder())
            .forEach(p => Files.deleteIfExists(p))
        finally stream.close()
    }.ignoreLogged

object SourceSnapshot:
  val live: ZLayer[Any, Nothing, SourceSnapshot] =
    ZLayer.succeed(SourceSnapshotLive(CpCloner()))
