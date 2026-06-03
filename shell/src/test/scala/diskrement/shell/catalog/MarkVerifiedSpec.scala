package diskrement.shell.catalog

import diskrement.core._
import diskrement.shell.model._
import zio._
import zio.test._
import java.time.Instant

object MarkVerifiedSpec extends ZIOSpecDefault:
  private val init = ProjectInit("p", "/src", "/work", DiscType.BD_R, 2.0, 1,
    Instant.parse("2026-06-01T00:00:00Z"))
  private val ts = Instant.parse("2026-06-01T10:00:00Z")
  private val u = CatalogUpdate(ProjectId(0), Nil, List(DiscWrite(
    1, isNew = true, DiscType.BD_R, 100L, DiscStatus.OPEN, "d1",
    SessionWrite(1, ts, "m1",
      List(FileWrite(RelPath("a"), ContentHash("h"), 10L, ts, "/data/a", 0)), Nil, Nil))))

  def spec = suite("CatalogStore.markVerified")(
    test("flips a BURNED session to VERIFIED"):
      for
        pid <- ZIO.serviceWithZIO[CatalogStore](_.createProject(init))
        _   <- ZIO.serviceWithZIO[CatalogStore](_.applyUpdate(u.copy(projectId = pid)))
        _   <- ZIO.serviceWithZIO[CatalogStore](_.markVerified(pid, 1, 1))
        v   <- ZIO.serviceWithZIO[CatalogStore](_.sessionsByState(pid, SessionState.VERIFIED))
        b   <- ZIO.serviceWithZIO[CatalogStore](_.sessionsByState(pid, SessionState.BURNED))
      yield assertTrue(
        v.map(s => (s.discOrdinal, s.ordinal)) == List((1, 1)),
        b.isEmpty),
    test("marking a non-BURNED session fails with IllegalStatusTransition"):
      for
        pid <- ZIO.serviceWithZIO[CatalogStore](_.createProject(init))
        err <- ZIO.serviceWithZIO[CatalogStore](_.markVerified(pid, 1, 1)).flip
      yield assertTrue(err.isInstanceOf[CatalogError.IllegalStatusTransition])
  ).provideLayer(TestDb.layer)
