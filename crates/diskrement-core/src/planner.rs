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

use crate::diff::ChangeSet;
use crate::model::Capacity;

/// A disc being filled. Local to the planner: ordinals are positional and are
/// assigned only after packing finishes.
struct Bin {
    is_new: bool,
    usable_bytes: u64,
    prior_used_bytes: u64,
    files: Vec<ScannedFile>,
}

impl Bin {
    fn fresh(cap: Capacity) -> Self {
        Bin {
            is_new: true,
            usable_bytes: cap.usable_bytes,
            prior_used_bytes: 0,
            files: Vec::new(),
        }
    }

    fn used_bytes(&self) -> u64 {
        self.prior_used_bytes + self.files.iter().map(|f| f.size_bytes).sum::<u64>()
    }

    /// Whether `f` fits in what is left of this disc.
    ///
    /// Deliberately an addition rather than `f.size_bytes <= free_bytes()`: the
    /// two disagree on an overfull disc, where a saturating `free_bytes()` of 0
    /// would accept a zero-byte file that the Scala's signed comparison
    /// rejects. `checked_add` keeps the addition form from wrapping.
    fn fits(&self, f: &ScannedFile) -> bool {
        self.used_bytes()
            .checked_add(f.size_bytes)
            .is_some_and(|total| total <= self.usable_bytes)
    }
}

