//! Output rendering (`Render.scala`). Success text goes to stdout; error text
//! is `PipelineError`'s `Display` and goes to stderr with its exit code.

use diskrement_shell::catalog::model::{Project, ProjectId};
use diskrement_shell::catalog::time;
use diskrement_shell::pipeline::reports::{ScanReport, StatusReport};

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

/// `f"${bytes / 1.0e9}%.1f GB"` (`Render.gb`).
#[allow(dead_code)]
pub fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1.0e9)
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
}
