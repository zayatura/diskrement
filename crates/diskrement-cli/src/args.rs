//! The clap surface (`Commands.scala`). Wording of clap's usage and argument
//! errors may drift from zio-cli — the one sanctioned divergence (spec §3.2);
//! the difftest harness declares such steps `Expect::ExitOnly`.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use diskrement_core::DiscType;

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
    pub burner: Option<String>,
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
    /// Show discs, totals, and last burn.
    Status { name: String },
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
}
