//! The clap surface (`Commands.scala`). Wording of clap's usage and argument
//! errors may drift from zio-cli — the one sanctioned divergence (spec §3.2);
//! the difftest harness declares such steps `Expect::ExitOnly`.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use diskrement_core::DiscType;

/// The burn backend, as a flag. zio-cli's `Options.enumeration` rejected an
/// unknown value as an argument error and clap's `ValueEnum` does the same.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
#[value(rename_all = "lower")]
pub enum BurnerKind {
    Real,
    Simulated,
}

#[derive(Parser, Debug)]
#[command(
    name = "diskrement",
    version,
    about = "Archival backup to optical discs"
)]
pub struct Cli {
    #[arg(long, global = true)]
    pub catalog: Option<PathBuf>,
    #[arg(long, global = true)]
    pub data_dir: Option<PathBuf>,
    #[arg(long, global = true)]
    pub device: Option<String>,
    #[arg(long, global = true)]
    pub burner: Option<BurnerKind>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Create a project and run filesystem checks.
    Init {
        name: String,
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        work_dir: PathBuf,
        #[arg(long, default_value = "BD_R")]
        disc_type: DiscType,
        #[arg(long, default_value_t = 2.0)]
        margin: f64,
        #[arg(long, default_value_t = 1)]
        copies: u32,
    },
    /// Snapshot the source and stage a diff.
    Scan {
        name: String,
        #[arg(long, default_value_t = false)]
        verify: bool,
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Plan the next disc from the staged scan.
    Plan {
        name: String,
        /// Override the project's default disc type for this plan only.
        #[arg(long)]
        disc_type: Option<DiscType>,
        /// Seal the disc even when everything fit.
        #[arg(long, default_value_t = false)]
        finalize: bool,
    },
    /// Show discs, totals, and last burn.
    Status { name: String },
    /// Burn the staged plan (with confirmation).
    Burn {
        name: String,
        #[arg(long, short = 'y', default_value_t = false)]
        yes: bool,
    },
    /// Discard staged scan/plan state.
    Reset { name: String },
    /// Re-hash burned disc(s) against the catalog.
    ///
    /// `allow_negative_numbers` and the `i64` type are deliberate: zio-cli let
    /// `--disc -1` reach the domain, which answers with exit 2. Rejecting it
    /// here would be exit 1, and the exit ladder is contract (spec §3.2).
    #[command(allow_negative_numbers = true)]
    Verify {
        name: String,
        /// Check only this disc instead of sweeping every live disc.
        #[arg(long)]
        disc: Option<i64>,
    },
    /// Show the full version history of one file.
    History { name: String, rel_path: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use diskrement_core::DiscType;

    #[test]
    fn init_with_defaults() {
        let cli = Cli::try_parse_from([
            "diskrement",
            "init",
            "photos",
            "--source",
            "/srv/p",
            "--work-dir",
            "/srv/w",
        ])
        .unwrap();
        let Command::Init {
            name,
            source,
            work_dir,
            disc_type,
            margin,
            copies,
        } = cli.command
        else {
            panic!("not init")
        };
        assert_eq!(name, "photos");
        assert_eq!(source, PathBuf::from("/srv/p"));
        assert_eq!(work_dir, PathBuf::from("/srv/w"));
        assert_eq!(disc_type, DiscType::BdR);
        assert_eq!(margin, 2.0);
        assert_eq!(copies, 1);
    }

    #[test]
    fn init_with_explicit_disc_type_margin_copies() {
        let cli = Cli::try_parse_from([
            "diskrement",
            "init",
            "photos",
            "--source",
            "/s",
            "--work-dir",
            "/w",
            "--disc-type",
            "BD_R_DL",
            "--margin",
            "5.0",
            "--copies",
            "2",
        ])
        .unwrap();
        let Command::Init {
            name,
            source,
            work_dir,
            disc_type,
            margin,
            copies,
        } = cli.command
        else {
            panic!("not init")
        };
        let _ = (name, source, work_dir);
        assert_eq!(disc_type, DiscType::BdRDl);
        assert_eq!(margin, 5.0);
        assert_eq!(copies, 2);
    }

    #[test]
    fn scan_flags_default_off_and_parse_on() {
        let off = Cli::try_parse_from(["diskrement", "scan", "photos"]).unwrap();
        let Command::Scan {
            name: _,
            verify,
            force,
        } = off.command
        else {
            panic!("not scan")
        };
        assert!(!verify && !force);
        let on =
            Cli::try_parse_from(["diskrement", "scan", "photos", "--verify", "--force"]).unwrap();
        let Command::Scan { verify, force, .. } = on.command else {
            panic!("not scan")
        };
        assert!(verify && force);
    }

    #[test]
    fn status_takes_only_a_name() {
        let cli = Cli::try_parse_from(["diskrement", "status", "photos"]).unwrap();
        assert!(matches!(cli.command, Command::Status { .. }));
    }

    #[test]
    fn the_global_catalog_flag_is_captured_before_or_after_the_subcommand() {
        let before =
            Cli::try_parse_from(["diskrement", "--catalog", "/c/x.sqlite", "status", "p"]).unwrap();
        assert_eq!(before.catalog, Some(PathBuf::from("/c/x.sqlite")));
        let after =
            Cli::try_parse_from(["diskrement", "status", "p", "--catalog", "/c/x.sqlite"]).unwrap();
        assert_eq!(after.catalog, Some(PathBuf::from("/c/x.sqlite")));
    }

    #[test]
    fn an_unknown_disc_type_is_rejected() {
        assert!(Cli::try_parse_from([
            "diskrement",
            "init",
            "p",
            "--source",
            "/s",
            "--work-dir",
            "/w",
            "--disc-type",
            "ZIP"
        ])
        .is_err());
    }

    #[test]
    fn an_unknown_subcommand_is_rejected() {
        assert!(Cli::try_parse_from(["diskrement", "frobnicate", "p"]).is_err());
    }

    #[test]
    fn plan_defaults_to_no_disc_type_override_and_no_finalize() {
        let cli = Cli::try_parse_from(["diskrement", "plan", "photos"]).unwrap();
        let Command::Plan {
            name,
            disc_type,
            finalize,
        } = cli.command
        else {
            panic!("not plan")
        };
        assert_eq!(name, "photos");
        assert_eq!(disc_type, None);
        assert!(!finalize);
    }

    #[test]
    fn plan_takes_a_disc_type_override_and_the_finalize_flag() {
        let cli = Cli::try_parse_from([
            "diskrement",
            "plan",
            "photos",
            "--disc-type",
            "DVD_DL",
            "--finalize",
        ])
        .unwrap();
        let Command::Plan {
            disc_type,
            finalize,
            ..
        } = cli.command
        else {
            panic!("not plan")
        };
        assert_eq!(disc_type, Some(DiscType::DvdDl));
        assert!(finalize);
    }

    #[test]
    fn plan_rejects_an_unknown_disc_type() {
        assert!(Cli::try_parse_from(["diskrement", "plan", "p", "--disc-type", "ZIP"]).is_err());
    }

    #[test]
    fn plan_requires_a_project_name() {
        assert!(Cli::try_parse_from(["diskrement", "plan"]).is_err());
    }

    #[test]
    fn verify_takes_an_optional_disc_ordinal() {
        let sweep = Cli::try_parse_from(["diskrement", "verify", "p"]).unwrap();
        let Command::Verify { name, disc } = sweep.command else {
            panic!("not verify")
        };
        assert_eq!(name, "p");
        assert_eq!(disc, None);

        let one = Cli::try_parse_from(["diskrement", "verify", "p", "--disc", "3"]).unwrap();
        let Command::Verify { disc, .. } = one.command else {
            panic!("not verify")
        };
        assert_eq!(disc, Some(3));
    }

    /// zio-cli let a negative ordinal through to the domain, which answers it
    /// with exit 2. Rejecting it in the parser would be exit 1 — an exit-code
    /// divergence, which §3.2 does not sanction.
    #[test]
    fn verify_accepts_a_negative_disc_ordinal_and_leaves_it_to_the_domain() {
        let cli = Cli::try_parse_from(["diskrement", "verify", "p", "--disc", "-1"]).unwrap();
        let Command::Verify { disc, .. } = cli.command else {
            panic!("not verify")
        };
        assert_eq!(disc, Some(-1));
    }

    #[test]
    fn verify_rejects_a_non_numeric_disc_ordinal() {
        assert!(Cli::try_parse_from(["diskrement", "verify", "p", "--disc", "abc"]).is_err());
    }

    #[test]
    fn history_takes_a_project_and_a_relative_path() {
        let cli = Cli::try_parse_from(["diskrement", "history", "p", "nested/a.bin"]).unwrap();
        let Command::History { name, rel_path } = cli.command else {
            panic!("not history")
        };
        assert_eq!(name, "p");
        assert_eq!(rel_path, "nested/a.bin");
    }

    #[test]
    fn history_requires_both_arguments() {
        assert!(Cli::try_parse_from(["diskrement", "history", "p"]).is_err());
        assert!(Cli::try_parse_from(["diskrement", "history"]).is_err());
    }

    #[test]
    fn reset_takes_only_a_name() {
        let cli = Cli::try_parse_from(["diskrement", "reset", "p"]).unwrap();
        let Command::Reset { name } = cli.command else {
            panic!("not reset")
        };
        assert_eq!(name, "p");
        assert!(Cli::try_parse_from(["diskrement", "reset"]).is_err());
    }

    #[test]
    fn burn_yes_accepts_the_y_alias_exactly_as_the_long_form() {
        for argv in [
            vec!["diskrement", "burn", "p", "--yes"],
            vec!["diskrement", "burn", "p", "-y"],
        ] {
            let cli = Cli::try_parse_from(argv).unwrap();
            let Command::Burn { name, yes } = cli.command else {
                panic!("not burn")
            };
            assert_eq!(name, "p");
            assert!(yes);
        }
        assert!(!matches!(
            Cli::try_parse_from(["diskrement", "burn", "p"])
                .unwrap()
                .command,
            Command::Burn { yes: true, .. }
        ));
    }
}
