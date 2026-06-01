package diskrement.core

/** Classification of a scan against the catalog's current view.
  *
  * `unchanged` is retained for reporting; it never contributes to a burn.
  */
final case class ChangeSet(
    added: List[ScannedFile],
    modified: List[ScannedFile],
    deleted: List[RelPath],
    unchanged: List[ScannedFile]
):
  /** True when there is nothing to back up (added/modified/deleted all empty). */
  def isEmpty: Boolean =
    added.isEmpty && modified.isEmpty && deleted.isEmpty

object ChangeSet:
  val empty: ChangeSet = ChangeSet(Nil, Nil, Nil, Nil)
