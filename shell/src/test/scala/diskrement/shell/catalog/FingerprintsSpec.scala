package diskrement.shell.catalog

import diskrement.core._
import diskrement.shell.model._
import zio._
import zio.test._
import java.time.Instant

object FingerprintsSpec extends ZIOSpecDefault:
  private val init = ProjectInit("p", "/src", "/work", DiscType.BD_R, 2.0, 1,
    Instant.parse("2026-06-01T00:00:00Z"))
  private val ts = Instant.parse("2026-06-01T10:00:00Z")

  private def fw(path: String, hash: String, size: Long, mt: Instant) =
    FileWrite(RelPath(path), ContentHash(hash), size, mt, "/data/" + path, 0)

  private def session(ord: Int, added: List[FileWrite] = Nil,
                      modified: List[FileWrite] = Nil, deleted: List[RelPath] = Nil) =
    CatalogUpdate(ProjectId(0), Nil, List(DiscWrite(
      ordinal = 1, isNew = ord == 1, discType = DiscType.BD_R, usableBytes = 1_000_000L,
      status = DiscStatus.OPEN, label = "d1",
      session = SessionWrite(ord, ts, "m" + ord, added, modified, deleted))))

  def spec = suite("CatalogStore.fingerprints")(
    test("current version per path with mtime; modified supersedes; deleted excluded"):
      val mtA  = Instant.parse("2026-05-01T00:00:00Z")
      val mtB1 = Instant.parse("2026-05-02T00:00:00Z")
      val mtB2 = Instant.parse("2026-05-03T00:00:00Z")
      val mtC  = Instant.parse("2026-05-04T00:00:00Z")
      for
        pid <- ZIO.serviceWithZIO[CatalogStore](_.createProject(init))
        st   = ZIO.serviceWithZIO[CatalogStore]
        _   <- st(_.applyUpdate(session(1, added =
                 List(fw("a", "a1", 10, mtA), fw("b", "b1", 20, mtB1), fw("c", "c1", 30, mtC)))
                 .copy(projectId = pid)))
        _   <- st(_.applyUpdate(session(2,
                 modified = List(fw("b", "b2", 25, mtB2)), deleted = List(RelPath("c")))
                 .copy(projectId = pid)))
        fps <- st(_.fingerprints(pid))
      yield assertTrue(
        fps.keySet.map(_.value) == Set("a", "b"),
        fps(RelPath("a")) == FileFingerprint(10, mtA, ContentHash("a1")),
        fps(RelPath("b")) == FileFingerprint(25, mtB2, ContentHash("b2")),
        !fps.contains(RelPath("c")))
  ).provideLayer(TestDb.layer)
