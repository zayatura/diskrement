package diskrement.shell.burn

import diskrement.core.{ContentHash, RelPath, ScannedFile, SessionManifest}
import diskrement.shell.model.ArchivePointer
import java.nio.file.Path
import java.time.Instant

final case class DiscIdentity(
    projectName: String,
    discOrdinal: Int,
    sessionOrdinal: Int,
    createdAt: Instant
)

final case class DiscMetadata(
    identity: DiscIdentity,
    manifest: SessionManifest,
    catalogSnapshot: Path,
    pointers: List[ArchivePointer]
)

final case class DiscImage(
    stagingDir: Path,
    snapshotRoot: Path,
    files: List[ScannedFile],
    metadata: DiscMetadata
)

final case class BurnResult(stagingDir: Path, imageBytes: Long)

sealed abstract class BurnError(msg: String, cause: Throwable = null)
    extends Exception(msg, cause)
object BurnError:
  final case class HashMismatch(relPath: RelPath, expected: ContentHash, actual: ContentHash)
      extends BurnError(
        s"content hash mismatch for ${relPath.value}: expected ${expected.value}, got ${actual.value}")
  final case class SourceFileMissing(relPath: RelPath)
      extends BurnError(s"source file missing from snapshot: ${relPath.value}")
  final case class BurnIoError(cause: Throwable)
      extends BurnError("burn I/O error", cause)
