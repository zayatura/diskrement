//! Output rendering (`Render.scala`). Success text goes to stdout; error text
//! is `PipelineError`'s `Display` and goes to stderr with its exit code.

use diskrement_shell::catalog::model::{Project, ProjectId};
use diskrement_shell::catalog::time;
use diskrement_shell::pipeline::reports::{BurnReport, PlanReport, ScanReport, StatusReport};

pub fn init_done(name: &str, id: ProjectId) -> String {
    format!("Created project {name} (id {}).", id.0)
}

/// `Scanned: …`, plus the `Hashed N of M files` line for a fresh scan only.
#[allow(dead_code)]
pub fn scan_report(r: &ScanReport) -> String {
    let summary = format!(
        "Scanned: {} added, {} modified, {} deleted, {} unchanged.",
        r.added, r.modified, r.deleted, r.unchanged
    );
    match r.hashed {
        Some(h) => format!(
            "{summary}\nHashed {h} of {} files ({} fingerprints reused).",
            r.scanned_files,
            r.scanned_files - h
        ),
        None => summary,
    }
}

/// `Render.plan`. Three shapes: nothing to do, a bare seal, and the summary
/// block with its optional deletion and oversize lines.
pub fn plan_report(p: &PlanReport) -> String {
    let d = &p.disc;
    if d.files.is_empty() && p.tombstones.is_empty() && p.oversized.is_empty() {
        return if d.will_finalize {
            format!("Plan: finalize disc {} (seal; no new files).", d.ordinal)
        } else {
            "Nothing to burn: no changes to plan.".to_string()
        };
    }
    let kind = if d.is_new {
        "new disc".to_string()
    } else {
        format!("appending to open disc {}", d.ordinal)
    };
    let fin = if d.will_finalize {
        "will FINALIZE"
    } else {
        "left OPEN"
    };
    let mut lines = vec![format!(
        "Plan: disc {} ({kind}, {}) — {} files, {}, {fin}",
        d.ordinal,
        p.disc_type,
        d.files.len(),
        gb(d.total_used_bytes())
    )];
    lines.push(if p.remaining.more_discs == 0 {
        "This disc completes the set.".to_string()
    } else {
        format!(
            "~{} more files (~{} more discs) remain after this one.",
            p.remaining.more_files, p.remaining.more_discs
        )
    });
    if !p.tombstones.is_empty() {
        lines.push(format!("Deletions: {}", p.tombstones.len()));
    }
    if !p.oversized.is_empty() {
        lines.push("Oversized (cannot be placed):".to_string());
        for f in &p.oversized {
            lines.push(format!("  {}  {}", f.rel_path.as_str(), gb(f.size_bytes)));
        }
    }
    lines.join("\n")
}

/// `f"${bytes / 1.0e9}%.1f GB"` (`Render.gb`).
#[allow(dead_code)]
pub fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1.0e9)
}

/// `Render.burn`. One disc per cycle, so there is exactly one block.
pub fn burn_report(r: &BurnReport) -> String {
    let d = &r.disc;
    let status = if d.finalized {
        "Status: FINALIZED — set this disc aside, it is sealed."
    } else {
        "Status: left OPEN — may receive more sessions later."
    };
    let next = if r.remaining.more_discs == 0 {
        "No more changes — the set is up to date.".to_string()
    } else {
        format!(
            "~{} more discs of changes remain. Run: scan → plan → burn",
            r.remaining.more_discs
        )
    };
    format!(
        "✔ Wrote disc {}  —  label: \"{}\"\n{status}\n{next}",
        d.ordinal, d.label
    )
}

