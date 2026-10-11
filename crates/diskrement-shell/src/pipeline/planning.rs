//! The pure half of `plan` (`PipelineLive.plan`): everything between reading
//! the staged scan and writing `staged/plan.json`.
//!
//! No filesystem, no catalog, no clock. `Pipeline::plan` is the thin edge
//! around this — which is what lets the fixture oracle re-plan the harvested
//! scenario straight out of `fixtures/`, with no environment variable and no
//! temporary catalog.

use diskrement_core::{diff, planner, Capacity, CatalogView, DiscState, DiscType, ScannedFile};

use crate::pipeline::reports::{PlanReport, RemainingEstimate};

/// `DISKREMENT_DISC_BYTES`, the test-only capacity override.
///
/// Deliberately undocumented in `--help` (CLAUDE.md). It makes multi-disc
/// scenarios cheap and deterministic, and it may only ever **shrink** usable
/// capacity, so it cannot plan a disc past the safety margin in production.
pub fn disc_bytes_from_env() -> Option<u64> {
    resolve_disc_bytes(std::env::var("DISKREMENT_DISC_BYTES").ok())
}

/// The pure half: `_.flatMap(_.toLongOption).filter(_ > 0L)`.
///
/// No trimming, matching `toLongOption` — `" 1000 "` is not a number, and a
/// value that is not a positive integer is silently treated as unset, with no
/// warning. That is deliberately unlike `DISKREMENT_HASH_PARALLELISM`, which
/// warns: this variable is a test hook rather than a documented knob.
pub fn resolve_disc_bytes(raw: Option<String>) -> Option<u64> {
    raw.and_then(|s| s.parse::<u64>().ok()).filter(|n| *n > 0)
}

/// Apply the shrink-only override to the margin-adjusted capacity.
pub fn effective_capacity(normal: Capacity, override_bytes: Option<u64>) -> Capacity {
    match override_bytes {
        Some(n) if n < normal.usable_bytes => Capacity::new(n),
        _ => normal,
    }
}

/// Diff, plan, and reduce the full multi-disc plan to the **one disc this
/// cycle writes** (`PipelineLive.plan`).
///
/// One disc per cycle: burn the first disc that actually carries new files.
/// The planner keeps the existing open disc first by contract; when a new file
/// cannot top it off, that open disc is an empty finalizing placeholder — skip
/// it so this cycle writes the new disc instead of wasting a cycle on an empty
/// session. The skipped disc stays open. When no disc has files (a no-op scan,
/// or an explicit `--finalize` of an empty open disc), keep the head as-is.
///
/// `tombstones` are the whole scan's deletions, not one disc's: on a
/// cross-cycle spill they attach to the session actually written this cycle.
pub fn plan_from(
    view: &CatalogView,
    scanned: &[ScannedFile],
    states: &[DiscState],
    disc_type: DiscType,
    cap: Capacity,
    finalize: bool,
) -> PlanReport {
    let changes = diff::diff(view, scanned);
    let full = planner::plan(&changes, states, cap);
    // `Planner::plan` always returns at least one disc, so both the index and
    // the `idx + 1` slice below are in range.
    let idx = full
        .discs
        .iter()
        .position(|d| !d.files.is_empty())
        .unwrap_or(0);
    let mut disc = full.discs[idx].clone();
    // The planner already seals this disc iff the plan spilled; `--finalize`
    // forces a seal even when everything fit.
    disc.will_finalize = disc.will_finalize || finalize;
    let tail = &full.discs[idx + 1..];
    let remaining = RemainingEstimate {
        more_discs: tail.len(),
        more_files: tail.iter().map(|d| d.files.len()).sum(),
        more_bytes: tail
            .iter()
            .flat_map(|d| d.files.iter())
            .map(|f| f.size_bytes)
            .sum(),
    };
    PlanReport {
        disc,
        disc_type,
        tombstones: full.tombstones,
        remaining,
        oversized: full.oversized,
    }
}

/// Why `report` no longer describes `scanned`, or `None` if it still does.
///
/// `scan --force` re-stages the scan without touching the plan, so the two
/// can drift apart. The plan is stale when a file it places is missing from
/// the scan or has a different hash or size there, or when a path it
/// tombstones is present again. A file the scan added that the plan does not
/// place is not staleness: the next cycle picks it up.
pub fn stale_detail(report: &PlanReport, scanned: &[ScannedFile]) -> Option<String> {
    let by_path: std::collections::HashMap<&diskrement_core::RelPath, &ScannedFile> =
        scanned.iter().map(|f| (&f.rel_path, f)).collect();
    for planned in &report.disc.files {
        let what = match by_path.get(&planned.rel_path) {
            None => "is gone from",
            Some(f) if f.content_hash != planned.content_hash => "changed in",
            Some(f) if f.size_bytes != planned.size_bytes => "changed in",
            Some(_) => continue,
        };
        return Some(format!(
            "{} {what} the staged scan after this plan was staged",
            planned.rel_path
        ));
    }
    report
        .tombstones
        .iter()
        .find(|t| by_path.contains_key(t))
        .map(|t| format!("{t} is back in the staged scan after this plan tombstoned it"))
}

