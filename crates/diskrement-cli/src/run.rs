#![allow(dead_code)] // until `backlog` lands
//! `diskrement run`: plan and burn cycle after cycle until the backlog is on
//! disc, each disc waited for rather than asked for (continuous-run spec).
//! The stop rules and the renderings are pure; `backlog` is the loop.

use std::time::Duration;

use diskrement_shell::error::PipelineError;
use diskrement_shell::pipeline::reports::{BurnReport, DiscBurn, DiscReadBack, PlanReport};

use crate::render;

#[derive(Debug, PartialEq, Eq)]
pub enum Next {
    Continue,
    Done,
}

/// A plan with no disc: nothing left to burn.
pub fn after_plan(plan: &PlanReport) -> Next {
    if plan.sets.is_empty() {
        Next::Done
    } else {
        Next::Continue
    }
}

/// A burned cycle that finalized nothing left its discs open: everything
/// left fitted, and what comes later is appended by ordinary cycles.
pub fn after_burn(burn: &BurnReport) -> Next {
    if burn.discs.iter().any(|d| d.finalized) {
        Next::Continue
    } else {
        Next::Done
    }
}

/// Cycles after this one: the longest spill among the sets.
fn more_cycles(plan: &PlanReport) -> usize {
    plan.sets
        .iter()
        .map(|s| s.remaining.more_discs)
        .max()
        .unwrap_or(0)
}

fn cycles(n: usize) -> String {
    if n == 1 {
        "1 cycle".to_string()
    } else {
        format!("{n} cycles")
    }
}

/// The one question a run asks, before its first burn.
pub fn confirm_question(name: &str, plan: &PlanReport) -> String {
    format!(
        "Burn the backlog of {name} (about {})?",
        cycles(1 + more_cycles(plan))
    )
}

/// `Cycle 3: 7A (24.5 GB, finalizes), 7B (24.5 GB, finalizes); about 10 more
/// cycles after this.`
pub fn cycle_line(n: usize, plan: &PlanReport) -> String {
    let discs: Vec<String> = plan
        .sets
        .iter()
        .map(|s| {
            format!(
                "{} ({}, {})",
                s.disc.key().name(plan.copies),
                render::gb(s.disc.total_used_bytes()),
                if s.disc.will_finalize {
                    "finalizes"
                } else {
                    "stays open"
                }
            )
        })
        .collect();
    let tail = match more_cycles(plan) {
        0 => "the last cycle".to_string(),
        1 => "about 1 more cycle after this".to_string(),
        more => format!("about {more} more cycles after this"),
    };
    format!("Cycle {n}: {}; {tail}.", discs.join(", "))
}

/// `1h 32m`, `12m`, `45s`.
pub fn elapsed(d: Duration) -> String {
    let s = d.as_secs();
    match (s / 3600, (s % 3600) / 60) {
        (0, 0) => format!("{s}s"),
        (0, m) => format!("{m}m"),
        (h, m) => format!("{h}h {m}m"),
    }
}

/// What the run burned, printed when it ends, however it ends.
pub fn summary(burned: &[DiscBurn], copies: u32, took: Duration) -> String {
    if burned.is_empty() {
        return format!("This run: no disc burned ({}).", elapsed(took));
    }
    let names = |want: fn(&Option<DiscReadBack>) -> bool| -> Vec<String> {
        burned
            .iter()
            .filter(|d| want(&d.read_back))
            .map(|d| d.disc.name(copies))
            .collect()
    };
    let verified = names(|r| matches!(r, Some(DiscReadBack::Verified { .. })));
    let unverified = names(|r| matches!(r, Some(DiscReadBack::Unverified)));
    let unchecked = names(|r| r.is_none());
    let mut parts = Vec::new();
    if !verified.is_empty() {
        parts.push(format!("verified: {}", verified.join(", ")));
    }
    if !unverified.is_empty() {
        let advice: Vec<String> = unverified
            .iter()
            .map(|n| format!("verify --disc {n}"))
            .collect();
        parts.push(format!(
            "recorded unverified: {} (run {})",
            unverified.join(", "),
            advice.join(", ")
        ));
    }
    if !unchecked.is_empty() {
        parts.push(format!("written: {}", unchecked.join(", ")));
    }
    let count = if burned.len() == 1 {
        "1 disc".to_string()
    } else {
        format!("{} discs", burned.len())
    };
    format!(
        "This run: {count} in {} — {}.",
        elapsed(took),
        parts.join("; ")
    )
}

