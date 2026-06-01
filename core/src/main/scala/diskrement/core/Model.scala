package diskrement.core

import java.time.Instant

/** Path of a file relative to the project's source root, using '/' separators. */
final case class RelPath(value: String)

object RelPath:
  given Ordering[RelPath] = Ordering.by(_.value)

/** Content hash of a file (SHA-256 hex in production; opaque to the core). */
final case class ContentHash(value: String)

/** A file observed during a scan, always fully hashed by the scanner. */
final case class ScannedFile(
    relPath: RelPath,
    contentHash: ContentHash,
    sizeBytes: Long,
    mtime: Instant
)

/** The current (live) version of one file as recorded in the catalog. */
final case class CatalogEntry(
    relPath: RelPath,
    contentHash: ContentHash,
    sizeBytes: Long
)

/** Immutable snapshot of the catalog's current view: relPath -> live version. */
final case class CatalogView(current: Map[RelPath, CatalogEntry])

object CatalogView:
  val empty: CatalogView = CatalogView(Map.empty)

  /** The catalog view that results from recording exactly these files as the
    * current versions (used to express diff idempotence). */
  def fromScanned(files: List[ScannedFile]): CatalogView =
    CatalogView(
      files.map(f => f.relPath -> CatalogEntry(f.relPath, f.contentHash, f.sizeBytes)).toMap
    )
