package diskrement.shell.pipeline

import diskrement.core.RelPath
import diskrement.shell.fs.{CacheError, FingerprintCache}
import diskrement.shell.model.FileFingerprint
import zio._

/** In-memory cache for pipeline specs. */
final case class FakeFingerprintCache(ref: Ref[Map[RelPath, FileFingerprint]])
    extends FingerprintCache:
  def readAll: IO[CacheError, Map[RelPath, FileFingerprint]] = ref.get
  def putAll(entries: Iterable[(RelPath, FileFingerprint)]): IO[CacheError, Unit] =
    ref.update(_ ++ entries)
