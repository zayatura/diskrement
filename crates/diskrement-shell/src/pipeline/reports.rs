//! Pipeline reports: plain data the CLI renders (`Pipeline.scala`'s result
//! types). The pipeline returns these and performs no path resolution.

use std::path::PathBuf;

use chrono::{DateTime, Utc};

use diskrement_core::DiscState;

/// `ScanResult` — `hashed` is `None` when a staged scan was reused.
#[derive(Debug)]
pub struct ScanReport {
    pub added: usize,
    pub modified: usize,
    pub deleted: usize,
    pub unchanged: usize,
    pub hashed: Option<usize>,
    pub scanned_files: usize,
}

/// `StatusReport`.
#[derive(Debug)]
pub struct StatusReport {
    pub discs: Vec<DiscState>,
    pub open_disc_ordinal: Option<u32>,
    pub total_current_files: usize,
    pub last_burn_at: Option<DateTime<Utc>>,
}

use diskrement_core::{DiscType, PlannedDisc, RelPath, ScannedFile};

/// How much outstanding change did NOT fit on the disc planned this cycle
/// (`Pipeline.scala`'s `RemainingEstimate`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemainingEstimate {
    pub more_discs: usize,
    pub more_files: usize,
    pub more_bytes: u64,
}

/// The single disc this cycle will write, plus what spills to later cycles
/// (`Pipeline.scala`'s `PlanReport`).
///
/// `tombstones` are the whole scan's deletions rather than this disc's: on a
/// cross-cycle spill they attach to the session actually written this cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanReport {
    pub disc: PlannedDisc,
    pub disc_type: DiscType,
    pub tombstones: Vec<RelPath>,
    pub remaining: RemainingEstimate,
    pub oversized: Vec<ScannedFile>,
}

/// What one disc's burn produced (`Pipeline.scala`'s `DiscBurn`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscBurn {
    pub ordinal: u32,
    pub label: String,
    pub finalized: bool,
    pub staging_dir: PathBuf,
    pub image_bytes: u64,
}

/// The result of one burn cycle: exactly one disc, plus what still remains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BurnReport {
    pub disc: DiscBurn,
    pub remaining: RemainingEstimate,
}

/// One disc's verification result (`Pipeline.scala`'s `VerifyReport`).
///
/// `skipped` is not the same as "zero files checked": a sealed disc whose last
/// session placed no files is a legitimate clean zero-file check, while a disc
/// with no staged data at all was never looked at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyReport {
    pub disc_ordinal: u32,
    pub files_checked: usize,
    pub mismatches: Vec<diskrement_core::RelPath>,
    pub skipped: bool,
}
