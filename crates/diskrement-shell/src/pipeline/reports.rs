//! Pipeline reports: plain data the CLI renders (`Pipeline.scala`'s result
//! types). The pipeline returns these and performs no path resolution.

use std::path::PathBuf;

use chrono::{DateTime, Utc};

use diskrement_core::{
    DiscKey, DiscState, DiscType, PlannedDisc, RelPath, ScannedFile, SetCoverage,
};

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

/// `StatusReport`. `open_discs` holds each copy set's open disc, if it has
/// one; `coverage` is indexed by copy set.
#[derive(Debug)]
pub struct StatusReport {
    pub discs: Vec<DiscState>,
    pub open_discs: Vec<DiscKey>,
    pub total_current_files: usize,
    pub last_burn_at: Option<DateTime<Utc>>,
    pub coverage: Vec<SetCoverage>,
}

/// How much outstanding change did NOT fit on the disc planned this cycle
/// (`Pipeline.scala`'s `RemainingEstimate`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemainingEstimate {
    pub more_discs: usize,
    pub more_files: usize,
    pub more_bytes: u64,
}

/// One copy set's share of the cycle: the disc it writes, plus what spills to
/// later cycles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetPlan {
    pub disc: PlannedDisc,
    /// How many of `disc.files` are catch-up copies rather than new files.
    pub catch_up: usize,
    /// The session ordinal this burn writes onto the disc (1 for a new disc).
    /// A catalog already one past it means the disc was committed before the
    /// staged plan could be rewritten, and `burn` skips it.
    pub session_ordinal: u32,
    pub remaining: RemainingEstimate,
}

/// The discs this cycle writes — one per copy set with something to do — in
/// set order.
///
/// `tombstones` are the whole scan's deletions. Every disc's manifest lists
/// them; the catalog records each one once, on the first disc to commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanReport {
    pub copies: u32,
    pub sets: Vec<SetPlan>,
    pub disc_type: DiscType,
    pub tombstones: Vec<RelPath>,
    pub oversized: Vec<ScannedFile>,
}

/// What one disc's burn produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscBurn {
    pub disc: DiscKey,
    pub label: String,
    pub finalized: bool,
    /// Where this disc was staged. With the real backend the tree has already
    /// been removed by the time the report is returned (issue #3); with the
    /// simulated backend it stays, as the disc `verify` reads back.
    pub staging_dir: PathBuf,
    pub image_bytes: u64,
}

/// The result of one burn cycle: every disc written this run, the largest
/// remaining estimate among their sets, and each set's coverage afterwards.
///
/// `discs` can be empty when every planned disc had already been committed by
/// an earlier, interrupted run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BurnReport {
    pub discs: Vec<DiscBurn>,
    pub remaining: RemainingEstimate,
    pub coverage: Vec<SetCoverage>,
}

/// One disc's verification result (`Pipeline.scala`'s `VerifyReport`).
///
/// `skipped` is not the same as "zero files checked": a sealed disc whose last
/// session placed no files is a legitimate clean zero-file check, while a disc
/// with no staged data at all was never looked at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyReport {
    pub disc: DiscKey,
    pub files_checked: usize,
    pub mismatches: Vec<diskrement_core::RelPath>,
    pub skipped: bool,
}
