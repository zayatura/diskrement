package diskrement.shell.fs

import diskrement.core.{RelPath, ScannedFile}
import diskrement.shell.model.FileFingerprint
import zio._
import java.nio.file.Path

/** Expected scan failure: a specific file could not be walked/statted/hashed. */
sealed abstract class ScanError(msg: String, cause: Throwable = null)
    extends Exception(msg, cause)
object ScanError:
  final case class FileFailed(relPath: RelPath, cause: Throwable)
      extends ScanError(s"failed scanning ${relPath.value}", cause)

/** Walks a frozen snapshot and produces fully-hashed ScannedFiles, reusing a
  * stored hash when size+mtime match (fast path) unless `verify` is set. */
trait FileScanner:
  def scan(
      snapshotRoot: Path,
      prior: Map[RelPath, FileFingerprint],
      verify: Boolean
  ): IO[ScanError, List[ScannedFile]]

final case class FileScannerLive(fs: FileSystem, hasher: Hasher) extends FileScanner:
  def scan(
      snapshotRoot: Path,
      prior: Map[RelPath, FileFingerprint],
      verify: Boolean
  ): IO[ScanError, List[ScannedFile]] =
    for
      paths <- fs.walk(snapshotRoot)
                 .mapError(e => ScanError.FileFailed(RelPath(snapshotRoot.toString), e))
      files <- ZIO.foreach(paths)(scanOne(snapshotRoot, _, prior, verify))
    yield files.sortBy(_.relPath)

  private def scanOne(
      root: Path, abs: Path,
      prior: Map[RelPath, FileFingerprint], verify: Boolean
  ): IO[ScanError, ScannedFile] =
    val rel = RelPath(root.relativize(abs).toString)
    val program =
      for
        st   <- fs.stat(abs)
        hash <- prior.get(rel) match
                  case Some(fp) if !verify && fp.sizeBytes == st.sizeBytes && fp.mtime == st.mtime =>
                    ZIO.succeed(fp.contentHash)
                  case _ => hasher.hash(abs)
      yield ScannedFile(rel, hash, st.sizeBytes, st.mtime)
    program.mapError(e => ScanError.FileFailed(rel, e))

object FileScanner:
  val live: ZLayer[FileSystem & Hasher, Nothing, FileScanner] =
    ZLayer {
      for
        fs     <- ZIO.service[FileSystem]
        hasher <- ZIO.service[Hasher]
      yield FileScannerLive(fs, hasher)
    }
