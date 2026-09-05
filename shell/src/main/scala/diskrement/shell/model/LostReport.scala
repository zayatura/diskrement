package diskrement.shell.model

import diskrement.core.{ContentHash, RelPath}

/** A version whose only copy was on the now-dead disc and whose path has moved
  * past it — those bytes are permanently gone. */
final case class LostVersionEntry(
    relPath: RelPath, contentHash: ContentHash, sizeBytes: Long, sessionOrdinal: Int)

/** A live file whose only burned copy was on the dead disc; the next
  * scan/plan/burn writes it again from the source. */
final case class ReburnEntry(relPath: RelPath, contentHash: ContentHash, sizeBytes: Long)

/** What marking a disc dead cost. Recorded in `lost_version`, rendered by the CLI. */
final case class LostReport(
    discOrdinal: Int, lost: List[LostVersionEntry], reburn: List[ReburnEntry])
