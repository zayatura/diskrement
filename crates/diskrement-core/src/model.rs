//! Pure domain types shared across the functional core.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

/// Path of a file relative to the project's source root, using '/' separators.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RelPath(pub String);

impl RelPath {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RelPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Content hash of a file (SHA-256 hex in production; opaque to the core).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentHash(pub String);

impl ContentHash {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ContentHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A file observed during a scan, always fully hashed by the scanner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedFile {
    pub rel_path: RelPath,
    pub content_hash: ContentHash,
    pub size_bytes: u64,
    pub mtime: DateTime<Utc>,
}

/// The current (live) version of one file as recorded in the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub rel_path: RelPath,
    pub content_hash: ContentHash,
    pub size_bytes: u64,
}

/// Immutable snapshot of the catalog's current view: rel path -> live version.
///
/// A `BTreeMap` rather than a `HashMap`, so key iteration is deterministic.
/// `diff` sorts its outputs explicitly anyway, but a nondeterministic core
/// would be the hardest bug here to see, so both guards stay.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CatalogView {
    pub current: BTreeMap<RelPath, CatalogEntry>,
}

impl CatalogView {
    pub fn empty() -> Self {
        Self::default()
    }

    /// The view that results from recording exactly these files as the current
    /// versions. Used to express diff idempotence (see `DiffPropertyTest`).
    pub fn from_scanned(files: &[ScannedFile]) -> Self {
        CatalogView {
            current: files
                .iter()
                .map(|f| {
                    (
                        f.rel_path.clone(),
                        CatalogEntry {
                            rel_path: f.rel_path.clone(),
                            content_hash: f.content_hash.clone(),
                            size_bytes: f.size_bytes,
                        },
                    )
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{entry, epoch, view_of};

    #[test]
    fn rel_path_orders_lexicographically_by_value() {
        let mut paths = [
            RelPath("b/a".to_string()),
            RelPath("a/z".to_string()),
            RelPath("a/a".to_string()),
        ];
        paths.sort();
        let values: Vec<&str> = paths.iter().map(RelPath::as_str).collect();
        assert_eq!(values, vec!["a/a", "a/z", "b/a"]);
    }

    #[test]
    fn an_empty_catalog_view_has_no_current_entries() {
        assert_eq!(CatalogView::empty().current.len(), 0);
    }

    #[test]
    fn catalog_view_lookups_are_by_rel_path() {
        let e = entry("x/y", "ab");
        let view = view_of(std::slice::from_ref(&e));
        assert_eq!(view.current.get(&RelPath("x/y".to_string())), Some(&e));
    }

    #[test]
    fn a_scanned_file_carries_path_hash_size_and_mtime() {
        let f = ScannedFile {
            rel_path: RelPath("p".to_string()),
            content_hash: ContentHash("cd".to_string()),
            size_bytes: 42,
            mtime: epoch(),
        };
        assert_eq!(f.size_bytes, 42);
        assert_eq!(f.content_hash, ContentHash("cd".to_string()));
        assert_eq!(f.mtime, epoch());
    }

    #[test]
    fn from_scanned_records_every_scanned_file_as_the_current_version() {
        // This is how `DiffPropertyTest` expresses idempotence, so it needs a
        // test of its own rather than only being exercised through a property.
        let files = vec![
            ScannedFile {
                rel_path: RelPath("a".to_string()),
                content_hash: ContentHash("h1".to_string()),
                size_bytes: 7,
                mtime: epoch(),
            },
            ScannedFile {
                rel_path: RelPath("b".to_string()),
                content_hash: ContentHash("h2".to_string()),
                size_bytes: 9,
                mtime: epoch(),
            },
        ];
        let view = CatalogView::from_scanned(&files);
        assert_eq!(view.current.len(), 2);
        assert_eq!(
            view.current.get(&RelPath("a".to_string())),
            Some(&CatalogEntry {
                rel_path: RelPath("a".to_string()),
                content_hash: ContentHash("h1".to_string()),
                size_bytes: 7,
            })
        );
    }
}
