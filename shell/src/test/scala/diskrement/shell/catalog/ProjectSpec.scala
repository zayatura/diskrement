package diskrement.shell.catalog

import diskrement.core.DiscType
import diskrement.shell.model._
import zio._
import zio.test._
import java.time.Instant

object ProjectSpec extends ZIOSpecDefault:
  private val init = ProjectInit(
    name = "photos", sourceRoot = "/src", workDir = "/work",
    defaultDiscType = DiscType.BD_R, marginPercent = 2.0, copies = 1,
    createdAt = Instant.parse("2026-06-01T00:00:00Z"))

  def spec = suite("CatalogStore projects")(
    test("createProject then findProject round-trips"):
      for
        id  <- ZIO.serviceWithZIO[CatalogStore](_.createProject(init))
        got <- ZIO.serviceWithZIO[CatalogStore](_.findProject("photos"))
      yield assertTrue(
        got.exists(p => p.id == id && p.name == "photos" && p.defaultDiscType == DiscType.BD_R)),
    test("createProject on a duplicate name fails with DuplicateProject"):
      for
        _   <- ZIO.serviceWithZIO[CatalogStore](_.createProject(init))
        err <- ZIO.serviceWithZIO[CatalogStore](_.createProject(init)).flip
      yield assertTrue(err == CatalogError.DuplicateProject("photos")),
    test("findProject returns None for an unknown name"):
      for got <- ZIO.serviceWithZIO[CatalogStore](_.findProject("nope"))
      yield assertTrue(got.isEmpty)
  ).provideLayer(TestDb.layer)
