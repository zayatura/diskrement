//! Test-only helpers shared by the catalog's unit tests.
//!
//! Every test gets a real SQLite file in its own `TempDir` rather than an
//! in-memory database: the production code sets `journal_mode=WAL`, which an
//! in-memory database silently ignores, and `snapshot_to` needs a real file.
//! Hold the returned `TempDir` for as long as the catalog — dropping it deletes
//! the directory.

use chrono::{DateTime, TimeZone, Utc};
use diskrement_core::{ContentHash, DiscStatus, DiscType, RelPath};
use tempfile::TempDir;

use super::model::{CatalogUpdate, DiscWrite, FileWrite, ProjectId, ProjectInit, SessionWrite};
use super::Catalog;

pub(crate) fn temp_catalog() -> (TempDir, Catalog) {
    let dir = TempDir::new().expect("a writable temp directory");
    let catalog = Catalog::open(&dir.path().join("catalog.sqlite")).expect("catalog opens");
    (dir, catalog)
}

/// The fixed "burned at" instant every unit test uses, matching the Scala
/// specs' `ts`.
pub(crate) fn ts() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 6, 1, 10, 0, 0).unwrap()
}

pub(crate) fn project_init(name: &str) -> ProjectInit {
    ProjectInit {
        name: name.to_string(),
        source_root: "/src".to_string(),
        work_dir: "/work".to_string(),
        default_disc_type: DiscType::BdR,
        margin_percent: 2.0,
        copies: 1,
        created_at: Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap(),
    }
}

/// A file write with the conventional test values: mtime `ts()`, disc path
/// `/data/<path>`, copy 0.
pub(crate) fn fw(path: &str, hash: &str, size: u64) -> FileWrite {
    fw_at(path, hash, size, ts())
}

pub(crate) fn fw_at(
    path: &str,
    hash: &str,
    size: u64,
    mtime: chrono::DateTime<chrono::Utc>,
) -> FileWrite {
    FileWrite {
        rel_path: RelPath(path.to_string()),
        content_hash: ContentHash(hash.to_string()),
        size_bytes: size,
        mtime,
        disc_path: format!("/data/{path}"),
        copy_index: 0,
    }
}

/// One session on one disc, the shape every ported spec's `session` helper
/// built: the disc is new exactly when this is its first session.
pub(crate) fn update(
    project: ProjectId,
    disc_ordinal: u32,
    session_ordinal: u32,
    added: Vec<FileWrite>,
    modified: Vec<FileWrite>,
    deleted: Vec<RelPath>,
) -> CatalogUpdate {
    CatalogUpdate {
        project_id: project,
        finalize_disc_ordinals: Vec::new(),
        discs: vec![DiscWrite {
            ordinal: disc_ordinal,
            copy_set: 0,
            is_new: session_ordinal == 1,
            disc_type: diskrement_core::DiscType::BdR,
            usable_bytes: 1_000_000,
            status: DiscStatus::Open,
            label: format!("d{disc_ordinal}"),
            session: SessionWrite {
                ordinal: session_ordinal,
                burned_at: ts(),
                manifest_hash: format!("m{disc_ordinal}-{session_ordinal}"),
                added,
                modified,
                deleted,
            },
        }],
    }
}

/// Row count of one table, for the atomicity assertions.
pub(crate) fn count(cat: &super::Catalog, table: &str) -> i64 {
    cat.raw_connection_for_tests()
        .query_row(&format!("select count(*) from {table}"), [], |r| r.get(0))
        .expect("count query succeeds")
}
