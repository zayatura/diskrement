package diskrement.shell.model

import diskrement.core.ContentHash

/** Lifecycle status of one file version relative to its path's current state. */
enum VersionStatus:
  case CURRENT, SUPERSEDED, DELETED

/** One version of a path in its full history, with where its bytes live. */
final case class FileHistoryEntry(
    contentHash: ContentHash,
    sizeBytes: Long,
    discOrdinal: Int,
    sessionOrdinal: Int,
    status: VersionStatus
)
