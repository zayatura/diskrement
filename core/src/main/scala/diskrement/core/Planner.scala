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

    val existingOpen = discs.filter(_.status == DiscStatus.OPEN)
      .sortBy(_.ordinal)
      .lastOption
    val maxOrdinal = discs.map(_.ordinal).maxOption.getOrElse(0)

    // The largest disc a file could ever land on this run: a new blank disc, or
    // the open disc at its full capacity. A file exceeding both can never be
    // placed and is oversized regardless of fill order.
    val openUsable = existingOpen.map(_.usableBytes).getOrElse(0L)
    val maxUsable = math.max(newDiscCap.usableBytes, openUsable)
    val (oversizedUpFront, placeable) =
      toPlace.partition(_.sizeBytes > maxUsable)

    // Mutable-free in-progress disc representation, local to the planner.
    // Ordinals are assigned after packing (a disc's ordinal is positional).
    final case class Bin(
        isNew: Boolean,
        usableBytes: Long,
        priorUsedBytes: Long,
        files: Vector[ScannedFile]
    ):
      def usedBytes: Long = priorUsedBytes + files.map(_.sizeBytes).sum
      def freeBytes: Long = usableBytes - usedBytes
      def fits(f: ScannedFile): Boolean = f.sizeBytes <= freeBytes
      def add(f: ScannedFile): Bin = copy(files = files :+ f)

    val freshBin: Bin =
      Bin(isNew = true, newDiscCap.usableBytes, 0L, Vector.empty)

    val initialBin: Bin = existingOpen match
      case Some(d) => Bin(isNew = false, d.usableBytes, d.usedBytes, Vector.empty)
      case None    => freshBin

    // Place each file in order. A file that doesn't fit the current disc spills
    // to a fresh disc; if it doesn't fit a fresh disc either (it only fit the
    // open disc's full capacity, not its remaining space), it is oversized.
    final case class Packing(
        completed: Vector[Bin],
        current: Bin,
        oversized: Vector[ScannedFile]
    )

    val packed = placeable.foldLeft(Packing(Vector.empty, initialBin, Vector.empty)) {
      (acc, f) =>
        if acc.current.fits(f) then acc.copy(current = acc.current.add(f))
        else if f.sizeBytes <= newDiscCap.usableBytes then
          acc.copy(completed = acc.completed :+ acc.current, current = freshBin.add(f))
        else acc.copy(oversized = acc.oversized :+ f)
    }

    val bins = packed.completed :+ packed.current
    val lastIdx = bins.size - 1
    val plannedDiscs = bins.zipWithIndex.map { case (b, i) =>
      // First disc keeps the existing open disc's ordinal; every fresh disc
      // takes the next ordinal above the highest existing one.
      val ordinal =
        if !b.isNew then existingOpen.get.ordinal
        else if existingOpen.isDefined then maxOrdinal + i
        else maxOrdinal + i + 1
      PlannedDisc(
        ordinal = ordinal,
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
      oversized = (oversizedUpFront ++ packed.oversized).sortBy(_.relPath)
    )