/// The line `run` adds after a burn error: re-running `run` re-plans, so it
/// picks up where it stopped. Not after a failed append, whose disc must be
/// marked dead first.
pub fn rerun_advice(name: &str, err: &PipelineError) -> Option<String> {
    let cause = match err {
        PipelineError::PartialBurn { cause, .. } => cause.as_ref(),
        other => other,
    };
    match cause {
        PipelineError::VerifyFailed { appended: true, .. } => None,
        _ => Some(format!(
            "Or re-run diskrement run {name}; it re-plans first."
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diskrement_core::{Coverage, DiscKey, DiscType, PlannedDisc};
    use diskrement_shell::pipeline::reports::{RemainingEstimate, SetPlan};

    fn set(ordinal: u32, copy_set: u32, will_finalize: bool, more_discs: usize) -> SetPlan {
        SetPlan {
            disc: PlannedDisc {
                ordinal,
                copy_set,
                is_new: true,
                will_finalize,
                files: vec![],
                prior_used_bytes: 24_500_000_000,
                usable_bytes: 24_500_000_000,
            },
            catch_up: 0,
            session_ordinal: 1,
            remaining: RemainingEstimate {
                more_discs,
                more_files: 0,
                more_bytes: 0,
            },
        }
    }

    fn plan(sets: Vec<SetPlan>) -> PlanReport {
        PlanReport {
            copies: 2,
            sets,
            disc_type: DiscType::BdR,
            tombstones: vec![],
            oversized: vec![],
            unplaceable_catch_up: vec![],
        }
    }

    fn burned(
        ordinal: u32,
        copy_set: u32,
        finalized: bool,
        read_back: Option<DiscReadBack>,
    ) -> DiscBurn {
        DiscBurn {
            disc: DiscKey::new(ordinal, copy_set),
            label: String::new(),
            finalized,
            staging_dir: "/w/staging".into(),
            image_bytes: 1,
            read_back,
        }
    }

    fn report(discs: Vec<DiscBurn>) -> BurnReport {
        BurnReport {
            discs,
            remaining: RemainingEstimate {
                more_discs: 0,
                more_files: 0,
                more_bytes: 0,
            },
            coverage: Coverage {
                sets: vec![],
                no_live_copy: Default::default(),
            },
            deferred_deletions: None,
        }
    }

    const OK: Option<DiscReadBack> = Some(DiscReadBack::Verified {
        files: 1,
        through_xorriso: false,
    });

    #[test]
    fn a_plan_with_no_disc_ends_the_run() {
        assert_eq!(after_plan(&plan(vec![])), Next::Done);
        assert_eq!(after_plan(&plan(vec![set(5, 0, true, 9)])), Next::Continue);
    }

    #[test]
    fn a_cycle_that_finalizes_nothing_ends_the_run() {
        let open = report(vec![burned(18, 0, false, OK), burned(18, 1, false, OK)]);
        assert_eq!(after_burn(&open), Next::Done);
        let sealed = report(vec![burned(5, 0, true, OK), burned(5, 1, true, OK)]);
        assert_eq!(after_burn(&sealed), Next::Continue);
        // A lagging set still sealing a disc keeps the run going.
        let mixed = report(vec![burned(5, 0, false, OK), burned(4, 1, true, OK)]);
        assert_eq!(after_burn(&mixed), Next::Continue);
        assert_eq!(after_burn(&report(vec![])), Next::Done);
    }

    #[test]
    fn the_question_counts_this_cycle_and_the_longest_spill() {
        let p = plan(vec![set(5, 0, true, 9), set(5, 1, true, 10)]);
        assert_eq!(
            confirm_question("immich-dsz", &p),
            "Burn the backlog of immich-dsz (about 11 cycles)?"
        );
        let last = plan(vec![set(18, 0, false, 0)]);
        assert_eq!(
            confirm_question("p", &last),
            "Burn the backlog of p (about 1 cycle)?"
        );
    }

    #[test]
    fn the_cycle_line_names_each_disc_and_what_is_left() {
        let p = plan(vec![set(7, 0, true, 10), set(7, 1, true, 10)]);
        assert_eq!(
            cycle_line(3, &p),
            "Cycle 3: 7A (24.5 GB, finalizes), 7B (24.5 GB, finalizes); \
             about 10 more cycles after this."
        );
        let one = plan(vec![set(17, 0, true, 1)]);
        assert!(cycle_line(13, &one).ends_with("; about 1 more cycle after this."));
        let last = plan(vec![set(18, 0, false, 0), set(18, 1, false, 0)]);
        assert_eq!(
            cycle_line(14, &last),
            "Cycle 14: 18A (24.5 GB, stays open), 18B (24.5 GB, stays open); the last cycle."
        );
    }

    #[test]
    fn elapsed_time_reads_in_hours_minutes_or_seconds() {
        assert_eq!(elapsed(Duration::from_secs(45)), "45s");
        assert_eq!(elapsed(Duration::from_secs(12 * 60 + 5)), "12m");
        assert_eq!(elapsed(Duration::from_secs(3600 + 32 * 60)), "1h 32m");
    }

    #[test]
    fn the_summary_groups_discs_by_their_read_back() {
        let discs = vec![
            burned(5, 0, true, OK),
            burned(5, 1, true, OK),
            burned(6, 0, true, OK),
            burned(6, 1, true, Some(DiscReadBack::Unverified)),
        ];
        assert_eq!(
            summary(&discs, 2, Duration::from_secs(3 * 3600)),
            "This run: 4 discs in 3h 0m — verified: 5A, 5B, 6A; \
             recorded unverified: 6B (run verify --disc 6B)."
        );
        assert_eq!(
            summary(&[], 2, Duration::from_secs(5)),
            "This run: no disc burned (5s)."
        );
    }

    #[test]
    fn rerunning_run_is_advised_except_after_a_failed_append() {
        let failed = |appended| PipelineError::VerifyFailed {
            disc: "6B".to_string(),
            session: if appended { 2 } else { 1 },
            bad: 1,
            checked: 9,
            appended,
        };
        assert_eq!(
            rerun_advice("p", &failed(false)).as_deref(),
            Some("Or re-run diskrement run p; it re-plans first.")
        );
        assert_eq!(rerun_advice("p", &failed(true)), None);
        let partial = PipelineError::PartialBurn {
            committed: vec!["6A".to_string()],
            unrecorded: None,
            pending: vec![],
            cause: Box::new(failed(true)),
        };
        assert_eq!(rerun_advice("p", &partial), None);
    }
}
