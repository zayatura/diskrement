package diskrement.shell.catalog

import diskrement.core._
import diskrement.shell.model._
import zio._
import zio.test._
import java.time.Instant

object SessionsByStateSpec extends ZIOSpecDefault:
  private val init = ProjectInit("p", "/src", "/work", DiscType.BD_R, 2.0, 1,
    Instant.parse("2026-06-01T00:00:00Z"))
  private val ts = Instant.parse("2026-06-01T10:00:00Z")

  private val u = CatalogUpdate(ProjectId(0), Nil, List(
    DiscWrite(1, isNew = true, DiscType.BD_R, 100L, DiscStatus.OPEN, "d1",
      SessionWrite(1, ts, "m1",
        List(FileWrite(RelPath("a"), ContentHash("h"), 10L, ts, "/data/a", 0)), Nil, Nil))))

  def spec = suite("CatalogStore.sessionsByState")(
    test("applyUpdate records sessions as BURNED; PLANNED is empty"):
      for
        pid     <- ZIO.serviceWithZIO[CatalogStore](_.createProject(init))
        _       <- ZIO.serviceWithZIO[CatalogStore](_.applyUpdate(u.copy(projectId = pid)))
        burned  <- ZIO.serviceWithZIO[CatalogStore](_.sessionsByState(pid, SessionState.BURNED))
        planned <- ZIO.serviceWithZIO[CatalogStore](_.sessionsByState(pid, SessionState.PLANNED))
      yield assertTrue(
        burned.map(s => (s.discOrdinal, s.ordinal, s.state)) == List((1, 1, SessionState.BURNED)),
        planned.isEmpty)
  ).provideLayer(TestDb.layer)
