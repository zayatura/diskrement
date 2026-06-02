package diskrement.shell.catalog

import diskrement.core._
import diskrement.shell.model._
import zio._
import zio.test._
import java.time.Instant

object DiscStatesSpec extends ZIOSpecDefault:
  private val init = ProjectInit("p", "/src", "/work", DiscType.BD_R, 2.0, 1,
    Instant.parse("2026-06-01T00:00:00Z"))
  private val ts = Instant.parse("2026-06-01T10:00:00Z")

  private def fw(path: String, size: Long) =
    FileWrite(RelPath(path), ContentHash("h-" + path), size, ts, "/data/" + path, 0)

  def spec = suite("CatalogStore.readDiscStates")(
    test("derives usedBytes from placements and reports status"):
      val u = CatalogUpdate(ProjectId(0), Nil, List(
        DiscWrite(1, isNew = true, DiscType.BD_R, 100L, DiscStatus.FINALIZED, "d1",
          SessionWrite(1, ts, "m1", List(fw("a", 10), fw("b", 20)), Nil, Nil)),
        DiscWrite(2, isNew = true, DiscType.BD_R, 100L, DiscStatus.OPEN, "d2",
          SessionWrite(1, ts, "m2", List(fw("c", 10)), Nil, Nil))))
      for
        pid    <- ZIO.serviceWithZIO[CatalogStore](_.createProject(init))
        _      <- ZIO.serviceWithZIO[CatalogStore](_.applyUpdate(u.copy(projectId = pid)))
        states <- ZIO.serviceWithZIO[CatalogStore](_.readDiscStates(pid))
      yield assertTrue(
        states.map(_.ordinal) == List(1, 2),
        states.head == DiscState(1, DiscStatus.FINALIZED, 100L, 30L),
        states(1)   == DiscState(2, DiscStatus.OPEN, 100L, 10L))
  ).provideLayer(TestDb.layer)
