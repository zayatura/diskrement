//! Pure, deterministic ordered-fill disc planner.

use crate::model::{RelPath, ScannedFile};

/// Whether a disc can still receive new sessions.
///
/// `Dead` means the physical disc is lost or unreadable (a "coaster"). It is
/// derived from a `MARKED_DEAD` catalog event at read time and never stored in
/// the disc row itself — `disc.status` keeps its
/// `check (status in ('OPEN','FINALIZED'))` constraint. The derivation lives in
/// the catalog query (`CatalogStoreLive.readDiscStates`), so by the time a
/// `DiscState` reaches the planner, `Dead` is a legitimate value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscStatus {
    Open,
    Finalized,
    Dead,
}

/// An existing disc as known to the catalog when planning begins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscState {
    pub ordinal: u32,
    pub status: DiscStatus,
    pub usable_bytes: u64,
    pub used_bytes: u64,
}

impl DiscState {
    /// Remaining space, saturating at zero.
    ///
    /// Reporting only. The planner decides whether a file fits with an
    /// addition rather than this subtraction, because the two disagree when
    /// `used_bytes > usable_bytes` — see `Bin::fits` in Task 5.
    pub fn free_bytes(&self) -> u64 {
        self.usable_bytes.saturating_sub(self.used_bytes)
    }
}

/// One disc as it will be written by the upcoming burn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedDisc {
    pub ordinal: u32,
    pub is_new: bool,
    pub will_finalize: bool,
    pub files: Vec<ScannedFile>,
    pub prior_used_bytes: u64,
    pub usable_bytes: u64,
}

impl PlannedDisc {
    pub fn added_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.size_bytes).sum()
    }

    pub fn total_used_bytes(&self) -> u64 {
        self.prior_used_bytes + self.added_bytes()
    }
}

/// The full plan for the next burn session(s).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BurnPlan {
    pub discs: Vec<PlannedDisc>,
    pub tombstones: Vec<RelPath>,
    pub oversized: Vec<ScannedFile>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::RelPath;
    use crate::testutil::sf;

    #[test]
    fn disc_state_reports_free_bytes() {
        let d = DiscState {
            ordinal: 1,
            status: DiscStatus::Open,
            usable_bytes: 100,
            used_bytes: 30,
        };
        assert_eq!(d.free_bytes(), 70);
    }

    #[test]
    fn an_overfull_disc_reports_zero_free_rather_than_underflowing() {
        // The Scala returned a negative `Long` here. Nothing renders this
        // value, so saturating at zero is a safe choice for `free_bytes` — but
        // it is NOT a safe basis for deciding whether a file fits. See
        // `a_zero_byte_file_does_not_fit_an_overfull_disc` in Task 5.
        let d = DiscState {
            ordinal: 1,
            status: DiscStatus::Open,
            usable_bytes: 100,
            used_bytes: 130,
        };
        assert_eq!(d.free_bytes(), 0);
    }

    #[test]
    fn planned_disc_reports_added_and_total_bytes() {
        let f = sf("a", 25);
        let pd = PlannedDisc {
            ordinal: 2,
            is_new: true,
            will_finalize: false,
            files: vec![f.clone(), f],
            prior_used_bytes: 10,
            usable_bytes: 100,
        };
        assert_eq!(pd.added_bytes(), 50);
        assert_eq!(pd.total_used_bytes(), 60);
    }

    #[test]
    fn burn_plan_groups_discs_tombstones_and_oversized() {
        let plan = BurnPlan {
            discs: vec![],
            tombstones: vec![RelPath("gone".to_string())],
            oversized: vec![],
        };
        assert_eq!(plan.tombstones, vec![RelPath("gone".to_string())]);
    }
}
