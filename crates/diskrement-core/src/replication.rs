//! Copy sets: what each set is missing, how a disc's files land in the
//! catalog when it commits, and how far each set lags.
//!
//! A project with `copies = N` keeps N independent disc series. Every current
//! version should have a live copy in every series; these functions are the
//! whole definition of "should".

use std::collections::BTreeMap;

use crate::diff::ChangeSet;
use crate::model::{CatalogView, RelPath, ScannedFile};

/// For each copy set, the unchanged files whose current version has no live
/// copy in that set — the catch-up copies its next disc must carry. Indexed by
/// copy set; each list is sorted by rel path.
///
/// Only `unchanged` files are considered: an added or modified file is a new
/// version, which every set receives anyway.
pub fn shortfall(view: &CatalogView, changes: &ChangeSet, copies: u32) -> Vec<Vec<ScannedFile>> {
    (0..copies)
        .map(|set| {
            let mut missing: Vec<ScannedFile> = changes
                .unchanged
                .iter()
                .filter(|f| {
                    view.current
                        .get(&f.rel_path)
                        .is_some_and(|e| !e.live_sets.contains(&set))
                })
                .cloned()
                .collect();
            missing.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
            missing
        })
        .collect()
}

/// How one disc's files land in the catalog at the moment the disc commits.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DiscContents {
    /// No current version: a new `file_version`, change kind `ADDED`.
    pub added: Vec<ScannedFile>,
    /// A current version with another hash: a new `file_version`, `MODIFIED`.
    pub modified: Vec<ScannedFile>,
    /// The current version itself: one more `placement` of it, nothing else.
    pub copied: Vec<ScannedFile>,
}

/// Classify `files` against `view` as it stands when their disc commits.
///
/// Called per disc, against the live catalog, so the outcome does not depend
/// on which set commits first: in a normal cycle disc 1A creates the versions
/// and 1B's files classify as copies of them. Input order is preserved within
/// each list.
pub fn classify(view: &CatalogView, files: &[ScannedFile]) -> DiscContents {
    let mut out = DiscContents::default();
    for f in files {
        match view.current.get(&f.rel_path) {
            None => out.added.push(f.clone()),
            Some(e) if e.content_hash == f.content_hash => out.copied.push(f.clone()),
            Some(_) => out.modified.push(f.clone()),
        }
    }
    out
}

/// How far one copy set lags: current versions with no live copy in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SetCoverage {
    pub missing_files: usize,
    pub missing_bytes: u64,
}

/// Copy coverage of the whole project.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Coverage {
    /// Indexed by copy set: current versions with no live copy in that set,
    /// including those with no live copy anywhere.
    pub sets: Vec<SetCoverage>,
    /// Current versions with no live copy in any set. The next scan re-adds
    /// them (the REBURN path), but until then no set holds them.
    pub no_live_copy: SetCoverage,
}

