package diskrement.core

/** One file's identity as recorded in a session manifest. */
final case class ManifestEntry(
    relPath: RelPath,
    contentHash: ContentHash,
    sizeBytes: Long
)

enum ManifestChangeKind:
  case ADDED, MODIFIED, DELETED

/** The pure manifest data for one session burned onto one disc.
  * Serialization to JSON is a shell concern (not part of the core). */
final case class SessionManifest(
    projectName: String,
    discOrdinal: Int,
    sessionOrdinal: Int,
    added: List[ManifestEntry],
    modified: List[ManifestEntry],
    deleted: List[RelPath]
)

object ManifestBuilder:

  /** Build the manifest for `disc`, classifying its files via `changes` and
    * attaching the given `tombstones` (typically only on the last/open disc). */
  def build(
      projectName: String,
      sessionOrdinal: Int,
      disc: PlannedDisc,
      changes: ChangeSet,
      tombstones: List[RelPath]
  ): SessionManifest =
    val addedPaths = changes.added.iterator.map(_.relPath).toSet
    val (addedFiles, modifiedFiles) =
      disc.files.partition(f => addedPaths.contains(f.relPath))
    SessionManifest(
      projectName = projectName,
      discOrdinal = disc.ordinal,
      sessionOrdinal = sessionOrdinal,
      added = addedFiles.map(toEntry),
      modified = modifiedFiles.map(toEntry),
      deleted = tombstones
    )

  private def toEntry(f: ScannedFile): ManifestEntry =
    ManifestEntry(f.relPath, f.contentHash, f.sizeBytes)
