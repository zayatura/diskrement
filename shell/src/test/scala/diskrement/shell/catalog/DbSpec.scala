package diskrement.shell.catalog

import zio.jdbc._
import zio.test._

object DbSpec extends ZIOSpecDefault:
  def spec = suite("Db.poolLayer")(
    test("opens SQLite and applies WAL + foreign_keys pragmas"):
      for
        jm  <- transaction(sql"pragma journal_mode".query[String].selectOne)
        fk  <- transaction(sql"pragma foreign_keys".query[Int].selectOne)
        sy  <- transaction(sql"pragma synchronous".query[Int].selectOne)
        one <- transaction(sql"select 1".query[Int].selectOne)
      yield assertTrue(jm.contains("wal"), fk.contains(1), sy.contains(2), one.contains(1))
  ).provideLayer(TestDb.poolLayer)
