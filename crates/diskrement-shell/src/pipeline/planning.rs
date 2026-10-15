//! The pure half of `plan`: everything between the scan it just took and
//! writing `staged/plan.json`. No filesystem, no catalog, no clock —
//! `Pipeline::plan` is the thin edge around this.

use std::collections::{HashMap, HashSet};

use diskrement_core::{
    diff, planner, replication, Capacity, CatalogView, DiscKey, DiscState, DiscType, FillOrder,
    PlannedDisc, RelPath, ScannedFile,
};

use crate::pipeline::reports::{PlanReport, RemainingEstimate, SetPlan};

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

/// Diff once, plan every copy set, and reduce each set's full plan to the
/// **one disc that set writes this cycle**.
///
/// Per set: burn the first disc that actually carries files. The planner
/// keeps a set's open disc first by contract; when a new file cannot top it
/// off, that open disc is an empty finalizing placeholder — skip it so this
/// cycle writes the new disc instead of wasting a cycle on an empty session.
/// The skipped disc stays open.
///
/// A set with nothing to place is left out, unless `finalize` and it has an
/// open disc to seal. That can leave **no set at all** (a no-op scan, or a
/// deletion-only one): the plan then writes nothing, and its `tombstones` are
/// deferred to the next disc that is burned anyway (issue #9). Never fall back
/// to a disc just to carry tombstones: at `copies > 1` that opened an untwinned
/// disc.
///
/// `tombstones` are the cycle's deletions (`diff::tombstones`, computed
/// against the catalog history rather than the live view), carried as-is.
///
/// A catch-up copy too large for this disc type (an existing file burned on a
/// larger one) is not `oversized`, which refuses the whole burn: no change to
/// the source can fix it. It goes to `unplaceable_catch_up` instead, is
/// skipped this cycle, and that set's coverage stays incomplete (issue #12).
///
/// `next_session` returns the session ordinal an existing disc gets next; it
/// is recorded per set so `burn` can tell a disc it already committed from one
/// it has not.
///
/// `order` is the project's fill order. Every set uses the same one, so
/// twin discs hold the same files.
#[allow(clippy::too_many_arguments)]
pub fn plan_from(
    view: &CatalogView,
    scanned: &[ScannedFile],
    tombstones: Vec<RelPath>,
    states: &[DiscState],
    disc_type: DiscType,
    cap: Capacity,
    finalize: bool,
    copies: u32,
    order: &FillOrder,
    next_session: impl Fn(DiscKey) -> u32,
) -> PlanReport {
    let changes = diff::diff(view, scanned);
    let new_files: Vec<ScannedFile> = changes
        .added
        .iter()
        .chain(changes.modified.iter())
        .cloned()
        .collect();
    let catch_up = replication::shortfall(view, &changes, copies);

    let mut sets = Vec::new();
    let mut oversized: Vec<ScannedFile> = Vec::new();
    let mut unplaceable_catch_up: Vec<ScannedFile> = Vec::new();
    for (copy_set, extra) in (0..copies).zip(catch_up.iter()) {
        let to_place: Vec<ScannedFile> = new_files.iter().chain(extra.iter()).cloned().collect();
        let full = planner::plan_set(copy_set, &to_place, &tombstones, states, cap, order);
        for f in full.oversized {
            // `shortfall` never lists a new or modified file, so a path in
            // `extra` is a copy of something already burned.
            let bucket = if extra.iter().any(|e| e.rel_path == f.rel_path) {
                &mut unplaceable_catch_up
            } else {
                &mut oversized
            };
            if !bucket.iter().any(|o| o.rel_path == f.rel_path) {
                bucket.push(f);
            }
        }
        // `plan_set` always returns at least one disc.
        let chosen = match full.discs.iter().position(|d| !d.files.is_empty()) {
            Some(i) => Some(i),
            None if finalize && !full.discs[0].is_new => Some(0),
            None => None,
        };
        if let Some(i) = chosen {
            sets.push(set_plan(&full.discs, i, finalize, extra, &next_session));
        }
    }
    oversized.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    unplaceable_catch_up.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));

    PlanReport {
        copies,
        sets,
        disc_type,
        tombstones,
        oversized,
        unplaceable_catch_up,
    }
}