#[cfg(test)]
mod tests {
    use super::*;
    // `CatalogView::from_scanned` is core's own helper for "the view that
    // results from recording exactly these files", which is what a
    // no-change or partial-change scenario needs here.
    use diskrement_core::{
        Capacity, CatalogView, ContentHash, DiscState, DiscStatus, DiscType, RelPath, ScannedFile,
    };

    fn ts() -> chrono::DateTime<chrono::Utc> {
        crate::catalog::time::parse("2026-06-01T00:00:00Z").unwrap()
    }

    fn file(rel: &str, size: u64) -> ScannedFile {
        ScannedFile {
            rel_path: RelPath(rel.into()),
            // The hash only has to be stable and distinct per (path, size).
            content_hash: ContentHash(format!("{rel}-{size}")),
            size_bytes: size,
            mtime: ts(),
        }
    }

    #[test]
    fn an_unset_override_leaves_the_margin_adjusted_capacity_alone() {
        let normal = Capacity::from_disc_type(DiscType::BdR, 2.0);
        assert_eq!(effective_capacity(normal, None), normal);
    }

    #[test]
    fn the_override_only_shrinks() {
        let normal = Capacity::from_disc_type(DiscType::BdR, 2.0);
        // Below the margin-adjusted capacity: honoured.
        assert_eq!(
            effective_capacity(normal, Some(1_000)),
            Capacity::new(1_000)
        );
        // At or above it: ignored, so the override can never plan past the
        // safety margin (CLAUDE.md).
        assert_eq!(
            effective_capacity(normal, Some(normal.usable_bytes)),
            normal
        );
        assert_eq!(effective_capacity(normal, Some(999_999_999_999)), normal);
    }

    #[test]
    fn only_a_positive_integer_is_a_usable_override() {
        // `toLongOption.filter(_ > 0)`: everything else is unset, silently.
        assert_eq!(resolve_disc_bytes(Some("1000".into())), Some(1000));
        assert_eq!(resolve_disc_bytes(None), None);
        assert_eq!(resolve_disc_bytes(Some("".into())), None);
        assert_eq!(resolve_disc_bytes(Some("0".into())), None);
        assert_eq!(resolve_disc_bytes(Some("-5".into())), None);
        assert_eq!(resolve_disc_bytes(Some("lots".into())), None);
        assert_eq!(resolve_disc_bytes(Some(" 1000 ".into())), None);
    }

    #[test]
    fn a_single_disc_plan_reports_nothing_remaining() {
        let files = vec![file("a.bin", 300), file("b.bin", 300)];
        let report = plan_from(
            &CatalogView::empty(),
            &files,
            &[],
            DiscType::BdR,
            Capacity::new(1_000),
            false,
        );
        assert_eq!(report.disc.ordinal, 1);
        assert!(report.disc.is_new);
        assert!(!report.disc.will_finalize);
        assert_eq!(report.disc.files.len(), 2);
        assert_eq!(
            report.remaining,
            RemainingEstimate {
                more_discs: 0,
                more_files: 0,
                more_bytes: 0
            }
        );
        assert!(report.oversized.is_empty());
        assert_eq!(report.disc_type, DiscType::BdR);
    }

    #[test]
    fn a_spill_seals_this_cycles_disc_and_reports_the_remainder() {
        let files = vec![file("a.bin", 800), file("b.bin", 800)];
        let report = plan_from(
            &CatalogView::empty(),
            &files,
            &[],
            DiscType::BdR,
            Capacity::new(1_000),
            false,
        );
        assert_eq!(report.disc.files.len(), 1);
        assert!(report.disc.will_finalize, "a spilled disc is sealed");
        assert_eq!(
            report.remaining,
            RemainingEstimate {
                more_discs: 1,
                more_files: 1,
                more_bytes: 800
            }
        );
    }

    #[test]
    fn finalize_seals_a_disc_that_would_have_been_left_open() {
        let files = vec![file("a.bin", 300)];
        let open = plan_from(
            &CatalogView::empty(),
            &files,
            &[],
            DiscType::BdR,
            Capacity::new(1_000),
            false,
        );
        let sealed = plan_from(
            &CatalogView::empty(),
            &files,
            &[],
            DiscType::BdR,
            Capacity::new(1_000),
            true,
        );
        assert!(!open.disc.will_finalize);
        assert!(sealed.disc.will_finalize);
    }

