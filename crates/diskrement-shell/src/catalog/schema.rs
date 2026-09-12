//! The catalog schema, transcribed from the Scala `Migrations` object.
//!
//! Frozen by the data-compatibility contract (spec §3.1). The two `check`
//! constraints are load-bearing, not decoration: `status in ('OPEN','FINALIZED')`
//! is what makes the derived-`DEAD` invariant enforceable by the database, and
//! `kind in ('MARKED_DEAD')` is what keeps `disc_event` a closed vocabulary.
//! `tests/fixture_schema.rs` compares what this produces against a catalog the
//! Scala build wrote.
//!
//! Migrations run forward only, all pending versions in one transaction.

use rusqlite::Connection;

use crate::error::CatalogError;

const V1: &[&str] = &[
    "create table if not exists project (
       id                integer primary key autoincrement,
       name              text    not null unique,
       source_root       text    not null,
       work_dir          text    not null,
       default_disc_type text    not null,
       margin_percent    real    not null,
       copies            integer not null default 1,
       created_at        text    not null
     )",
    "create table if not exists disc (
       id          integer primary key autoincrement,
       project_id  integer not null references project(id),
       ordinal     integer not null,
       disc_type   text    not null,
       usable_bytes integer not null,
       status      text    not null check (status in ('OPEN','FINALIZED')),
       label       text    not null,
       unique (project_id, ordinal)
     )",
    "create table if not exists session (
       id            integer primary key autoincrement,
       disc_id       integer not null references disc(id),
       ordinal       integer not null,
       state         text    not null check (state in ('PLANNED','BURNED','VERIFIED')),
       burned_at     text,
       manifest_hash text,
       unique (disc_id, ordinal)
     )",
    "create table if not exists file_version (
       id            integer primary key autoincrement,
       project_id    integer not null references project(id),
       rel_path      text    not null,
       content_hash  text    not null,
       size_bytes    integer not null,
       mtime         text    not null,
       first_seen_at text    not null
     )",
    "create table if not exists placement (
       id              integer primary key autoincrement,
       file_version_id integer not null references file_version(id),
       session_id      integer not null references session(id),
       disc_path       text    not null,
       copy_index      integer not null
     )",
    "create table if not exists change_record (
       id              integer primary key autoincrement,
       session_id      integer not null references session(id),
       file_version_id integer not null references file_version(id),
       kind            text    not null check (kind in ('ADDED','MODIFIED','DELETED'))
     )",
    "create index if not exists idx_file_version_path on file_version(project_id, rel_path)",
    "create index if not exists idx_placement_session on placement(session_id)",
    "create index if not exists idx_change_record_fv on change_record(file_version_id)",
];

const V2: &[&str] = &[
    "create table if not exists disc_event (
       id      integer primary key autoincrement,
       disc_id integer not null references disc(id),
       kind    text    not null check (kind in ('MARKED_DEAD')),
       at      text    not null
     )",
    // The immutable record of each version's fate at mark-dead time.
    // `file_history` re-derives LOST from disc_event rather than reading this
    // table, and cannot reproduce it: a REBURN-fated version that is later
    // superseded reads LOST from then on. Keep it as the audit trail of what a
    // given event actually cost.
    "create table if not exists lost_version (
       id              integer primary key autoincrement,
       event_id        integer not null references disc_event(id),
       file_version_id integer not null references file_version(id),
       fate            text    not null check (fate in ('LOST','REBURN'))
     )",
    "create index if not exists idx_disc_event_disc on disc_event(disc_id)",
    // `mark_dead`'s "is this version also on another live disc?" probe is a
    // correlated subquery keyed on placement.file_version_id; without this it
    // full-scans placement once per version on the dying disc.
    "create index if not exists idx_placement_fv on placement(file_version_id)",
];

const MIGRATIONS: &[(i64, &[&str])] = &[(1, V1), (2, V2)];

/// Apply every migration whose version exceeds the stored one, in order.
pub(crate) fn run(conn: &mut Connection) -> Result<(), CatalogError> {
    run_inner(conn).map_err(CatalogError::MigrationFailed)
}

fn run_inner(conn: &mut Connection) -> Result<(), rusqlite::Error> {
    conn.execute(
        "create table if not exists schema_version (version integer not null)",
        [],
    )?;
    let current: i64 = conn.query_row(
        "select coalesce(max(version), 0) from schema_version",
        [],
        |row| row.get(0),
    )?;

    let tx = conn.transaction()?;
    for (version, statements) in MIGRATIONS.iter().filter(|(v, _)| *v > current) {
        for statement in *statements {
            tx.execute(statement, [])?;
        }
        // One row, always: the Scala replaced rather than appended, and
        // `MigrationsSpec` pins that the table holds exactly one version.
        tx.execute("delete from schema_version", [])?;
        tx.execute(
            "insert into schema_version (version) values (?1)",
            [version],
        )?;
    }
    tx.commit()
}
