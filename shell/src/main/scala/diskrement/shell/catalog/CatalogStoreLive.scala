package diskrement.shell.catalog

import diskrement.core.{CatalogEntry, CatalogView, ContentHash, DiscState, DiscStatus, DiscType, ManifestChangeKind, RelPath}
import diskrement.shell.model._
import diskrement.shell.model.CatalogError._
import zio._
import zio.jdbc._
import java.nio.file.Path
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

  def sessionsByState(project: ProjectId, state: SessionState): IO[CatalogError, List[Session]] =
    tx {
      sql"""select d.ordinal, s.ordinal, s.state, s.burned_at, s.manifest_hash
            from session s
            join disc d on d.id = s.disc_id
            where d.project_id = ${project.value} and s.state = ${state.toString}
            order by d.ordinal, s.ordinal"""
        .query[(Int, Int, String, Option[String], Option[String])].selectAll
    }.map(_.toList.map { case (dOrd, sOrd, st, burnedAt, manifestHash) =>
      Session(dOrd, sOrd, SessionState.valueOf(st),
              burnedAt.map(Instant.parse), manifestHash)
    })

  def fingerprints(project: ProjectId): IO[CatalogError, Map[RelPath, FileFingerprint]] =
    tx {
      sql"""select fv.rel_path, fv.size_bytes, fv.mtime, fv.content_hash
            from (
              select cr.kind as kind, cr.file_version_id as fvid,
                     row_number() over (partition by fv.rel_path order by cr.id desc) as rn
              from change_record cr
              join file_version fv on fv.id = cr.file_version_id
              where fv.project_id = ${project.value}
            ) l
            join file_version fv on fv.id = l.fvid
            where l.rn = 1 and l.kind <> 'DELETED'"""
        .query[(String, Long, String, String)].selectAll
    }.map { rows =>
      rows.iterator.map { case (p, sz, mt, h) =>
        RelPath(p) -> FileFingerprint(sz, Instant.parse(mt), ContentHash(h))
      }.toMap
    }

  def readDiscStates(project: ProjectId): IO[CatalogError, List[DiscState]] =
    tx {
      sql"""select d.ordinal, d.status, d.usable_bytes,
                   coalesce((select sum(fv.size_bytes)
                             from placement p
                             join session s on s.id = p.session_id
                             join file_version fv on fv.id = p.file_version_id
                             where s.disc_id = d.id), 0) as used
            from disc d
            where d.project_id = ${project.value}
            order by d.ordinal"""
        .query[(Int, String, Long, Long)].selectAll
    }.map(_.toList.map { case (ord, status, usable, used) =>
      DiscState(ord, DiscStatus.valueOf(status), usable, used)
    })

  def archivePointers(project: ProjectId): IO[CatalogError, List[ArchivePointer]] =
    tx {
      sql"""select fv.rel_path, fv.content_hash, fv.size_bytes,
                   d.ordinal, s.ordinal, p.disc_path, fvk.kind
            from file_version fv
            join placement p on p.file_version_id = fv.id
            join session s   on s.id = p.session_id
            join disc d      on d.id = s.disc_id
            join (
              select cr.file_version_id as fvid, cr.kind as kind,
                     row_number() over (partition by cr.file_version_id order by cr.id desc) as rn
              from change_record cr
            ) fvk on fvk.fvid = fv.id and fvk.rn = 1
            where fv.project_id = ${project.value}
              and fv.id not in (
                select fvid from (
                  select cr.file_version_id as fvid, cr.kind as kind,
                         row_number() over (partition by fv2.rel_path order by cr.id desc) as rn
                  from change_record cr
                  join file_version fv2 on fv2.id = cr.file_version_id
                  where fv2.project_id = ${project.value}
                ) cur
                where cur.rn = 1 and cur.kind <> 'DELETED'
              )
            order by fv.rel_path, d.ordinal, s.ordinal"""
        .query[(String, String, Long, Int, Int, String, String)].selectAll
    }.map(_.toList.map { case (rp, ch, sz, dOrd, sOrd, dpath, kind) =>
      ArchivePointer(RelPath(rp), ContentHash(ch), sz, dOrd, sOrd, dpath, ManifestChangeKind.valueOf(kind))
    })

  def fileHistory(project: ProjectId, relPath: RelPath): IO[CatalogError, List[FileHistoryEntry]] =
    tx {
      sql"""select fv.content_hash, fv.size_bytes, d.ordinal, s.ordinal, fvk.kind,
                   case when fv.id = cur.fvid then 1 else 0 end as is_current
            from file_version fv
            join placement p on p.file_version_id = fv.id
            join session s   on s.id = p.session_id
            join disc d      on d.id = s.disc_id
            join (
              select cr.file_version_id as fvid, cr.kind as kind,
                     row_number() over (partition by cr.file_version_id order by cr.id desc) as rn
              from change_record cr
            ) fvk on fvk.fvid = fv.id and fvk.rn = 1
            left join (
              select fvid from (
                select cr.file_version_id as fvid, cr.kind as kind,
                       row_number() over (partition by fv2.rel_path order by cr.id desc) as rn
                from change_record cr
                join file_version fv2 on fv2.id = cr.file_version_id
                where fv2.project_id = ${project.value} and fv2.rel_path = ${relPath.value}
              ) c where c.rn = 1 and c.kind <> 'DELETED'
            ) cur on cur.fvid = fv.id
            where fv.project_id = ${project.value} and fv.rel_path = ${relPath.value}
            order by fv.id"""
        .query[(String, Long, Int, Int, String, Int)].selectAll
    }.map(_.toList.map { case (ch, sz, dOrd, sOrd, kind, isCurrent) =>
      val status =
        if isCurrent == 1 then VersionStatus.CURRENT
        else if kind == "DELETED" then VersionStatus.DELETED
        else VersionStatus.SUPERSEDED
      FileHistoryEntry(ContentHash(ch), sz, dOrd, sOrd, status)
    })

  def markVerified(project: ProjectId, discOrdinal: Int, sessionOrdinal: Int): IO[CatalogError, Unit] =
    tx {
      sql"""update session set state = 'VERIFIED'
            where state = 'BURNED' and ordinal = $sessionOrdinal
              and disc_id = (select id from disc where project_id = ${project.value} and ordinal = $discOrdinal)"""
        .update.flatMap(n =>
          ZIO.when(n == 0L)(
            ZIO.fail(IllegalStatusTransition(
              s"no BURNED session $sessionOrdinal on disc $discOrdinal"))).unit)
    }

  def placementsForDisc(project: ProjectId, discOrdinal: Int): IO[CatalogError, Map[RelPath, ContentHash]] =
    tx {
      sql"""select fv.rel_path, fv.content_hash
            from placement p
            join session s   on s.id = p.session_id
            join disc d      on d.id = s.disc_id
            join file_version fv on fv.id = p.file_version_id
            where d.project_id = ${project.value} and d.ordinal = ${discOrdinal}
              and s.ordinal = (select max(s2.ordinal) from session s2 where s2.disc_id = d.id)"""
        .query[(String, String)].selectAll
    }.map(_.toList.map { case (rp, ch) => RelPath(rp) -> ContentHash(ch) }.toMap)

  def snapshotTo(dest: Path): IO[CatalogError, Unit] =
    transaction(sql"vacuum into ${dest.toString}".execute.unit)
      .provideEnvironment(ZEnvironment(pool))
      .refineOrDie {
        case e: CatalogError                              => e
        case e: java.sql.SQLException if isConstraint(e)  => ConstraintViolation(e)
      }
