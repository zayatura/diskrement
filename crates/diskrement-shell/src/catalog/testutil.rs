//! Test-only helpers shared by the catalog's unit tests.
//!
//! Every test gets a real SQLite file in its own `TempDir` rather than an
//! in-memory database: the production code sets `journal_mode=WAL`, which an
//! in-memory database silently ignores, and `snapshot_to` needs a real file.
//! Hold the returned `TempDir` for as long as the catalog — dropping it deletes
//! the directory.

use chrono::{DateTime, TimeZone, Utc};
use diskrement_core::DiscType;
use tempfile::TempDir;

use super::model::ProjectInit;
use super::Catalog;

pub(crate) fn temp_catalog() -> (TempDir, Catalog) {
    let dir = TempDir::new().expect("a writable temp directory");
    let catalog = Catalog::open(&dir.path().join("catalog.sqlite")).expect("catalog opens");
    (dir, catalog)
}

/// The fixed "burned at" instant every unit test uses, matching the Scala
/// specs' `ts`.
// Same situation as Task 2's `Catalog::tx` marker: the brief adds the helper
// one task before its first caller, and `.cargo/config.toml` denies warnings.
#[expect(dead_code, reason = "first used by store.rs tests in task 5")]
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
