package diskrement.shell.pipeline

import diskrement.core.RelPath
import diskrement.shell.fs.{CacheError, FingerprintCache}
import diskrement.shell.model.FileFingerprint
import zio._

/** In-memory cache for pipeline specs. `failRead`/`failWrite` simulate the
  * adapter's degradation paths, so specs can pin that a scan still succeeds
  * when the cache fails. */
final case class FakeFingerprintCache(
    ref: Ref[Map[RelPath, FileFingerprint]],
    failRead: Boolean = false,
    failWrite: Boolean = false) extends FingerprintCache:
  def readAll: IO[CacheError, Map[RelPath, FileFingerprint]] =
    if failRead then ZIO.fail(CacheError.CacheIoError(new RuntimeException("boom"))) else ref.get
  def putAll(entries: Iterable[(RelPath, FileFingerprint)]): IO[CacheError, Unit] =
    if failWrite then ZIO.fail(CacheError.CacheIoError(new RuntimeException("boom")))
    else ref.update(_ ++ entries)
