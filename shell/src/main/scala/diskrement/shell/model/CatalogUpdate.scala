package diskrement.shell.model

import diskrement.core.{ContentHash, DiscStatus, DiscType, RelPath}
import java.time.Instant

/** One file written this run: its identity, where it landed, and which copy. */
final case class FileWrite(
    relPath: RelPath,
    contentHash: ContentHash,
    sizeBytes: Long,
    mtime: Instant,
    discPath: String,
    copyIndex: Int
)

/** One session burned onto one disc this run. */
final case class SessionWrite(
    ordinal: Int,
    burnedAt: Instant,
    manifestHash: String,
    added: List[FileWrite],
    modified: List[FileWrite],
    deleted: List[RelPath]   // tombstones: reference the current version of each path
)

/** One disc touched or created this run, with its single new session. */
final case class DiscWrite(
    ordinal: Int,
    isNew: Boolean,
    discType: DiscType,
    usableBytes: Long,
    status: DiscStatus,      // resulting status (OPEN for the last disc, else FINALIZED)
    label: String,
    session: SessionWrite
)

/** The full set of catalog changes from one successful burn run. */
final case class CatalogUpdate(
    projectId: ProjectId,
    finalizeDiscOrdinals: List[Int],   // pre-existing OPEN discs to flip to FINALIZED
    discs: List[DiscWrite]
)
