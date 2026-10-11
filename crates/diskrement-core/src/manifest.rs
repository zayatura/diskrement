//! The pure manifest data for one session burned onto one disc.

use crate::model::{ContentHash, RelPath, ScannedFile};
use crate::planner::PlannedDisc;
use crate::replication::DiscContents;

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
/// `copied` holds files this session placed as another copy of a version an
/// earlier session — on any disc — already recorded. Serialization to JSON is
/// a shell concern and is not part of this crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionManifest {
    pub project_name: String,
    pub disc_ordinal: u32,
    pub copy_set: u32,
    pub session_ordinal: u32,
    pub added: Vec<ManifestEntry>,
    pub modified: Vec<ManifestEntry>,
    pub copied: Vec<ManifestEntry>,
    pub deleted: Vec<RelPath>,
}

/// Build the manifest for `disc` from its commit-time classification.
///
/// `tombstones` are the cycle's deletions. Every disc of the cycle lists them,
/// so each disc describes the cycle on its own, even though the catalog
/// records each deletion once.
pub fn build(
    project_name: &str,
    session_ordinal: u32,
    disc: &PlannedDisc,
    contents: &DiscContents,
    tombstones: Vec<RelPath>,
) -> SessionManifest {
    SessionManifest {
        project_name: project_name.to_string(),
        disc_ordinal: disc.ordinal,
        copy_set: disc.copy_set,
        session_ordinal,
        added: contents.added.iter().map(to_entry).collect(),
        modified: contents.modified.iter().map(to_entry).collect(),
        copied: contents.copied.iter().map(to_entry).collect(),
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
    use crate::model::{ContentHash, RelPath};
    use crate::planner::PlannedDisc;
    use crate::replication::DiscContents;
    use crate::testutil::sf;

    #[test]
    fn a_manifest_records_added_modified_and_copied_files_and_its_set() {
        let contents = DiscContents {
            added: vec![sf("new1", 10)],
            modified: vec![sf("edit1", 10)],
            copied: vec![sf("copy1", 10)],
        };
        let disc = PlannedDisc {
            ordinal: 1,
            copy_set: 1,
            is_new: true,
            will_finalize: false,
            files: vec![sf("copy1", 10), sf("edit1", 10), sf("new1", 10)],
            prior_used_bytes: 0,
            usable_bytes: 100,
        };
        let m = build(
            "photos",
            1,
            &disc,
            &contents,
            vec![RelPath("gone".to_string())],
        );
        assert_eq!(m.project_name, "photos");
        assert_eq!((m.disc_ordinal, m.copy_set, m.session_ordinal), (1, 1, 1));
        let names =
            |v: &[ManifestEntry]| v.iter().map(|e| e.rel_path.0.clone()).collect::<Vec<_>>();
        assert_eq!(names(&m.added), vec!["new1"]);
        assert_eq!(names(&m.modified), vec!["edit1"]);
        assert_eq!(names(&m.copied), vec!["copy1"]);
        assert_eq!(m.deleted, vec![RelPath("gone".to_string())]);
    }

    #[test]
    fn a_manifest_entry_carries_hash_and_size() {
        let f = sf("a", 42);
        let contents = DiscContents {
            added: vec![f.clone()],
            ..DiscContents::default()
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
        let m = build("p", 1, &disc, &contents, vec![]);
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
