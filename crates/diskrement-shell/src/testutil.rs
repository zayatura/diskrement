//! Logical comparison helpers, shared by the fixture-corpus tests and the
//! differential harness (spec §4, §8.1).
//!
//! Feature-gated behind `testutil` so that nothing ships in the binary. It
//! lives in the library rather than in a test module because two crates need
//! it, and two copies of a comparison rule are two rules that drift.
//!
//! Every comparison here is **logical**, never byte-wise: SQLite file layout is
//! not stable across library builds, and xerial's SQLite is not
//! `libsqlite3-sys`'s.

/// Catalog tables, in schema order.
pub const CATALOG_TABLES: &[&str] = &[
    "schema_version",
    "project",
    "disc",
    "session",
    "file_version",
    "placement",
    "change_record",
    "disc_event",
    "lost_version",
];

/// Fingerprint-cache sidecar tables. The sidecar carries its schema version in
/// `PRAGMA user_version` rather than in a table, so this list has one entry.
pub const SCANCACHE_TABLES: &[&str] = &["fingerprint"];

fn render(value: rusqlite::types::ValueRef<'_>) -> String {
    use rusqlite::types::ValueRef;
    match value {
        ValueRef::Null => "NULL".to_string(),
        ValueRef::Integer(i) => i.to_string(),
        // `{:?}` so 2.0 stays "2.0" rather than collapsing to "2".
        ValueRef::Real(f) => format!("{f:?}"),
        ValueRef::Text(t) => format!("{:?}", String::from_utf8_lossy(t)),
        ValueRef::Blob(b) => format!("<{} bytes>", b.len()),
    }
}

/// Every row of the named tables as `table: col=value …` lines, sorted within
/// each table.
///
/// Callers that normalize the lines afterwards must re-sort: normalization can
/// change a line's sort key, and two runs whose rows differ only in a timestamp
/// would otherwise sort differently and diff as a false positive.
pub fn dump_tables(conn: &rusqlite::Connection, tables: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for table in tables {
        let mut stmt = conn
            .prepare(&format!("select * from {table}"))
            .unwrap_or_else(|e| panic!("preparing dump of {table}: {e}"));
        let columns: Vec<String> = stmt.column_names().iter().map(|c| c.to_string()).collect();
        let mut rows = stmt.query([]).expect("dump query runs");
        let mut lines = Vec::new();
        while let Some(row) = rows.next().expect("readable row") {
            let cells: Vec<String> = columns
                .iter()
                .enumerate()
                .map(|(i, name)| {
                    format!("{name}={}", render(row.get_ref(i).expect("column exists")))
                })
                .collect();
            lines.push(format!("{table}: {}", cells.join(" ")));
        }
        lines.sort();
        out.extend(lines);
    }
    out
}

/// Every row of every catalog table.
pub fn dump_catalog(conn: &rusqlite::Connection) -> Vec<String> {
    dump_tables(conn, CATALOG_TABLES)
}

/// Compare two dumps, reporting the first difference rather than two walls of
/// text.
pub fn assert_dumps_match(expected: &[String], actual: &[String]) {
    for (i, (want, got)) in expected.iter().zip(actual.iter()).enumerate() {
        assert_eq!(want, got, "dumps diverge at line {i}");
    }
    assert_eq!(
        expected.len(),
        actual.len(),
        "row counts differ:\nexpected {expected:#?}\nactual {actual:#?}"
    );
    assert!(!expected.is_empty(), "the dump was empty");
}
