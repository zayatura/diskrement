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
