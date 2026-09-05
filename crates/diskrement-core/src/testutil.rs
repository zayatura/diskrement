//! Test-only fixtures shared by the unit test modules, and (from Task 7) the
//! proptest strategies that replace `Generators.scala`.

use chrono::{DateTime, Utc};

use crate::model::{CatalogEntry, CatalogView, ContentHash, RelPath, ScannedFile};

/// The Unix epoch — every ported test's stand-in mtime, matching the Scala
/// tests' `Instant.EPOCH`. Centralised so the constructor appears exactly once:
/// if the pinned `chrono` predates `DateTime::from_timestamp`, change it here to
/// `DateTime::UNIX_EPOCH` and nothing else moves.
pub fn epoch() -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(0, 0).expect("the epoch is a valid timestamp")
}

/// A scanned file whose hash is derived from its path, mirroring the `sf`
/// helper in `PlannerTest.scala` and `ManifestBuilderTest.scala`.
pub fn sf(path: &str, size: u64) -> ScannedFile {
    ScannedFile {
        rel_path: RelPath(path.to_string()),
        content_hash: ContentHash(format!("h-{path}")),
        size_bytes: size,
        mtime: epoch(),
    }
}

/// A scanned file with an explicit hash and the size-1 default from
/// `DiffTest.scala`, whose tests need hash control rather than size control.
pub fn sf_hash(path: &str, hash: &str) -> ScannedFile {
    ScannedFile {
        rel_path: RelPath(path.to_string()),
        content_hash: ContentHash(hash.to_string()),
        size_bytes: 1,
        mtime: epoch(),
    }
}

/// A catalog entry, mirroring the `entry` helper in `DiffTest.scala`.
pub fn entry(path: &str, hash: &str) -> CatalogEntry {
    CatalogEntry {
        rel_path: RelPath(path.to_string()),
        content_hash: ContentHash(hash.to_string()),
        size_bytes: 1,
    }
}

/// A catalog view over the given entries, mirroring `viewOf` in `DiffTest.scala`.
pub fn view_of(entries: &[CatalogEntry]) -> CatalogView {
    CatalogView {
        current: entries
            .iter()
            .map(|e| (e.rel_path.clone(), e.clone()))
            .collect(),
    }
}