#[allow(dead_code)]
pub fn status_report(p: &Project, r: &StatusReport) -> String {
    // Two spaces after the project name; margin is %.1f plus a literal %.
    let mut out = format!(
        "Project {}  (source {}, {}, margin {:.1}%)\n",
        p.name, p.source_root, p.default_disc_type, p.margin_percent
    );
    out.push_str("Discs:\n");
    for d in &r.discs {
        let open = if r.open_disc_ordinal == Some(d.ordinal) {
            "  <- open"
        } else {
            ""
        };
        out.push_str(&format!(
            "  #{:<3} {:<10} {} / {}{}\n",
            d.ordinal,
            d.status.as_str(),
            gb(d.used_bytes),
            gb(d.usable_bytes),
            open
        ));
    }
    let last = r
        .last_burn_at
        .as_ref()
        .map(time::format)
        .unwrap_or_else(|| "never".to_string());
    // Six spaces between the count and "Last burn:"; no trailing newline —
    // `main` prints with println!.
    out.push_str(&format!(
        "Current files: {}      Last burn: {}",
        r.total_current_files, last
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use diskrement_core::{DiscState, DiscStatus, DiscType};

    fn ts(y: i32, mo: u32, d: u32) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc.with_ymd_and_hms(y, mo, d, 14, 3, 0).unwrap()
    }

    fn project() -> Project {
        Project {
            id: ProjectId(1),
            name: "photos".into(),
            source_root: "/srv/photos".into(),
            work_dir: "/srv/work".into(),
            default_disc_type: DiscType::BdR,
            margin_percent: 2.0,
            copies: 1,
            created_at: ts(2026, 1, 1),
        }
    }

    fn report() -> StatusReport {
        StatusReport {
            discs: vec![
                DiscState {
                    ordinal: 1,
                    status: DiscStatus::Finalized,
                    usable_bytes: 25_000_000_000,
                    used_bytes: 24_800_000_000,
                },
                DiscState {
                    ordinal: 2,
                    status: DiscStatus::Open,
                    usable_bytes: 25_000_000_000,
                    used_bytes: 3_200_000_000,
                },
            ],
            open_disc_ordinal: Some(2),
            total_current_files: 556,
            last_burn_at: Some(ts(2026, 6, 21)),
        }
    }

    #[test]
    fn the_init_confirmation_line_is_exact() {
        assert_eq!(
            init_done("photos", ProjectId(7)),
            "Created project photos (id 7)."
        );
    }

    #[test]
    fn a_fresh_scan_reports_hashed_and_reused() {
        let r = ScanReport {
            added: 2,
            modified: 1,
            deleted: 0,
            unchanged: 3,
            hashed: Some(3),
            scanned_files: 6,
        };
        assert_eq!(
            scan_report(&r),
            "Scanned: 2 added, 1 modified, 0 deleted, 3 unchanged.\n\
             Hashed 3 of 6 files (3 fingerprints reused)."
        );
    }

    #[test]
    fn a_reused_staged_scan_omits_the_hashed_line() {
        let r = ScanReport {
            added: 2,
            modified: 1,
            deleted: 0,
            unchanged: 3,
            hashed: None,
            scanned_files: 6,
        };
        assert_eq!(
            scan_report(&r),
            "Scanned: 2 added, 1 modified, 0 deleted, 3 unchanged."
        );
    }

    #[test]
    fn the_status_table_has_the_exact_layout() {
        let out = status_report(&project(), &report());
        assert!(
            out.contains("Project photos  (source /srv/photos, BD_R, margin 2.0%)"),
            "{out}"
        );
        assert!(out.contains("Discs:"), "{out}");
        assert!(out.contains("  #1   FINALIZED  24.8 GB / 25.0 GB"), "{out}");
        assert!(
            out.contains("  #2   OPEN       3.2 GB / 25.0 GB  <- open"),
            "{out}"
        );
        assert!(
            out.contains("Current files: 556      Last burn: 2026-06-21T14:03:00Z"),
            "{out}"
        );
    }

    #[test]
    fn never_is_printed_when_nothing_was_burned() {
        let mut r = report();
        r.last_burn_at = None;
        r.discs.clear();
        let out = status_report(&project(), &r);
        assert!(
            out.contains("Current files: 556      Last burn: never"),
            "{out}"
        );
    }

    #[test]
    fn gb_formats_one_decimal() {
        assert_eq!(gb(24_800_000_000), "24.8 GB");
        assert_eq!(gb(25_000_000_000), "25.0 GB");
        assert_eq!(gb(0), "0.0 GB");
    }

    use diskrement_core::{ContentHash, PlannedDisc, RelPath, ScannedFile};
    use diskrement_shell::pipeline::reports::{PlanReport, RemainingEstimate};

    fn scanned(rel: &str, size: u64) -> ScannedFile {
        ScannedFile {
            rel_path: RelPath(rel.into()),
            content_hash: ContentHash("deadbeef".into()),
            size_bytes: size,
            mtime: ts(2026, 9, 5),
        }
    }

    fn plan_of(
        disc: PlannedDisc,
        tombstones: Vec<&str>,
        remaining: (usize, usize, u64),
        oversized: Vec<ScannedFile>,
    ) -> PlanReport {
        PlanReport {
            disc,
            disc_type: DiscType::BdR,
            tombstones: tombstones.into_iter().map(|t| RelPath(t.into())).collect(),
            remaining: RemainingEstimate {
                more_discs: remaining.0,
                more_files: remaining.1,
                more_bytes: remaining.2,
            },
            oversized,
        }
    }

    /// `fixtures/run-manifest.txt` step 03 — the cross-implementation render
    /// oracle, byte for byte.
    #[test]
    fn a_spilling_plan_renders_exactly_as_the_scala_build_printed_it() {
        let report = plan_of(
            PlannedDisc {
                ordinal: 1,
                is_new: true,
                will_finalize: true,
                files: vec![scanned("a.bin", 800)],
                prior_used_bytes: 0,
                usable_bytes: 1_000,
            },
            vec![],
            (1, 1, 800),
            vec![],
        );
        assert_eq!(
            plan_report(&report),
            "Plan: disc 1 (new disc, BD_R) — 1 files, 0.0 GB, will FINALIZE\n\
             ~1 more files (~1 more discs) remain after this one."
        );
    }

    /// `fixtures/run-manifest.txt` step 10 — a deletion rides along.
    #[test]
    fn a_plan_with_a_deletion_renders_exactly_as_the_scala_build_printed_it() {
        let report = plan_of(
            PlannedDisc {
                ordinal: 3,
                is_new: true,
                will_finalize: false,
                files: vec![scanned("a.bin", 800)],
                prior_used_bytes: 0,
                usable_bytes: 1_000,
            },
            vec!["b.bin"],
            (0, 0, 0),
            vec![],
        );
        assert_eq!(
            plan_report(&report),
            "Plan: disc 3 (new disc, BD_R) — 1 files, 0.0 GB, left OPEN\n\
             This disc completes the set.\n\
             Deletions: 1"
        );
    }

    /// `fixtures/run-manifest.txt` step 18 — `plan --finalize` on a scan that
    /// found nothing.
    #[test]
    fn sealing_an_empty_disc_renders_exactly_as_the_scala_build_printed_it() {
        let report = plan_of(
            PlannedDisc {
                ordinal: 4,
                is_new: false,
                will_finalize: true,
                files: vec![],
                prior_used_bytes: 800,
                usable_bytes: 1_000,
            },
            vec![],
            (0, 0, 0),
            vec![],
        );
        assert_eq!(
            plan_report(&report),
            "Plan: finalize disc 4 (seal; no new files)."
        );
    }

    #[test]
    fn a_plan_with_nothing_to_do_says_so() {
        let report = plan_of(
            PlannedDisc {
                ordinal: 1,
                is_new: false,
                will_finalize: false,
                files: vec![],
                prior_used_bytes: 0,
                usable_bytes: 25_000_000_000,
            },
            vec![],
            (0, 0, 0),
            vec![],
        );
        assert_eq!(plan_report(&report), "Nothing to burn: no changes to plan.");
    }

    #[test]
    fn appending_to_an_open_disc_names_the_disc_and_says_left_open() {
        let report = plan_of(
            PlannedDisc {
                ordinal: 2,
                is_new: false,
                will_finalize: false,
                files: vec![scanned("a.bin", 3_200_000_000)],
                prior_used_bytes: 10_000_000_000,
                usable_bytes: 25_000_000_000,
            },
            vec![],
            (0, 0, 0),
            vec![],
        );
        assert_eq!(
            plan_report(&report),
            "Plan: disc 2 (appending to open disc 2, BD_R) — 1 files, 13.2 GB, left OPEN\n\
             This disc completes the set."
        );
    }

    #[test]
    fn oversized_files_are_listed_under_the_summary() {
        let report = plan_of(
            PlannedDisc {
                ordinal: 1,
                is_new: true,
                will_finalize: false,
                files: vec![scanned("small.bin", 10)],
                prior_used_bytes: 0,
                usable_bytes: 25_000_000_000,
            },
            vec![],
            (0, 0, 0),
            vec![scanned("huge.bin", 99_000_000_000)],
        );
        assert_eq!(
            plan_report(&report),
            "Plan: disc 1 (new disc, BD_R) — 1 files, 0.0 GB, left OPEN\n\
             This disc completes the set.\n\
             Oversized (cannot be placed):\n\
             \x20 huge.bin  99.0 GB"
        );
    }

    /// An oversize-only plan still prints the summary block rather than
    /// "nothing to burn": the disc is empty but there is something to say.
    #[test]
    fn an_oversize_only_plan_is_not_nothing_to_burn() {
        let report = plan_of(
            PlannedDisc {
                ordinal: 1,
                is_new: true,
                will_finalize: false,
                files: vec![],
                prior_used_bytes: 0,
                usable_bytes: 1_000,
            },
            vec![],
            (0, 0, 0),
            vec![scanned("huge.bin", 5_000)],
        );
        assert!(plan_report(&report).starts_with("Plan: disc 1 (new disc, BD_R) — 0 files"));
    }
}

#[cfg(test)]
mod burn_render_tests {
    use super::*;
    use diskrement_shell::pipeline::reports::{BurnReport, DiscBurn, RemainingEstimate};

    fn report(finalized: bool, more_discs: usize) -> BurnReport {
        BurnReport {
            disc: DiscBurn {
                ordinal: 1,
                label: "p disc 1".to_string(),
                finalized,
                staging_dir: "/w/staging/disc-1".into(),
                image_bytes: 4096,
            },
            remaining: RemainingEstimate {
                more_discs,
                more_files: 1,
                more_bytes: 800,
            },
        }
    }

    /// `fixtures/run-manifest.txt` step 04, byte for byte.
    #[test]
    fn a_finalized_disc_with_more_to_come_matches_the_render_oracle() {
        assert_eq!(
            burn_report(&report(true, 1)),
            "✔ Wrote disc 1  —  label: \"p disc 1\"\n\
             Status: FINALIZED — set this disc aside, it is sealed.\n\
             ~1 more discs of changes remain. Run: scan → plan → burn"
        );
    }

    /// `fixtures/run-manifest.txt` step 19: the set is complete.
    #[test]
    fn an_open_disc_with_nothing_left_matches_the_render_oracle() {
        assert_eq!(
            burn_report(&report(false, 0)),
            "✔ Wrote disc 1  —  label: \"p disc 1\"\n\
             Status: left OPEN — may receive more sessions later.\n\
             No more changes — the set is up to date."
        );
    }

    /// The first line's spacing is easy to lose in a reformat and the oracle
    /// is byte-exact, so pin it on its own.
    #[test]
    fn the_wrote_line_uses_two_spaces_around_the_dash() {
        let line = burn_report(&report(false, 0))
            .lines()
            .next()
            .unwrap()
            .to_string();
        assert_eq!(line, "✔ Wrote disc 1  —  label: \"p disc 1\"");
    }
}
