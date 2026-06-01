package diskrement.core

/** Pure classification of a scan against the catalog's current view. */
object Diff:

  def diff(catalog: CatalogView, scanned: List[ScannedFile]): ChangeSet =
    val scannedPaths = scanned.iterator.map(_.relPath).toSet

    val added = scanned.filter(f => !catalog.current.contains(f.relPath))

    val present: List[(ScannedFile, CatalogEntry)] =
      scanned.flatMap(f => catalog.current.get(f.relPath).map(e => (f, e)))

    val (modifiedPairs, unchangedPairs) =
      present.partition((f, e) => f.contentHash != e.contentHash)

    val deleted =
      catalog.current.keys.filterNot(scannedPaths.contains).toList

    ChangeSet(
      added = added.sortBy(_.relPath),
      modified = modifiedPairs.map(_._1).sortBy(_.relPath),
      deleted = deleted.sorted,
      unchanged = unchangedPairs.map(_._1).sortBy(_.relPath)
    )
