package diskrement.core

/** Whether a disc can still receive new sessions. */
enum DiscStatus:
  case OPEN, FINALIZED

/** An existing disc as known to the catalog when planning begins. */
final case class DiscState(
    ordinal: Int,
    status: DiscStatus,
    usableBytes: Long,
    usedBytes: Long
):
  def freeBytes: Long = usableBytes - usedBytes

/** One disc as it will be written by the upcoming burn. */
final case class PlannedDisc(
    ordinal: Int,
    isNew: Boolean,
    finalize: Boolean,
    files: List[ScannedFile],
    priorUsedBytes: Long,
    usableBytes: Long
):
  def addedBytes: Long      = files.map(_.sizeBytes).sum
  def totalUsedBytes: Long  = priorUsedBytes + addedBytes

/** The full plan for the next burn session(s). */
final case class BurnPlan(
    discs: List[PlannedDisc],
    tombstones: List[RelPath],
    oversized: List[ScannedFile]
)
