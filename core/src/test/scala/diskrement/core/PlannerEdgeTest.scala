package diskrement.core

import java.time.Instant

class PlannerEdgeTest extends munit.FunSuite:
  private def sf(path: String, size: Long): ScannedFile =
    ScannedFile(RelPath(path), ContentHash("h-" + path), size, Instant.EPOCH)
  private def added(fs: ScannedFile*): ChangeSet =
    ChangeSet(fs.toList, Nil, Nil, Nil)

  test("empty changeset yields a single empty open disc"):
    val plan = Planner.plan(ChangeSet.empty, Nil, Capacity(100L))
    assertEquals(plan.discs.size, 1)
    assert(plan.discs.head.files.isEmpty)
    assert(!plan.discs.head.finalize)
    assert(plan.oversized.isEmpty)

  test("a file exactly equal to free space fits"):
    val plan = Planner.plan(added(sf("a", 100)), Nil, Capacity(100L))
    assertEquals(plan.discs.size, 1)
    assertEquals(plan.discs.head.files.map(_.relPath.value), List("a"))
    assert(!plan.discs.head.finalize)

  test("only deletions: tombstones recorded, no data discs filled"):
    val cs = ChangeSet(Nil, Nil, List(RelPath("x")), Nil)
    val plan = Planner.plan(cs, Nil, Capacity(100L))
    assertEquals(plan.tombstones, List(RelPath("x")))
    assert(plan.discs.forall(_.files.isEmpty))

  test("a move (delete old path + add new path) plans the add and tombstones the old"):
    val cs = ChangeSet(added = List(sf("new/a", 10)), modified = Nil,
                       deleted = List(RelPath("old/a")), unchanged = Nil)
    val plan = Planner.plan(cs, Nil, Capacity(100L))
    assertEquals(plan.discs.head.files.map(_.relPath.value), List("new/a"))
    assertEquals(plan.tombstones, List(RelPath("old/a")))

  test("mixed capacity: BD-R open disc tops off, new disc uses BD-50 capacity"):
    val bdrUsable  = Capacity.fromDiscType(DiscType.BD_R, 0.0).usableBytes
    val bd50Usable = Capacity.fromDiscType(DiscType.BD_R_DL, 0.0).usableBytes
    // open BD-R with only 5 bytes free
    val open = DiscState(1, DiscStatus.OPEN, usableBytes = bdrUsable, usedBytes = bdrUsable - 5L)
    val cs = added(sf("a", 5), sf("b", bd50Usable)) // b only fits a BD-50
    val plan = Planner.plan(cs, discs = List(open), newDiscCap = Capacity(bd50Usable))
    assertEquals(plan.discs.map(_.ordinal), List(1, 2))
    assertEquals(plan.discs(0).files.map(_.relPath.value), List("a"))
    assertEquals(plan.discs(0).usableBytes, bdrUsable)
    assert(plan.discs(0).finalize)
    assertEquals(plan.discs(1).files.map(_.relPath.value), List("b"))
    assertEquals(plan.discs(1).usableBytes, bd50Usable)
    assert(!plan.discs(1).finalize)
    assert(plan.oversized.isEmpty)
