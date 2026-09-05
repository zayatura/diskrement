package diskrement.shell.model

import diskrement.core.ContentHash

/** Lifecycle status of one file version relative to its path's current state.
  * LOST = a non-current version whose disc was marked dead: the bytes are gone. */
enum VersionStatus:
  case CURRENT, SUPERSEDED, DELETED, LOST

/** One version of a path in its full history, with where its bytes live. */
final case class FileHistoryEntry(
    contentHash: ContentHash,
    sizeBytes: Long,
    discOrdinal: Int,
    sessionOrdinal: Int,
    status: VersionStatus
)
