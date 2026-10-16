//! Output rendering (`Render.scala`). Success text goes to stdout; error text
//! is `PipelineError`'s `Display` and goes to stderr with its exit code.

use diskrement_core::{set_letter, RelPath, SetCoverage, Sources};
use diskrement_shell::catalog::model::{
    FileHistoryEntry, LostReport, Project, ProjectId, VersionStatus,
};
use diskrement_shell::catalog::time;
use diskrement_shell::error::PipelineError;
use diskrement_shell::pipeline::reports::{
    BurnReport, PlanReport, ScanReport, StatusReport, VerifyReport,
};

pub fn init_done(name: &str, id: ProjectId) -> String {
    format!("Created project {name} (id {}).", id.0)
}

/// `Scanned: …`, then how many files were hashed and how many reused a
/// fingerprint.
pub fn scan_report(r: &ScanReport) -> String {
    format!(
        "Scanned: {} added, {} modified, {} deleted, {} unchanged.\n\
         Hashed {} of {} files ({} fingerprints reused).",
        r.added,
        r.modified,
        r.deleted,
        r.unchanged,
        r.hashed,
        r.scanned_files,
        r.scanned_files - r.hashed
    )
}

/// `Render.plan`. Four shapes: nothing to do, deletions waiting for the next
/// disc (no disc is planned), bare seals, and one summary pair per copy set
/// with the optional deletion, oversize and skipped catch-up lines. At
/// `copies = 1` there is one set and the output is the single-disc text.
pub fn plan_report(p: &PlanReport) -> String {
    let copies = p.copies;
    if p.sets.is_empty() {
        // No disc this cycle (issue #9): deletions wait for the next one.
        let mut lines = Vec::new();
        match p.tombstones.len() {
            0 => {}
            1 => lines.push("Nothing to burn: 1 deletion waits for the next disc.".to_string()),
            n => lines.push(format!(
                "Nothing to burn: {n} deletions wait for the next disc."
            )),
        }
        lines.extend(oversized_lines(p));
        lines.extend(unplaceable_catch_up_lines(p));
        if lines.is_empty() {
            return "Nothing to burn: no changes to plan.".to_string();
        }
        return lines.join("\n");
    }
    let nothing = p.sets.iter().all(|s| s.disc.files.is_empty())
        && p.tombstones.is_empty()
        && p.oversized.is_empty()
        && p.unplaceable_catch_up.is_empty();
    if nothing {
        let seals: Vec<String> = p
            .sets
            .iter()
            .filter(|s| s.disc.will_finalize)
            .map(|s| {
                format!(
                    "Plan: finalize disc {} (seal; no new files).",
                    s.disc.key().name(copies)
                )
            })
            .collect();
        return if seals.is_empty() {
            "Nothing to burn: no changes to plan.".to_string()
        } else {
            seals.join("\n")
        };
    }
    let mut lines = Vec::new();
    for s in &p.sets {
        let d = &s.disc;
        let name = d.key().name(copies);
        let kind = if d.is_new {
            "new disc".to_string()
        } else {
            format!("appending to open disc {name}")
        };
        let fin = if d.will_finalize {
            "will FINALIZE"
        } else {
            "left OPEN"
        };
        let count = if copies > 1 {
            format!(
                "{} files ({} new, {} catch-up)",
                d.files.len(),
                d.files.len() - s.catch_up,
                s.catch_up
            )
        } else {
            format!("{} files", d.files.len())
        };
        lines.push(format!(
            "Plan: disc {name} ({kind}, {}) — {count}, {}, {fin}",
            p.disc_type,
            gb(d.total_used_bytes())
        ));
        lines.push(if s.remaining.more_discs > 0 {
            format!(
                "~{} more files (~{} more discs) remain after this one.",
                s.remaining.more_files, s.remaining.more_discs
            )
        } else if copies > 1 {
            // "the set" would read as copy set A/B here.
            format!(
                "Nothing more for set {} after this disc.",
                set_letter(d.copy_set)
            )
        } else {
            "This disc completes the set.".to_string()
        });
    }
    if !p.tombstones.is_empty() {
        lines.push(format!("Deletions: {}", p.tombstones.len()));
    }
    lines.extend(oversized_lines(p));
    lines.extend(unplaceable_catch_up_lines(p));
    lines.join("\n")
}

/// The oversize block: a header and one line per file, or nothing.
fn oversized_lines(p: &PlanReport) -> Vec<String> {
    if p.oversized.is_empty() {
        return Vec::new();
    }
    let mut lines = vec!["Oversized (cannot be placed):".to_string()];
    for f in &p.oversized {
        lines.push(format!("  {}  {}", f.rel_path.as_str(), gb(f.size_bytes)));
    }
    lines
}

/// Catch-up copies this disc type cannot hold (issue #12): a header naming
/// the disc type and the way out, and one line per file, or nothing. Unlike
/// the oversize block this never stops `burn`.
fn unplaceable_catch_up_lines(p: &PlanReport) -> Vec<String> {
    if p.unplaceable_catch_up.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![format!(
        "Catch-up copies too large for {} (skipped; plan with a larger --disc-type to place them):",
        p.disc_type
    )];
    for f in &p.unplaceable_catch_up {
        lines.push(format!("  {}  {}", f.rel_path.as_str(), gb(f.size_bytes)));
    }
    lines
}

/// `f"${bytes / 1.0e9}%.1f GB"` (`Render.gb`).
pub fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1.0e9)
}

