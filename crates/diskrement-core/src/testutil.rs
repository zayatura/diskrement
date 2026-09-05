//! Test-only fixtures shared by the unit test modules, and (from Task 7) the
//! proptest strategies that replace `Generators.scala`.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use proptest::prelude::*;

use crate::diff::ChangeSet;
use crate::model::{Capacity, CatalogEntry, CatalogView, ContentHash, RelPath, ScannedFile};
use crate::planner::{DiscState, DiscStatus};

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

/// A change set holding only additions, mirroring the `changesOf` / `added`
/// helpers in `PlannerTest.scala` and `PlannerEdgeTest.scala`.
pub fn added(files: Vec<ScannedFile>) -> ChangeSet {
    ChangeSet {
        added: files,
        ..ChangeSet::empty()
    }
}

/// A rel path of 1-4 identifier-ish segments joined by '/', mirroring
/// `Generators.genRelPath`.
pub fn rel_path_strategy() -> impl Strategy<Value = RelPath> {
    proptest::collection::vec("[a-zA-Z][a-zA-Z0-9_]{0,7}", 1..=4)
        .prop_map(|segs| RelPath(segs.join("/")))
}

/// An 8-character hex hash, mirroring `Generators.genHash`.
pub fn content_hash_strategy() -> impl Strategy<Value = ContentHash> {
    "[0-9a-f]{8}".prop_map(ContentHash)
}

/// A scanned file with a positive size, mirroring `Generators.genScannedFile`.
pub fn scanned_file_strategy() -> impl Strategy<Value = ScannedFile> {
    (
        rel_path_strategy(),
        content_hash_strategy(),
        1u64..=5_000_000_000,
    )
        .prop_map(|(rel_path, content_hash, size_bytes)| ScannedFile {
            rel_path,
            content_hash,
            size_bytes,
            mtime: epoch(),
        })
}

/// Scanned files with unique rel paths, mirroring `Generators.genScannedFiles`.
///
/// The Scala deduped with `groupBy(_.relPath).values.map(_.head)`, which loses
/// input order; keeping first-seen order costs nothing and makes shrinking
/// reproducible.
pub fn scanned_files_strategy() -> impl Strategy<Value = Vec<ScannedFile>> {
    proptest::collection::vec(scanned_file_strategy(), 0..12).prop_map(|files| {
        let mut seen: HashSet<RelPath> = HashSet::new();
        files
            .into_iter()
            .filter(|f| seen.insert(f.rel_path.clone()))
            .collect()
    })
}

/// A capacity comfortably larger than the largest generated file, so that
/// "oversized" stays a deliberate, separate concern.
pub fn new_disc_cap_strategy() -> impl Strategy<Value = Capacity> {
    (1_000_000_000u64..=10_000_000_000).prop_map(Capacity::new)
}

/// Files guaranteed to fit on a disc of the given capacity.
pub fn files_within_strategy(cap: Capacity) -> impl Strategy<Value = Vec<ScannedFile>> {
    scanned_files_strategy().prop_map(move |files| {
        files
            .into_iter()
            .map(|mut f| {
                f.size_bytes = (f.size_bytes % cap.usable_bytes).max(1);
                f
            })
            .collect()
    })
}

/// A pre-existing disc chain: zero to four discs with ascending ordinals, each
/// FINALIZED except possibly the last, which may be OPEN.
///
/// This mirrors the catalog's auto-finalize invariant — only the newest disc
/// stays open — and exercises multi-disc catalogs and open discs at ordinals
/// other than 1.
pub fn disc_states_strategy(cap: Capacity) -> impl Strategy<Value = Vec<DiscState>> {
    (
        proptest::collection::vec(0u64..=cap.usable_bytes, 0..=4),
        any::<bool>(),
    )
        .prop_map(move |(useds, last_open)| {
            let n = useds.len();
            useds
                .into_iter()
                .enumerate()
                .map(|(idx, used_bytes)| DiscState {
                    ordinal: idx as u32 + 1,
                    status: if idx + 1 == n && last_open {
                        DiscStatus::Open
                    } else {
                        DiscStatus::Finalized
                    },
                    usable_bytes: cap.usable_bytes,
                    used_bytes,
                })
                .collect()
        })
}

/// A full planner input: capacity, a pre-existing disc chain, and changes that
/// fit. Mirrors `Generators.genPlannerInput`, including the arbitrary split of
/// the same files across the `added` and `modified` buckets.
pub fn planner_input_strategy() -> impl Strategy<Value = (ChangeSet, Vec<DiscState>, Capacity)> {
    new_disc_cap_strategy().prop_flat_map(|cap| {
        (
            disc_states_strategy(cap),
            files_within_strategy(cap),
            any::<proptest::sample::Index>(),
        )
            .prop_map(move |(discs, files, idx)| {
                let split = idx.index(files.len() + 1);
                let (add, modi) = files.split_at(split);
                (
                    ChangeSet {
                        added: add.to_vec(),
                        modified: modi.to_vec(),
                        ..ChangeSet::empty()
                    },
                    discs,
                    cap,
                )
            })
    })
}