/// Coverage of every copy set over every current version.
///
/// `current` maps each current version's path to its size: every path whose
/// latest change is not `DELETED`, whether or not a live copy remains
/// (`Catalog::fingerprints`). `view` leaves out a version with no live copy,
/// so a path in `current` but not in `view` is missing from every set and is
/// counted in `no_live_copy`.
pub fn coverage(view: &CatalogView, current: &BTreeMap<RelPath, u64>, copies: u32) -> Coverage {
    let mut no_live_copy = SetCoverage::default();
    for (path, &size) in current {
        if !view.current.contains_key(path) {
            no_live_copy.missing_files += 1;
            no_live_copy.missing_bytes += size;
        }
    }
    let sets = (0..copies)
        .map(|set| {
            let mut c = no_live_copy;
            for e in view.current.values() {
                if !e.live_sets.contains(&set) {
                    c.missing_files += 1;
                    c.missing_bytes += e.size_bytes;
                }
            }
            c
        })
        .collect();
    Coverage { sets, no_live_copy }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::diff;
    use crate::testutil::{entry_in, sf, sf_hash, view_of};

    fn paths(files: &[ScannedFile]) -> Vec<&str> {
        files.iter().map(|f| f.rel_path.as_str()).collect()
    }

    #[test]
    fn an_unchanged_file_missing_from_a_set_is_that_sets_shortfall() {
        let view = view_of(&[
            entry_in("a", "h-a", 1, &[0]),
            entry_in("b", "h-b", 1, &[0, 1]),
        ]);
        let changes = diff(&view, &[sf_hash("a", "h-a"), sf_hash("b", "h-b")]);
        let s = shortfall(&view, &changes, 2);
        assert!(s[0].is_empty());
        assert_eq!(paths(&s[1]), vec!["a"]);
    }

    #[test]
    fn new_and_modified_files_are_never_shortfall() {
        let view = view_of(&[entry_in("m", "old", 1, &[0])]);
        let changes = diff(&view, &[sf_hash("m", "new"), sf_hash("n", "h")]);
        assert_eq!(shortfall(&view, &changes, 2), vec![vec![], vec![]]);
    }

    #[test]
    fn a_single_copy_project_has_one_set_and_no_shortfall_after_a_burn() {
        let view = view_of(&[entry_in("a", "h-a", 1, &[0])]);
        let changes = diff(&view, &[sf_hash("a", "h-a")]);
        assert_eq!(shortfall(&view, &changes, 1), vec![vec![]]);
    }

    #[test]
    fn classify_splits_added_modified_and_copied() {
        let view = view_of(&[
            entry_in("same", "h-same", 1, &[0]),
            entry_in("edit", "old", 1, &[0]),
        ]);
        let files = vec![sf("edit", 1), sf("new", 1), sf("same", 1)];
        let c = classify(&view, &files);
        assert_eq!(paths(&c.added), vec!["new"]);
        assert_eq!(paths(&c.modified), vec!["edit"]);
        assert_eq!(paths(&c.copied), vec!["same"]);
    }

    fn sizes(entries: &[(&str, u64)]) -> BTreeMap<RelPath, u64> {
        entries
            .iter()
            .map(|(p, s)| (RelPath((*p).to_string()), *s))
            .collect()
    }

    fn missing(files: usize, bytes: u64) -> SetCoverage {
        SetCoverage {
            missing_files: files,
            missing_bytes: bytes,
        }
    }

    #[test]
    fn coverage_counts_files_and_bytes_missing_per_set() {
        let view = view_of(&[entry_in("a", "h", 10, &[0]), entry_in("b", "h", 5, &[1])]);
        let c = coverage(&view, &sizes(&[("a", 10), ("b", 5)]), 2);
        assert_eq!(c.sets, vec![missing(1, 5), missing(1, 10)]);
        assert_eq!(c.no_live_copy, SetCoverage::default());
    }

    #[test]
    fn a_current_version_with_no_live_copy_is_missing_from_every_set() {
        // "gone" is current in the history, but every copy of it died, so
        // the view leaves it out.
        let view = view_of(&[entry_in("a", "h", 10, &[0, 1])]);
        let c = coverage(&view, &sizes(&[("a", 10), ("gone", 7)]), 2);
        assert_eq!(c.no_live_copy, missing(1, 7));
        assert_eq!(c.sets, vec![missing(1, 7), missing(1, 7)]);
    }

    #[test]
    fn a_single_copy_project_counts_a_file_with_no_live_copy() {
        let c = coverage(&view_of(&[]), &sizes(&[("gone", 7)]), 1);
        assert_eq!(c.sets, vec![missing(1, 7)]);
        assert_eq!(c.no_live_copy, missing(1, 7));
    }

    #[test]
    fn a_fully_covered_catalog_has_nothing_missing() {
        let view = view_of(&[entry_in("a", "h", 10, &[0, 1])]);
        assert_eq!(
            coverage(&view, &sizes(&[("a", 10)]), 2),
            Coverage {
                sets: vec![SetCoverage::default(); 2],
                no_live_copy: SetCoverage::default(),
            }
        );
    }

    use crate::testutil::scanned_files_strategy;
    use proptest::prelude::*;

    proptest! {
        /// For every set, the files it receives this cycle (new ∪ shortfall)
        /// and the files it already holds partition the scan exactly: nothing
        /// is skipped and nothing lands twice in one set.
        #[test]
        fn each_set_receives_exactly_what_it_does_not_already_hold(
            files in scanned_files_strategy(),
            held in proptest::collection::vec(0u8..4, 0..12),
            copies in 1u32..=3,
        ) {
            // Burned history: file i is live in the sets whose bit is set in
            // held[i]; files beyond `held`, or with no bit set, are new.
            let mut entries = Vec::new();
            for (f, bits) in files.iter().zip(held.iter()) {
                let sets: Vec<u32> = (0..copies).filter(|s| bits & (1 << s) != 0).collect();
                if !sets.is_empty() {
                    entries.push(entry_in(f.rel_path.as_str(), f.content_hash.as_str(), f.size_bytes, &sets));
                }
            }
            let view = view_of(&entries);
            let changes = diff(&view, &files);
            let short = shortfall(&view, &changes, copies);
            for set in 0..copies {
                let mut received: Vec<&str> = changes.added.iter()
                    .chain(changes.modified.iter())
                    .chain(short[set as usize].iter())
                    .map(|f| f.rel_path.as_str())
                    .collect();
                let mut held_here: Vec<&str> = files.iter()
                    .filter(|f| view.current.get(&f.rel_path)
                        .is_some_and(|e| e.live_sets.contains(&set)))
                    .map(|f| f.rel_path.as_str())
                    .collect();
                let mut all: Vec<&str> = received.clone();
                all.append(&mut held_here.clone());
                all.sort_unstable();
                let mut scan: Vec<&str> = files.iter().map(|f| f.rel_path.as_str()).collect();
                scan.sort_unstable();
                prop_assert_eq!(&all, &scan);
                received.sort_unstable();
                held_here.sort_unstable();
                prop_assert!(received.iter().all(|p| held_here.binary_search(p).is_err()));
            }
        }

        /// Coverage against an independent oracle: a set misses exactly the
        /// current files with no live copy in it, and `no_live_copy` is
        /// exactly the current files with none in any set — which every set
        /// therefore also misses.
        #[test]
        fn coverage_counts_every_current_file_missing_from_a_set(
            files in scanned_files_strategy(),
            held in proptest::collection::vec(0u8..8, 0..12),
            copies in 1u32..=3,
        ) {
            // File i is current, live in the sets whose bit is set in
            // held[i]; no bit (within `copies`) means no live copy, so the
            // view leaves it out. Files beyond `held` are not current at all.
            let live = |bits: u8, set: u32| bits & (1 << set) != 0;
            let mut entries = Vec::new();
            let mut current = BTreeMap::new();
            for (f, &bits) in files.iter().zip(held.iter()) {
                current.insert(f.rel_path.clone(), f.size_bytes);
                let sets: Vec<u32> = (0..copies).filter(|&s| live(bits, s)).collect();
                if !sets.is_empty() {
                    entries.push(entry_in(f.rel_path.as_str(), f.content_hash.as_str(), f.size_bytes, &sets));
                }
            }
            let c = coverage(&view_of(&entries), &current, copies);
            let pairs: Vec<(&ScannedFile, u8)> =
                files.iter().zip(held.iter().copied()).collect();

            prop_assert_eq!(c.sets.len(), copies as usize);
            for set in 0..copies {
                let lacking: Vec<&ScannedFile> = pairs.iter()
                    .filter(|(_, bits)| !live(*bits, set))
                    .map(|(f, _)| *f)
                    .collect();
                prop_assert_eq!(c.sets[set as usize].missing_files, lacking.len());
                prop_assert_eq!(
                    c.sets[set as usize].missing_bytes,
                    lacking.iter().map(|f| f.size_bytes).sum::<u64>()
                );
                prop_assert!(c.sets[set as usize].missing_files >= c.no_live_copy.missing_files);
            }
            let orphans = pairs.iter()
                .filter(|(_, bits)| (0..copies).all(|s| !live(*bits, s)))
                .count();
            prop_assert_eq!(c.no_live_copy.missing_files, orphans);
        }
    }
}
