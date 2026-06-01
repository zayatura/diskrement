package diskrement.core

/** Physical optical media types and their nominal byte capacities. */
enum DiscType(val nominalBytes: Long):
  case BD_R    extends DiscType(25_000_000_000L)
  case BD_R_DL extends DiscType(50_000_000_000L)
  case DVD     extends DiscType(4_700_000_000L)
  case DVD_DL  extends DiscType(8_500_000_000L)

/** Usable bytes on a disc after subtracting filesystem/UDF overhead margin. */
final case class Capacity(usableBytes: Long)

object Capacity:
  /** Nominal capacity reduced by `marginPercent` (e.g. 2.0 == 2%), floored.
    *
    * Computed in `BigDecimal` rather than `Double`: at byte scale, IEEE-754
    * rounding can floor an exact value one byte short (e.g. DVD-DL at 7% margin).
    */
  def fromDiscType(discType: DiscType, marginPercent: Double): Capacity =
    val usable =
      (BigDecimal(discType.nominalBytes) * (100 - BigDecimal(marginPercent)) / 100)
        .setScale(0, BigDecimal.RoundingMode.FLOOR)
    Capacity(usable.toLong)
