package diskrement.shell.model

import diskrement.core.{ContentHash, RelPath}

/** A file version on a disc hidden by a later session of the SAME disc. */
final case class SupersededVersion(sessionOrdinal: Int, relPath: RelPath, contentHash: ContentHash)

/** Everything the catalog records as physically on one disc: the merged view a
  * mount shows (`visible`), and versions hidden within the disc (`superseded`). */
final case class DiscRecordedContents(
    visible: Map[RelPath, ContentHash],
    superseded: List[SupersededVersion])
