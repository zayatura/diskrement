//! The pure manifest data for one session burned onto one disc.

use std::collections::HashSet;

use crate::diff::ChangeSet;
use crate::model::{ContentHash, RelPath, ScannedFile};
use crate::planner::PlannedDisc;

/// One file's identity as recorded in a session manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestEntry {
    pub rel_path: RelPath,
    pub content_hash: ContentHash,
    pub size_bytes: u64,
}

/// How a file changed, as recorded on the disc.
///
/// Serialized as the bare uppercase name and frozen by the compatibility
/// contract (spec §3.1); `shell` renders these strings, this crate defines
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestChangeKind {
    Added,
    Modified,
    Deleted,
}

impl ManifestChangeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ManifestChangeKind::Added => "ADDED",
            ManifestChangeKind::Modified => "MODIFIED",
            ManifestChangeKind::Deleted => "DELETED",
        }
    }
}

impl std::fmt::Display for ManifestChangeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ManifestChangeKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ADDED" => Ok(ManifestChangeKind::Added),
            "MODIFIED" => Ok(ManifestChangeKind::Modified),
            "DELETED" => Ok(ManifestChangeKind::Deleted),
            other => Err(format!("unknown manifest change kind: {other}")),
        }
    }
}

/// The pure manifest data for one session burned onto one disc.
///
/// Serialization to JSON is a shell concern and is not part of this crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionManifest {
    pub project_name: String,
    pub disc_ordinal: u32,
    pub session_ordinal: u32,
    pub added: Vec<ManifestEntry>,
    pub modified: Vec<ManifestEntry>,
    pub deleted: Vec<RelPath>,
}

/// Build the manifest for `disc`, classifying its files via `changes` and
/// attaching the given `tombstones` (typically only on the last, open disc).
///
/// The partition preserves the disc's file order, which the planner already
/// sorted by rel path.
pub fn build(
    project_name: &str,
    session_ordinal: u32,
    disc: &PlannedDisc,
    changes: &ChangeSet,
    tombstones: Vec<RelPath>,
) -> SessionManifest {
    let added_paths: HashSet<&RelPath> = changes.added.iter().map(|f| &f.rel_path).collect();
    let (added_files, modified_files): (Vec<&ScannedFile>, Vec<&ScannedFile>) = disc
        .files
        .iter()
        .partition(|f| added_paths.contains(&f.rel_path));

    SessionManifest {
        project_name: project_name.to_string(),
        disc_ordinal: disc.ordinal,
        session_ordinal,
        added: added_files.into_iter().map(to_entry).collect(),
        modified: modified_files.into_iter().map(to_entry).collect(),
        deleted: tombstones,
    }
}

fn to_entry(f: &ScannedFile) -> ManifestEntry {
    ManifestEntry {
        rel_path: f.rel_path.clone(),
        content_hash: f.content_hash.clone(),
        size_bytes: f.size_bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::ChangeSet;
    use crate::model::{ContentHash, RelPath};
    use crate::planner::PlannedDisc;
    use crate::testutil::sf;

    #[test]
    fn a_manifest_splits_a_discs_files_into_added_vs_modified_using_the_change_set() {
        let added = vec![sf("new1", 10)];
        let modified = vec![sf("edit1", 10)];
        let cs = ChangeSet {
            added: added.clone(),
            modified: modified.clone(),
            ..ChangeSet::empty()
        };
        let disc = PlannedDisc {
            ordinal: 1,
            copy_set: 0,
            is_new: true,
            will_finalize: false,
            files: [added, modified].concat(),
            prior_used_bytes: 0,
            usable_bytes: 100,
        };
        let m = build("photos", 1, &disc, &cs, vec![RelPath("gone".to_string())]);
        assert_eq!(m.project_name, "photos");
        assert_eq!(m.disc_ordinal, 1);
        assert_eq!(m.session_ordinal, 1);
        assert_eq!(
            m.added
                .iter()
                .map(|e| e.rel_path.as_str())
                .collect::<Vec<_>>(),
            vec!["new1"]
        );
        assert_eq!(
            m.modified
                .iter()
                .map(|e| e.rel_path.as_str())
                .collect::<Vec<_>>(),
            vec!["edit1"]
        );
        assert_eq!(m.deleted, vec![RelPath("gone".to_string())]);
    }

    #[test]
    fn a_manifest_entry_carries_hash_and_size() {
        let f = sf("a", 42);
        let cs = ChangeSet {
            added: vec![f.clone()],
            ..ChangeSet::empty()
        };
        let disc = PlannedDisc {
            ordinal: 1,
            copy_set: 0,
            is_new: true,
            will_finalize: false,
            files: vec![f],
            prior_used_bytes: 0,
            usable_bytes: 100,
        };
        let m = build("p", 1, &disc, &cs, vec![]);
        assert_eq!(
            m.added[0],
            ManifestEntry {
                rel_path: RelPath("a".to_string()),
                content_hash: ContentHash("h-a".to_string()),
                size_bytes: 42,
            }
        );
    }

    #[test]
    fn manifest_change_kind_wire_names_round_trip() {
        // Frozen by the data-compatibility contract (spec §3.1). Nothing
        // validates these strings once they reach `manifest.json`, so this
        // test is the only thing standing between a typo and an unreadable
        // disc.
        for (kind, name) in [
            (ManifestChangeKind::Added, "ADDED"),
            (ManifestChangeKind::Modified, "MODIFIED"),
            (ManifestChangeKind::Deleted, "DELETED"),
        ] {
            assert_eq!(kind.as_str(), name);
            assert_eq!(name.parse::<ManifestChangeKind>().expect("parses"), kind);
        }
        assert!("REMOVED".parse::<ManifestChangeKind>().is_err());
    }
}
