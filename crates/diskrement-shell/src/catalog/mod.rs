//! The SQLite catalog: schema, migrations, and every persistence operation.
//!
//! One `rusqlite::Connection`, no pool. diskrement opens the catalog, does its
//! work, and exits; there is no concurrency to pool for (spec §5.5). That one
//! decision retires four of the six gotchas the Scala build accumulated around
//! zio-jdbc: no `DriverManager` registry to serialise tests against, no pool
//! TTL fiber needing a live clock, no `getClientInfo()` proxy, and — because a
//! `Transaction` rolls back on drop unless committed — no way to write a
//! transaction that silently auto-commits.

pub mod model;
mod read;
mod schema;
mod store;
pub mod time;

#[cfg(test)]
pub(crate) mod testutil;

use std::path::Path;

use rusqlite::{Connection, Transaction};

use crate::error::CatalogError;

/// A project catalog: one open SQLite database, migrated to the current schema.
pub struct Catalog {
    conn: Connection,
}

impl Catalog {
    /// Open the catalog at `path`, creating the file if absent, and run every
    /// pending migration.
    ///
    /// `foreign_keys` is a per-connection pragma in SQLite, so all three are
    /// applied on every open, exactly as `Db.acquireConn` did.
    pub fn open(path: &Path) -> Result<Self, CatalogError> {
        let mut conn = Connection::open(path).map_err(CatalogError::MigrationFailed)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;",
        )
        .map_err(CatalogError::MigrationFailed)?;
        schema::run(&mut conn)?;
        Ok(Catalog { conn })
    }

    /// Run `f` inside one transaction, committing only if it returns `Ok`.
    ///
    /// This is the structural replacement for the Scala `tx` helper, which had
    /// to prepend `setAutoCommit(false)` to every transaction body because
    /// xerial's driver opened connections in auto-commit mode and zio-jdbc
    /// would then never roll back. Here an early return drops the guard, and
    /// dropping an uncommitted `Transaction` rolls it back.
    pub(crate) fn tx<T>(
        &mut self,
        f: impl FnOnce(&Transaction<'_>) -> Result<T, CatalogError>,
    ) -> Result<T, CatalogError> {
        let tx = self.conn.transaction().map_err(CatalogError::from_sqlite)?;
        let out = f(&tx)?;
        tx.commit().map_err(CatalogError::from_sqlite)?;
        Ok(out)
    }

    /// Direct access to the underlying connection, for tests that need raw
    /// SQL: schema assertions, row counts, and the occasional placement a test
    /// inserts by hand. Not part of the operational API — every caller in
    /// `src/` goes through a named method.
    #[doc(hidden)]
    pub fn raw_connection_for_tests(&self) -> &Connection {
        &self.conn
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::temp_catalog;

    #[test]
    fn a_fresh_database_migrates_to_version_4_with_every_domain_table() {
        let (_dir, cat) = temp_catalog();
        let version: i64 = cat
            .conn
            .query_row("select max(version) from schema_version", [], |r| r.get(0))
            .expect("schema_version is populated");
        assert_eq!(version, 4);

        let mut stmt = cat
            .conn
            .prepare("select name from sqlite_master where type='table' order by name")
            .expect("sqlite_master is queryable");
        let tables: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .expect("names are text")
            .map(|r| r.expect("readable row"))
            .collect();

        for expected in [
            "change_record",
            "disc",
            "disc_event",
            "file_version",
            "lost_version",
            "placement",
            "project",
            "project_root",
            "session",
        ] {
            assert!(tables.contains(&expected.to_string()), "missing {expected}");
        }
    }

    #[test]
    fn re_opening_a_catalog_re_runs_migrations_idempotently() {
        let (dir, cat) = temp_catalog();
        let path = dir.path().join("catalog.sqlite");
        drop(cat);

        let cat = super::Catalog::open(&path).expect("re-open migrates cleanly");
        let mut stmt = cat
            .conn
            .prepare("select version from schema_version")
            .expect("schema_version is queryable");
        let versions: Vec<i64> = stmt
            .query_map([], |r| r.get::<_, i64>(0))
            .expect("versions are integers")
            .map(|r| r.expect("readable row"))
            .collect();
        assert_eq!(versions, vec![4], "the version table must hold one row");
    }

    #[test]
    fn a_v3_catalog_gains_project_root_and_keeps_its_projects() {
        let (dir, mut cat) = temp_catalog();
        cat.create_project(&super::testutil::project_init("p"))
            .expect("project");
        cat.conn
            .execute_batch("drop table project_root; update schema_version set version = 3;")
            .expect("back to V3");
        drop(cat);

        let cat = super::Catalog::open(&dir.path().join("catalog.sqlite")).expect("V3 → V4");
        let version: i64 = cat
            .conn
            .query_row("select version from schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 4);
        let roots: i64 = cat
            .conn
            .query_row("select count(*) from project_root", [], |r| r.get(0))
            .unwrap();
        assert_eq!(roots, 0);
        assert!(cat.find_project("p").unwrap().is_some());
    }

    #[test]
    fn opening_applies_the_wal_foreign_key_and_synchronous_pragmas() {
        // Note the catalog is a real file: an in-memory database always reports
        // journal_mode=memory, which would make this test vacuous.
        let (_dir, cat) = temp_catalog();
        let journal: String = cat
            .conn
            .query_row("pragma journal_mode", [], |r| r.get(0))
            .expect("pragma readable");
        let foreign_keys: i64 = cat
            .conn
            .query_row("pragma foreign_keys", [], |r| r.get(0))
            .expect("pragma readable");
        let synchronous: i64 = cat
            .conn
            .query_row("pragma synchronous", [], |r| r.get(0))
            .expect("pragma readable");
        assert_eq!(journal, "wal");
        assert_eq!(foreign_keys, 1);
        assert_eq!(synchronous, 2);
    }

    #[test]
    fn the_disc_status_check_constraint_rejects_dead() {
        // The derived-DEAD invariant depends on this constraint existing, so it
        // is pinned here rather than assumed from the DDL text.
        let (_dir, cat) = temp_catalog();
        cat.conn
            .execute(
                "insert into project (name, source_root, work_dir, default_disc_type,
                                      margin_percent, copies, created_at)
                 values ('p', '/src', '/work', 'BD_R', 2.0, 1, '2026-06-01T00:00:00Z')",
                [],
            )
            .expect("project inserts");
        let err = cat.conn.execute(
            "insert into disc (project_id, ordinal, disc_type, usable_bytes, status, label)
             values (1, 1, 'BD_R', 100, 'DEAD', 'd1')",
            [],
        );
        assert!(err.is_err(), "the disc table must refuse a DEAD status");
    }

    #[test]
    fn disc_ordinals_are_unique_per_copy_set_not_per_project() {
        let (_dir, cat) = temp_catalog();
        cat.conn
            .execute(
                "insert into project (name, source_root, work_dir, default_disc_type,
                                      margin_percent, copies, created_at)
                 values ('p', '/src', '/work', 'BD_R', 2.0, 2, '2026-06-01T00:00:00Z')",
                [],
            )
            .expect("project inserts");
        let insert = |set: i64| {
            cat.conn.execute(
                "insert into disc (project_id, ordinal, disc_type, usable_bytes, status, label, copy_set)
                 values (1, 1, 'BD_R', 100, 'OPEN', 'd', ?1)",
                [set],
            )
        };
        insert(0).expect("1A inserts");
        insert(1).expect("1B inserts beside 1A");
        assert!(insert(1).is_err(), "a second 1B must be refused");
        assert!(insert(-1).is_err(), "a negative copy set must be refused");
    }

    #[test]
    fn the_migration_leaves_foreign_keys_on() {
        let (_dir, cat) = temp_catalog();
        let on: i64 = cat
            .conn
            .query_row("pragma foreign_keys", [], |r| r.get(0))
            .expect("pragma readable");
        assert_eq!(on, 1);
    }

    #[test]
    fn foreign_keys_into_disc_still_name_disc_after_the_rebuild() {
        // The V3 rebuild drops `disc` and renames `disc_v3` over it; the
        // tables that reference it must still point at `disc`, not at a
        // vanished or renamed table.
        let (_dir, cat) = temp_catalog();
        for table in ["session", "disc_event"] {
            let targets: Vec<String> = cat
                .conn
                .prepare(&format!(
                    "select \"table\" from pragma_foreign_key_list('{table}')"
                ))
                .expect("pragma readable")
                .query_map([], |r| r.get(0))
                .expect("rows")
                .map(|r| r.expect("readable row"))
                .collect();
            assert_eq!(targets, vec!["disc".to_string()], "{table}");
        }
        let stray: i64 = cat
            .conn
            .query_row(
                "select count(*) from sqlite_master where sql like '%disc_v3%'",
                [],
                |r| r.get(0),
            )
            .expect("sqlite_master readable");
        assert_eq!(stray, 0);
    }
}
