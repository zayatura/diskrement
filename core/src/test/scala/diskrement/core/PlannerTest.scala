package diskrement.core

import java.time.Instant

class PlannerTest extends munit.FunSuite:
  private def sf(path: String, size: Long): ScannedFile =
    ScannedFile(RelPath(path), ContentHash("h-" + path), size, Instant.EPOCH)

  private def changesOf(added: List[ScannedFile]): ChangeSet =
    ChangeSet(added = added, modified = Nil, deleted = Nil, unchanged = Nil)

  private val cap100 = Capacity(100L)

  test("all files fit on one fresh disc, which stays open"):
    val cs = changesOf(List(sf("a", 30), sf("b", 30)))
    val plan = Planner.plan(cs, discs = Nil, newDiscCap = cap100)
    assertEquals(plan.discs.size, 1)
    val d = plan.discs.head
    assert(d.isNew)
    assert(!d.willFinalize)
    assertEquals(d.files.map(_.relPath.value), List("a", "b"))
    assertEquals(d.ordinal, 1)

  test("files are placed in lexicographic order regardless of input order"):
    val cs = changesOf(List(sf("c", 10), sf("a", 10), sf("b", 10)))
    val plan = Planner.plan(cs, Nil, cap100)
    assertEquals(plan.discs.head.files.map(_.relPath.value), List("a", "b", "c"))

  test("a file that doesn't fit spills wholly to a new disc; full disc finalized"):
    // 60 + 60 cannot share a 100-byte disc
    val cs = changesOf(List(sf("a", 60), sf("b", 60)))
    val plan = Planner.plan(cs, Nil, cap100)
    assertEquals(plan.discs.size, 2)
    val first = plan.discs(0)
    val last  = plan.discs(1)
    assertEquals(first.files.map(_.relPath.value), List("a"))
    assert(first.willFinalize, "spilled disc must be finalized")
    assertEquals(last.files.map(_.relPath.value), List("b"))
    assert(!last.willFinalize, "last disc stays open")
    assertEquals(last.ordinal, 2)

  test("existing OPEN disc is topped off before opening a new disc"):
    // open disc ordinal 1: 100 usable, 90 used -> 10 free
    val open = DiscState(1, DiscStatus.OPEN, usableBytes = 100L, usedBytes = 90L)
    val cs = changesOf(List(sf("a", 10), sf("b", 50)))
    val plan = Planner.plan(cs, discs = List(open), newDiscCap = cap100)
    assertEquals(plan.discs.size, 2)
    val topped = plan.discs(0)
    assertEquals(topped.ordinal, 1)
    assert(!topped.isNew)
    assertEquals(topped.files.map(_.relPath.value), List("a"))
    assert(topped.willFinalize)           // spilled -> finalized
    assertEquals(topped.totalUsedBytes, 100L)
    val fresh = plan.discs(1)
    assertEquals(fresh.ordinal, 2)
    assert(fresh.isNew)
    assertEquals(fresh.files.map(_.relPath.value), List("b"))
    assert(!fresh.willFinalize)

  test("deletions become tombstones and cost zero bytes"):
    val cs = ChangeSet(added = Nil, modified = Nil,
                       deleted = List(RelPath("x"), RelPath("y")), unchanged = Nil)
    val plan = Planner.plan(cs, Nil, cap100)
    assertEquals(plan.tombstones, List(RelPath("x"), RelPath("y")))
    assertEquals(plan.discs.flatMap(_.files), Nil)

  test("modified files are planned alongside added, by relPath"):
    val cs = ChangeSet(added = List(sf("a", 10)), modified = List(sf("b", 10)),
                       deleted = Nil, unchanged = Nil)
    val plan = Planner.plan(cs, Nil, cap100)
    assertEquals(plan.discs.head.files.map(_.relPath.value), List("a", "b"))

  test("a file larger than a fresh disc is reported oversized, not placed"):
    val cs = changesOf(List(sf("big", 150), sf("ok", 10)))
    val plan = Planner.plan(cs, Nil, cap100)
    assertEquals(plan.oversized.map(_.relPath.value), List("big"))
    assertEquals(plan.discs.flatMap(_.files).map(_.relPath.value), List("ok"))
