package diskrement.shell.model

import diskrement.core.{ContentHash, ManifestChangeKind, RelPath}

/** One non-current (superseded or deleted) file version and where its bytes
  * physically live. `kind` is the latest change-record kind for that version:
  * ADDED/MODIFIED for a superseded older version, DELETED for a tombstoned one. */
final case class ArchivePointer(
    relPath: RelPath,
    contentHash: ContentHash,
    sizeBytes: Long,
    discOrdinal: Int,
    sessionOrdinal: Int,
    discPath: String,
    kind: ManifestChangeKind
)
