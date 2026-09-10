//! `DiscOracle.scala`: read a staged `disc-N/` back and expose facts.
//!
//! Deserialization goes through the production `burn::json` DTOs rather than
//! ad-hoc structs, so a serde mistake in the shipping codec fails here too.

// Consumed by the scenario files as they land, not by the harness self-tests.
#![allow(dead_code)]

use diskrement_shell::burn::json::{ManifestJson, PointersJson, ProjectJson};
use std::collections::BTreeSet;
use std::path::Path;

/// Relative paths of every regular file under `disc-N/data`.
pub fn data_files(disc_dir: &Path) -> BTreeSet<String> {
    let data = disc_dir.join("data");
    if !data.is_dir() {
        return BTreeSet::new();
    }
    walkdir::WalkDir::new(&data)
        .into_iter()
        .map(|e| e.expect("walking the staged data dir"))
        .filter(|e| e.file_type().is_file())
        .map(|e| {
            e.path()
                .strip_prefix(&data)
                .expect("walkdir yields paths under its root")
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

pub fn manifest(disc_dir: &Path) -> ManifestJson {
    read_json(disc_dir, "manifest.json")
}

pub fn project_json(disc_dir: &Path) -> ProjectJson {
    read_json(disc_dir, "project.json")
}

pub fn pointers(disc_dir: &Path) -> PointersJson {
    read_json(disc_dir, "pointers.json")
}

/// Files staged on this disc per the manifest: added ++ modified.
pub fn manifest_files(m: &ManifestJson) -> BTreeSet<String> {
    m.added
        .iter()
        .chain(m.modified.iter())
        .map(|e| e.rel_path.clone())
        .collect()
}

pub fn manifest_added(m: &ManifestJson) -> BTreeSet<String> {
    m.added.iter().map(|e| e.rel_path.clone()).collect()
}

pub fn manifest_modified(m: &ManifestJson) -> BTreeSet<String> {
    m.modified.iter().map(|e| e.rel_path.clone()).collect()
}

pub fn manifest_deleted(m: &ManifestJson) -> BTreeSet<String> {
    m.deleted.iter().cloned().collect()
}

/// True if the staged copy of `rel_path` is byte-identical to the source.
pub fn content_matches(disc_dir: &Path, source: &Path, rel_path: &str) -> bool {
    let staged = std::fs::read(disc_dir.join("data").join(rel_path));
    let src = std::fs::read(source.join(rel_path));
    match (staged, src) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Number of tables in the embedded catalog snapshot — proof that it opens.
///
/// Opened `?immutable=1` with `SQLITE_OPEN_URI`: a plain read-only open of a
/// WAL database creates `-wal`/`-shm` beside it, which here would mean
/// writing two files into a staged disc tree that a later `verify` in the
/// same scenario walks.
pub fn catalog_table_count(disc_dir: &Path) -> i64 {
    let path = disc_dir.join(".diskrement/catalog.sqlite");
    let uri = format!("file:{}?immutable=1", path.display());
    let conn = rusqlite::Connection::open_with_flags(
        uri,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .expect("the embedded catalog opens");
    conn.query_row(
        "select count(*) from sqlite_master where type='table'",
        [],
        |r| r.get(0),
    )
    .expect("counting tables")
}

fn read_json<T: serde::de::DeserializeOwned>(disc_dir: &Path, name: &str) -> T {
    let path = disc_dir.join(".diskrement").join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("decoding {}: {e}", path.display()))
}
