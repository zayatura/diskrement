package diskrement.shell.catalog

import diskrement.core._
import diskrement.shell.model._
import zio._
import zio.jdbc._
import zio.test._
import java.time.Instant

object ReadViewSpec extends ZIOSpecDefault:
  private val init = ProjectInit("p", "/src", "/work", DiscType.BD_R, 2.0, 1,
    Instant.parse("2026-06-01T00:00:00Z"))
  private val ts = Instant.parse("2026-06-01T10:00:00Z")

  private def rowCount(q: SqlFragment): URIO[ZConnectionPool, Int] =
    transaction(q.query[Int].selectOne).map(_.getOrElse(0)).orDie

  private def fw(path: String, hash: String, size: Long) =
    FileWrite(RelPath(path), ContentHash(hash), size, ts, "/data/" + path, 0)

  private def session(ord: Int, added: List[FileWrite] = Nil,
                      modified: List[FileWrite] = Nil, deleted: List[RelPath] = Nil) =
    CatalogUpdate(ProjectId(0), Nil, List(DiscWrite(
      ordinal = 1, isNew = ord == 1, discType = DiscType.BD_R, usableBytes = 1_000_000L,
      status = DiscStatus.OPEN, label = "d1",
      session = SessionWrite(ord, ts, "m" + ord, added, modified, deleted))))

  def spec = suite("CatalogStore.readView")(
    test("added files appear; a modified file supersedes; a deleted file disappears"):
      for
        pid  <- ZIO.serviceWithZIO[CatalogStore](_.createProject(init))
        store = ZIO.serviceWithZIO[CatalogStore]
        _    <- store(_.applyUpdate(session(1,
                  added = List(fw("a", "a1", 10), fw("b", "b1", 20), fw("c", "c1", 30))).copy(projectId = pid)))
        _    <- store(_.applyUpdate(session(2,
                  modified = List(fw("b", "b2", 25)), deleted = List(RelPath("c"))).copy(projectId = pid)))
        view  <- store(_.readView(pid))
        cRows <- rowCount(sql"select count(*) from file_version where rel_path = 'c'")
      yield assertTrue(
        view.current.keySet.map(_.value) == Set("a", "b"),
        view.current(RelPath("a")).contentHash == ContentHash("a1"),
        view.current(RelPath("b")).contentHash == ContentHash("b2"),
        !view.current.contains(RelPath("c")),
        cRows >= 1)
  ).provideLayer(TestDb.layer)
