package diskrement.core

import java.time.Instant

class PlannerTypesTest extends munit.FunSuite:
  test("DiscState reports free bytes"):
    val d = DiscState(ordinal = 1, status = DiscStatus.OPEN,
                      usableBytes = 100L, usedBytes = 30L)
    assertEquals(d.freeBytes, 70L)

  test("PlannedDisc reports added and total bytes"):
    val f = ScannedFile(RelPath("a"), ContentHash("h"), 25L, Instant.EPOCH)
    val pd = PlannedDisc(ordinal = 2, isNew = true, finalize = false,
                         files = List(f, f), priorUsedBytes = 10L, usableBytes = 100L)
    assertEquals(pd.addedBytes, 50L)
    assertEquals(pd.totalUsedBytes, 60L)

  test("BurnPlan groups planned discs, tombstones, and oversized files"):
    val plan = BurnPlan(discs = Nil, tombstones = List(RelPath("gone")), oversized = Nil)
    assertEquals(plan.tombstones, List(RelPath("gone")))
