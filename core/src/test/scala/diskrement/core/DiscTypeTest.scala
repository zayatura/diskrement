package diskrement.core

class DiscTypeTest extends munit.FunSuite:
  test("disc types expose nominal capacities"):
    assertEquals(DiscType.BD_R.nominalBytes, 25_000_000_000L)
    assertEquals(DiscType.BD_R_DL.nominalBytes, 50_000_000_000L)
    assertEquals(DiscType.DVD.nominalBytes, 4_700_000_000L)
    assertEquals(DiscType.DVD_DL.nominalBytes, 8_500_000_000L)

  test("Capacity.fromDiscType subtracts the margin percent and floors"):
    val cap = Capacity.fromDiscType(DiscType.BD_R, marginPercent = 2.0)
    assertEquals(cap.usableBytes, 24_500_000_000L)

  test("zero margin yields nominal capacity"):
    assertEquals(
      Capacity.fromDiscType(DiscType.DVD, 0.0).usableBytes,
      4_700_000_000L
    )

  test("capacity is floored exactly, with no floating-point drift"):
    // 8_500_000_000 * 0.93 == 7_905_000_000 exactly. Computing this in Double
    // floors to 7_904_999_999 (one byte short); integer/decimal math must not.
    assertEquals(
      Capacity.fromDiscType(DiscType.DVD_DL, 7.0).usableBytes,
      7_905_000_000L
    )

  test("a fractional margin percent is honoured exactly"):
    // 25_000_000_000 * (1 - 2.5/100) == 24_375_000_000 exactly.
    assertEquals(
      Capacity.fromDiscType(DiscType.BD_R, 2.5).usableBytes,
      24_375_000_000L
    )
