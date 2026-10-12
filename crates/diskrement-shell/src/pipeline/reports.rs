//! Pipeline reports: plain data the CLI renders (`Pipeline.scala`'s result
//! types). The pipeline returns these and performs no path resolution.

use std::path::PathBuf;

use chrono::{DateTime, Utc};

use diskrement_core::{Coverage, DiscKey, DiscState, DiscType, PlannedDisc, RelPath, ScannedFile};

/// What `plan`'s scan found. `hashed` counts the files hashed this run; the rest reused a fingerprint.
#[derive(Debug)]
pub struct ScanReport {
    pub added: usize,
    pub modified: usize,
    pub deleted: usize,
    pub unchanged: usize,
    pub hashed: usize,
    pub scanned_files: usize,
}

/// What `plan` returns: the scan it took and the plan it staged (#4).
#[derive(Debug)]
pub struct PlanOutcome {
    pub scan: ScanReport,
    pub plan: PlanReport,
}

/// `StatusReport`. `open_discs` holds each copy set's open disc, if it has
/// one; `coverage` is indexed by copy set.
#[derive(Debug)]
pub struct StatusReport {
    pub discs: Vec<DiscState>,
    pub open_discs: Vec<DiscKey>,
    /// Every current version, including those with no live copy left.
    pub total_current_files: usize,
    pub last_burn_at: Option<DateTime<Utc>>,
    pub coverage: Coverage,
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
/// set order. `sets` may be empty (nothing to place); the tombstones are then
/// deferred to the next disc that is burned anyway (issue #9).
///
/// `tombstones` are the whole scan's deletions, computed against the catalog
/// history (`diff::tombstones`), not the live view. Every disc's manifest lists
/// them; the catalog records each one once, on the first disc to commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanReport {
    pub copies: u32,
    pub sets: Vec<SetPlan>,
    pub disc_type: DiscType,
    pub tombstones: Vec<RelPath>,
    pub oversized: Vec<ScannedFile>,
    /// Catch-up copies too large for `disc_type`: skipped, never refused
    /// (issue #12). Sorted by path, each listed once.
    pub unplaceable_catch_up: Vec<ScannedFile>,
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
/// `discs` can be empty for two reasons. Either every planned disc had already
/// been committed by an earlier, interrupted run (`deferred_deletions` is
/// `None`), or the plan had no disc at all (`deferred_deletions` is `Some`,
/// holding the number of deletions left for the next burned disc, issue #9).
/// A zero-set `plan.json` can only come from `plan`: `burn` rewrites the plan
/// only while discs remain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BurnReport {
    pub discs: Vec<DiscBurn>,
    pub remaining: RemainingEstimate,
    pub coverage: Coverage,
    pub deferred_deletions: Option<usize>,
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
