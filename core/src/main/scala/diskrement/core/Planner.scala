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
    willFinalize: Boolean,
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

/** Pure, deterministic ordered-fill disc planner. */
object Planner:

  def plan(
      changes: ChangeSet,
      discs: List[DiscState],
      newDiscCap: Capacity
  ): BurnPlan =
    val toPlace = (changes.added ++ changes.modified).sortBy(_.relPath)
    val (oversized, placeable) =
      toPlace.partition(_.sizeBytes > newDiscCap.usableBytes)

    val existingOpen = discs.filter(_.status == DiscStatus.OPEN)
      .sortBy(_.ordinal)
      .lastOption
    val maxOrdinal = discs.map(_.ordinal).maxOption.getOrElse(0)

    // Mutable-free in-progress disc representation, local to the planner.
    final case class Bin(
        ordinal: Int,
        isNew: Boolean,
        usableBytes: Long,
        priorUsedBytes: Long,
        files: Vector[ScannedFile]
    ):
      def usedBytes: Long = priorUsedBytes + files.map(_.sizeBytes).sum
      def freeBytes: Long = usableBytes - usedBytes
      def fits(f: ScannedFile): Boolean = f.sizeBytes <= freeBytes
      def add(f: ScannedFile): Bin = copy(files = files :+ f)

    def freshBin(prevOrdinal: Int): Bin =
      Bin(prevOrdinal + 1, isNew = true, newDiscCap.usableBytes, 0L, Vector.empty)

    val initialBin: Bin = existingOpen match
      case Some(d) => Bin(d.ordinal, isNew = false, d.usableBytes, d.usedBytes, Vector.empty)
      case None    => freshBin(maxOrdinal)

    val (completed, current) =
      placeable.foldLeft((Vector.empty[Bin], initialBin)) {
        case ((done, cur), f) =>
          if cur.fits(f) then (done, cur.add(f))
          else (done :+ cur, freshBin(cur.ordinal).add(f))
      }

    val bins = completed :+ current
    val lastIdx = bins.size - 1
    val plannedDiscs = bins.zipWithIndex.map { case (b, i) =>
      PlannedDisc(
        ordinal = b.ordinal,
        isNew = b.isNew,
        willFinalize = i != lastIdx,
        files = b.files.toList,
        priorUsedBytes = b.priorUsedBytes,
        usableBytes = b.usableBytes
      )
    }.toList

    BurnPlan(
      discs = plannedDiscs,
      tombstones = changes.deleted,
      oversized = oversized
    )
