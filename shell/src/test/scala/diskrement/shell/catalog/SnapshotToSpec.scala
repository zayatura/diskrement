package diskrement.shell.catalog

import diskrement.core._
import diskrement.shell.model._
import zio._
import zio.test._
import java.nio.file.Files
import java.time.Instant

object SnapshotToSpec extends ZIOSpecDefault:
  private val init = ProjectInit("p", "/src", "/work", DiscType.BD_R, 2.0, 1,
    Instant.parse("2026-06-01T00:00:00Z"))
  private val ts = Instant.parse("2026-06-01T10:00:00Z")
  private def fw(path: String, hash: String, size: Long) =
    FileWrite(RelPath(path), ContentHash(hash), size, ts, "/data/" + path, 0)
  private val u = CatalogUpdate(ProjectId(0), Nil, List(DiscWrite(
    1, isNew = true, DiscType.BD_R, 100L, DiscStatus.OPEN, "d1",
    SessionWrite(1, ts, "m1", List(fw("a", "a1", 10), fw("b", "b1", 20)), Nil, Nil))))

  private def deleteRecursive(dir: java.nio.file.Path): Unit =
    val s = Files.walk(dir)
    try s.sorted(java.util.Comparator.reverseOrder()).forEach(p => Files.deleteIfExists(p))
    finally s.close()

  def spec = suite("CatalogStore.snapshotTo")(
    test("VACUUM INTO produces a self-contained DB with the source's rows"):
      ZIO.scoped {
        for
          tmpDir <- ZIO.acquireRelease(ZIO.attempt(Files.createTempDirectory("snap-to")))(
                      d => ZIO.attempt(deleteRecursive(d)).orDie)
          dest = tmpDir.resolve("snapshot.sqlite")
          pid  <- ZIO.serviceWithZIO[CatalogStore](_.createProject(init))
          _    <- ZIO.serviceWithZIO[CatalogStore](_.applyUpdate(u.copy(projectId = pid)))
          _    <- ZIO.serviceWithZIO[CatalogStore](_.snapshotTo(dest))
          exists <- ZIO.attempt(java.nio.file.Files.exists(dest))
          count  <- ZIO.attempt {
                      val c = java.sql.DriverManager.getConnection(s"jdbc:sqlite:$dest")
                      try
                        val rs = c.createStatement().executeQuery("select count(*) from file_version")
                        rs.next(); rs.getInt(1)
                      finally c.close()
                    }
        yield assertTrue(exists, count == 2)
      }
  ).provideLayer(TestDb.layer)