/// `Render.burn`: one block per disc written (or why none was), the copy
/// coverage when the project keeps more than one copy, any files with no live
/// copy at all, then what to do next.
pub fn burn_report(r: &BurnReport, copies: u32) -> String {
    let mut lines = Vec::new();
    match r.deferred_deletions {
        Some(0) => lines.push("Nothing burned: no disc in this plan.".to_string()),
        Some(1) => lines
            .push("Nothing burned: 1 deletion will be recorded with the next disc.".to_string()),
        Some(n) => lines.push(format!(
            "Nothing burned: {n} deletions will be recorded with the next disc."
        )),
        None if r.discs.is_empty() => {
            lines.push("No disc written: every planned disc was already committed.".to_string())
        }
        None => {}
    }
    for d in &r.discs {
        lines.push(format!(
            "✔ Wrote disc {}  —  label: \"{}\"",
            d.disc.name(copies),
            d.label
        ));
        lines.push(
            if d.finalized {
                "Status: FINALIZED — set this disc aside, it is sealed."
            } else {
                "Status: left OPEN — may receive more sessions later."
            }
            .to_string(),
        );
    }
    if copies > 1 {
        lines.extend(coverage_lines(&r.coverage.sets));
    }
    // While a file has no live copy, or a set lacks one, "up to date" would
    // be false.
    let no_live_copy = no_live_copy_line(&r.coverage.no_live_copy);
    let sets_complete = r.coverage.sets.iter().all(|c| c.missing_files == 0);
    let next = if r.remaining.more_discs > 0 {
        Some(format!(
            "~{} more discs of changes remain. Run: plan → burn",
            r.remaining.more_discs
        ))
    } else if no_live_copy.is_some() {
        // That line already says what to run; those files also count as
        // missing from every set.
        None
    } else if !sets_complete {
        // A lagging set catches up one disc per cycle; `plan` names any copy
        // this disc type cannot hold (issue #12).
        Some("Copy sets are incomplete. Run: plan → burn".to_string())
    } else if copies > 1 {
        Some("No more changes — every copy set is up to date.".to_string())
    } else {
        Some("No more changes — the set is up to date.".to_string())
    };
    lines.extend(no_live_copy);
    lines.extend(next);
    lines.join("\n")
}

/// One line when every set is complete, otherwise one line per lagging set.
/// Nothing at all for an empty coverage, rather than "0 copies".
fn coverage_lines(coverage: &[SetCoverage]) -> Vec<String> {
    if coverage.is_empty() {
        return Vec::new();
    }
    if coverage.iter().all(|c| c.missing_files == 0) {
        return vec![format!(
            "Copies: every current file has {} copies.",
            coverage.len()
        )];
    }
    coverage
        .iter()
        .enumerate()
        .filter(|(_, c)| c.missing_files > 0)
        .map(|(set, c)| {
            format!(
                "Copies: set {} is missing {} files ({}).",
                set_letter(set as u32),
                c.missing_files,
                gb(c.missing_bytes)
            )
        })
        .collect()
}

/// The files with no live copy in any set, or nothing when there are none.
/// Worded after `mark_dead`'s "Re-burn on next plan/burn".
fn no_live_copy_line(c: &SetCoverage) -> Option<String> {
    (c.missing_files > 0).then(|| {
        format!(
            "No live copy: {} files ({}) — re-burn on next plan/burn.",
            c.missing_files,
            gb(c.missing_bytes)
        )
    })
}

