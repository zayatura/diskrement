package diskrement.shell.catalog

import diskrement.core.{CatalogView, ContentHash, DiscState, RelPath}
import diskrement.shell.model._
import zio._
import zio.jdbc._
import java.nio.file.Path
import java.time.Instant

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
  def fileHistory(project: ProjectId, relPath: RelPath): IO[CatalogError, List[FileHistoryEntry]]
  def markVerified(project: ProjectId, discOrdinal: Int, sessionOrdinal: Int): IO[CatalogError, Unit]
  /** The `relPath -> contentHash` of file versions placed in `discOrdinal`'s LATEST
    * session — i.e. exactly what the (simulated) burner stages on that disc. Used by
    * verify to validate a disc against its own recorded contents, not the current view. */
  def placementsForDisc(project: ProjectId, discOrdinal: Int): IO[CatalogError, Map[RelPath, ContentHash]]
  /** Everything recorded as physically on `discOrdinal`: the merged view a mount
    * of the disc shows (later sessions hide same-path versions from earlier
    * ones), plus those hidden ("superseded within disc") versions with the
    * session that recorded them. Powers two-tier read-back verify. */
  def discRecordedContents(project: ProjectId, discOrdinal: Int): IO[CatalogError, DiscRecordedContents]
  /** Append a MARKED_DEAD event for `discOrdinal` and record its casualties:
    * every version whose only copy was on this disc becomes either REBURN (the
    * path's current version — the next scan/plan/burn re-writes it from source)
    * or LOST (a superseded/deleted version — permanently gone). Fails with
    * IllegalStatusTransition if the disc is already dead. */
  def markDead(project: ProjectId, discOrdinal: Int, at: Instant): IO[CatalogError, LostReport]

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

  /** Open a standalone catalog file at `dbPath`, apply `update`, then close it.
    * Used to materialize the post-burn catalog state into the per-disc snapshot
    * (`burn-catalog.sqlite`) without committing anything to the live catalog —
    * the live commit happens separately, only after the discs are staged. */
  def applyUpdateTo(dbPath: Path, update: CatalogUpdate): IO[CatalogError, Unit] =
    ZIO.serviceWithZIO[CatalogStore](_.applyUpdate(update))
      .provide(
        ZLayer.succeed(DbPath(dbPath)),
        Db.poolLayer,
        CatalogStore.live)
      .mapError {
        case e: CatalogError => e
        case t               => CatalogError.MigrationFailed(t)
      }
