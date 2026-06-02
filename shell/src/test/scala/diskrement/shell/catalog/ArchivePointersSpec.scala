package diskrement.shell.catalog

import diskrement.core._
import diskrement.shell.model._
import zio._
import zio.test._
import java.time.Instant

object ArchivePointersSpec extends ZIOSpecDefault:
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

  def spec = suite("CatalogStore.archivePointers")(
    test("returns superseded + deleted versions with locations; excludes live"):
      for
        pid <- ZIO.serviceWithZIO[CatalogStore](_.createProject(init))
        st   = ZIO.serviceWithZIO[CatalogStore]
        _   <- st(_.applyUpdate(session(1,
                 added = List(fw("a","a1",10), fw("b","b1",20), fw("c","c1",30))).copy(projectId = pid)))
        _   <- st(_.applyUpdate(session(2,
                 modified = List(fw("b","b2",25)), deleted = List(RelPath("c"))).copy(projectId = pid)))
        ps  <- st(_.archivePointers(pid))
      yield assertTrue(
        ps.toSet == Set(
          ArchivePointer(RelPath("b"), ContentHash("b1"), 20, 1, 1, "/data/b", ManifestChangeKind.ADDED),
          ArchivePointer(RelPath("c"), ContentHash("c1"), 30, 1, 1, "/data/c", ManifestChangeKind.DELETED)))
  ).provideLayer(TestDb.layer)
