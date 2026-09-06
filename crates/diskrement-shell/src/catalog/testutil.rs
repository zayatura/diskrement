//! Test-only helpers shared by the catalog's unit tests.
//!
//! Every test gets a real SQLite file in its own `TempDir` rather than an
//! in-memory database: the production code sets `journal_mode=WAL`, which an
//! in-memory database silently ignores, and `snapshot_to` needs a real file.
//! Hold the returned `TempDir` for as long as the catalog — dropping it deletes
//! the directory.

use tempfile::TempDir;

use super::Catalog;

pub(crate) fn temp_catalog() -> (TempDir, Catalog) {
    let dir = TempDir::new().expect("a writable temp directory");
    let catalog = Catalog::open(&dir.path().join("catalog.sqlite")).expect("catalog opens");
    (dir, catalog)
}
