package diskrement.shell.catalog

import zio._
import zio.jdbc._
import java.nio.file.{Files, Path}
import scala.util.Using

/** Per-test SQLite database in a temp directory, deleted on release. */
object TestDb:
  val pathLayer: ZLayer[Any, Throwable, DbPath] =
    ZLayer.scoped {
      ZIO.acquireRelease(
        ZIO.attempt {
          val dir = Files.createTempDirectory("diskrement-test")
          DbPath(dir.resolve("catalog.sqlite"))
        }
      ) { dbPath =>
        ZIO.attempt {
          val dir = dbPath.value.getParent
          Using.resource(Files.list(dir))(_.forEach(f => Files.deleteIfExists(f)))
          Files.deleteIfExists(dir)
        }.orDie
      }
    }

  /** Just the connection pool (for tests that only run raw SQL). */
  val poolLayer: ZLayer[Any, Throwable, ZConnectionPool] =
    pathLayer >>> Db.poolLayer

  /** Pool + migrated CatalogStore, both visible to tests (>+> keeps the pool). */
  val layer: ZLayer[Any, Throwable, ZConnectionPool & CatalogStore] =
    pathLayer >>> (Db.poolLayer >+> CatalogStore.live)
