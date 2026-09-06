//! Proves the transcribed DDL produces the same schema the Scala build did.
//!
//! Comparison is on normalised text, not bytes: SQLite stores each object's
//! `CREATE` statement as it was typed (minus `IF NOT EXISTS`), so the two
//! implementations differ only in indentation. Collapsing whitespace keeps the
//! comparison strict about everything that matters — column order, types,
//! defaults, and every `check` and `unique` clause.

mod support;

use rusqlite::Connection;
use support::open_fixture;

fn schema_of(conn: &Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare(
            "select type, name, tbl_name, coalesce(sql, '')
             from sqlite_master order by type, name",
        )
        .expect("sqlite_master is queryable");
    let mut objects: Vec<String> = stmt
        .query_map([], |row| {
            let kind: String = row.get(0)?;
            let name: String = row.get(1)?;
            let table: String = row.get(2)?;
            let sql: String = row.get(3)?;
            let normalised = sql.split_whitespace().collect::<Vec<_>>().join(" ");
            Ok(format!("{kind} {name} on {table}: {normalised}"))
        })
        .expect("rows are text")
        .map(|r| r.expect("readable row"))
        .collect();
    objects.sort();
    objects
}

#[test]
fn a_freshly_migrated_catalog_has_the_schema_the_scala_build_wrote() {
    let dir = tempfile::TempDir::new().expect("a writable temp directory");
    let ours = diskrement_shell::catalog::Catalog::open(&dir.path().join("catalog.sqlite"))
        .expect("catalog opens");
    let theirs = open_fixture("catalog/01-after-init.sqlite");

    let expected = schema_of(&theirs);
    let actual = schema_of(ours.raw_connection_for_tests());

    assert!(
        expected.len() >= 17,
        "the fixture schema looks truncated: {expected:#?}"
    );
    for (want, got) in expected.iter().zip(actual.iter()) {
        assert_eq!(want, got);
    }
    assert_eq!(expected.len(), actual.len(), "object count differs");
}
