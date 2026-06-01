package diskrement.shell.catalog

import zio._
import zio.jdbc._

/** Hand-rolled forward-migration runner. The caller wraps `run` in a single
  * `transaction`, so all pending migrations commit atomically. Cycle 1 ships one
  * version, so this equals one-transaction-per-version; if a future upgrade applies
  * several versions at once and per-version commit isolation becomes desirable,
  * split `run` into one transaction per version. */
object Migrations:

  private val v1: List[SqlFragment] = List(
    sql"""create table if not exists project (
            id                integer primary key autoincrement,
            name              text    not null unique,
            source_root       text    not null,
            work_dir          text    not null,
            default_disc_type text    not null,
            margin_percent    real    not null,
            copies            integer not null default 1,
            created_at        text    not null
          )""",
    sql"""create table if not exists disc (
            id          integer primary key autoincrement,
            project_id  integer not null references project(id),
            ordinal     integer not null,
            disc_type   text    not null,
            usable_bytes integer not null,
            status      text    not null check (status in ('OPEN','FINALIZED')),
            label       text    not null,
            unique (project_id, ordinal)
          )""",
    sql"""create table if not exists session (
            id            integer primary key autoincrement,
            disc_id       integer not null references disc(id),
            ordinal       integer not null,
            state         text    not null check (state in ('PLANNED','BURNED','VERIFIED')),
            burned_at     text,
            manifest_hash text,
            unique (disc_id, ordinal)
          )""",
    sql"""create table if not exists file_version (
            id            integer primary key autoincrement,
            project_id    integer not null references project(id),
            rel_path      text    not null,
            content_hash  text    not null,
            size_bytes    integer not null,
            mtime         text    not null,
            first_seen_at text    not null
          )""",
    sql"""create table if not exists placement (
            id              integer primary key autoincrement,
            file_version_id integer not null references file_version(id),
            session_id      integer not null references session(id),
            disc_path       text    not null,
            copy_index      integer not null
          )""",
    sql"""create table if not exists change_record (
            id              integer primary key autoincrement,
            session_id      integer not null references session(id),
            file_version_id integer not null references file_version(id),
            kind            text    not null check (kind in ('ADDED','MODIFIED','DELETED'))
          )""",
    sql"create index if not exists idx_file_version_path on file_version(project_id, rel_path)",
    sql"create index if not exists idx_placement_session on placement(session_id)",
    sql"create index if not exists idx_change_record_fv on change_record(file_version_id)"
  )

  private val migrations: List[(Int, List[SqlFragment])] = List(1 -> v1)

  private val ensureVersionTable: ZIO[ZConnection, Throwable, Unit] =
    sql"create table if not exists schema_version (version integer not null)".execute

  private val currentVersion: ZIO[ZConnection, Throwable, Int] =
    sql"select max(version) from schema_version".query[Option[Int]].selectOne.map(_.flatten.getOrElse(0))

  private def setVersion(v: Int): ZIO[ZConnection, Throwable, Unit] =
    sql"delete from schema_version".execute *>
      sql"insert into schema_version (version) values ($v)".insert.unit

  /** Apply every migration whose version exceeds the stored one, in order. */
  val run: ZIO[ZConnection, Throwable, Unit] =
    for
      _   <- ensureVersionTable
      cur <- currentVersion
      _   <- ZIO.foreachDiscard(migrations.filter(_._1 > cur).sortBy(_._1)) { case (v, stmts) =>
               ZIO.foreachDiscard(stmts)(_.execute) *> setVersion(v)
             }
    yield ()
