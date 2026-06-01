package diskrement.core

import org.scalacheck.Gen
import org.scalacheck.Prop.forAll

class PlannerPropertyTest extends munit.ScalaCheckSuite:

  private def placeable(cs: ChangeSet): List[ScannedFile] =
    cs.added ++ cs.modified

  property("1. conservation: every placeable file is placed or oversized, exactly once"):
    forAll(Generators.genPlannerInput): (cs, discs, cap) =>
      val plan = Planner.plan(cs, discs, cap)
      val placedPaths = plan.discs.flatMap(_.files).map(_.relPath)
      val oversizedPaths = plan.oversized.map(_.relPath)
      val all = placedPaths ++ oversizedPaths
      val expected = placeable(cs).map(_.relPath)
      all.sortBy(_.value) == expected.sortBy(_.value) &&
        all.distinct.size == all.size

  property("2. capacity: no disc exceeds its usable capacity"):
    forAll(Generators.genPlannerInput): (cs, discs, cap) =>
      val plan = Planner.plan(cs, discs, cap)
      plan.discs.forall(d => d.totalUsedBytes <= d.usableBytes)

  property("3. no split: each placed file appears on exactly one disc"):
    forAll(Generators.genPlannerInput): (cs, discs, cap) =>
      val plan = Planner.plan(cs, discs, cap)
      val placed = plan.discs.flatMap(_.files).map(_.relPath)
      placed.distinct.size == placed.size

  property("4. order: placed files are in lexicographic relPath order across discs"):
    forAll(Generators.genPlannerInput): (cs, discs, cap) =>
      val plan = Planner.plan(cs, discs, cap)
      val placed = plan.discs.flatMap(_.files).map(_.relPath.value)
      placed == placed.sorted

  property("5. determinism: any permutation or redistribution of inputs yields an identical plan"):
    forAll(Generators.genPlannerInput, Gen.long): (input, seed) =>
      val (cs, discs, cap) = input
      val rng = new scala.util.Random(seed)
      // Arbitrarily reorder all placeable files and re-split them across the
      // added/modified buckets — the plan must be invariant to both.
      val combined = rng.shuffle(cs.added ++ cs.modified)
      val k = if combined.isEmpty then 0 else rng.nextInt(combined.size + 1)
      val (add2, mod2) = combined.splitAt(k)
      val reshuffled = cs.copy(added = add2, modified = mod2)
      Planner.plan(cs, discs, cap) == Planner.plan(reshuffled, discs, cap)

  property("6. open-disc-first: if an open disc exists, it is the first planned disc"):
    forAll(Generators.genPlannerInput): (cs, discs, cap) =>
      val plan = Planner.plan(cs, discs, cap)
      discs.find(_.status == DiscStatus.OPEN) match
        case Some(open) if plan.discs.nonEmpty =>
          val first = plan.discs.head
          first.ordinal == open.ordinal && !first.isNew
        case _ => true

  property("7. finalize: at most one planned disc stays open (the last)"):
    forAll(Generators.genPlannerInput): (cs, discs, cap) =>
      val plan = Planner.plan(cs, discs, cap)
      val openCount = plan.discs.count(!_.willFinalize)
      openCount <= 1 &&
        (plan.discs.isEmpty || !plan.discs.last.willFinalize)

  property("8. tombstones cost nothing: deletions add no bytes to any disc"):
    forAll(Generators.genPlannerInput): (cs, discs, cap) =>
      val withDeletes = cs.copy(deleted = List(RelPath("zzz/deleted")))
      val a = Planner.plan(cs, discs, cap).discs.map(_.addedBytes).sum
      val b = Planner.plan(withDeletes, discs, cap).discs.map(_.addedBytes).sum
      a == b
