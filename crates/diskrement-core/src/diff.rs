//! Pure classification of a scan against the catalog's current view.

use std::collections::HashSet;

use crate::model::{CatalogView, RelPath, ScannedFile};

/// Classification of a scan against the catalog's current view.
///
/// `unchanged` is retained for reporting; it never contributes to a burn.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChangeSet {
    pub added: Vec<ScannedFile>,
    pub modified: Vec<ScannedFile>,
    pub deleted: Vec<RelPath>,
    pub unchanged: Vec<ScannedFile>,
}

impl ChangeSet {
    pub fn empty() -> Self {
        Self::default()
    }

    /// True when there is nothing to back up. `unchanged` is deliberately not
    /// consulted: a scan that found only unchanged files has nothing to burn.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.modified.is_empty() && self.deleted.is_empty()
    }
}

/// Classify `scanned` against the catalog's current view.
///
/// Every output list is sorted by rel path, so the result depends on the *set*
/// of scanned files and not on the order the scanner produced them.
pub fn diff(catalog: &CatalogView, scanned: &[ScannedFile]) -> ChangeSet {
    let mut added = Vec::new();
    let mut modified = Vec::new();
    let mut unchanged = Vec::new();

    for f in scanned {
        match catalog.current.get(&f.rel_path) {
            None => added.push(f.clone()),
            Some(e) if e.content_hash != f.content_hash => modified.push(f.clone()),
            Some(_) => unchanged.push(f.clone()),
        }
    }

    let scanned_paths: HashSet<&RelPath> = scanned.iter().map(|f| &f.rel_path).collect();
    let mut deleted: Vec<RelPath> = catalog
        .current
        .keys()
        .filter(|p| !scanned_paths.contains(*p))
        .cloned()
        .collect();

    added.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    modified.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    unchanged.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    deleted.sort();

    ChangeSet {
        added,
        modified,
        deleted,
        unchanged,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CatalogView, ContentHash, RelPath};
    use crate::testutil::{entry, sf_hash, view_of};

    fn paths(files: &[crate::model::ScannedFile]) -> Vec<&str> {
        files.iter().map(|f| f.rel_path.as_str()).collect()
    }

    // --- ChangeSetTest.scala ---

    #[test]
    fn an_empty_change_set_has_no_entries_and_reports_nothing_to_do() {
        assert!(ChangeSet::empty().is_empty());
    }

    #[test]
    fn a_change_set_with_any_added_modified_or_deleted_is_non_empty() {
        let cs = ChangeSet {
            deleted: vec![RelPath("gone".to_string())],
            ..ChangeSet::empty()
        };
        assert!(!cs.is_empty());
    }

    // --- DiffTest.scala ---

    #[test]
    fn a_new_path_is_classified_as_added() {
        let cs = diff(&CatalogView::empty(), &[sf_hash("a", "h1")]);
        assert_eq!(paths(&cs.added), vec!["a"]);
        assert!(cs.modified.is_empty() && cs.deleted.is_empty() && cs.unchanged.is_empty());
    }

    #[test]
    fn the_same_path_with_a_different_hash_is_modified() {
        let cs = diff(&view_of(&[entry("a", "old")]), &[sf_hash("a", "new")]);
        assert_eq!(
            cs.modified
                .iter()
                .map(|f| f.content_hash.clone())
                .collect::<Vec<_>>(),
            vec![ContentHash("new".to_string())]
        );
        assert!(cs.added.is_empty());
    }

    #[test]
    fn the_same_path_and_hash_is_unchanged() {
        let cs = diff(&view_of(&[entry("a", "h")]), &[sf_hash("a", "h")]);
        assert_eq!(paths(&cs.unchanged), vec!["a"]);
        assert!(cs.added.is_empty() && cs.modified.is_empty());
    }

    #[test]
    fn a_catalog_path_absent_from_the_scan_is_deleted() {
        let cs = diff(&view_of(&[entry("a", "h")]), &[]);
        assert_eq!(cs.deleted, vec![RelPath("a".to_string())]);
    }

    #[test]
    fn outputs_are_sorted_by_rel_path_for_determinism() {
        let cs = diff(
            &CatalogView::empty(),
            &[sf_hash("b", "h"), sf_hash("a", "h")],
        );
        assert_eq!(paths(&cs.added), vec!["a", "b"]);
    }

    #[test]
    fn a_mixed_scenario_classifies_every_file_exactly_once() {
        let catalog = view_of(&[entry("keep", "h"), entry("edit", "old"), entry("rm", "h")]);
        let scan = [
            sf_hash("keep", "h"),
            sf_hash("edit", "new"),
            sf_hash("add", "h"),
        ];
        let cs = diff(&catalog, &scan);
        assert_eq!(paths(&cs.added), vec!["add"]);
        assert_eq!(paths(&cs.modified), vec!["edit"]);
        assert_eq!(paths(&cs.unchanged), vec!["keep"]);
        assert_eq!(cs.deleted, vec![RelPath("rm".to_string())]);
    }
}
