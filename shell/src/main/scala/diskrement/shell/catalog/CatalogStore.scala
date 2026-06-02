package diskrement.shell.catalog

import diskrement.core.{CatalogView, DiscState, RelPath}
import diskrement.shell.model._
import zio._
import zio.jdbc._
import java.nio.file.Path

/** SQLite-backed persistence port for the catalog. Grows across Plan 2a tasks;
  * currently it creates and finds projects. */
trait CatalogStore:
  def createProject(p: ProjectInit): IO[CatalogError, ProjectId]
  def findProject(name: String):     IO[CatalogError, Option[Project]]
  def applyUpdate(update: CatalogUpdate): IO[CatalogError, Unit]
  def readView(project: ProjectId): IO[CatalogError, CatalogView]
  def readDiscStates(project: ProjectId): IO[CatalogError, List[DiscState]]
  def sessionsByState(project: ProjectId, state: SessionState): IO[CatalogError, List[Session]]
  def fingerprints(project: ProjectId): IO[CatalogError, Map[RelPath, FileFingerprint]]
  /** Write a consistent full-database snapshot to `dest` via SQLite `VACUUM INTO`
    * (run outside a transaction). `dest` must NOT already exist — `VACUUM INTO`
    * fails if it does. The destination filename is passed as a bound parameter,
    * which sqlite-jdbc supports. */
  def snapshotTo(dest: Path): IO[CatalogError, Unit]
  def archivePointers(project: ProjectId): IO[CatalogError, List[ArchivePointer]]

object CatalogStore:
  /** Build the store from a pool, running migrations once at layer construction. */
  val live: ZLayer[ZConnectionPool, Throwable, CatalogStore] =
    ZLayer {
      for
        pool <- ZIO.service[ZConnectionPool]
        _    <- transaction(
                  ZIO.serviceWithZIO[ZConnection](_.access(_.setAutoCommit(false))) *> Migrations.run)
                  .provideEnvironment(ZEnvironment(pool))
                  .mapError(CatalogError.MigrationFailed(_))
      yield CatalogStoreLive(pool)
    }
