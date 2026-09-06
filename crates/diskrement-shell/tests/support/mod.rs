//! Shared helpers for the fixture-compatibility tests.
//!
//! The corpus under `fixtures/` was harvested from the Scala build in P0 and is
//! the reference for every compatibility test from P2 onward (spec §4). It is
//! read-only in the strongest sense available: see `open_fixture`.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

/// Absolute path of the committed fixture corpus.
pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .canonicalize()
        .expect("fixtures/ is committed at the repository root")
}

/// Open a fixture database without touching it.
///
/// The corpus databases were left in WAL mode by the Scala build, and a plain
/// read-only open of a WAL database creates `-wal` and `-shm` files beside it —
/// which would dirty `fixtures/` and invalidate `SHA256SUMS`. The
/// `immutable=1` URI parameter tells SQLite the file cannot change, so it skips
/// the WAL machinery entirely and writes nothing.
pub fn open_fixture(rel: &str) -> Connection {
    let path = fixtures_dir().join(rel);
    assert!(path.is_file(), "missing fixture: {}", path.display());
    let uri = format!("file:{}?immutable=1", path.display());
    Connection::open_with_flags(
        &uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .unwrap_or_else(|e| panic!("opening fixture {rel}: {e}"))
}

/// Every `*.sqlite` under one fixture subdirectory, in filename order (the
/// filenames are numbered, so that is scenario order).
pub fn fixture_dbs(subdir: &str) -> Vec<String> {
    let dir = fixtures_dir().join(subdir);
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .map(|entry| entry.expect("readable dir entry").file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".sqlite"))
        .collect();
    names.sort();
    assert!(!names.is_empty(), "no fixture databases under {subdir}");
    names.into_iter().map(|n| format!("{subdir}/{n}")).collect()
}
