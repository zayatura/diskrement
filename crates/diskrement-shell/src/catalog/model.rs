//! The row and update types the catalog reads and writes.
//!
//! These are the Scala `diskrement.shell.model` package: values that exist only
//! because something is persisted. Domain types the planner and manifest
//! builder share — `RelPath`, `ContentHash`, `DiscType`, `DiscStatus` — stay in
//! `diskrement-core`.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use diskrement_core::{ContentHash, DiscKey, DiscStatus, DiscType, ManifestChangeKind, RelPath};

/// Primary key of a project row (a SQLite rowid).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectId(pub i64);

/// Fields needed to create a project (no id yet).
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectInit {
    pub name: String,
    pub source_root: String,
    pub work_dir: String,
    pub default_disc_type: DiscType,
    pub margin_percent: f64,
    pub copies: u32,
    pub created_at: DateTime<Utc>,
}

/// A persisted project row.
#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub source_root: String,
    pub work_dir: String,
    pub default_disc_type: DiscType,
    pub margin_percent: f64,
    pub copies: u32,
    pub created_at: DateTime<Utc>,
}

/// One file written this run: its identity and where it landed. Its
/// `copy_index` is its disc's copy set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileWrite {
    pub rel_path: RelPath,
    pub content_hash: ContentHash,
    pub size_bytes: u64,
    pub mtime: DateTime<Utc>,
    pub disc_path: String,
}

/// One session burned onto one disc this run.
///
/// `deleted` holds tombstones, which reference the *current* version of each
/// path rather than carrying a version of their own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionWrite {
    pub ordinal: u32,
    pub burned_at: DateTime<Utc>,
    pub manifest_hash: String,
    pub added: Vec<FileWrite>,
    pub modified: Vec<FileWrite>,
    /// Further copies of versions already recorded: one `placement` each, no
    /// new `file_version`, no `change_record`.
    pub copied: Vec<FileWrite>,
    pub deleted: Vec<RelPath>,
}

/// One disc touched or created this run, with its single new session.
///
/// `disc_type`, `usable_bytes` and `label` are persisted only when `is_new` is
/// true; for an existing disc they are ignored and the stored row is kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscWrite {
    pub ordinal: u32,
    /// The copy set of the disc (0 = `A`). Persisted only when `is_new`.
    pub copy_set: u32,
    pub is_new: bool,
    pub disc_type: DiscType,
    pub usable_bytes: u64,
    /// The resulting status: `Open` for the last disc, `Finalized` otherwise.
    pub status: DiscStatus,
    pub label: String,
    pub session: SessionWrite,
}

/// The full set of catalog changes from one successful burn run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogUpdate {
    pub project_id: ProjectId,
    /// Pre-existing OPEN discs to flip to FINALIZED.
    pub finalize_disc_ordinals: Vec<DiscKey>,
    pub discs: Vec<DiscWrite>,
}

/// Lifecycle state of a burn session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Planned,
    Burned,
    Verified,
}

impl SessionState {
    /// The catalog spelling, constrained by `check (state in (...))`.
    pub fn as_str(self) -> &'static str {
        match self {
            SessionState::Planned => "PLANNED",
            SessionState::Burned => "BURNED",
            SessionState::Verified => "VERIFIED",
        }
    }
}

impl std::fmt::Display for SessionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for SessionState {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "PLANNED" => Ok(SessionState::Planned),
            "BURNED" => Ok(SessionState::Burned),
            "VERIFIED" => Ok(SessionState::Verified),
            other => Err(format!("unknown session state: {other}")),
        }
    }
}

/// A persisted session row, joined to its disc's key for convenience.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub disc: DiscKey,
    pub ordinal: u32,
    pub state: SessionState,
    pub burned_at: Option<DateTime<Utc>>,
    pub manifest_hash: Option<String>,
}

/// The prior-version identity the scanner needs to decide whether a file is
/// unchanged (size + mtime match -> reuse the stored content hash).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileFingerprint {
    pub size_bytes: u64,
    pub mtime: DateTime<Utc>,
    pub content_hash: ContentHash,
}

