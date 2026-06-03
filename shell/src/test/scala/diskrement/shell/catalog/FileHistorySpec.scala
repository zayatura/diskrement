package diskrement.shell.catalog

import diskrement.core._
import diskrement.shell.model._
import zio._
import zio.test._
import java.time.Instant

object FileHistorySpec extends ZIOSpecDefault:
  private val init = ProjectInit("p", "/src", "/work", DiscType.BD_R, 2.0, 1,
    Instant.parse("2026-06-01T00:00:00Z"))
  private val ts = Instant.parse("2026-06-01T10:00:00Z")
  private def fw(path: String, hash: String, size: Long) =
    FileWrite(RelPath(path), ContentHash(hash), size, ts, "/data/" + path, 0)
  private def session(ord: Int, added: List[FileWrite] = Nil,
                      modified: List[FileWrite] = Nil, deleted: List[RelPath] = Nil) =
    CatalogUpdate(ProjectId(0), Nil, List(DiscWrite(
      1, isNew = ord == 1, DiscType.BD_R, 1_000_000L, DiscStatus.OPEN, "d1",
      SessionWrite(ord, ts, "m" + ord, added, modified, deleted))))

  def spec = suite("CatalogStore.fileHistory")(
    test("orders versions oldest->newest with CURRENT/SUPERSEDED/DELETED status"):
      for
        pid <- ZIO.serviceWithZIO[CatalogStore](_.createProject(init))
        st   = ZIO.serviceWithZIO[CatalogStore]
        _   <- st(_.applyUpdate(session(1, added = List(fw("a","a1",10), fw("b","b1",20))).copy(projectId = pid)))
        _   <- st(_.applyUpdate(session(2, modified = List(fw("a","a2",15))).copy(projectId = pid)))
        _   <- st(_.applyUpdate(session(3, deleted = List(RelPath("b"))).copy(projectId = pid)))
        ha  <- st(_.fileHistory(pid, RelPath("a")))
        hb  <- st(_.fileHistory(pid, RelPath("b")))
      yield assertTrue(
        ha == List(
          FileHistoryEntry(ContentHash("a1"), 10, 1, 1, VersionStatus.SUPERSEDED),
          FileHistoryEntry(ContentHash("a2"), 15, 1, 2, VersionStatus.CURRENT)),
        hb == List(
          FileHistoryEntry(ContentHash("b1"), 20, 1, 1, VersionStatus.DELETED)))
  ).provideLayer(TestDb.layer)
