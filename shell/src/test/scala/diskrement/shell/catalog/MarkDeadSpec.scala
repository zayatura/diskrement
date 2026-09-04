package diskrement.shell.catalog

import diskrement.core._
import diskrement.shell.model._
import zio._
import zio.jdbc._
import zio.test._
import java.time.Instant

object MarkDeadSpec extends ZIOSpecDefault:
  private val init = ProjectInit("p", "/src", "/work", DiscType.BD_R, 2.0, 1,
    Instant.parse("2026-06-01T00:00:00Z"))
  private val ts      = Instant.parse("2026-06-01T10:00:00Z")
  private val deadAt  = Instant.parse("2026-06-02T09:00:00Z")
  private val deadAt2 = Instant.parse("2026-06-03T09:00:00Z")

  private def fw(path: String, hash: String, size: Long) =
    FileWrite(RelPath(path), ContentHash(hash), size, ts, "/data/" + path, 0)

  private def session(disc: Int, ord: Int, added: List[FileWrite] = Nil,
                      modified: List[FileWrite] = Nil, deleted: List[RelPath] = Nil) =
    CatalogUpdate(ProjectId(0), Nil, List(DiscWrite(
      ordinal = disc, isNew = ord == 1, discType = DiscType.BD_R, usableBytes = 1_000_000L,
      status = DiscStatus.OPEN, label = s"d$disc",
      session = SessionWrite(ord, ts, s"m$disc-$ord", added, modified, deleted))))

  /** Disc 1: session 1 adds a,b; session 2 modifies b — so b@b1 is superseded in place. */
  private def seed = for
    pid  <- ZIO.serviceWithZIO[CatalogStore](_.createProject(init))
    store = ZIO.serviceWithZIO[CatalogStore]
    _    <- store(_.applyUpdate(session(1, 1,
              added = List(fw("a", "a1", 10), fw("b", "b1", 20))).copy(projectId = pid)))
    _    <- store(_.applyUpdate(session(1, 2,
              modified = List(fw("b", "b2", 25))).copy(projectId = pid)))
  yield pid

  /** Every recorded casualty, as (relPath, fate). */
  private val fateRows =
    transaction(
      sql"""select fv.rel_path, lv.fate from lost_version lv
            join file_version fv on fv.id = lv.file_version_id"""
        .query[(String, String)].selectAll).map(_.toList)

  /** Put a's version on disc 2 as well, with a raw extra placement — exactly what
    * copies>1 will eventually do for real. */
  private val alsoPlaceAOnDisc2 =
    transaction(
      sql"""insert into placement (file_version_id, session_id, disc_path, copy_index)
            select (select id from file_version where rel_path = 'a'),
                   (select s.id from session s join disc d on d.id = s.disc_id
                    where d.ordinal = 2),
                   '/data/a', 1""".execute)

  def spec = suite("CatalogStore.markDead")(
    test("splits the disc's versions into lost (superseded) and reburn (current), records both"):
      for
        pid    <- seed
        rep    <- ZIO.serviceWithZIO[CatalogStore](_.markDead(pid, 1, deadAt))
        states <- ZIO.serviceWithZIO[CatalogStore](_.readDiscStates(pid))
        fates  <- transaction(
                    sql"""select fv.rel_path, lv.fate from lost_version lv
                          join file_version fv on fv.id = lv.file_version_id"""
                      .query[(String, String)].selectAll)
      yield assertTrue(
        rep.discOrdinal == 1,
        rep.lost.map(e => (e.relPath.value, e.contentHash.value, e.sessionOrdinal)) ==
          List(("b", "b1", 1)),
        rep.reburn.map(e => (e.relPath.value, e.contentHash.value)).toSet ==
          Set(("a", "a1"), ("b", "b2")),
        states == List(DiscState(1, DiscStatus.DEAD, 1_000_000L, 55L)),
        fates.toList.sorted == List(("a", "REBURN"), ("b", "LOST"), ("b", "REBURN")))
    ,
    test("a version that also lives on another live disc is neither lost nor reburned"):
      for
        pid   <- seed
        // disc 2 exists via its own file; then wire a's version onto it too
        _     <- ZIO.serviceWithZIO[CatalogStore](_.applyUpdate(session(2, 1,
                   added = List(fw("c", "c1", 5))).copy(projectId = pid)))
        _     <- alsoPlaceAOnDisc2
        rep   <- ZIO.serviceWithZIO[CatalogStore](_.markDead(pid, 1, deadAt))
        fates <- fateRows
      yield assertTrue(
        rep.reburn.map(_.relPath.value) == List("b"),
        rep.lost.map(_.relPath.value) == List("b"),
        // nothing was recorded against a: its bytes still exist on live disc 2
        fates.sorted == List(("b", "LOST"), ("b", "REBURN")))
    ,
    test("a copy on an already-dead disc no longer shields a version"):
      for
        pid  <- seed
        _    <- ZIO.serviceWithZIO[CatalogStore](_.applyUpdate(session(2, 1,
                  added = List(fw("c", "c1", 5))).copy(projectId = pid)))
        _    <- alsoPlaceAOnDisc2
        rep2 <- ZIO.serviceWithZIO[CatalogStore](_.markDead(pid, 2, deadAt))
        rep1 <- ZIO.serviceWithZIO[CatalogStore](_.markDead(pid, 1, deadAt2))
      yield assertTrue(
        // while disc 1 lived, a's copy there kept it off disc 2's casualty list
        rep2.reburn.map(_.relPath.value) == List("c"),
        rep2.lost.isEmpty,
        // now that disc 2 is dead too, a's only surviving copy is gone
        rep1.reburn.map(e => (e.relPath.value, e.contentHash.value)) ==
          List(("a", "a1"), ("b", "b2")),
        rep1.lost.map(_.relPath.value) == List("b"))
    ,
    test("a path whose latest change is DELETED is lost, never reburned"):
      for
        pid   <- seed
        _     <- ZIO.serviceWithZIO[CatalogStore](_.applyUpdate(
                   session(1, 3, deleted = List(RelPath("a"))).copy(projectId = pid)))
        rep   <- ZIO.serviceWithZIO[CatalogStore](_.markDead(pid, 1, deadAt))
        fates <- fateRows
      yield assertTrue(
        rep.lost.map(e => (e.relPath.value, e.contentHash.value, e.sessionOrdinal)) ==
          List(("a", "a1", 1), ("b", "b1", 1)),
        rep.reburn.map(e => (e.relPath.value, e.contentHash.value)) == List(("b", "b2")),
        fates.sorted == List(("a", "LOST"), ("b", "LOST"), ("b", "REBURN")))
    ,
    test("several placements of one version on the dying disc make one casualty"):
      for
        pid   <- seed
        // b@b2 was burned in session 2; pretend a second copy also landed in session 1
        _     <- transaction(
                   sql"""insert into placement (file_version_id, session_id, disc_path, copy_index)
                         select (select max(id) from file_version where rel_path = 'b'),
                                (select s.id from session s join disc d on d.id = s.disc_id
                                 where d.ordinal = 1 and s.ordinal = 1),
                                '/data/b', 1""".execute)
        rep   <- ZIO.serviceWithZIO[CatalogStore](_.markDead(pid, 1, deadAt))
        fates <- fateRows
      yield assertTrue(
        rep.reburn.map(e => (e.relPath.value, e.contentHash.value)) ==
          List(("a", "a1"), ("b", "b2")),
        rep.lost.map(_.relPath.value) == List("b"),
        fates.sorted == List(("a", "REBURN"), ("b", "LOST"), ("b", "REBURN")))
    ,
    test("marking a dead disc again fails; unknown ordinal fails"):
      for
        pid <- seed
        _   <- ZIO.serviceWithZIO[CatalogStore](_.markDead(pid, 1, deadAt))
        dup <- ZIO.serviceWithZIO[CatalogStore](_.markDead(pid, 1, deadAt)).exit
        bad <- ZIO.serviceWithZIO[CatalogStore](_.markDead(pid, 9, deadAt)).exit
      yield assertTrue(
        dup.causeOption.flatMap(_.failureOption)
          .exists(_.isInstanceOf[CatalogError.IllegalStatusTransition]),
        bad.causeOption.flatMap(_.failureOption)
          .exists(_.isInstanceOf[CatalogError.ConstraintViolation]))
    ,
    test("readView drops current versions whose only copy is on a dead disc"):
      for
        pid    <- seed
        before <- ZIO.serviceWithZIO[CatalogStore](_.readView(pid))
        _      <- ZIO.serviceWithZIO[CatalogStore](_.markDead(pid, 1, deadAt))
        after  <- ZIO.serviceWithZIO[CatalogStore](_.readView(pid))
      yield assertTrue(
        before.current.keySet.map(_.value) == Set("a", "b"),
        after.current.isEmpty)
    ,
    test("fileHistory flags versions on a dead disc as LOST; the current version stays CURRENT"):
      for
        pid <- seed
        _   <- ZIO.serviceWithZIO[CatalogStore](_.markDead(pid, 1, deadAt))
        hb  <- ZIO.serviceWithZIO[CatalogStore](_.fileHistory(pid, RelPath("b")))
      yield assertTrue(hb.map(_.status) == List(VersionStatus.LOST, VersionStatus.CURRENT))
    ,
    test("a current version with a surviving copy on a live disc stays in readView"):
      for
        pid  <- seed
        // disc 2 exists via its own file; then wire a's version onto it too
        _    <- ZIO.serviceWithZIO[CatalogStore](_.applyUpdate(session(2, 1,
                  added = List(fw("c", "c1", 5))).copy(projectId = pid)))
        _    <- alsoPlaceAOnDisc2
        _    <- ZIO.serviceWithZIO[CatalogStore](_.markDead(pid, 1, deadAt))
        view <- ZIO.serviceWithZIO[CatalogStore](_.readView(pid))
      yield assertTrue(
        // a survives on live disc 2; b@b2 is disc-1-only and drops; c is native to disc 2
        view.current.keySet.map(_.value) == Set("a", "c"))
    ,
    test("a current version on a dead disc still renders CURRENT, not LOST"):
      for
        pid <- seed
        _   <- ZIO.serviceWithZIO[CatalogStore](_.markDead(pid, 1, deadAt))
        ha  <- ZIO.serviceWithZIO[CatalogStore](_.fileHistory(pid, RelPath("a")))
      yield assertTrue(
        // a@a1 is both a's only version and its current one, and disc 1 (its only copy)
        // is dead: is_current wins over disc_dead, per the plan's Step 5 note — the
        // source still has the file and it is about to re-burn, so it stays CURRENT.
        ha.map(_.status) == List(VersionStatus.CURRENT))
  ).provideLayer(TestDb.layer)
