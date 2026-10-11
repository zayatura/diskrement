//! Output rendering (`Render.scala`). Success text goes to stdout; error text
//! is `PipelineError`'s `Display` and goes to stderr with its exit code.

use diskrement_core::RelPath;
use diskrement_shell::catalog::model::{FileHistoryEntry, LostReport, Project, ProjectId};
use diskrement_shell::catalog::time;
use diskrement_shell::error::PipelineError;
use diskrement_shell::pipeline::reports::{
    BurnReport, PlanReport, ScanReport, StatusReport, VerifyReport,
};

pub fn init_done(name: &str, id: ProjectId) -> String {
    format!("Created project {name} (id {}).", id.0)
}

/// `Scanned: …`, plus the `Hashed N of M files` line for a fresh scan only.
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

/// `Render.verify`. One block per disc, in sweep order; an empty sweep is a
/// sentence rather than a blank line.
pub fn verify_report(reports: &[VerifyReport]) -> String {
    if reports.is_empty() {
        return "Nothing to verify: no burned discs.".to_string();
    }
    reports
        .iter()
        .map(|v| {
            if v.skipped {
                format!("disc {}: skipped (not checked)", v.disc_ordinal)
            } else if v.mismatches.is_empty() {
                // "files" is not pluralised in the Scala original, and
                // run-manifest step 21 prints "disc 4: 0 files OK".
                format!("disc {}: {} files OK ✓", v.disc_ordinal, v.files_checked)
            } else {
                let listed = v
                    .mismatches
                    .iter()
                    .map(|p| format!("  {}", p.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n");
                format!(
                    "disc {}: {} checked, {} MISMATCH:\n{listed}",
                    v.disc_ordinal,
                    v.files_checked,
                    v.mismatches.len()
                )
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Result-driven exit (`Render.verifyExit`): a verify that finds corruption is
/// a **successful run with a bad outcome**, so it exits 4 rather than taking
/// the error ladder. A skipped disc found nothing and is not corruption.
pub fn verify_exit(reports: &[VerifyReport]) -> u8 {
    if reports.iter().any(|r| !r.mismatches.is_empty()) {
        4
    } else {
        0
    }
}

/// `Render.history`. The path, then one `v<n>` line per version, oldest first.
///
/// The column spacing is contract: two spaces after `v<n>`, **three** after the
/// session, two around the hash.
pub fn history(rel_path: &RelPath, entries: &[FileHistoryEntry]) -> String {
    if entries.is_empty() {
        return format!("No history for {}.", rel_path.as_str());
    }
    let mut lines = vec![rel_path.as_str().to_string()];
    for (i, e) in entries.iter().enumerate() {
        lines.push(format!(
            "  v{}  disc {} / session {}   {}  {}  {}",
            i + 1,
            e.disc.ordinal,
            e.session_ordinal,
            gb(e.size_bytes),
            e.content_hash,
            e.status
        ));
    }
    lines.join("\n")
}

/// `Render.markDead`. A header, then two blocks: what is gone forever, and
/// what will be written again.
///
/// Each block is either a count and its list, or a single "nothing" line —
/// the two are never both empty *and* silent, because an operator who just
/// destroyed a disc deserves to be told explicitly that it cost nothing. The
/// two-space indent and the double-space column gaps are contract
/// (`run-manifest.txt` step 12).
pub fn mark_dead(r: &LostReport) -> String {
    let mut lines = vec![format!("Disc {} marked DEAD.", r.disc_ordinal)];

    if r.lost.is_empty() {
        lines.push("No versions permanently lost.".to_string());
    } else {
        lines.push(format!("Permanently lost ({}):", r.lost.len()));
        lines.extend(r.lost.iter().map(|e| {
            format!(
                "  {}  {}  {}  (was session {})",
                e.rel_path.as_str(),
                gb(e.size_bytes),
                e.content_hash,
                e.session_ordinal
            )
        }));
    }

    if r.reburn.is_empty() {
        lines.push("No live files to re-burn.".to_string());
    } else {
        lines.push(format!(
            "Re-burn on next scan/plan/burn ({}):",
            r.reburn.len()
        ));
        // Only the path: the hash and size are already in the catalog, and
        // the operator's next action is `scan`, not an inspection.
        lines.extend(
            r.reburn
                .iter()
                .map(|e| format!("  {}", e.rel_path.as_str())),
        );
    }

    lines.join("\n")
}

/// The `reset` confirmation. It lives in `Runner.scala`'s dispatch rather than
/// in `Render.scala`; it is a renderer here so the snapshot gallery can cover
/// every line the CLI prints.
pub fn reset_done(name: &str) -> String {
    format!("Reset {name}.")
}

/// The stderr line for a failed run (`Render.handle`'s message half).
///
/// `PipelineError`'s own `Display` is the message everywhere except an aborted
/// burn: that one is not a fault to report but an action the operator took, so
/// it reads as `Aborted.` and exits 0.
pub fn error_line(err: &PipelineError) -> String {
    match err {
        PipelineError::Burn(diskrement_shell::burn::BurnError::Aborted) => "Aborted.".to_string(),
        other => other.to_string(),
    }
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
                    copy_set: 0,
                    status: DiscStatus::Finalized,
                    usable_bytes: 25_000_000_000,
                    used_bytes: 24_800_000_000,
                },
                DiscState {
                    ordinal: 2,
                    copy_set: 0,
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
                copy_set: 0,
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
                copy_set: 0,
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
                copy_set: 0,
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
                copy_set: 0,
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
                copy_set: 0,
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
                copy_set: 0,
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
                copy_set: 0,
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

    /// `Render.handle`'s one special case: an aborted burn is a successful run
    /// that wrote nothing, so it renders as `Aborted.` and exits 0 — not as
    /// the error's own "aborted by operator" wording.
    #[test]
    fn an_aborted_burn_renders_as_aborted_and_nothing_else_is_special() {
        use diskrement_shell::burn::BurnError;
        use diskrement_shell::error::PipelineError;

        assert_eq!(
            error_line(&PipelineError::Burn(BurnError::Aborted)),
            "Aborted."
        );
        assert_eq!(PipelineError::Burn(BurnError::Aborted).exit_code(), 0);
        assert_eq!(
            error_line(&PipelineError::Burn(BurnError::NoMedia("/dev/sr0".into()))),
            "no disc in /dev/sr0"
        );
        assert_eq!(
            error_line(&PipelineError::ProjectNotFound("photos".into())),
            "project not found: photos"
        );
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

#[cfg(test)]
mod p6_render_tests {
    use super::*;
    use diskrement_core::{ContentHash, RelPath};
    use diskrement_shell::catalog::model::{
        FileHistoryEntry, LostReport, LostVersionEntry, ReburnEntry, VersionStatus,
    };
    use diskrement_shell::pipeline::reports::VerifyReport;

    fn report(ordinal: u32, checked: usize, mismatches: &[&str], skipped: bool) -> VerifyReport {
        VerifyReport {
            disc_ordinal: ordinal,
            files_checked: checked,
            mismatches: mismatches
                .iter()
                .map(|m| RelPath((*m).to_string()))
                .collect(),
            skipped,
        }
    }

    fn entry(
        hash: &str,
        size: u64,
        disc: u32,
        session: u32,
        status: VersionStatus,
    ) -> FileHistoryEntry {
        FileHistoryEntry {
            content_hash: ContentHash(hash.to_string()),
            size_bytes: size,
            disc: diskrement_core::DiscKey::new(disc, 0),
            session_ordinal: session,
            status,
        }
    }

    #[test]
    fn an_empty_verify_says_there_is_nothing_to_check() {
        assert_eq!(verify_report(&[]), "Nothing to verify: no burned discs.");
    }

    /// `fixtures/run-manifest.txt` step 05, byte for byte.
    #[test]
    fn a_clean_disc_matches_the_render_oracle() {
        assert_eq!(
            verify_report(&[report(1, 1, &[], false)]),
            "disc 1: 1 files OK ✓"
        );
    }

    /// `fixtures/run-manifest.txt` step 21, byte for byte — including the
    /// sealed disc's zero-file line and the absence of the dead disc 3.
    #[test]
    fn a_three_disc_sweep_matches_the_render_oracle() {
        assert_eq!(
            verify_report(&[
                report(1, 1, &[], false),
                report(2, 1, &[], false),
                report(4, 0, &[], false),
            ]),
            "disc 1: 1 files OK ✓\n\
             disc 2: 1 files OK ✓\n\
             disc 4: 0 files OK ✓"
        );
    }

    #[test]
    fn a_skipped_disc_is_reported_as_not_checked() {
        assert_eq!(
            verify_report(&[report(3, 0, &[], true)]),
            "disc 3: skipped (not checked)"
        );
    }

    /// Transcribed from the staged Scala launcher on a corrupted staging tree.
    #[test]
    fn mismatches_are_listed_under_their_disc() {
        assert_eq!(
            verify_report(&[report(1, 2, &["a.bin"], false)]),
            "disc 1: 2 checked, 1 MISMATCH:\n  a.bin"
        );
        assert_eq!(
            verify_report(&[report(2, 5, &["bad/file.jpg", "other.raw"], false)]),
            "disc 2: 5 checked, 2 MISMATCH:\n  bad/file.jpg\n  other.raw"
        );
    }

    #[test]
    fn a_mixed_sweep_puts_one_disc_per_block_in_order() {
        assert_eq!(
            verify_report(&[
                report(1, 1, &[], false),
                report(2, 0, &[], true),
                report(3, 2, &["gone.bin"], false),
            ]),
            "disc 1: 1 files OK ✓\n\
             disc 2: skipped (not checked)\n\
             disc 3: 2 checked, 1 MISMATCH:\n  gone.bin"
        );
    }

    /// A verify that finds corruption is a *successful run with a bad
    /// outcome*: exit 4, not an error exit (`Render.verifyExit`).
    #[test]
    fn the_verify_exit_code_is_driven_by_the_result() {
        assert_eq!(verify_exit(&[]), 0);
        assert_eq!(verify_exit(&[report(1, 10, &[], false)]), 0);
        assert_eq!(verify_exit(&[report(1, 0, &[], true)]), 0);
        assert_eq!(
            verify_exit(&[report(1, 10, &[], false), report(2, 5, &["bad"], false)]),
            4
        );
    }

    #[test]
    fn an_empty_history_names_the_path() {
        assert_eq!(
            history(&RelPath("nope.bin".to_string()), &[]),
            "No history for nope.bin."
        );
    }

    /// `fixtures/run-manifest.txt` step 16, byte for byte. LOST is per
    /// physical copy: v2 and v3 share a content hash and read differently.
    #[test]
    fn the_three_version_history_matches_the_render_oracle() {
        let a1 = "8e19511b7991430a1950f27907234183151bb0dbf1fbf28cd5b765088feb9f70";
        let a2 = "e148cf0e8c66a7c18ea6be52a5215dd2a770d74b6c014d51d1e1e0463e882e77";
        assert_eq!(
            history(
                &RelPath("a.bin".to_string()),
                &[
                    entry(a1, 800, 1, 1, VersionStatus::Superseded),
                    entry(a2, 800, 3, 1, VersionStatus::Lost),
                    entry(a2, 800, 4, 1, VersionStatus::Current),
                ]
            ),
            format!(
                "a.bin\n\
                 \x20 v1  disc 1 / session 1   0.0 GB  {a1}  SUPERSEDED\n\
                 \x20 v2  disc 3 / session 1   0.0 GB  {a2}  LOST\n\
                 \x20 v3  disc 4 / session 1   0.0 GB  {a2}  CURRENT"
            )
        );
    }

    /// `fixtures/run-manifest.txt` step 22, byte for byte.
    #[test]
    fn a_deleted_paths_history_matches_the_render_oracle() {
        let b1 = "3413ff9e166e40ad8798e36609f28433496ee3a0f8984f73d33b0a80ffef2a89";
        assert_eq!(
            history(
                &RelPath("b.bin".to_string()),
                &[entry(b1, 800, 2, 1, VersionStatus::Deleted)]
            ),
            format!("b.bin\n\x20 v1  disc 2 / session 1   0.0 GB  {b1}  DELETED")
        );
    }

    #[test]
    fn the_history_gb_column_uses_the_same_one_decimal_format() {
        assert_eq!(
            history(
                &RelPath("big/movie.mkv".to_string()),
                &[entry(
                    "a1b2c3",
                    12_300_000_000,
                    7,
                    2,
                    VersionStatus::Current
                )]
            ),
            "big/movie.mkv\n\x20 v1  disc 7 / session 2   12.3 GB  a1b2c3  CURRENT"
        );
    }

    #[test]
    fn reset_names_the_project_it_cleared() {
        assert_eq!(reset_done("p"), "Reset p.");
        assert_eq!(reset_done("photos"), "Reset photos.");
    }

    fn lost(rel: &str, size: u64, hash: &str, session: u32) -> LostVersionEntry {
        LostVersionEntry {
            rel_path: RelPath(rel.to_string()),
            content_hash: ContentHash(hash.to_string()),
            size_bytes: size,
            session_ordinal: session,
        }
    }

    fn reburn(rel: &str, size: u64, hash: &str) -> ReburnEntry {
        ReburnEntry {
            rel_path: RelPath(rel.to_string()),
            content_hash: ContentHash(hash.to_string()),
            size_bytes: size,
        }
    }

    /// `run-manifest.txt` step 12, byte for byte. Disc 3 held one current
    /// version and nothing superseded, so the lost half is the "nothing" line.
    #[test]
    fn the_mark_dead_render_matches_the_oracle_for_step_12() {
        let a2 = "e148cf0e8c66a7c18ea6be52a5215dd2a770d74b6c014d51d1e1e0463e882e77";
        assert_eq!(
            mark_dead(&LostReport {
                disc_ordinal: 3,
                lost: vec![],
                reburn: vec![reburn("a.bin", 800, a2)],
            }),
            "Disc 3 marked DEAD.\n\
             No versions permanently lost.\n\
             Re-burn on next scan/plan/burn (1):\n\
             \x20 a.bin"
        );
    }

    /// Both halves populated. Captured from the staged Scala launcher against
    /// a two-session disc: b's first version is superseded and permanently
    /// gone, while both current versions re-queue.
    #[test]
    fn both_casualty_classes_render_with_their_counts() {
        let b1 = "b9749f157163b4bbb7faec0a55cfb88c69968a25eac4cfe13e45d6286874709a";
        let b2 = "49a7906775663af7970f00f946d1d540822233477e75f5b2912ca497780a01bc";
        let a1 = "0868513521f77faae1cf5200d9ff9603007fa04e0caef9f81183b44d46cbba86";
        assert_eq!(
            mark_dead(&LostReport {
                disc_ordinal: 1,
                lost: vec![lost("b.bin", 800, b1, 1)],
                reburn: vec![reburn("a.bin", 800, a1), reburn("b.bin", 800, b2)],
            }),
            format!(
                "Disc 1 marked DEAD.\n\
                 Permanently lost (1):\n\
                 \x20 b.bin  0.0 GB  {b1}  (was session 1)\n\
                 Re-burn on next scan/plan/burn (2):\n\
                 \x20 a.bin\n\
                 \x20 b.bin"
            )
        );
    }

    /// `RenderSpec`'s "markDead with no casualties": a disc whose every
    /// version had a live copy elsewhere costs nothing at all.
    #[test]
    fn a_disc_that_cost_nothing_says_so_twice() {
        assert_eq!(
            mark_dead(&LostReport {
                disc_ordinal: 2,
                lost: vec![],
                reburn: vec![],
            }),
            "Disc 2 marked DEAD.\n\
             No versions permanently lost.\n\
             No live files to re-burn."
        );
    }

    /// The GB column is the same one-decimal format every other renderer uses.
    #[test]
    fn the_lost_line_uses_the_shared_gb_format() {
        assert_eq!(
            mark_dead(&LostReport {
                disc_ordinal: 9,
                lost: vec![lost("big/movie.mkv", 12_300_000_000, "a1b2c3", 4)],
                reburn: vec![],
            }),
            "Disc 9 marked DEAD.\n\
             Permanently lost (1):\n\
             \x20 big/movie.mkv  12.3 GB  a1b2c3  (was session 4)\n\
             No live files to re-burn."
        );
    }
}

/// Snapshot galleries covering every line the CLI prints.
///
/// These pin Rust against Rust: they catch drift introduced by a refactor, and
/// they cannot catch a divergence from the Scala build — `run-manifest.txt` and
/// the `assert_eq!` oracles above are what do that. Snapshots are authored by
/// hand for the same reason; one generated from the code under test would
/// assert nothing.
#[cfg(test)]
mod snapshots {
    use super::*;
    use chrono::TimeZone as _;
    use diskrement_core::{
        ContentHash, DiscState, DiscStatus, DiscType, PlannedDisc, RelPath, ScannedFile,
    };
    use diskrement_shell::catalog::model::{
        FileHistoryEntry, LostReport, LostVersionEntry, ReburnEntry, VersionStatus,
    };
    use diskrement_shell::pipeline::reports::{
        BurnReport, DiscBurn, PlanReport, RemainingEstimate, ScanReport, StatusReport, VerifyReport,
    };

    /// Join labelled renders into one reviewable document.
    fn gallery(sections: &[(&str, String)]) -> String {
        sections
            .iter()
            .map(|(title, body)| format!("## {title}\n{body}"))
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// `prepend_module_to_snapshot => false` makes the file name exactly
    /// `<name>.snap`, so the committed paths are predictable and hand-writable.
    fn check(name: &str, doc: String) {
        insta::with_settings!({
            snapshot_path => "snapshots",
            prepend_module_to_snapshot => false,
        }, {
            insta::assert_snapshot!(name, doc);
        });
    }

    fn ts(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap()
    }

    fn scanned(rel: &str, size: u64) -> ScannedFile {
        ScannedFile {
            rel_path: RelPath(rel.to_string()),
            content_hash: ContentHash("deadbeef".to_string()),
            size_bytes: size,
            mtime: ts(2026, 9, 5, 14, 3, 0),
        }
    }

    fn scan_of(
        a: usize,
        m: usize,
        d: usize,
        u: usize,
        hashed: Option<usize>,
        total: usize,
    ) -> ScanReport {
        ScanReport {
            added: a,
            modified: m,
            deleted: d,
            unchanged: u,
            hashed,
            scanned_files: total,
        }
    }

    fn plan_of(
        disc: PlannedDisc,
        tombstones: &[&str],
        remaining: (usize, usize, u64),
        oversized: Vec<ScannedFile>,
    ) -> PlanReport {
        PlanReport {
            disc,
            disc_type: DiscType::BdR,
            tombstones: tombstones
                .iter()
                .map(|t| RelPath((*t).to_string()))
                .collect(),
            remaining: RemainingEstimate {
                more_discs: remaining.0,
                more_files: remaining.1,
                more_bytes: remaining.2,
            },
            oversized,
        }
    }

    fn disc(ordinal: u32, status: DiscStatus, used: u64, usable: u64) -> DiscState {
        DiscState {
            ordinal,
            copy_set: 0,
            status,
            usable_bytes: usable,
            used_bytes: used,
        }
    }

    fn project(name: &str, source: &str) -> Project {
        Project {
            id: ProjectId(1),
            name: name.to_string(),
            source_root: source.to_string(),
            work_dir: "/srv/work".to_string(),
            default_disc_type: DiscType::BdR,
            margin_percent: 2.0,
            copies: 1,
            created_at: ts(2026, 1, 1, 0, 0, 0),
        }
    }

    fn burn_of(ordinal: u32, label: &str, finalized: bool, more_discs: usize) -> BurnReport {
        BurnReport {
            disc: DiscBurn {
                ordinal,
                label: label.to_string(),
                finalized,
                staging_dir: "/srv/work/staging/disc-1".into(),
                image_bytes: 4096,
            },
            remaining: RemainingEstimate {
                more_discs,
                more_files: 1,
                more_bytes: 800,
            },
        }
    }

    fn verify_of(ordinal: u32, checked: usize, mismatches: &[&str], skipped: bool) -> VerifyReport {
        VerifyReport {
            disc_ordinal: ordinal,
            files_checked: checked,
            mismatches: mismatches
                .iter()
                .map(|m| RelPath((*m).to_string()))
                .collect(),
            skipped,
        }
    }

    fn version(hash: &str, size: u64, d: u32, s: u32, status: VersionStatus) -> FileHistoryEntry {
        FileHistoryEntry {
            content_hash: ContentHash(hash.to_string()),
            size_bytes: size,
            disc: diskrement_core::DiscKey::new(d, 0),
            session_ordinal: s,
            status,
        }
    }

    #[test]
    fn init_gallery() {
        check(
            "init",
            gallery(&[
                ("a fresh project", init_done("photos", ProjectId(7))),
                ("a multi-digit id", init_done("archive", ProjectId(42))),
            ]),
        );
    }

    #[test]
    fn scan_gallery() {
        check(
            "scan",
            gallery(&[
                (
                    "a fresh scan",
                    scan_report(&scan_of(2, 1, 0, 3, Some(3), 6)),
                ),
                (
                    "a reused staged scan (no Hashed line)",
                    scan_report(&scan_of(2, 1, 0, 3, None, 6)),
                ),
                (
                    "nothing changed",
                    scan_report(&scan_of(0, 0, 0, 6, Some(0), 6)),
                ),
            ]),
        );
    }

    #[test]
    fn plan_gallery() {
        check(
            "plan",
            gallery(&[
                (
                    "nothing to do",
                    plan_report(&plan_of(
                        PlannedDisc {
                            ordinal: 1,
                            copy_set: 0,
                            is_new: false,
                            will_finalize: false,
                            files: vec![],
                            prior_used_bytes: 0,
                            usable_bytes: 25_000_000_000,
                        },
                        &[],
                        (0, 0, 0),
                        vec![],
                    )),
                ),
                (
                    "a bare seal (run-manifest step 18)",
                    plan_report(&plan_of(
                        PlannedDisc {
                            ordinal: 4,
                            copy_set: 0,
                            is_new: false,
                            will_finalize: true,
                            files: vec![],
                            prior_used_bytes: 800,
                            usable_bytes: 1_000,
                        },
                        &[],
                        (0, 0, 0),
                        vec![],
                    )),
                ),
                (
                    "a spilling plan (run-manifest step 03)",
                    plan_report(&plan_of(
                        PlannedDisc {
                            ordinal: 1,
                            copy_set: 0,
                            is_new: true,
                            will_finalize: true,
                            files: vec![scanned("a.bin", 800)],
                            prior_used_bytes: 0,
                            usable_bytes: 1_000,
                        },
                        &[],
                        (1, 1, 800),
                        vec![],
                    )),
                ),
                (
                    "appending to an open disc",
                    plan_report(&plan_of(
                        PlannedDisc {
                            ordinal: 2,
                            copy_set: 0,
                            is_new: false,
                            will_finalize: false,
                            files: vec![scanned("a.bin", 3_200_000_000)],
                            prior_used_bytes: 10_000_000_000,
                            usable_bytes: 25_000_000_000,
                        },
                        &[],
                        (0, 0, 0),
                        vec![],
                    )),
                ),
                (
                    "a deletion rides along (run-manifest step 10)",
                    plan_report(&plan_of(
                        PlannedDisc {
                            ordinal: 3,
                            copy_set: 0,
                            is_new: true,
                            will_finalize: false,
                            files: vec![scanned("a.bin", 800)],
                            prior_used_bytes: 0,
                            usable_bytes: 1_000,
                        },
                        &["b.bin"],
                        (0, 0, 0),
                        vec![],
                    )),
                ),
                (
                    "oversized files",
                    plan_report(&plan_of(
                        PlannedDisc {
                            ordinal: 1,
                            copy_set: 0,
                            is_new: true,
                            will_finalize: false,
                            files: vec![scanned("small.bin", 10)],
                            prior_used_bytes: 0,
                            usable_bytes: 25_000_000_000,
                        },
                        &[],
                        (0, 0, 0),
                        vec![scanned("huge.bin", 99_000_000_000)],
                    )),
                ),
            ]),
        );
    }

    #[test]
    fn status_gallery() {
        let two = StatusReport {
            discs: vec![
                disc(1, DiscStatus::Finalized, 24_800_000_000, 25_000_000_000),
                disc(2, DiscStatus::Open, 3_200_000_000, 25_000_000_000),
            ],
            open_disc_ordinal: Some(2),
            total_current_files: 556,
            last_burn_at: Some(ts(2026, 6, 21, 14, 3, 0)),
        };
        let fresh = StatusReport {
            discs: vec![],
            open_disc_ordinal: None,
            total_current_files: 0,
            last_burn_at: None,
        };
        let with_dead = StatusReport {
            discs: vec![
                disc(1, DiscStatus::Finalized, 0, 0),
                disc(2, DiscStatus::Open, 0, 0),
                disc(3, DiscStatus::Dead, 0, 0),
                disc(4, DiscStatus::Finalized, 0, 0),
            ],
            open_disc_ordinal: Some(2),
            total_current_files: 1,
            last_burn_at: Some(ts(2026, 9, 5, 20, 1, 3)),
        };
        check(
            "status",
            gallery(&[
                (
                    "two discs, one open",
                    status_report(&project("photos", "/srv/photos"), &two),
                ),
                (
                    "nothing burned yet",
                    status_report(&project("photos", "/srv/photos"), &fresh),
                ),
                (
                    "a dead disc in the table (run-manifest step 20 shape)",
                    status_report(&project("p", "/srv/source"), &with_dead),
                ),
            ]),
        );
    }

    #[test]
    fn burn_gallery() {
        check(
            "burn",
            gallery(&[
                (
                    "a finalized disc with more to come (run-manifest step 04)",
                    burn_report(&burn_of(1, "p disc 1", true, 1)),
                ),
                (
                    "an open disc, set complete (run-manifest step 08)",
                    burn_report(&burn_of(2, "p disc 2", false, 0)),
                ),
            ]),
        );
    }

    #[test]
    fn verify_gallery() {
        check(
            "verify",
            gallery(&[
                ("nothing burned", verify_report(&[])),
                (
                    "a clean sweep (run-manifest step 21)",
                    verify_report(&[
                        verify_of(1, 1, &[], false),
                        verify_of(2, 1, &[], false),
                        verify_of(4, 0, &[], false),
                    ]),
                ),
                (
                    "a skipped disc",
                    verify_report(&[verify_of(3, 0, &[], true)]),
                ),
                (
                    "mismatches",
                    verify_report(&[verify_of(2, 5, &["bad/file.jpg", "other.raw"], false)]),
                ),
                (
                    "a mixed sweep",
                    verify_report(&[
                        verify_of(1, 1, &[], false),
                        verify_of(2, 0, &[], true),
                        verify_of(3, 2, &["gone.bin"], false),
                    ]),
                ),
            ]),
        );
    }

    #[test]
    fn history_gallery() {
        let a1 = "8e19511b7991430a1950f27907234183151bb0dbf1fbf28cd5b765088feb9f70";
        let a2 = "e148cf0e8c66a7c18ea6be52a5215dd2a770d74b6c014d51d1e1e0463e882e77";
        let b1 = "3413ff9e166e40ad8798e36609f28433496ee3a0f8984f73d33b0a80ffef2a89";
        check(
            "history",
            gallery(&[
                ("no history", history(&RelPath("nope.bin".to_string()), &[])),
                (
                    "three versions, one lost (run-manifest step 16)",
                    history(
                        &RelPath("a.bin".to_string()),
                        &[
                            version(a1, 800, 1, 1, VersionStatus::Superseded),
                            version(a2, 800, 3, 1, VersionStatus::Lost),
                            version(a2, 800, 4, 1, VersionStatus::Current),
                        ],
                    ),
                ),
                (
                    "a deleted path (run-manifest step 22)",
                    history(
                        &RelPath("b.bin".to_string()),
                        &[version(b1, 800, 2, 1, VersionStatus::Deleted)],
                    ),
                ),
                (
                    "the GB column",
                    history(
                        &RelPath("big/movie.mkv".to_string()),
                        &[version(
                            "a1b2c3",
                            12_300_000_000,
                            7,
                            2,
                            VersionStatus::Current,
                        )],
                    ),
                ),
            ]),
        );
    }

    #[test]
    fn reset_gallery() {
        check(
            "reset",
            gallery(&[
                ("a one-word project", reset_done("p")),
                ("a longer name", reset_done("photos")),
            ]),
        );
    }

    fn lost_entry(rel: &str, size: u64, hash: &str, session: u32) -> LostVersionEntry {
        LostVersionEntry {
            rel_path: RelPath(rel.to_string()),
            content_hash: ContentHash(hash.to_string()),
            size_bytes: size,
            session_ordinal: session,
        }
    }

    fn reburn_entry(rel: &str, size: u64, hash: &str) -> ReburnEntry {
        ReburnEntry {
            rel_path: RelPath(rel.to_string()),
            content_hash: ContentHash(hash.to_string()),
            size_bytes: size,
        }
    }

    #[test]
    fn mark_dead_gallery() {
        let a1 = "0868513521f77faae1cf5200d9ff9603007fa04e0caef9f81183b44d46cbba86";
        let a2 = "e148cf0e8c66a7c18ea6be52a5215dd2a770d74b6c014d51d1e1e0463e882e77";
        let b1 = "b9749f157163b4bbb7faec0a55cfb88c69968a25eac4cfe13e45d6286874709a";
        check(
            "mark_dead",
            gallery(&[
                (
                    "nothing lost, nothing to re-burn",
                    mark_dead(&LostReport {
                        disc_ordinal: 2,
                        lost: vec![],
                        reburn: vec![],
                    }),
                ),
                (
                    "one live file re-queued (run-manifest step 12)",
                    mark_dead(&LostReport {
                        disc_ordinal: 3,
                        lost: vec![],
                        reburn: vec![reburn_entry("a.bin", 800, a2)],
                    }),
                ),
                (
                    "a superseded version gone forever",
                    mark_dead(&LostReport {
                        disc_ordinal: 1,
                        lost: vec![lost_entry("b.bin", 800, b1, 1)],
                        reburn: vec![],
                    }),
                ),
                (
                    "both halves, with the GB column",
                    mark_dead(&LostReport {
                        disc_ordinal: 4,
                        lost: vec![
                            lost_entry("b.bin", 800, b1, 1),
                            lost_entry("big/movie.mkv", 12_300_000_000, "a1b2c3", 2),
                        ],
                        reburn: vec![
                            reburn_entry("a.bin", 800, a1),
                            reburn_entry("nested/c.bin", 100, "c0ffee"),
                        ],
                    }),
                ),
            ]),
        );
    }
}
