//! Holds `catalog::time` to the strings the Scala build actually wrote.
//!
//! This is the test the spec's §11 risk exists for. It reads every timestamp
//! column of every fixture database and asserts `format(parse(s)) == s`, so a
//! drift in fractional-precision rules fails here rather than on burned media.

mod support;

use diskrement_shell::catalog::time;
use support::{fixture_dbs, open_fixture};

/// (table, column) pairs holding `Instant.toString()` output in the catalog.
const CATALOG_COLUMNS: &[(&str, &str)] = &[
    ("project", "created_at"),
    ("session", "burned_at"),
    ("file_version", "mtime"),
    ("file_version", "first_seen_at"),
    ("disc_event", "at"),
];

/// The same, for the fingerprint sidecar written by `plan`'s scan.
const SCANCACHE_COLUMNS: &[(&str, &str)] =
    &[("fingerprint", "mtime"), ("fingerprint", "updated_at")];

fn sweep(dbs: &[String], columns: &[(&str, &str)]) -> (usize, Vec<usize>) {
    let mut checked = 0usize;
    let mut widths = Vec::new();
    for db in dbs {
        let conn = open_fixture(db);
        for (table, column) in columns {
            let sql = format!("select {column} from {table} where {column} is not null");
            let mut stmt = conn.prepare(&sql).expect("fixture column exists");
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .expect("column is text");
            for row in rows {
                let stored = row.expect("readable row");
                let parsed =
                    time::parse(&stored).unwrap_or_else(|e| panic!("{db} {table}.{column}: {e}"));
                assert_eq!(
                    time::format(&parsed),
                    stored,
                    "{db} {table}.{column} did not round-trip"
                );
                let width = stored
                    .split_once('.')
                    .map(|(_, frac)| frac.trim_end_matches('Z').len())
                    .unwrap_or(0);
                widths.push(width);
                checked += 1;
            }
        }
    }
    (checked, widths)
}

#[test]
fn every_catalog_timestamp_round_trips() {
    let (checked, widths) = sweep(&fixture_dbs("catalog"), CATALOG_COLUMNS);
    // A sweep that examined nothing would pass silently; say what it saw.
    assert!(checked > 0, "the sweep read no timestamps at all");
    println!("checked {checked} catalog timestamps");
    // The corpus is documented (fixtures/README.md) to contain both a
    // millisecond-precision shape (file mtimes) and a nanosecond one
    // (createdAt, first_seen_at). If a re-harvest ever loses one of those, the
    // sweep stops covering the case it exists for.
    assert!(widths.contains(&3), "no 3-digit fraction in the corpus");
    assert!(widths.contains(&9), "no 9-digit fraction in the corpus");
}

#[test]
fn every_scancache_timestamp_round_trips() {
    let (checked, _) = sweep(&fixture_dbs("scancache"), SCANCACHE_COLUMNS);
    assert!(checked > 0, "the sweep read no timestamps at all");
    println!("checked {checked} scancache timestamps");
}
