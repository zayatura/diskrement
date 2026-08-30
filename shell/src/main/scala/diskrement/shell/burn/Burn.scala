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
    metadata: DiscMetadata,
    closeDisc: Boolean
)

final case class BurnResult(stagingDir: Path, imageBytes: Long)

/** Where a burn physically goes: a real optical drive, or (tests only) a plain
  * file addressed through xorriso's stdio: pseudo-drive. */
enum BurnDevice:
  case RealDevice(path: String)
  case ImageFile(path: Path)

  def xorrisoAddress: String = this match
    case RealDevice(p) => p
    case ImageFile(p)  => s"stdio:$p"

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
  final case class ToolMissing(detail: String)
      extends BurnError(s"xorriso is not available: $detail (install the libisoburn/xorriso package)")
  final case class ToolTooOld(version: String)
      extends BurnError(s"xorriso $version is too old (need 1.5 or newer)")
  final case class DeviceNotFound(device: String)
      extends BurnError(s"burner device not found: $device")
  final case class DevicePermissionDenied(device: String)
      extends BurnError(
        s"no permission to open $device — add your user to the 'optical' group " +
          "(sudo usermod -aG optical <user>) and log in again")
  final case class NoMedia(device: String)
      extends BurnError(s"no disc in $device")
  final case class NotBlank(device: String)
      extends BurnError(s"expected a blank disc in $device but found written media")
  final case class DiscClosed(device: String)
      extends BurnError(s"the disc in $device is finalized (closed) and cannot be appended")
  final case class ForeignDisc(device: String)
      extends BurnError(s"the disc in $device is not a diskrement disc for this project")
  final case class WrongDisc(expected: String, found: String)
      extends BurnError(s"wrong disc inserted: expected $expected, found $found")
  final case class InsufficientCapacity(neededBytes: Long, freeBytes: Long)
      extends BurnError(s"insufficient media capacity: need $neededBytes bytes, only $freeBytes free")
  final case class BurnFailed(exitCode: Int, outputTail: List[String])
      extends BurnError(s"xorriso failed (exit $exitCode): ${outputTail.mkString(" | ")}")
  final case class PostBurnCheckFailed(detail: String)
      extends BurnError(s"post-burn sanity check failed: $detail")
  final case class MountFailed(detail: String)
      extends BurnError(s"could not mount the disc for verification: $detail")
  case object Aborted extends BurnError("aborted by operator")