/// One non-current (superseded or deleted) file version and where its bytes
/// physically live. `kind` is the latest change-record kind for that version:
/// `Added`/`Modified` for a superseded older version, `Deleted` for a
/// tombstoned one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivePointer {
    pub rel_path: RelPath,
    pub content_hash: ContentHash,
    pub size_bytes: u64,
    pub disc_ordinal: u32,
    pub copy_set: u32,
    pub session_ordinal: u32,
    pub disc_path: String,
    pub kind: ManifestChangeKind,
}

/// Lifecycle status of one file version relative to its path's current state.
///
/// `Lost` = a non-current version whose disc was marked dead: the bytes are
/// gone. It is per *physical copy*, not per content hash — after a recovery
/// burn two versions sharing a hash can legitimately read `Lost` and `Current`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionStatus {
    Current,
    Superseded,
    Deleted,
    Lost,
}

impl VersionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            VersionStatus::Current => "CURRENT",
            VersionStatus::Superseded => "SUPERSEDED",
            VersionStatus::Deleted => "DELETED",
            VersionStatus::Lost => "LOST",
        }
    }
}

impl std::fmt::Display for VersionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One version of a path in its full history, with where its bytes live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileHistoryEntry {
    pub content_hash: ContentHash,
    pub size_bytes: u64,
    pub disc: DiscKey,
    pub session_ordinal: u32,
    pub status: VersionStatus,
}

/// A file version on a disc hidden by a later session of the SAME disc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupersededVersion {
    pub session_ordinal: u32,
    pub rel_path: RelPath,
    pub content_hash: ContentHash,
}

/// Everything the catalog records as physically on one disc: the merged view a
/// mount shows (`visible`), and versions hidden within the disc
/// (`superseded`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscRecordedContents {
    pub visible: BTreeMap<RelPath, ContentHash>,
    pub superseded: Vec<SupersededVersion>,
}

/// A version whose only copy was on the now-dead disc and whose path has moved
/// past it — those bytes are permanently gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LostVersionEntry {
    pub rel_path: RelPath,
    pub content_hash: ContentHash,
    pub size_bytes: u64,
    pub session_ordinal: u32,
}

/// A live file whose only burned copy was on the dead disc; the next
/// scan/plan/burn writes it again from the source. Also used for
/// `LostReport::degraded`, where the file still has a live copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReburnEntry {
    pub rel_path: RelPath,
    pub content_hash: ContentHash,
    pub size_bytes: u64,
}

/// What marking a disc dead cost. `lost` and `reburn` are recorded in
/// `lost_version`; `degraded` and `superseded_survivors` are derived and only
/// reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LostReport {
    pub disc: DiscKey,
    pub lost: Vec<LostVersionEntry>,
    pub reburn: Vec<ReburnEntry>,
    /// Current versions that still have a live copy on another disc. The next
    /// cycle restores the missing copy.
    pub degraded: Vec<ReburnEntry>,
    /// Superseded or deleted versions that still have a live copy elsewhere.
    /// They are not lost, and not restored either: the source moved past them.
    pub superseded_survivors: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_state_wire_names_round_trip() {
        // `session.state` carries a check constraint over exactly these three.
        for (state, name) in [
            (SessionState::Planned, "PLANNED"),
            (SessionState::Burned, "BURNED"),
            (SessionState::Verified, "VERIFIED"),
        ] {
            assert_eq!(state.as_str(), name);
            assert_eq!(name.parse::<SessionState>().expect("parses"), state);
        }
        assert!("DONE".parse::<SessionState>().is_err());
    }

    #[test]
    fn version_status_renders_the_names_the_cli_prints() {
        // Not a stored value — `file_history` derives it — but `history`
        // prints it, so the spellings are part of the render oracle.
        assert_eq!(VersionStatus::Current.as_str(), "CURRENT");
        assert_eq!(VersionStatus::Superseded.as_str(), "SUPERSEDED");
        assert_eq!(VersionStatus::Deleted.as_str(), "DELETED");
        assert_eq!(VersionStatus::Lost.as_str(), "LOST");
    }
}
