//! Shared helpers for the fixture-compatibility tests.
//!
//! The corpus under `fixtures/` was harvested from the Scala build before any
//! Rust was written and is the reference for every compatibility test (spec §4). It is
//! read-only in the strongest sense available: see `open_fixture`.

// The comparison helpers are re-exported from the library's `testutil` module.
// Test binaries that include `support` but never touch those re-exports would
// otherwise fail on an unused import; the corpus-specific helpers below stay
// dead-code-allowed for the same reason.
#![allow(dead_code, unused_imports)]

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

/// Copy a fixture database into a fresh temp directory and return both.
///
/// Hold the `TempDir` for as long as the copy is in use. Opening the corpus
/// itself through `Catalog::open` would run migrations against it and write to
/// `fixtures/`; a copy is also the only honest way to test that opening an
/// already-current catalog changes nothing.
pub fn fixture_copy(rel: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::TempDir::new().expect("a writable temp directory");
    let dest = dir.path().join("catalog.sqlite");
    std::fs::copy(fixtures_dir().join(rel), &dest)
        .unwrap_or_else(|e| panic!("copying fixture {rel}: {e}"));
    (dir, dest)
}

/// Read a fixture file as text. Never writes, so no `immutable=1` dance is
/// needed — unlike the SQLite fixtures.
pub fn fixture_text(rel: &str) -> String {
    let path = fixtures_dir().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

/// Recursively copy a fixture directory into `dest`, preserving file mtimes.
///
/// Mtimes matter because the staged tree inherits them from the snapshot; a
/// copy that dropped them would still pass a byte comparison, but it would
/// quietly stop testing the metadata half of the stager.
pub fn copy_tree(src: &Path, dest: &Path) {
    std::fs::create_dir_all(dest).expect("creating the destination");
    for entry in walkdir::WalkDir::new(src).follow_links(false) {
        let entry = entry.expect("readable fixture entry");
        let rel = entry.path().strip_prefix(src).expect("walked from src");
        let target = dest.join(rel);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target).expect("creating a directory");
        } else if entry.file_type().is_file() {
            std::fs::copy(entry.path(), &target).expect("copying a file");
            let mtime = entry
                .metadata()
                .expect("readable metadata")
                .modified()
                .expect("a modification time");
            std::fs::File::options()
                .write(true)
                .open(&target)
                .expect("reopening the copy")
                .set_modified(mtime)
                .expect("restoring the mtime");
        }
    }
}

/// Copy a whole fixture work dir into a temp dir and return both.
pub fn fixture_work_copy(rel: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::TempDir::new().expect("a writable temp directory");
    let dest = dir.path().join("work");
    copy_tree(&fixtures_dir().join(rel), &dest);
    (dir, dest)
}

pub use diskrement_shell::testutil::{assert_dumps_match, dump_catalog, CATALOG_TABLES};