/// One set's chosen disc, with what spills behind it.
fn set_plan(
    discs: &[PlannedDisc],
    idx: usize,
    finalize: bool,
    catch_up: &[ScannedFile],
    next_session: &impl Fn(DiscKey) -> u32,
) -> SetPlan {
    let mut disc = discs[idx].clone();
    // The planner already seals this disc iff the plan spilled; `--finalize`
    // forces a seal even when everything fit.
    disc.will_finalize = disc.will_finalize || finalize;
    let tail = &discs[idx + 1..];
    let catch_up_paths: HashSet<&RelPath> = catch_up.iter().map(|f| &f.rel_path).collect();
    SetPlan {
        catch_up: disc
            .files
            .iter()
            .filter(|f| catch_up_paths.contains(&f.rel_path))
            .count(),
        session_ordinal: if disc.is_new {
            1
        } else {
            next_session(disc.key())
        },
        remaining: RemainingEstimate {
            more_discs: tail.len(),
            more_files: tail.iter().map(|d| d.files.len()).sum(),
            more_bytes: tail
                .iter()
                .flat_map(|d| d.files.iter())
                .map(|f| f.size_bytes)
                .sum(),
        },
        disc,
    }
}

/// Why `disc` no longer describes `scanned`, or `None` if it still does.
///
/// `plan` stages the scan and the plan together, but the old `scan` command
/// (before #4) could re-stage the scan alone, so a work dir staged by an older
/// diskrement, or edited by hand, can hold two that drifted apart. The disc is
/// stale when a file it places is missing from the scan or has a different
/// hash or size there, or when a path the cycle tombstones is present again.
/// A file the scan added that the plan does not place is not staleness: the
/// next cycle picks it up.
pub fn stale_detail(
    disc: &PlannedDisc,
    tombstones: &[RelPath],
    scanned: &[ScannedFile],
) -> Option<String> {
    let by_path: HashMap<&RelPath, &ScannedFile> =
        scanned.iter().map(|f| (&f.rel_path, f)).collect();
    for planned in &disc.files {
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
    tombstones
        .iter()
        .find(|t| by_path.contains_key(t))
        .map(|t| format!("{t} is back in the staged scan after this plan tombstoned it"))
}

#[cfg(test)]
mod tests {
    use super::*;
    // `CatalogView::from_scanned` is core's own helper for "the view that
    // results from recording exactly these files". It leaves every
    // `live_sets` empty, so a scenario where the files are already burned
    // uses `live_in` below to put them on a live disc in the right sets.
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
            Vec::new(),
            &[],
            DiscType::BdR,
            Capacity::new(1_000),
            false,
            1,
            &FillOrder::Path,
            |_| 1,
        );
        assert_eq!(report.sets[0].disc.ordinal, 1);
        assert!(report.sets[0].disc.is_new);
        assert!(!report.sets[0].disc.will_finalize);
        assert_eq!(report.sets[0].disc.files.len(), 2);
        assert_eq!(
            report.sets[0].remaining,
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
            Vec::new(),
            &[],
            DiscType::BdR,
            Capacity::new(1_000),
            false,
            1,
            &FillOrder::Path,
            |_| 1,
        );
        assert_eq!(report.sets[0].disc.files.len(), 1);
        assert!(
            report.sets[0].disc.will_finalize,
            "a spilled disc is sealed"
        );
        assert_eq!(
            report.sets[0].remaining,
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
            Vec::new(),
            &[],
            DiscType::BdR,
            Capacity::new(1_000),
            false,
            1,
            &FillOrder::Path,
            |_| 1,
        );
        let sealed = plan_from(
            &CatalogView::empty(),
            &files,
            Vec::new(),
            &[],
            DiscType::BdR,
            Capacity::new(1_000),
            true,
            1,
            &FillOrder::Path,
            |_| 1,
        );
        assert!(!open.sets[0].disc.will_finalize);
        assert!(sealed.sets[0].disc.will_finalize);
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
            &live_in(&files, &[0]),
            &files,
            Vec::new(),
            &states,
            DiscType::BdR,
            Capacity::new(1_000),
            true,
            1,
            &FillOrder::Path,
            |_| 1,
        );
        assert_eq!(report.sets[0].disc.ordinal, 4);
        assert!(report.sets[0].disc.files.is_empty());
        assert!(!report.sets[0].disc.is_new);
        assert!(report.sets[0].disc.will_finalize);
        assert_eq!(report.sets[0].remaining.more_discs, 0);
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
            &live_in(&[existing], &[0]),
            &files,
            Vec::new(),
            &states,
            DiscType::BdR,
            Capacity::new(1_000),
            false,
            1,
            &FillOrder::Path,
            |_| 1,
        );
        assert_eq!(
            report.sets[0].disc.ordinal, 3,
            "a fresh disc, not the open one"
        );
        assert!(report.sets[0].disc.is_new);
        assert_eq!(report.sets[0].disc.files.len(), 1);
        assert_eq!(report.sets[0].disc.files[0].rel_path.as_str(), "b.bin");
        assert_eq!(report.sets[0].remaining.more_discs, 0);
    }

    #[test]
    fn oversized_files_are_reported_and_the_rest_is_still_planned() {
        let files = vec![file("big.bin", 5_000), file("small.bin", 100)];
        let report = plan_from(
            &CatalogView::empty(),
            &files,
            Vec::new(),
            &[],
            DiscType::BdR,
            Capacity::new(1_000),
            false,
            1,
            &FillOrder::Path,
            |_| 1,
        );
        assert_eq!(report.oversized.len(), 1);
        assert_eq!(report.oversized[0].rel_path.as_str(), "big.bin");
        assert_eq!(report.sets[0].disc.files.len(), 1);
        assert_eq!(report.sets[0].disc.files[0].rel_path.as_str(), "small.bin");
    }

    #[test]
    fn tombstones_ride_this_cycles_disc_when_one_is_planned() {
        let report = plan_from(
            &CatalogView::from_scanned(&[file("gone.bin", 100)]),
            &[file("a.bin", 100)],
            vec![RelPath("gone.bin".into())],
            &[],
            DiscType::BdR,
            Capacity::new(1_000),
            false,
            1,
            &FillOrder::Path,
            |_| 1,
        );
        assert_eq!(report.sets.len(), 1);
        assert_eq!(report.sets[0].disc.files[0].rel_path.as_str(), "a.bin");
        assert_eq!(report.tombstones, vec![RelPath("gone.bin".into())]);
    }

    #[test]
    fn a_no_op_scan_plans_no_disc() {
        let files = vec![file("a.bin", 300)];
        let report = plan_from(
            &live_in(&files, &[0]),
            &files,
            Vec::new(),
            &[],
            DiscType::BdR,
            Capacity::new(1_000),
            false,
            1,
            &FillOrder::Path,
            |_| 1,
        );
        assert!(report.sets.is_empty());
        assert!(report.tombstones.is_empty());
    }

    fn plan_of(scanned: &[ScannedFile]) -> PlanReport {
        plan_from(
            &CatalogView::empty(),
            scanned,
            Vec::new(),
            &[],
            DiscType::BdR,
            Capacity::new(1_000_000),
            false,
            1,
            &FillOrder::Path,
            |_| 1,
        )
    }

    #[test]
    fn a_plan_matching_its_scan_is_not_stale() {
        let scanned = vec![file("a", 10), file("b", 20)];
        let report = plan_of(&scanned);
        assert_eq!(
            stale_detail(&report.sets[0].disc, &report.tombstones, &scanned),
            None
        );
    }

    #[test]
    fn a_file_added_to_the_scan_after_planning_is_not_staleness() {
        let scanned = vec![file("a", 10)];
        let report = plan_of(&scanned);
        assert_eq!(
            stale_detail(
                &report.sets[0].disc,
                &report.tombstones,
                &[file("a", 10), file("new", 5)]
            ),
            None
        );
    }

    #[test]
    fn a_planned_file_gone_from_the_scan_makes_the_plan_stale() {
        let report = plan_of(&[file("a", 10), file("b", 20)]);
        let detail = stale_detail(&report.sets[0].disc, &report.tombstones, &[file("b", 20)])
            .expect("stale");
        assert!(detail.contains("a is gone"), "{detail}");
    }

    fn plan1(
        view: &CatalogView,
        files: &[ScannedFile],
        states: &[DiscState],
        finalize: bool,
    ) -> PlanReport {
        plan_from(
            view,
            files,
            Vec::new(),
            states,
            DiscType::BdR,
            Capacity::new(1_000),
            finalize,
            1,
            &FillOrder::Path,
            |_| 1,
        )
    }

    fn plan2(
        view: &CatalogView,
        files: &[ScannedFile],
        states: &[DiscState],
        finalize: bool,
    ) -> PlanReport {
        plan_from(
            view,
            files,
            Vec::new(),
            states,
            DiscType::BdR,
            Capacity::new(1_000),
            finalize,
            2,
            &FillOrder::Path,
            |_| 7,
        )
    }

    fn live_in(files: &[ScannedFile], sets: &[u32]) -> CatalogView {
        let mut view = CatalogView::from_scanned(files);
        for e in view.current.values_mut() {
            e.live_sets = sets.iter().copied().collect();
        }
        view
    }

    fn open(ordinal: u32, copy_set: u32, used: u64) -> DiscState {
        DiscState {
            ordinal,
            copy_set,
            status: DiscStatus::Open,
            usable_bytes: 1_000,
            used_bytes: used,
        }
    }

    #[test]
    fn a_fresh_two_copy_project_plans_twin_new_discs() {
        let files = vec![file("a.bin", 300), file("b.bin", 300)];
        let report = plan2(&CatalogView::empty(), &files, &[], false);
        assert_eq!(report.copies, 2);
        assert_eq!(report.sets.len(), 2);
        for (set, sp) in report.sets.iter().enumerate() {
            assert_eq!(sp.disc.key(), diskrement_core::DiscKey::new(1, set as u32));
            assert!(sp.disc.is_new);
            assert_eq!(sp.disc.files, files);
            assert_eq!(sp.catch_up, 0);
            assert_eq!(
                sp.session_ordinal, 1,
                "a new disc always starts at session 1"
            );
        }
    }

    #[test]
    fn a_set_missing_a_copy_gets_it_as_catch_up_and_the_full_set_writes_nothing() {
        let files = vec![file("a.bin", 300)];
        let view = live_in(&files, &[0]);
        let report = plan2(&view, &files, &[open(1, 0, 300)], false);
        assert_eq!(report.sets.len(), 1, "set A has nothing to do");
        let sp = &report.sets[0];
        assert_eq!(sp.disc.key(), diskrement_core::DiscKey::new(1, 1));
        assert_eq!(sp.catch_up, 1);
        assert_eq!(sp.disc.files, files);
    }

    /// A catch-up copy larger than this plan's disc (an earlier disc type was
    /// bigger) is skipped, not oversized: `burn` must not refuse the plan over
    /// something no source change can fix (#12). A new file that does not
    /// fit is still oversized.
    #[test]
    fn a_catch_up_copy_too_large_for_the_disc_is_skipped_not_oversized() {
        let burned = vec![file("film.mkv", 5_000), file("a.bin", 300)];
        let view = live_in(&burned, &[0]);
        let mut scanned = burned.clone();
        scanned.push(file("new-huge.bin", 9_000));
        scanned.push(file("new.bin", 10));
        let report = plan2(&view, &scanned, &[open(1, 0, 300)], false);

        assert_eq!(
            report.unplaceable_catch_up,
            vec![file("film.mkv", 5_000)],
            "listed once, though only set B lacks it"
        );
        assert_eq!(report.oversized, vec![file("new-huge.bin", 9_000)]);
        let b = report
            .sets
            .iter()
            .find(|s| s.disc.copy_set == 1)
            .expect("set B still writes its other catch-up copy");
        assert_eq!(b.catch_up, 1);
        assert!(b
            .disc
            .files
            .iter()
            .all(|f| f.rel_path.as_str() != "film.mkv"));
    }

    #[test]
    fn an_existing_open_disc_records_the_session_it_will_receive() {
        let files = vec![file("a.bin", 300), file("n.bin", 10)];
        let view = live_in(&files[..1], &[0, 1]);
        let report = plan2(&view, &files, &[open(1, 0, 300), open(1, 1, 300)], false);
        assert!(report.sets.iter().all(|sp| sp.session_ordinal == 7));
    }

    #[test]
    fn finalize_with_nothing_to_place_seals_every_sets_open_disc() {
        let files = vec![file("a.bin", 300)];
        let view = live_in(&files, &[0, 1]);
        let report = plan2(&view, &files, &[open(1, 0, 300), open(1, 1, 300)], true);
        assert_eq!(report.sets.len(), 2);
        assert!(report
            .sets
            .iter()
            .all(|sp| sp.disc.will_finalize && sp.disc.files.is_empty()));
    }

    #[test]
    fn a_deletion_only_cycle_plans_no_disc_and_keeps_its_tombstones() {
        let kept = vec![file("a.bin", 300)];
        let view = live_in(&[file("a.bin", 300), file("gone.bin", 10)], &[0, 1]);
        let states = [open(1, 0, 310), open(1, 1, 310)];
        let report = plan_from(
            &view,
            &kept,
            vec![RelPath("gone.bin".into())],
            &states,
            DiscType::BdR,
            Capacity::new(1_000),
            false,
            2,
            &FillOrder::Path,
            |_| 7,
        );
        assert!(report.sets.is_empty(), "no disc, in either set");
        assert_eq!(report.tombstones, vec![RelPath("gone.bin".into())]);
    }

    #[test]
    fn a_single_copy_deletion_only_cycle_plans_no_disc_either() {
        let kept = vec![file("a.bin", 300)];
        let view = live_in(&[file("a.bin", 300), file("gone.bin", 10)], &[0]);
        let report = plan_from(
            &view,
            &kept,
            vec![RelPath("gone.bin".into())],
            &[],
            DiscType::BdR,
            Capacity::new(1_000),
            false,
            1,
            &FillOrder::Path,
            |_| 1,
        );
        assert!(report.sets.is_empty(), "no fresh empty disc for tombstones");
        assert_eq!(report.tombstones.len(), 1);
    }

    #[test]
    fn a_two_copy_no_op_cycle_plans_no_disc() {
        let files = vec![file("a.bin", 300)];
        let view = live_in(&files, &[0, 1]);
        let report = plan2(&view, &files, &[open(1, 0, 300), open(1, 1, 300)], false);
        assert!(report.sets.is_empty());
        assert!(report.tombstones.is_empty());
    }

    #[test]
    fn finalize_with_deletions_only_carries_the_tombstones_on_the_seals() {
        let kept = vec![file("a.bin", 300)];
        let view = live_in(&[file("a.bin", 300), file("gone.bin", 10)], &[0, 1]);
        let states = [open(1, 0, 310), open(1, 1, 310)];
        let report = plan_from(
            &view,
            &kept,
            vec![RelPath("gone.bin".into())],
            &states,
            DiscType::BdR,
            Capacity::new(1_000),
            true,
            2,
            &FillOrder::Path,
            |_| 7,
        );
        assert_eq!(report.sets.len(), 2);
        assert!(report
            .sets
            .iter()
            .all(|sp| sp.disc.will_finalize && sp.disc.files.is_empty()));
        assert_eq!(report.tombstones, vec![RelPath("gone.bin".into())]);
    }

    fn finalized(ordinal: u32, copy_set: u32, used: u64) -> DiscState {
        DiscState {
            status: DiscStatus::Finalized,
            ..open(ordinal, copy_set, used)
        }
    }

    #[test]
    fn finalize_with_deletions_only_and_no_open_disc_plans_no_disc() {
        let kept = vec![file("a.bin", 300)];
        let view = live_in(&[file("a.bin", 300), file("gone.bin", 10)], &[0, 1]);
        let states = [finalized(1, 0, 310), finalized(1, 1, 310)];
        let report = plan_from(
            &view,
            &kept,
            vec![RelPath("gone.bin".into())],
            &states,
            DiscType::BdR,
            Capacity::new(1_000),
            true,
            2,
            &FillOrder::Path,
            |_| 7,
        );
        assert!(report.sets.is_empty(), "nothing open to seal in either set");
        assert_eq!(report.tombstones, vec![RelPath("gone.bin".into())]);
    }

    #[test]
    fn finalize_with_deletions_only_seals_just_the_set_that_has_an_open_disc() {
        let kept = vec![file("a.bin", 300)];
        let view = live_in(&[file("a.bin", 300), file("gone.bin", 10)], &[0, 1]);
        let states = [open(1, 0, 310), finalized(1, 1, 310)];
        let report = plan_from(
            &view,
            &kept,
            vec![RelPath("gone.bin".into())],
            &states,
            DiscType::BdR,
            Capacity::new(1_000),
            true,
            2,
            &FillOrder::Path,
            |_| 7,
        );
        assert_eq!(report.sets.len(), 1);
        let disc = &report.sets[0].disc;
        assert_eq!(disc.key(), diskrement_core::DiscKey::new(1, 0));
        assert!(disc.will_finalize && disc.files.is_empty() && !disc.is_new);
        assert_eq!(report.tombstones, vec![RelPath("gone.bin".into())]);
    }

    #[test]
    fn a_single_copy_plan_has_exactly_one_set() {
        let report = plan1(&CatalogView::empty(), &[file("a.bin", 300)], &[], false);
        assert_eq!(report.copies, 1);
        assert_eq!(report.sets.len(), 1);
    }

    #[test]
    fn every_set_fills_in_the_projects_order() {
        let files = vec![
            file("family/anna/2021/a.jpg", 400),
            file("family/anna/2023/d.jpg", 400),
            file("family/bela/2020/b.jpg", 400),
        ];
        let order = FillOrder::Rooted(vec![
            "family/anna".parse().unwrap(),
            "family/bela".parse().unwrap(),
        ]);
        let report = plan_from(
            &CatalogView::empty(),
            &files,
            Vec::new(),
            &[],
            DiscType::BdR,
            Capacity::new(1_000),
            false,
            2,
            &order,
            |_| 1,
        );
        assert_eq!(report.sets.len(), 2);
        for set in &report.sets {
            assert_eq!(
                set.disc
                    .files
                    .iter()
                    .map(|f| f.rel_path.as_str())
                    .collect::<Vec<_>>(),
                vec!["family/bela/2020/b.jpg", "family/anna/2021/a.jpg"]
            );
        }
    }
}
