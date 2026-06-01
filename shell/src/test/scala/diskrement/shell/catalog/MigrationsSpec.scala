package diskrement.shell.catalog

import zio.jdbc._
import zio.test._

object MigrationsSpec extends ZIOSpecDefault:
  def spec = suite("Migrations")(
    test("fresh database migrates to version 1 with all six domain tables"):
      for
        _      <- transaction(Migrations.run)
        ver    <- transaction(sql"select max(version) from schema_version".query[Int].selectOne)
        tables <- transaction(
                    sql"select name from sqlite_master where type='table' order by name"
                      .query[String].selectAll)
      yield assertTrue(
        ver.contains(1),
        tables.contains("project"),
        tables.contains("disc"),
        tables.contains("session"),
        tables.contains("file_version"),
        tables.contains("placement"),
        tables.contains("change_record")
      ),
    test("running migrations twice is idempotent"):
      for
        _    <- transaction(Migrations.run)
        _    <- transaction(Migrations.run)
        rows <- transaction(sql"select version from schema_version".query[Int].selectAll)
      yield assertTrue(rows == zio.Chunk(1))
  ).provideLayer(TestDb.poolLayer)