/// Plan the next burn: top off the open disc, then spill onto fresh discs.
///
/// Deterministic — the result depends only on the set of placeable files, not
/// on their order, and not on how they were split between `added` and
/// `modified`.
pub fn plan(changes: &ChangeSet, discs: &[DiscState], new_disc_cap: Capacity) -> BurnPlan {
    let mut to_place: Vec<ScannedFile> = changes
        .added
        .iter()
        .chain(changes.modified.iter())
        .cloned()
        .collect();
    to_place.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));

    // The newest open disc is the one to top off. A dead or finalized disc is
    // never a candidate, however much room it appears to have.
    let existing_open = discs
        .iter()
        .filter(|d| d.status == DiscStatus::Open)
        .max_by_key(|d| d.ordinal);
    let max_ordinal = discs.iter().map(|d| d.ordinal).max().unwrap_or(0);

    // The largest disc a file could ever land on this run: a fresh blank disc,
    // or the open disc at its full capacity. A file exceeding both can never be
    // placed and is oversized regardless of fill order.
    let open_usable = existing_open.map_or(0, |d| d.usable_bytes);
    let max_usable = new_disc_cap.usable_bytes.max(open_usable);

    let mut oversized: Vec<ScannedFile> = Vec::new();
    let mut placeable: Vec<ScannedFile> = Vec::new();
    for f in to_place {
        if f.size_bytes > max_usable {
            oversized.push(f);
        } else {
            placeable.push(f);
        }
    }

    let mut current = match existing_open {
        Some(d) => Bin {
            is_new: false,
            usable_bytes: d.usable_bytes,
            prior_used_bytes: d.used_bytes,
            files: Vec::new(),
        },
        None => Bin::fresh(new_disc_cap),
    };
    let mut completed: Vec<Bin> = Vec::new();

    // Place each file in order. A file that does not fit the current disc
    // spills to a fresh one; if it does not fit a fresh disc either — it only
    // fit the open disc's full capacity, not its remaining space — it is
    // oversized. This is the second stage of the oversize check.
    for f in placeable {
        if current.fits(&f) {
            current.files.push(f);
        } else if f.size_bytes <= new_disc_cap.usable_bytes {
            completed.push(std::mem::replace(&mut current, Bin::fresh(new_disc_cap)));
            current.files.push(f);
        } else {
            oversized.push(f);
        }
    }
    completed.push(current);

    let last_idx = completed.len() - 1;
    let planned_discs: Vec<PlannedDisc> = completed
        .into_iter()
        .enumerate()
        .map(|(i, b)| {
            // The first disc keeps the existing open disc's ordinal; every
            // fresh disc takes the next ordinal above the highest existing one.
            let ordinal = if !b.is_new {
                existing_open
                    .expect("a non-new bin exists only when an open disc does")
                    .ordinal
            } else if existing_open.is_some() {
                max_ordinal + i as u32
            } else {
                max_ordinal + i as u32 + 1
            };
            PlannedDisc {
                ordinal,
                is_new: b.is_new,
                will_finalize: i != last_idx,
                files: b.files,
                prior_used_bytes: b.prior_used_bytes,
                usable_bytes: b.usable_bytes,
            }
        })
        .collect();

    oversized.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));

    BurnPlan {
        discs: planned_discs,
        tombstones: changes.deleted.clone(),
        oversized,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DiscType, RelPath};
    use crate::testutil::{added, sf};

    const CAP100: Capacity = Capacity { usable_bytes: 100 };

    fn disc_paths(d: &PlannedDisc) -> Vec<&str> {
        d.files.iter().map(|f| f.rel_path.as_str()).collect()
    }

    fn all_placed(plan: &BurnPlan) -> Vec<&str> {
        plan.discs.iter().flat_map(disc_paths).collect()
    }

    // --- planner types (Task 4) ---

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
        // `a_zero_byte_file_does_not_fit_an_overfull_disc` below.
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

    // --- PlannerTest.scala ---

    #[test]
    fn all_files_fit_on_one_fresh_disc_which_stays_open() {
        let plan = plan(&added(vec![sf("a", 30), sf("b", 30)]), &[], CAP100);
        assert_eq!(plan.discs.len(), 1);
        let d = &plan.discs[0];
        assert!(d.is_new);
        assert!(!d.will_finalize);
        assert_eq!(disc_paths(d), vec!["a", "b"]);
        assert_eq!(d.ordinal, 1);
    }

    #[test]
    fn files_are_placed_in_lexicographic_order_regardless_of_input_order() {
        let plan = plan(
            &added(vec![sf("c", 10), sf("a", 10), sf("b", 10)]),
            &[],
            CAP100,
        );
        assert_eq!(disc_paths(&plan.discs[0]), vec!["a", "b", "c"]);
    }

    #[test]
    fn a_file_that_does_not_fit_spills_wholly_to_a_new_disc() {
        // 60 + 60 cannot share a 100-byte disc.
        let plan = plan(&added(vec![sf("a", 60), sf("b", 60)]), &[], CAP100);
        assert_eq!(plan.discs.len(), 2);
        assert_eq!(disc_paths(&plan.discs[0]), vec!["a"]);
        assert!(
            plan.discs[0].will_finalize,
            "spilled disc must be finalized"
        );
        assert_eq!(disc_paths(&plan.discs[1]), vec!["b"]);
        assert!(!plan.discs[1].will_finalize, "last disc stays open");
        assert_eq!(plan.discs[1].ordinal, 2);
    }

    #[test]
    fn an_existing_open_disc_is_topped_off_before_opening_a_new_disc() {
        // open disc 1: 100 usable, 90 used -> 10 free.
        let open = DiscState {
            ordinal: 1,
            status: DiscStatus::Open,
            usable_bytes: 100,
            used_bytes: 90,
        };
        let plan = plan(&added(vec![sf("a", 10), sf("b", 50)]), &[open], CAP100);
        assert_eq!(plan.discs.len(), 2);
        let topped = &plan.discs[0];
        assert_eq!(topped.ordinal, 1);
        assert!(!topped.is_new);
        assert_eq!(disc_paths(topped), vec!["a"]);
        assert!(topped.will_finalize, "spilled, so finalized");
        assert_eq!(topped.total_used_bytes(), 100);
        let fresh = &plan.discs[1];
        assert_eq!(fresh.ordinal, 2);
        assert!(fresh.is_new);
        assert_eq!(disc_paths(fresh), vec!["b"]);
        assert!(!fresh.will_finalize);
    }

    #[test]
    fn deletions_become_tombstones_and_cost_zero_bytes() {
        let cs = ChangeSet {
            deleted: vec![RelPath("x".to_string()), RelPath("y".to_string())],
            ..ChangeSet::empty()
        };
        let plan = plan(&cs, &[], CAP100);
        assert_eq!(
            plan.tombstones,
            vec![RelPath("x".to_string()), RelPath("y".to_string())]
        );
        assert!(all_placed(&plan).is_empty());
    }

    #[test]
    fn modified_files_are_planned_alongside_added_by_rel_path() {
        let cs = ChangeSet {
            added: vec![sf("a", 10)],
            modified: vec![sf("b", 10)],
            ..ChangeSet::empty()
        };
        let plan = plan(&cs, &[], CAP100);
        assert_eq!(disc_paths(&plan.discs[0]), vec!["a", "b"]);
    }

    #[test]
    fn a_file_larger_than_a_fresh_disc_is_reported_oversized_not_placed() {
        let plan = plan(&added(vec![sf("big", 150), sf("ok", 10)]), &[], CAP100);
        assert_eq!(
            plan.oversized
                .iter()
                .map(|f| f.rel_path.as_str())
                .collect::<Vec<_>>(),
            vec!["big"]
        );
        assert_eq!(all_placed(&plan), vec!["ok"]);
    }

    #[test]
    fn a_dead_disc_is_never_topped_off() {
        // The dead disc has plenty of free room but must be ignored entirely;
        // it keeps its ordinal, and the fresh disc takes the next one.
        let dead = DiscState {
            ordinal: 1,
            status: DiscStatus::Dead,
            usable_bytes: 100,
            used_bytes: 10,
        };
        let plan = plan(&added(vec![sf("a", 30)]), &[dead], CAP100);
        assert_eq!(plan.discs.len(), 1);
        let d = &plan.discs[0];
        assert!(d.is_new);
        assert!(!d.will_finalize);
        assert_eq!(d.ordinal, 2);
    }

    // --- PlannerEdgeTest.scala ---

    #[test]
    fn an_empty_change_set_yields_a_single_empty_open_disc() {
        let plan = plan(&ChangeSet::empty(), &[], CAP100);
        assert_eq!(plan.discs.len(), 1);
        assert!(plan.discs[0].files.is_empty());
        assert!(!plan.discs[0].will_finalize);
        assert!(plan.oversized.is_empty());
    }

    #[test]
    fn a_file_exactly_equal_to_free_space_fits() {
        let plan = plan(&added(vec![sf("a", 100)]), &[], CAP100);
        assert_eq!(plan.discs.len(), 1);
        assert_eq!(disc_paths(&plan.discs[0]), vec!["a"]);
        assert!(!plan.discs[0].will_finalize);
    }

    #[test]
    fn only_deletions_records_tombstones_and_fills_no_data_discs() {
        let cs = ChangeSet {
            deleted: vec![RelPath("x".to_string())],
            ..ChangeSet::empty()
        };
        let plan = plan(&cs, &[], CAP100);
        assert_eq!(plan.tombstones, vec![RelPath("x".to_string())]);
        assert!(plan.discs.iter().all(|d| d.files.is_empty()));
    }

    #[test]
    fn a_move_plans_the_add_and_tombstones_the_old_path() {
        let cs = ChangeSet {
            added: vec![sf("new/a", 10)],
            deleted: vec![RelPath("old/a".to_string())],
            ..ChangeSet::empty()
        };
        let plan = plan(&cs, &[], CAP100);
        assert_eq!(disc_paths(&plan.discs[0]), vec!["new/a"]);
        assert_eq!(plan.tombstones, vec![RelPath("old/a".to_string())]);
    }

    #[test]
    fn a_file_fitting_only_the_open_disc_is_placed_not_oversized() {
        // Open BD-50 (empty); new discs are BD-25. A 30 GB file fits only the
        // open disc, and the up-front oversize filter must know that.
        let open = DiscState {
            ordinal: 1,
            status: DiscStatus::Open,
            usable_bytes: 50_000_000_000,
            used_bytes: 0,
        };
        let plan = plan(
            &added(vec![sf("big", 30_000_000_000)]),
            &[open],
            Capacity::new(25_000_000_000),
        );
        assert!(plan.oversized.is_empty(), "fits the open disc");
        assert_eq!(plan.discs[0].ordinal, 1);
        assert_eq!(disc_paths(&plan.discs[0]), vec!["big"]);
    }

    #[test]
    fn a_file_fitting_neither_the_open_disc_nor_a_fresh_one_is_oversized() {
        // Open BD-50 with only 10 GB free; new discs BD-25; a 30 GB file fits
        // neither. This is the second stage of the oversize check.
        let open = DiscState {
            ordinal: 1,
            status: DiscStatus::Open,
            usable_bytes: 50_000_000_000,
            used_bytes: 40_000_000_000,
        };
        let plan = plan(
            &added(vec![sf("big", 30_000_000_000)]),
            &[open],
            Capacity::new(25_000_000_000),
        );
        assert_eq!(
            plan.oversized
                .iter()
                .map(|f| f.rel_path.as_str())
                .collect::<Vec<_>>(),
            vec!["big"]
        );
        assert!(all_placed(&plan).is_empty());
    }

    #[test]
    fn spilled_discs_get_ordinals_above_the_highest_existing_ordinal() {
        // Unusual catalog: OPEN disc 2 (full) but a FINALIZED disc 5 exists.
        // The spilled disc must clear the max ordinal (5), not just 2 + 1.
        let discs = [
            DiscState {
                ordinal: 2,
                status: DiscStatus::Open,
                usable_bytes: 100,
                used_bytes: 100,
            },
            DiscState {
                ordinal: 5,
                status: DiscStatus::Finalized,
                usable_bytes: 100,
                used_bytes: 100,
            },
        ];
        let plan = plan(&added(vec![sf("a", 50)]), &discs, CAP100);
        assert_eq!(
            plan.discs.iter().map(|d| d.ordinal).collect::<Vec<_>>(),
            vec![2, 6]
        );
        assert_eq!(disc_paths(&plan.discs[1]), vec!["a"]);
        assert!(plan.discs[0].will_finalize);
        assert!(!plan.discs[1].will_finalize);
    }

    #[test]
    fn mixed_capacity_tops_off_a_bd_r_open_disc_then_uses_bd_50_for_the_new_one() {
        let bdr_usable = Capacity::from_disc_type(DiscType::BdR, 0.0).usable_bytes;
        let bd50_usable = Capacity::from_disc_type(DiscType::BdRDl, 0.0).usable_bytes;
        // Open BD-R with only 5 bytes free.
        let open = DiscState {
            ordinal: 1,
            status: DiscStatus::Open,
            usable_bytes: bdr_usable,
            used_bytes: bdr_usable - 5,
        };
        let cs = added(vec![sf("a", 5), sf("b", bd50_usable)]); // b only fits a BD-50
        let plan = plan(&cs, &[open], Capacity::new(bd50_usable));
        assert_eq!(
            plan.discs.iter().map(|d| d.ordinal).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(disc_paths(&plan.discs[0]), vec!["a"]);
        assert_eq!(plan.discs[0].usable_bytes, bdr_usable);
        assert!(plan.discs[0].will_finalize);
        assert_eq!(disc_paths(&plan.discs[1]), vec!["b"]);
        assert_eq!(plan.discs[1].usable_bytes, bd50_usable);
        assert!(!plan.discs[1].will_finalize);
        assert!(plan.oversized.is_empty());
    }

    // --- new in the Rust port ---

    #[test]
    fn a_zero_byte_file_does_not_fit_an_overfull_disc() {
        // This pins the one place where the obvious `u64` translation changes
        // behavior. The Scala asks `f.sizeBytes <= usableBytes - usedBytes` in
        // signed arithmetic: for an overfull disc that is `0 <= -30`, false, so
        // the file spills to a fresh disc. A `saturating_sub` would make it
        // `0 <= 0`, true, and quietly place the file on a disc with no room.
        //
        // `DiscState` is a public type whose field values this crate does not
        // validate, the Scala has defined behavior at this boundary, and a
        // rewrite reproduces behavior rather than inventing it at an edge no
        // existing test covers.
        let overfull = DiscState {
            ordinal: 1,
            status: DiscStatus::Open,
            usable_bytes: 100,
            used_bytes: 130,
        };
        let plan = plan(&added(vec![sf("empty", 0)]), &[overfull], CAP100);
        assert_eq!(plan.discs.len(), 2, "the file must spill to a fresh disc");
        assert!(
            plan.discs[0].files.is_empty(),
            "nothing may be added to the overfull disc"
        );
        assert_eq!(disc_paths(&plan.discs[1]), vec!["empty"]);
        assert_eq!(plan.discs[1].ordinal, 2);
    }
}
