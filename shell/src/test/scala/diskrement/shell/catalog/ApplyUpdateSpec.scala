package diskrement.shell.catalog

import diskrement.core._
import diskrement.shell.model._
import zio._
import zio.jdbc._
import zio.test._
import java.time.Instant

object ApplyUpdateSpec extends ZIOSpecDefault:
  private val init = ProjectInit("p", "/src", "/work", DiscType.BD_R, 2.0, 1,
    Instant.parse("2026-06-01T00:00:00Z"))
  private val ts = Instant.parse("2026-06-01T10:00:00Z")

  private def file(path: String, size: Long) =
    FileWrite(RelPath(path), ContentHash("h-" + path), size, ts, "/data/" + path, 0)

  private def newDiscUpdate(pid: ProjectId) =
    CatalogUpdate(
      projectId = pid,
      finalizeDiscOrdinals = Nil,
      discs = List(DiscWrite(
        ordinal = 1, isNew = true, discType = DiscType.BD_R, usableBytes = 100L,
        status = DiscStatus.OPEN, label = "disc 1",
        session = SessionWrite(ordinal = 1, burnedAt = ts, manifestHash = "m1",
          added = List(file("a", 10), file("b", 20)), modified = Nil, deleted = Nil))))

  private def rowCount(q: SqlFragment): URIO[ZConnectionPool, Int] =
    transaction(q.query[Int].selectOne).map(_.getOrElse(0)).orDie

  def spec = suite("CatalogStore.applyUpdate")(
    test("writes discs, sessions, file_versions, placements and change_records atomically"):
      for
        pid <- ZIO.serviceWithZIO[CatalogStore](_.createProject(init))
        _   <- ZIO.serviceWithZIO[CatalogStore](_.applyUpdate(newDiscUpdate(pid)))
        d   <- rowCount(sql"select count(*) from disc")
        s   <- rowCount(sql"select count(*) from session")
        fv  <- rowCount(sql"select count(*) from file_version")
        pl  <- rowCount(sql"select count(*) from placement")
        cr  <- rowCount(sql"select count(*) from change_record")
      yield assertTrue(d == 1, s == 1, fv == 2, pl == 2, cr == 2),
    test("a failing applyUpdate rolls back every insert"):
      val bad = newDiscUpdate(ProjectId(0)).copy(discs =
        newDiscUpdate(ProjectId(0)).discs :+ DiscWrite(
          ordinal = 99, isNew = false, discType = DiscType.BD_R, usableBytes = 100L,
          status = DiscStatus.FINALIZED, label = "ghost",
          session = SessionWrite(2, ts, "m2", Nil, Nil, Nil)))
      for
        pid <- ZIO.serviceWithZIO[CatalogStore](_.createProject(init))
        err <- ZIO.serviceWithZIO[CatalogStore](_.applyUpdate(bad.copy(projectId = pid))).flip
        d   <- rowCount(sql"select count(*) from disc")
        s   <- rowCount(sql"select count(*) from session")
        fv  <- rowCount(sql"select count(*) from file_version")
        pl  <- rowCount(sql"select count(*) from placement")
        cr  <- rowCount(sql"select count(*) from change_record")
      yield assertTrue(err.isInstanceOf[CatalogError], d == 0, s == 0, fv == 0, pl == 0, cr == 0)
  ).provideLayer(TestDb.layer)