    #[test]
    fn finalize_of_an_unchanged_tree_seals_the_open_disc_with_no_files() {
        // Corpus step 18: `plan p --finalize` after a scan that found nothing.
        let files = vec![file("a.bin", 300)];
        let states = [DiscState {
            ordinal: 4,
            copy_set: 0,
            status: DiscStatus::Open,
            usable_bytes: 1_000,
            used_bytes: 300,
        }];
        let report = plan_from(
            &CatalogView::from_scanned(&files),
            &files,
            &states,
            DiscType::BdR,
            Capacity::new(1_000),
            true,
        );
        assert_eq!(report.disc.ordinal, 4);
        assert!(report.disc.files.is_empty());
        assert!(!report.disc.is_new);
        assert!(report.disc.will_finalize);
        assert_eq!(report.remaining.more_discs, 0);
    }

    #[test]
    fn a_file_that_cannot_top_off_the_open_disc_opens_a_fresh_one_this_cycle() {
        // The empty-open-disc skip: the planner keeps the open disc first, but
        // a cycle must not burn an empty finalizing placeholder. Corpus
        // fixture `work/02-after-plan3` is this case.
        let existing = file("a.bin", 800);
        let mut files = vec![existing.clone(), file("b.bin", 800)];
        files.sort_by(|x, y| x.rel_path.cmp(&y.rel_path));
        let states = [DiscState {
            ordinal: 2,
            copy_set: 0,
            status: DiscStatus::Open,
            usable_bytes: 1_000,
            used_bytes: 800,
        }];
        let report = plan_from(
            &CatalogView::from_scanned(&[existing]),
            &files,
            &states,
            DiscType::BdR,
            Capacity::new(1_000),
            false,
        );
        assert_eq!(report.disc.ordinal, 3, "a fresh disc, not the open one");
        assert!(report.disc.is_new);
        assert_eq!(report.disc.files.len(), 1);
        assert_eq!(report.disc.files[0].rel_path.as_str(), "b.bin");
        assert_eq!(report.remaining.more_discs, 0);
    }

    #[test]
    fn oversized_files_are_reported_and_the_rest_is_still_planned() {
        let files = vec![file("big.bin", 5_000), file("small.bin", 100)];
        let report = plan_from(
            &CatalogView::empty(),
            &files,
            &[],
            DiscType::BdR,
            Capacity::new(1_000),
            false,
        );
        assert_eq!(report.oversized.len(), 1);
        assert_eq!(report.oversized[0].rel_path.as_str(), "big.bin");
        assert_eq!(report.disc.files.len(), 1);
        assert_eq!(report.disc.files[0].rel_path.as_str(), "small.bin");
    }

    #[test]
    fn deletions_become_tombstones_on_this_cycles_disc() {
        let gone = file("gone.bin", 100);
        let report = plan_from(
            &CatalogView::from_scanned(&[gone]),
            &[file("a.bin", 100)],
            &[],
            DiscType::BdR,
            Capacity::new(1_000),
            false,
        );
        assert_eq!(report.tombstones.len(), 1);
        assert_eq!(report.tombstones[0].as_str(), "gone.bin");
    }

    #[test]
    fn a_no_op_scan_plans_an_empty_disc_rather_than_panicking() {
        // `position` finds nothing; the Scala fell back to index 0 and so does
        // this. `Planner::plan` always returns at least one disc, so indexing
        // is safe — this test is what keeps that assumption honest.
        let files = vec![file("a.bin", 300)];
        let report = plan_from(
            &CatalogView::from_scanned(&files),
            &files,
            &[],
            DiscType::BdR,
            Capacity::new(1_000),
            false,
        );
        assert!(report.disc.files.is_empty());
        assert!(report.tombstones.is_empty());
        assert_eq!(report.remaining.more_discs, 0);
    }

    fn plan_of(scanned: &[ScannedFile]) -> PlanReport {
        plan_from(
            &CatalogView::empty(),
            scanned,
            &[],
            DiscType::BdR,
            Capacity::new(1_000_000),
            false,
        )
    }

    #[test]
    fn a_plan_matching_its_scan_is_not_stale() {
        let scanned = vec![file("a", 10), file("b", 20)];
        assert_eq!(stale_detail(&plan_of(&scanned), &scanned), None);
    }

    #[test]
    fn a_file_added_to_the_scan_after_planning_is_not_staleness() {
        let scanned = vec![file("a", 10)];
        let report = plan_of(&scanned);
        assert_eq!(
            stale_detail(&report, &[file("a", 10), file("new", 5)]),
            None
        );
    }

    #[test]
    fn a_planned_file_gone_from_the_scan_makes_the_plan_stale() {
        let report = plan_of(&[file("a", 10), file("b", 20)]);
        let detail = stale_detail(&report, &[file("b", 20)]).expect("stale");
        assert!(detail.contains("a is gone"), "{detail}");
    }
}
