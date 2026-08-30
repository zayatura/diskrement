package diskrement.shell.catalog

import diskrement.core._
import diskrement.shell.model._
import zio._
import zio.test._
import java.time.Instant

object DiscRecordedContentsSpec extends ZIOSpecDefault:
  private val init = ProjectInit("p", "/src", "/work", DiscType.BD_R, 2.0, 1,
    Instant.parse("2026-08-30T00:00:00Z"))
  private val ts = Instant.parse("2026-08-30T10:00:00Z")
  private def fw(path: String, hash: String, size: Long) =
    FileWrite(RelPath(path), ContentHash(hash), size, ts, "/data/" + path, 0)
  private def session(discOrd: Int, ord: Int, added: List[FileWrite] = Nil,
                      modified: List[FileWrite] = Nil) =
    CatalogUpdate(ProjectId(0), Nil, List(DiscWrite(
      discOrd, isNew = ord == 1, DiscType.BD_R, 1_000_000L, DiscStatus.OPEN, s"d$discOrd",
      SessionWrite(ord, ts, "m" + ord, added, modified, Nil))))

  def spec = suite("CatalogStore.discRecordedContents")(
    test("merged view hides same-path earlier versions; superseded lists them with their session"):
      for
        pid <- ZIO.serviceWithZIO[CatalogStore](_.createProject(init))
        st   = ZIO.serviceWithZIO[CatalogStore]
        _   <- st(_.applyUpdate(session(1, 1,
                 added = List(fw("a.txt", "aa", 10), fw("b.txt", "bb", 20))).copy(projectId = pid)))
        _   <- st(_.applyUpdate(session(1, 2,
                 modified = List(fw("a.txt", "a2", 12))).copy(projectId = pid)))
        rec <- st(_.discRecordedContents(pid, 1))
      yield assertTrue(
        rec.visible == Map(RelPath("a.txt") -> ContentHash("a2"), RelPath("b.txt") -> ContentHash("bb")),
        rec.superseded == List(SupersededVersion(1, RelPath("a.txt"), ContentHash("aa")))),
    test("versions on other discs do not leak in; a fresh disc has no superseded"):
      for
        pid <- ZIO.serviceWithZIO[CatalogStore](_.createProject(init))
        st   = ZIO.serviceWithZIO[CatalogStore]
        _   <- st(_.applyUpdate(session(1, 1, added = List(fw("a.txt", "aa", 10))).copy(projectId = pid)))
        _   <- st(_.applyUpdate(session(2, 1, added = List(fw("z.txt", "zz", 30))).copy(projectId = pid)))
        r1  <- st(_.discRecordedContents(pid, 1))
        r2  <- st(_.discRecordedContents(pid, 2))
      yield assertTrue(
        r1 == DiscRecordedContents(Map(RelPath("a.txt") -> ContentHash("aa")), Nil),
        r2 == DiscRecordedContents(Map(RelPath("z.txt") -> ContentHash("zz")), Nil))
  ).provideLayer(TestDb.layer)
