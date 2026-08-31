package diskrement.shell.fs

import diskrement.core.{RelPath, ScannedFile}
import diskrement.shell.ProgressSink
import diskrement.shell.model.FileFingerprint
import zio._
import java.nio.file.Path

/** Expected scan failure: a specific file could not be walked/statted/hashed. */
sealed abstract class ScanError(msg: String, cause: Throwable = null)
    extends Exception(msg, cause)
object ScanError:
  final case class FileFailed(relPath: RelPath, cause: Throwable)
      extends ScanError(s"failed scanning ${relPath.value}", cause)

/** The scanner's result: hashed files plus how many actually got hashed this
  * run (fast-path fingerprint hits are the difference). */
final case class ScanOutput(files: List[ScannedFile], hashedFiles: Int)

/** Walks a frozen snapshot and produces fully-hashed ScannedFiles, reusing a
  * stored hash when size+mtime match (fast path) unless `verify` is set.
  * Fast-path misses are hashed in bounded parallel; progress is reported for
  * the miss set only (that is the work the operator is waiting on). */
trait FileScanner:
  def scan(
      snapshotRoot: Path,
      prior: Map[RelPath, FileFingerprint],
      verify: Boolean
  ): IO[ScanError, ScanOutput]

final case class FileScannerLive(
    fs: FileSystem, hasher: Hasher, sink: ProgressSink, parallelism: Int
) extends FileScanner:

  def scan(
      snapshotRoot: Path,
      prior: Map[RelPath, FileFingerprint],
      verify: Boolean
  ): IO[ScanError, ScanOutput] =
    for
      paths  <- fs.walk(snapshotRoot)
                  .mapError(e => ScanError.FileFailed(RelPath(snapshotRoot.toString), e))
      stats  <- ZIO.foreach(paths) { abs =>
                  val rel = RelPath(snapshotRoot.relativize(abs).toString)
                  fs.stat(abs).mapBoth(ScanError.FileFailed(rel, _), st => (abs, rel, st))
                }
      parts   = stats.partition { case (_, rel, st) =>
                  !verify && prior.get(rel)
                    .exists(fp => fp.sizeBytes == st.sizeBytes && fp.mtime == st.mtime)
                }
      hits    = parts._1
      misses  = parts._2
      reused  = hits.map { case (_, rel, st) =>
                  ScannedFile(rel, prior(rel).contentHash, st.sizeBytes, st.mtime)
                }
      // phase() brackets the hash work: start before, done on ANY exit
      // (success, failure, interrupt) — never leave the TTY line half-drawn.
      hashed <- sink.phase("hashing", misses.map(_._3.sizeBytes).sum, misses.size) {
                  ZIO.foreachPar(misses) { case (abs, rel, st) =>
                    hasher.hash(abs)
                      .mapBoth(ScanError.FileFailed(rel, _),
                               h => ScannedFile(rel, h, st.sizeBytes, st.mtime))
                      .zipLeft(sink.advance(st.sizeBytes, 1))
                  }.withParallelism(parallelism)
                }
    yield ScanOutput((reused ++ hashed).sortBy(_.relPath), hashed.size)

object FileScanner:
  val live: ZLayer[FileSystem & Hasher & ProgressSink & HashParallelism, Nothing, FileScanner] =
    ZLayer {
      for
        fs   <- ZIO.service[FileSystem]
        h    <- ZIO.service[Hasher]
        sink <- ZIO.service[ProgressSink]
        par  <- ZIO.service[HashParallelism]
      yield FileScannerLive(fs, h, sink, par.value)
    }
