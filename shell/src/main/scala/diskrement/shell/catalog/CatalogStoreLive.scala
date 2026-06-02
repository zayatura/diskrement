package diskrement.shell.catalog

import diskrement.core.{CatalogEntry, CatalogView, ContentHash, DiscStatus, DiscType, ManifestChangeKind, RelPath}
import diskrement.shell.model._
import diskrement.shell.model.CatalogError._
import zio._
import zio.jdbc._
import java.time.Instant

private[catalog] final case class CatalogStoreLive(pool: ZConnectionPool) extends CatalogStore:

  /** Run a connection program as one transaction, mapping expected faults to
    * CatalogError and dying on anything unexpected.
    *
    * SQLite JDBC opens connections with autoCommit=true (each statement commits
    * immediately).  zio-jdbc 0.1.2 only performs rollback-on-failure when the
    * connection is NOT in autoCommit mode, and commits on success by restoring
    * the original autoCommit value.  We therefore call setAutoCommit(false) at
    * the very start of every transaction body through the ZConnection wrapper;
    * this marks the AutoCommit flag inside ZConnection.Restorable so that
    * restore() will call setAutoCommit(true) on exit — which in JDBC semantics
    * commits the pending transaction on success, and rollback + restore handles
    * the failure path. */
  private def tx[A](z: ZIO[ZConnection, Throwable, A]): IO[CatalogError, A] =
    transaction(
      ZIO.serviceWithZIO[ZConnection](_.access(_.setAutoCommit(false))) *> z
    )
      .provideEnvironment(ZEnvironment(pool))
      .refineOrDie {
        case e: CatalogError                              => e
        case e: java.sql.SQLException if isConstraint(e)  => ConstraintViolation(e)
      }

  private def isConstraint(e: java.sql.SQLException): Boolean =
    Option(e.getMessage).exists(_.toLowerCase.contains("constraint"))

  private def isUnique(cause: Throwable): Boolean =
    Option(cause.getMessage).exists(_.toLowerCase.contains("unique"))

  /** Last auto-increment id on the current connection (valid within a transaction). */
  private val lastId: ZIO[ZConnection, Throwable, Long] =
    sql"select last_insert_rowid()".query[Long].selectOne
      .someOrFail(new IllegalStateException("last_insert_rowid() returned no row"))

  def createProject(p: ProjectInit): IO[CatalogError, ProjectId] =
    tx {
      sql"""insert into project
              (name, source_root, work_dir, default_disc_type, margin_percent, copies, created_at)
            values
              (${p.name}, ${p.sourceRoot}, ${p.workDir}, ${p.defaultDiscType.toString},
               ${p.marginPercent}, ${p.copies}, ${p.createdAt.toString})""".insert
        *> lastId.map(ProjectId(_))
    }.catchSome {
      case ConstraintViolation(c) if isUnique(c) => ZIO.fail(DuplicateProject(p.name))
    }

  def applyUpdate(u: CatalogUpdate): IO[CatalogError, Unit] =
    tx {
      ZIO.foreachDiscard(u.finalizeDiscOrdinals)(finalizeDisc(u.projectId, _)) *>
        ZIO.foreachDiscard(u.discs)(writeDisc(u.projectId, _))
    }

  private def finalizeDisc(pid: ProjectId, ordinal: Int): ZIO[ZConnection, Throwable, Unit] =
    sql"""update disc set status = 'FINALIZED'
          where project_id = ${pid.value} and ordinal = $ordinal and status = 'OPEN'""".update
      .flatMap(n =>
        ZIO.when(n == 0L)(
          ZIO.fail(IllegalStatusTransition(s"disc $ordinal is not OPEN"))).unit)

  private def discIdByOrdinal(pid: ProjectId, ordinal: Int): ZIO[ZConnection, Throwable, Long] =
    sql"select id from disc where project_id = ${pid.value} and ordinal = $ordinal"
      .query[Long].selectOne
      .flatMap {
        case Some(id) => ZIO.succeed(id)
        case None     => ZIO.fail(ConstraintViolation(
                           new RuntimeException(s"no disc with ordinal $ordinal")))
      }

  private def writeDisc(pid: ProjectId, d: DiscWrite): ZIO[ZConnection, Throwable, Unit] =
    for
      discId <-
        if d.isNew then
          sql"""insert into disc
                  (project_id, ordinal, disc_type, usable_bytes, status, label)
                values
                  (${pid.value}, ${d.ordinal}, ${d.discType.toString},
                   ${d.usableBytes}, ${d.status.toString}, ${d.label})""".insert *> lastId
        else
          for
            id <- discIdByOrdinal(pid, d.ordinal)
            _  <- ZIO.when(d.status == DiscStatus.FINALIZED)(
                    sql"update disc set status = 'FINALIZED' where id = $id and status = 'OPEN'"
                      .update.flatMap(n =>
                        ZIO.when(n == 0L)(
                          ZIO.fail(IllegalStatusTransition(s"disc ${d.ordinal} is not OPEN"))).unit))
          yield id
      sessId <- sql"""insert into session (disc_id, ordinal, state, burned_at, manifest_hash)
                      values ($discId, ${d.session.ordinal}, 'BURNED',
                              ${d.session.burnedAt.toString}, ${d.session.manifestHash})""".insert *> lastId
      _ <- ZIO.foreachDiscard(d.session.added)(
             writeFile(pid, sessId, _, ManifestChangeKind.ADDED, d.session.burnedAt))
      _ <- ZIO.foreachDiscard(d.session.modified)(
             writeFile(pid, sessId, _, ManifestChangeKind.MODIFIED, d.session.burnedAt))
      _ <- ZIO.foreachDiscard(d.session.deleted)(writeTombstone(pid, sessId, _))
    yield ()

  private def writeFile(pid: ProjectId, sessId: Long, f: FileWrite,
                        kind: ManifestChangeKind, firstSeenAt: Instant): ZIO[ZConnection, Throwable, Unit] =
    for
      fvId <- sql"""insert into file_version
                      (project_id, rel_path, content_hash, size_bytes, mtime, first_seen_at)
                    values
                      (${pid.value}, ${f.relPath.value}, ${f.contentHash.value},
                       ${f.sizeBytes}, ${f.mtime.toString}, ${firstSeenAt.toString})""".insert *> lastId
      _ <- sql"""insert into placement (file_version_id, session_id, disc_path, copy_index)
                 values ($fvId, $sessId, ${f.discPath}, ${f.copyIndex})""".insert
      _ <- sql"insert into change_record (session_id, file_version_id, kind) values ($sessId, $fvId, ${kind.toString})".insert
    yield ()

  /** Tombstone the path's current version (the latest file_version for that path). */
  private def writeTombstone(pid: ProjectId, sessId: Long, rel: RelPath): ZIO[ZConnection, Throwable, Unit] =
    sql"""select id from file_version
          where project_id = ${pid.value} and rel_path = ${rel.value}
          order by id desc limit 1""".query[Long].selectOne
      .flatMap {
        case Some(fvId) =>
          sql"insert into change_record (session_id, file_version_id, kind) values ($sessId, $fvId, 'DELETED')".insert.unit
        case None =>
          ZIO.fail(ConstraintViolation(
            new RuntimeException(s"cannot tombstone unknown path ${rel.value}")))
      }

  def findProject(name: String): IO[CatalogError, Option[Project]] =
    tx {
      sql"""select id, name, source_root, work_dir, default_disc_type,
                   margin_percent, copies, created_at
            from project where name = $name"""
        .query[(Long, String, String, String, String, Double, Int, String)].selectOne
    }.map(_.map { case (id, n, sr, wd, dt, mp, cp, ca) =>
      Project(ProjectId(id), n, sr, wd, DiscType.valueOf(dt), mp, cp, Instant.parse(ca))
    })

  def readView(project: ProjectId): IO[CatalogError, CatalogView] =
    tx {
      sql"""select fv.rel_path, fv.content_hash, fv.size_bytes
            from (
              select cr.id as cr_id, cr.kind as kind, cr.file_version_id as fvid,
                     fv.rel_path as rp,
                     row_number() over (partition by fv.rel_path order by cr.id desc) as rn
              from change_record cr
              join file_version fv on fv.id = cr.file_version_id
              where fv.project_id = ${project.value}
            ) l
            join file_version fv on fv.id = l.fvid
            where l.rn = 1 and l.kind <> 'DELETED'"""
        .query[(String, String, Long)].selectAll
    }.map { rows =>
      CatalogView(rows.iterator.map { case (p, h, s) =>
        val rel = RelPath(p)
        rel -> CatalogEntry(rel, ContentHash(h), s)
      }.toMap)
    }
