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
  private val ts     = Instant.parse("2026-06-01T10:00:00Z")
  private val deadAt = Instant.parse("2026-06-02T09:00:00Z")

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
        pid <- seed
        // disc 2 exists via its own file; then wire a's version onto it with a raw
        // extra placement — exactly what copies>1 will eventually do for real
        _   <- ZIO.serviceWithZIO[CatalogStore](_.applyUpdate(session(2, 1,
                 added = List(fw("c", "c1", 5))).copy(projectId = pid)))
        _   <- transaction(
                 sql"""insert into placement (file_version_id, session_id, disc_path, copy_index)
                       select (select id from file_version where rel_path = 'a'),
                              (select s.id from session s join disc d on d.id = s.disc_id
                               where d.ordinal = 2),
                              '/data/a', 1""".execute)
        rep <- ZIO.serviceWithZIO[CatalogStore](_.markDead(pid, 1, deadAt))
      yield assertTrue(
        rep.reburn.map(_.relPath.value) == List("b"),
        rep.lost.map(_.relPath.value) == List("b"))
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
  ).provideLayer(TestDb.layer)