pub fn status_report(p: &Project, r: &StatusReport) -> String {
    // Two spaces after the project name; margin is %.1f plus a literal %.
    let copies = if p.copies > 1 {
        format!(", {} copies", p.copies)
    } else {
        String::new()
    };
    let source = match &p.sources {
        Sources::Single(path) => format!("source {}", path.display()),
        Sources::Rooted(roots) if roots.len() == 1 => "1 root".to_string(),
        Sources::Rooted(roots) => format!("{} roots", roots.len()),
    };
    let mut out = format!(
        "Project {}  ({source}, {}, margin {:.1}%{copies})\n",
        p.name, p.default_disc_type, p.margin_percent
    );
    if let Sources::Rooted(roots) = &p.sources {
        out.push_str("Roots:\n");
        for (name, path) in roots {
            out.push_str(&format!("  {name} → {}\n", path.display()));
        }
    }
    out.push_str("Discs:\n");
    for d in &r.discs {
        let open = if r.open_discs.contains(&d.key()) {
            "  <- open"
        } else {
            ""
        };
        out.push_str(&format!(
            "  #{:<3} {:<10} {} / {}{}\n",
            d.key().name(p.copies),
            d.status.as_str(),
            gb(d.used_bytes),
            gb(d.usable_bytes),
            open
        ));
    }
    if p.copies > 1 {
        out.push_str("Copies:\n");
        for (set, c) in r.coverage.sets.iter().enumerate() {
            let letter = set_letter(set as u32);
            if c.missing_files == 0 {
                out.push_str(&format!("  set {letter}: complete\n"));
            } else {
                out.push_str(&format!(
                    "  set {letter}: {} files ({}) missing\n",
                    c.missing_files,
                    gb(c.missing_bytes)
                ));
            }
        }
    }
    if let Some(line) = no_live_copy_line(&r.coverage.no_live_copy) {
        out.push_str(&line);
        out.push('\n');
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
pub fn verify_report(reports: &[VerifyReport], copies: u32) -> String {
    if reports.is_empty() {
        return "Nothing to verify: no burned discs.".to_string();
    }
    reports
        .iter()
        .map(|v| {
            if v.skipped {
                format!("disc {}: skipped (not checked)", v.disc.name(copies))
            } else if v.mismatches.is_empty() {
                // "files" is not pluralised in the Scala original, and
                // run-manifest step 21 prints "disc 4: 0 files OK".
                format!(
                    "disc {}: {} files OK ✓",
                    v.disc.name(copies),
                    v.files_checked
                )
            } else {
                let listed = v
                    .mismatches
                    .iter()
                    .map(|p| format!("  {}", p.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n");
                format!(
                    "disc {}: {} checked, {} MISMATCH:\n{listed}",
                    v.disc.name(copies),
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

/// `Render.history`. The path, then each version, oldest first.
///
/// At `copies = 1` a version has one copy and keeps its one-line form; the
/// column spacing is contract: two spaces after `v<n>`, **three** after the
/// session, two around the hash. At `copies > 1` a version prints its size and
/// hash once, then one line per copy with its disc and status, so the copies
/// of one version never read as separate versions (issue #7).
///
/// A `CURRENT` copy on a dead disc says so: the status precedence keeps it
/// `CURRENT` (the source still has the file and the next burn re-writes it),
/// but the copy itself is gone.
pub fn history(rel_path: &RelPath, entries: &[FileHistoryEntry], copies: u32) -> String {
    if entries.is_empty() {
        return format!("No history for {}.", rel_path.as_str());
    }
    let status = |e: &FileHistoryEntry| {
        if e.on_dead_disc && e.status == VersionStatus::Current {
            format!("{} (on a dead disc)", e.status)
        } else {
            e.status.to_string()
        }
    };
    let mut lines = vec![rel_path.as_str().to_string()];
    if copies == 1 {
        for e in entries {
            lines.push(format!(
                "  v{}  disc {} / session {}   {}  {}  {}",
                e.version,
                e.disc.name(copies),
                e.session_ordinal,
                gb(e.size_bytes),
                e.content_hash,
                status(e)
            ));
        }
        return lines.join("\n");
    }
    for (i, e) in entries.iter().enumerate() {
        if i == 0 || entries[i - 1].version != e.version {
            lines.push(format!(
                "  v{}  {}  {}",
                e.version,
                gb(e.size_bytes),
                e.content_hash
            ));
        }
        lines.push(format!(
            "      disc {} / session {}   {}",
            e.disc.name(copies),
            e.session_ordinal,
            status(e)
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
pub fn mark_dead(r: &LostReport, copies: u32) -> String {
    let mut lines = vec![format!("Disc {} marked DEAD.", r.disc.name(copies))];

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
        lines.push(format!("Re-burn on next plan/burn ({}):", r.reburn.len()));
        // Only the path: the hash and size are already in the catalog, and
        // the operator's next action is `plan`, not an inspection.
        lines.extend(
            r.reburn
                .iter()
                .map(|e| format!("  {}", e.rel_path.as_str())),
        );
    }

    if !r.degraded.is_empty() {
        lines.push(format!(
            "Short of a copy — restored on next plan/burn ({}):",
            r.degraded.len()
        ));
        lines.extend(
            r.degraded
                .iter()
                .map(|e| format!("  {}", e.rel_path.as_str())),
        );
    }
    if r.superseded_survivors > 0 {
        lines.push(format!(
            "Superseded versions still held on another disc: {}",
            r.superseded_survivors
        ));
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
        PipelineError::PartialBurn {
            committed,
            unrecorded,
            pending,
            cause,
        } if matches!(
            **cause,
            PipelineError::Burn(diskrement_shell::burn::BurnError::Aborted)
        ) =>
        {
            format!(
                "Aborted. {}.",
                diskrement_shell::error::partial_burn_tail(committed, unrecorded, pending, cause)
            )
        }
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
            sources: Sources::Single("/srv/photos".into()),
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
            open_discs: vec![diskrement_core::DiscKey::new(2, 0)],
            coverage: diskrement_core::Coverage::default(),
            total_current_files: 556,
            last_burn_at: Some(ts(2026, 6, 21)),
        }
    }

    #[test]
    fn a_two_copy_status_names_discs_by_set_and_shows_coverage() {
        let mut p = project();
        p.copies = 2;
        let d = |ordinal, copy_set, status| DiscState {
            ordinal,
            copy_set,
            status,
            usable_bytes: 25_000_000_000,
            used_bytes: 1_000_000_000,
        };
        let r = StatusReport {
            discs: vec![d(1, 0, DiscStatus::Open), d(1, 1, DiscStatus::Dead)],
            open_discs: vec![diskrement_core::DiscKey::new(1, 0)],
            total_current_files: 2,
            last_burn_at: None,
            coverage: diskrement_core::Coverage {
                sets: vec![
                    SetCoverage::default(),
                    SetCoverage {
                        missing_files: 2,
                        missing_bytes: 300_000_000,
                    },
                ],
                no_live_copy: SetCoverage::default(),
            },
        };
        assert_eq!(
            status_report(&p, &r),
            "Project photos  (source /srv/photos, BD_R, margin 2.0%, 2 copies)\n\
             Discs:\n  \
             #1A  OPEN       1.0 GB / 25.0 GB  <- open\n  \
             #1B  DEAD       1.0 GB / 25.0 GB\n\
             Copies:\n  \
             set A: complete\n  \
             set B: 2 files (0.3 GB) missing\n\
             Current files: 2      Last burn: never"
        );
    }

    #[test]
    fn a_single_copy_status_names_the_files_with_no_live_copy() {
        let gone = SetCoverage {
            missing_files: 2,
            missing_bytes: 800_000_000,
        };
        let r = StatusReport {
            discs: vec![],
            open_discs: vec![],
            total_current_files: 2,
            last_burn_at: None,
            coverage: diskrement_core::Coverage {
                sets: vec![gone],
                no_live_copy: gone,
            },
        };
        assert_eq!(
            status_report(&project(), &r),
            "Project photos  (source /srv/photos, BD_R, margin 2.0%)\n\
             Discs:\n\
             No live copy: 2 files (0.8 GB) — re-burn on next plan/burn.\n\
             Current files: 2      Last burn: never"
        );
    }

    fn nothing_burned() -> StatusReport {
        StatusReport {
            discs: vec![],
            open_discs: vec![],
            coverage: diskrement_core::Coverage::default(),
            total_current_files: 0,
            last_burn_at: None,
        }
    }

    #[test]
    fn a_rooted_status_names_its_roots_before_the_discs() {
        let mut p = project();
        p.sources = Sources::rooted(vec![
            ("others/cecil".parse().unwrap(), "/lib/u3".into()),
            ("family/anna".parse().unwrap(), "/lib/u1".into()),
        ])
        .unwrap();
        assert_eq!(
            status_report(&p, &nothing_burned()),
            "Project photos  (2 roots, BD_R, margin 2.0%)\n\
             Roots:\n  \
             family/anna → /lib/u1\n  \
             others/cecil → /lib/u3\n\
             Discs:\n\
             Current files: 0      Last burn: never"
        );
    }

    #[test]
    fn a_status_with_one_root_says_one_root() {
        let mut p = project();
        p.sources = Sources::rooted(vec![("anna".parse().unwrap(), "/lib/u1".into())]).unwrap();
        assert!(status_report(&p, &nothing_burned()).starts_with(
            "Project photos  (1 root, BD_R, margin 2.0%)\nRoots:\n  anna → /lib/u1\nDiscs:\n"
        ));
    }

    #[test]
    fn a_two_copy_status_with_no_live_copy_shows_no_set_complete() {
        let mut p = project();
        p.copies = 2;
        let r = StatusReport {
            discs: vec![],
            open_discs: vec![],
            total_current_files: 3,
            last_burn_at: None,
            coverage: diskrement_core::Coverage {
                sets: vec![
                    SetCoverage {
                        missing_files: 2,
                        missing_bytes: 800_000_000,
                    },
                    SetCoverage {
                        missing_files: 3,
                        missing_bytes: 900_000_000,
                    },
                ],
                no_live_copy: SetCoverage {
                    missing_files: 2,
                    missing_bytes: 800_000_000,
                },
            },
        };
        assert_eq!(
            status_report(&p, &r),
            "Project photos  (source /srv/photos, BD_R, margin 2.0%, 2 copies)\n\
             Discs:\n\
             Copies:\n  \
             set A: 2 files (0.8 GB) missing\n  \
             set B: 3 files (0.9 GB) missing\n\
             No live copy: 2 files (0.8 GB) — re-burn on next plan/burn.\n\
             Current files: 3      Last burn: never"
        );
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
            hashed: 3,
            scanned_files: 6,
        };
        assert_eq!(
            scan_report(&r),
            "Scanned: 2 added, 1 modified, 0 deleted, 3 unchanged.\n\
             Hashed 3 of 6 files (3 fingerprints reused)."
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

    use diskrement_core::{ContentHash, DiscKey, PlannedDisc, RelPath, ScannedFile};
    use diskrement_shell::pipeline::reports::{
        BurnReport, DiscBurn, PlanReport, RemainingEstimate, SetPlan,
    };

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
            copies: 1,
            sets: vec![SetPlan {
                disc,
                catch_up: 0,
                session_ordinal: 1,
                remaining: RemainingEstimate {
                    more_discs: remaining.0,
                    more_files: remaining.1,
                    more_bytes: remaining.2,
                },
            }],
            disc_type: DiscType::BdR,
            tombstones: tombstones.into_iter().map(|t| RelPath(t.into())).collect(),
            oversized,
            unplaceable_catch_up: vec![],
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

    fn zero_set_plan(tombstones: &[&str], oversized: Vec<ScannedFile>) -> PlanReport {
        PlanReport {
            copies: 2,
            sets: vec![],
            disc_type: DiscType::BdR,
            tombstones: tombstones.iter().map(|t| RelPath((*t).into())).collect(),
            oversized,
            unplaceable_catch_up: vec![],
        }
    }

    #[test]
    fn a_plan_with_no_disc_and_no_deletions_has_nothing_to_burn() {
        assert_eq!(
            plan_report(&zero_set_plan(&[], vec![])),
            "Nothing to burn: no changes to plan."
        );
    }

    #[test]
    fn a_deletion_only_plan_says_the_deletions_wait_for_the_next_disc() {
        assert_eq!(
            plan_report(&zero_set_plan(&["gone"], vec![])),
            "Nothing to burn: 1 deletion waits for the next disc."
        );
        assert_eq!(
            plan_report(&zero_set_plan(&["a", "b"], vec![])),
            "Nothing to burn: 2 deletions wait for the next disc."
        );
    }

    #[test]
    fn a_plan_with_no_disc_still_lists_oversized_files() {
        assert_eq!(
            plan_report(&zero_set_plan(
                &[],
                vec![scanned("huge.bin", 99_000_000_000)]
            )),
            "Oversized (cannot be placed):\n\
             \x20 huge.bin  99.0 GB"
        );
    }

    fn deferred_burn(n: usize, copies: usize) -> BurnReport {
        BurnReport {
            discs: vec![],
            remaining: RemainingEstimate {
                more_discs: 0,
                more_files: 0,
                more_bytes: 0,
            },
            coverage: diskrement_core::Coverage {
                sets: vec![SetCoverage::default(); copies],
                no_live_copy: SetCoverage::default(),
            },
            deferred_deletions: Some(n),
        }
    }

    #[test]
    fn a_burn_of_a_plan_with_no_disc_says_nothing_was_burned() {
        assert_eq!(
            burn_report(&deferred_burn(0, 2), 2),
            "Nothing burned: no disc in this plan.\n\
             Copies: every current file has 2 copies.\n\
             No more changes — every copy set is up to date."
        );
        assert!(burn_report(&deferred_burn(1, 2), 2)
            .starts_with("Nothing burned: 1 deletion will be recorded with the next disc.\n"));
        assert!(burn_report(&deferred_burn(3, 1), 1)
            .starts_with("Nothing burned: 3 deletions will be recorded with the next disc.\n"));
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

    /// A skipped catch-up copy follows the oversize block, and a plan whose
    /// only content is one still prints rather than "nothing to burn".
    #[test]
    fn unplaceable_catch_up_copies_are_listed_after_the_oversize_block() {
        let mut report = zero_set_plan(&[], vec![scanned("huge.bin", 99_000_000_000)]);
        report.disc_type = DiscType::Dvd;
        report.unplaceable_catch_up = vec![scanned("film.mkv", 20_000_000_000)];
        assert_eq!(
            plan_report(&report),
            "Oversized (cannot be placed):\n\
             \x20 huge.bin  99.0 GB\n\
             Catch-up copies too large for DVD (skipped; plan with a larger --disc-type \
             to place them):\n\
             \x20 film.mkv  20.0 GB"
        );

        report.oversized.clear();
        assert!(
            plan_report(&report).starts_with("Catch-up copies too large for DVD"),
            "{}",
            plan_report(&report)
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

    #[test]
    fn a_two_copy_plan_names_each_set_and_splits_new_from_catch_up() {
        let disc = |set, files| PlannedDisc {
            ordinal: 2,
            copy_set: set,
            is_new: true,
            will_finalize: false,
            files,
            prior_used_bytes: 0,
            usable_bytes: 25_000_000_000,
        };
        let none = RemainingEstimate {
            more_discs: 0,
            more_files: 0,
            more_bytes: 0,
        };
        let p = PlanReport {
            copies: 2,
            sets: vec![
                SetPlan {
                    disc: disc(0, vec![scanned("n", 1)]),
                    catch_up: 0,
                    session_ordinal: 1,
                    remaining: none.clone(),
                },
                SetPlan {
                    disc: disc(1, vec![scanned("c", 1), scanned("n", 1)]),
                    catch_up: 1,
                    session_ordinal: 1,
                    remaining: none,
                },
            ],
            disc_type: DiscType::BdR,
            tombstones: vec![],
            oversized: vec![],
            unplaceable_catch_up: vec![],
        };
        assert_eq!(
            plan_report(&p),
            "Plan: disc 2A (new disc, BD_R) — 1 files (1 new, 0 catch-up), 0.0 GB, left OPEN\n\
             Nothing more for set A after this disc.\n\
             Plan: disc 2B (new disc, BD_R) — 2 files (1 new, 1 catch-up), 0.0 GB, left OPEN\n\
             Nothing more for set B after this disc."
        );
    }

    #[test]
    fn a_two_copy_burn_lists_both_discs_and_the_coverage() {
        let d = |set| DiscBurn {
            disc: DiscKey::new(1, set),
            label: format!("p disc 1{}", if set == 0 { 'A' } else { 'B' }),
            finalized: false,
            staging_dir: "/w/staging".into(),
            image_bytes: 1,
        };
        let r = BurnReport {
            discs: vec![d(0), d(1)],
            remaining: RemainingEstimate {
                more_discs: 0,
                more_files: 0,
                more_bytes: 0,
            },
            coverage: diskrement_core::Coverage {
                sets: vec![
                    SetCoverage::default(),
                    SetCoverage {
                        missing_files: 3,
                        missing_bytes: 2_000_000_000,
                    },
                ],
                no_live_copy: SetCoverage::default(),
            },
            deferred_deletions: None,
        };
        assert_eq!(
            burn_report(&r, 2),
            "✔ Wrote disc 1A  —  label: \"p disc 1A\"\n\
             Status: left OPEN — may receive more sessions later.\n\
             ✔ Wrote disc 1B  —  label: \"p disc 1B\"\n\
             Status: left OPEN — may receive more sessions later.\n\
             Copies: set B is missing 3 files (2.0 GB).\n\
             Copy sets are incomplete. Run: plan → burn"
        );
    }

    #[test]
    fn a_two_copy_burn_lists_lagging_sets_then_the_files_with_no_live_copy() {
        let gone = SetCoverage {
            missing_files: 1,
            missing_bytes: 4_000_000_000,
        };
        let r = BurnReport {
            discs: vec![DiscBurn {
                disc: DiscKey::new(2, 0),
                label: "p disc 2A".to_string(),
                finalized: false,
                staging_dir: "/w/staging".into(),
                image_bytes: 1,
            }],
            remaining: RemainingEstimate {
                more_discs: 0,
                more_files: 0,
                more_bytes: 0,
            },
            coverage: diskrement_core::Coverage {
                sets: vec![gone, gone],
                no_live_copy: gone,
            },
            deferred_deletions: None,
        };
        assert_eq!(
            burn_report(&r, 2),
            "✔ Wrote disc 2A  —  label: \"p disc 2A\"\n\
             Status: left OPEN — may receive more sessions later.\n\
             Copies: set A is missing 1 files (4.0 GB).\n\
             Copies: set B is missing 1 files (4.0 GB).\n\
             No live copy: 1 files (4.0 GB) — re-burn on next plan/burn."
        );
    }

    #[test]
    fn a_burn_with_no_coverage_rows_claims_no_copy_count() {
        let r = BurnReport {
            discs: vec![],
            remaining: RemainingEstimate {
                more_discs: 0,
                more_files: 0,
                more_bytes: 0,
            },
            coverage: diskrement_core::Coverage::default(),
            deferred_deletions: None,
        };
        let out = burn_report(&r, 2);
        assert!(!out.contains("Copies:"), "{out}");
    }

    #[test]
    fn an_aborted_second_disc_names_what_was_committed() {
        let err = PipelineError::PartialBurn {
            committed: vec!["1A".into()],
            unrecorded: None,
            pending: vec!["1B".into()],
            cause: Box::new(PipelineError::Burn(
                diskrement_shell::burn::BurnError::Aborted,
            )),
        };
        assert_eq!(
            error_line(&err),
            "Aborted. 1A committed; 1B not written (re-run burn)."
        );
        assert_eq!(err.exit_code(), 0);
    }
}

#[cfg(test)]
mod burn_render_tests {
    use super::*;
    use diskrement_shell::pipeline::reports::{BurnReport, DiscBurn, RemainingEstimate};

    fn report(finalized: bool, more_discs: usize) -> BurnReport {
        BurnReport {
            discs: vec![DiscBurn {
                disc: diskrement_core::DiscKey::new(1, 0),
                label: "p disc 1".to_string(),
                finalized,
                staging_dir: "/w/staging/disc-1".into(),
                image_bytes: 4096,
            }],
            remaining: RemainingEstimate {
                more_discs,
                more_files: 1,
                more_bytes: 800,
            },
            coverage: diskrement_core::Coverage::default(),
            deferred_deletions: None,
        }
    }

    /// `fixtures/run-manifest.txt` step 04, byte for byte.
    #[test]
    fn a_finalized_disc_with_more_to_come_matches_the_render_oracle() {
        assert_eq!(
            burn_report(&report(true, 1), 1),
            "✔ Wrote disc 1  —  label: \"p disc 1\"\n\
             Status: FINALIZED — set this disc aside, it is sealed.\n\
             ~1 more discs of changes remain. Run: plan → burn"
        );
    }

    #[test]
    fn a_single_copy_burn_with_no_live_copy_does_not_claim_up_to_date() {
        let gone = SetCoverage {
            missing_files: 1,
            missing_bytes: 4_000_000_000,
        };
        let mut r = report(false, 0);
        r.coverage = diskrement_core::Coverage {
            sets: vec![gone],
            no_live_copy: gone,
        };
        assert_eq!(
            burn_report(&r, 1),
            "✔ Wrote disc 1  —  label: \"p disc 1\"\n\
             Status: left OPEN — may receive more sessions later.\n\
             No live copy: 1 files (4.0 GB) — re-burn on next plan/burn."
        );

        // More discs to come: the line stays, and so does the remaining one.
        let mut r = report(false, 1);
        r.coverage = diskrement_core::Coverage {
            sets: vec![gone],
            no_live_copy: gone,
        };
        assert!(
            burn_report(&r, 1).ends_with(
                "No live copy: 1 files (4.0 GB) — re-burn on next plan/burn.\n\
                 ~1 more discs of changes remain. Run: plan → burn"
            ),
            "{}",
            burn_report(&r, 1)
        );
    }

    /// `fixtures/run-manifest.txt` step 19: the set is complete.
    #[test]
    fn an_open_disc_with_nothing_left_matches_the_render_oracle() {
        assert_eq!(
            burn_report(&report(false, 0), 1),
            "✔ Wrote disc 1  —  label: \"p disc 1\"\n\
             Status: left OPEN — may receive more sessions later.\n\
             No more changes — the set is up to date."
        );
    }

    /// The first line's spacing is easy to lose in a reformat and the oracle
    /// is byte-exact, so pin it on its own.
    #[test]
    fn the_wrote_line_uses_two_spaces_around_the_dash() {
        let line = burn_report(&report(false, 0), 1)
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
            disc: diskrement_core::DiscKey::new(ordinal, 0),
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
            version: 1,
            content_hash: ContentHash(hash.to_string()),
            size_bytes: size,
            disc: diskrement_core::DiscKey::new(disc, 0),
            session_ordinal: session,
            status,
            on_dead_disc: status == VersionStatus::Lost,
        }
    }

    /// Number single-copy entries v1, v2, … in order.
    pub(super) fn numbered(mut entries: Vec<FileHistoryEntry>) -> Vec<FileHistoryEntry> {
        for (i, e) in entries.iter_mut().enumerate() {
            e.version = i as u32 + 1;
        }
        entries
    }

    #[test]
    fn an_empty_verify_says_there_is_nothing_to_check() {
        assert_eq!(verify_report(&[], 1), "Nothing to verify: no burned discs.");
    }

    /// `fixtures/run-manifest.txt` step 05, byte for byte.
    #[test]
    fn a_clean_disc_matches_the_render_oracle() {
        assert_eq!(
            verify_report(&[report(1, 1, &[], false)], 1),
            "disc 1: 1 files OK ✓"
        );
    }

    /// `fixtures/run-manifest.txt` step 21, byte for byte — including the
    /// sealed disc's zero-file line and the absence of the dead disc 3.
    #[test]
    fn a_three_disc_sweep_matches_the_render_oracle() {
        assert_eq!(
            verify_report(
                &[
                    report(1, 1, &[], false),
                    report(2, 1, &[], false),
                    report(4, 0, &[], false),
                ],
                1
            ),
            "disc 1: 1 files OK ✓\n\
             disc 2: 1 files OK ✓\n\
             disc 4: 0 files OK ✓"
        );
    }

    #[test]
    fn a_skipped_disc_is_reported_as_not_checked() {
        assert_eq!(
            verify_report(&[report(3, 0, &[], true)], 1),
            "disc 3: skipped (not checked)"
        );
    }

    /// Transcribed from the staged Scala launcher on a corrupted staging tree.
    #[test]
    fn mismatches_are_listed_under_their_disc() {
        assert_eq!(
            verify_report(&[report(1, 2, &["a.bin"], false)], 1),
            "disc 1: 2 checked, 1 MISMATCH:\n  a.bin"
        );
        assert_eq!(
            verify_report(&[report(2, 5, &["bad/file.jpg", "other.raw"], false)], 1),
            "disc 2: 5 checked, 2 MISMATCH:\n  bad/file.jpg\n  other.raw"
        );
    }

    #[test]
    fn a_mixed_sweep_puts_one_disc_per_block_in_order() {
        assert_eq!(
            verify_report(
                &[
                    report(1, 1, &[], false),
                    report(2, 0, &[], true),
                    report(3, 2, &["gone.bin"], false),
                ],
                1
            ),
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
            history(&RelPath("nope.bin".to_string()), &[], 1),
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
                &numbered(vec![
                    entry(a1, 800, 1, 1, VersionStatus::Superseded),
                    entry(a2, 800, 3, 1, VersionStatus::Lost),
                    entry(a2, 800, 4, 1, VersionStatus::Current),
                ]),
                1
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
                &[entry(b1, 800, 2, 1, VersionStatus::Deleted)],
                1
            ),
            format!("b.bin\n\x20 v1  disc 2 / session 1   0.0 GB  {b1}  DELETED")
        );
    }

    /// At copies > 1 each version prints once, with its copies under it, twins
    /// together (#7). A copy on a dead disc reads LOST, or says it is on a dead
    /// disc where the status is still CURRENT.
    #[test]
    fn a_two_copy_history_lists_each_versions_copies_under_it() {
        let copy = |version, disc, set, status, dead| FileHistoryEntry {
            version,
            content_hash: ContentHash(if version == 1 { "h1" } else { "h2" }.to_string()),
            size_bytes: 2_000_000_000,
            disc: diskrement_core::DiscKey::new(disc, set),
            session_ordinal: 1,
            status,
            on_dead_disc: dead,
        };
        assert_eq!(
            history(
                &RelPath("a.bin".to_string()),
                &[
                    copy(1, 1, 0, VersionStatus::Superseded, false),
                    copy(1, 1, 1, VersionStatus::Lost, true),
                    copy(2, 2, 0, VersionStatus::Current, false),
                    copy(2, 2, 1, VersionStatus::Current, true),
                ],
                2
            ),
            "a.bin\n\
             \x20 v1  2.0 GB  h1\n\
             \x20     disc 1A / session 1   SUPERSEDED\n\
             \x20     disc 1B / session 1   LOST\n\
             \x20 v2  2.0 GB  h2\n\
             \x20     disc 2A / session 1   CURRENT\n\
             \x20     disc 2B / session 1   CURRENT (on a dead disc)"
        );
    }

    /// The annotation is not a copies>1 feature: at copies=1 a current
    /// version whose only copy died says so until it is re-burned.
    #[test]
    fn a_single_copy_current_version_on_a_dead_disc_says_so() {
        let mut gone = entry("h", 800, 3, 1, VersionStatus::Current);
        gone.on_dead_disc = true;
        assert_eq!(
            history(&RelPath("a.bin".to_string()), &[gone], 1),
            "a.bin\n\x20 v1  disc 3 / session 1   0.0 GB  h  CURRENT (on a dead disc)"
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
                )],
                1
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
            mark_dead(
                &LostReport {
                    disc: diskrement_core::DiscKey::new(3, 0),
                    degraded: vec![],
                    superseded_survivors: 0,
                    lost: vec![],
                    reburn: vec![reburn("a.bin", 800, a2)],
                },
                1
            ),
            "Disc 3 marked DEAD.\n\
             No versions permanently lost.\n\
             Re-burn on next plan/burn (1):\n\
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
            mark_dead(
                &LostReport {
                    disc: diskrement_core::DiscKey::new(1, 0),
                    degraded: vec![],
                    superseded_survivors: 0,
                    lost: vec![lost("b.bin", 800, b1, 1)],
                    reburn: vec![reburn("a.bin", 800, a1), reburn("b.bin", 800, b2)],
                },
                1
            ),
            format!(
                "Disc 1 marked DEAD.\n\
                 Permanently lost (1):\n\
                 \x20 b.bin  0.0 GB  {b1}  (was session 1)\n\
                 Re-burn on next plan/burn (2):\n\
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
            mark_dead(
                &LostReport {
                    disc: diskrement_core::DiscKey::new(2, 0),
                    degraded: vec![],
                    superseded_survivors: 0,
                    lost: vec![],
                    reburn: vec![],
                },
                1
            ),
            "Disc 2 marked DEAD.\n\
             No versions permanently lost.\n\
             No live files to re-burn."
        );
    }

    #[test]
    fn mark_dead_reports_degraded_files_and_surviving_superseded_versions() {
        let r = LostReport {
            disc: diskrement_core::DiscKey::new(1, 1),
            lost: vec![],
            reburn: vec![],
            degraded: vec![ReburnEntry {
                rel_path: RelPath("a.bin".into()),
                content_hash: ContentHash("aa".into()),
                size_bytes: 4,
            }],
            superseded_survivors: 1,
        };
        assert_eq!(
            mark_dead(&r, 2),
            "Disc 1B marked DEAD.\n\
             No versions permanently lost.\n\
             No live files to re-burn.\n\
             Short of a copy — restored on next plan/burn (1):\n  \
             a.bin\n\
             Superseded versions still held on another disc: 1"
        );
    }

    /// The GB column is the same one-decimal format every other renderer uses.
    #[test]
    fn the_lost_line_uses_the_shared_gb_format() {
        assert_eq!(
            mark_dead(
                &LostReport {
                    disc: diskrement_core::DiscKey::new(9, 0),
                    degraded: vec![],
                    superseded_survivors: 0,
                    lost: vec![lost("big/movie.mkv", 12_300_000_000, "a1b2c3", 4)],
                    reburn: vec![],
                },
                1
            ),
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
        ContentHash, DiscKey, DiscState, DiscStatus, DiscType, PlannedDisc, RelPath, ScannedFile,
    };
    use diskrement_shell::catalog::model::{
        FileHistoryEntry, LostReport, LostVersionEntry, ReburnEntry, VersionStatus,
    };
    use diskrement_shell::pipeline::reports::{
        BurnReport, DiscBurn, PlanReport, RemainingEstimate, ScanReport, SetPlan, StatusReport,
        VerifyReport,
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

    fn scan_of(a: usize, m: usize, d: usize, u: usize, hashed: usize, total: usize) -> ScanReport {
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
            copies: 1,
            sets: vec![SetPlan {
                disc,
                catch_up: 0,
                session_ordinal: 1,
                remaining: RemainingEstimate {
                    more_discs: remaining.0,
                    more_files: remaining.1,
                    more_bytes: remaining.2,
                },
            }],
            disc_type: DiscType::BdR,
            tombstones: tombstones
                .iter()
                .map(|t| RelPath((*t).to_string()))
                .collect(),
            oversized,
            unplaceable_catch_up: vec![],
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
            sources: Sources::Single(source.into()),
            work_dir: "/srv/work".to_string(),
            default_disc_type: DiscType::BdR,
            margin_percent: 2.0,
            copies: 1,
            created_at: ts(2026, 1, 1, 0, 0, 0),
        }
    }

    fn burn_of(ordinal: u32, label: &str, finalized: bool, more_discs: usize) -> BurnReport {
        BurnReport {
            discs: vec![DiscBurn {
                disc: DiscKey::new(ordinal, 0),
                label: label.to_string(),
                finalized,
                staging_dir: "/srv/work/staging/disc-1".into(),
                image_bytes: 4096,
            }],
            remaining: RemainingEstimate {
                more_discs,
                more_files: 1,
                more_bytes: 800,
            },
            coverage: diskrement_core::Coverage::default(),
            deferred_deletions: None,
        }
    }

    fn verify_of(ordinal: u32, checked: usize, mismatches: &[&str], skipped: bool) -> VerifyReport {
        VerifyReport {
            disc: diskrement_core::DiscKey::new(ordinal, 0),
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
            version: 1,
            content_hash: ContentHash(hash.to_string()),
            size_bytes: size,
            disc: diskrement_core::DiscKey::new(d, 0),
            session_ordinal: s,
            status,
            on_dead_disc: status == VersionStatus::Lost,
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
                ("a fresh scan", scan_report(&scan_of(2, 1, 0, 3, 3, 6))),
                ("nothing changed", scan_report(&scan_of(0, 0, 0, 6, 0, 6))),
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
            open_discs: vec![diskrement_core::DiscKey::new(2, 0)],
            coverage: diskrement_core::Coverage::default(),
            total_current_files: 556,
            last_burn_at: Some(ts(2026, 6, 21, 14, 3, 0)),
        };
        let fresh = StatusReport {
            discs: vec![],
            open_discs: vec![],
            coverage: diskrement_core::Coverage::default(),
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
            open_discs: vec![diskrement_core::DiscKey::new(2, 0)],
            coverage: diskrement_core::Coverage::default(),
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
                    burn_report(&burn_of(1, "p disc 1", true, 1), 1),
                ),
                (
                    "an open disc, set complete (run-manifest step 08)",
                    burn_report(&burn_of(2, "p disc 2", false, 0), 1),
                ),
            ]),
        );
    }

    #[test]
    fn verify_gallery() {
        check(
            "verify",
            gallery(&[
                ("nothing burned", verify_report(&[], 1)),
                (
                    "a clean sweep (run-manifest step 21)",
                    verify_report(
                        &[
                            verify_of(1, 1, &[], false),
                            verify_of(2, 1, &[], false),
                            verify_of(4, 0, &[], false),
                        ],
                        1,
                    ),
                ),
                (
                    "a skipped disc",
                    verify_report(&[verify_of(3, 0, &[], true)], 1),
                ),
                (
                    "mismatches",
                    verify_report(&[verify_of(2, 5, &["bad/file.jpg", "other.raw"], false)], 1),
                ),
                (
                    "a mixed sweep",
                    verify_report(
                        &[
                            verify_of(1, 1, &[], false),
                            verify_of(2, 0, &[], true),
                            verify_of(3, 2, &["gone.bin"], false),
                        ],
                        1,
                    ),
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
                (
                    "no history",
                    history(&RelPath("nope.bin".to_string()), &[], 1),
                ),
                (
                    "three versions, one lost (run-manifest step 16)",
                    history(
                        &RelPath("a.bin".to_string()),
                        &super::p6_render_tests::numbered(vec![
                            version(a1, 800, 1, 1, VersionStatus::Superseded),
                            version(a2, 800, 3, 1, VersionStatus::Lost),
                            version(a2, 800, 4, 1, VersionStatus::Current),
                        ]),
                        1,
                    ),
                ),
                (
                    "a deleted path (run-manifest step 22)",
                    history(
                        &RelPath("b.bin".to_string()),
                        &[version(b1, 800, 2, 1, VersionStatus::Deleted)],
                        1,
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
                        1,
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
                    mark_dead(
                        &LostReport {
                            disc: diskrement_core::DiscKey::new(2, 0),
                            degraded: vec![],
                            superseded_survivors: 0,
                            lost: vec![],
                            reburn: vec![],
                        },
                        1,
                    ),
                ),
                (
                    "one live file re-queued (run-manifest step 12)",
                    mark_dead(
                        &LostReport {
                            disc: diskrement_core::DiscKey::new(3, 0),
                            degraded: vec![],
                            superseded_survivors: 0,
                            lost: vec![],
                            reburn: vec![reburn_entry("a.bin", 800, a2)],
                        },
                        1,
                    ),
                ),
                (
                    "a superseded version gone forever",
                    mark_dead(
                        &LostReport {
                            disc: diskrement_core::DiscKey::new(1, 0),
                            degraded: vec![],
                            superseded_survivors: 0,
                            lost: vec![lost_entry("b.bin", 800, b1, 1)],
                            reburn: vec![],
                        },
                        1,
                    ),
                ),
                (
                    "both halves, with the GB column",
                    mark_dead(
                        &LostReport {
                            disc: diskrement_core::DiscKey::new(4, 0),
                            degraded: vec![],
                            superseded_survivors: 0,
                            lost: vec![
                                lost_entry("b.bin", 800, b1, 1),
                                lost_entry("big/movie.mkv", 12_300_000_000, "a1b2c3", 2),
                            ],
                            reburn: vec![
                                reburn_entry("a.bin", 800, a1),
                                reburn_entry("nested/c.bin", 100, "c0ffee"),
                            ],
                        },
                        1,
                    ),
                ),
            ]),
        );
    }
}
