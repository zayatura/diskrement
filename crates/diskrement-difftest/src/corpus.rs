//! The named scenarios.
//!
//! A scenario never hard-codes an expected exit code or line of output: both
//! binaries run the same step and the assertion is that they *agree*. That is
//! what makes an error-path scenario safe to write before the error is ported —
//! until then only the all-Scala mode runs it, and it passes.
//!
//! Later phases append scenarios here as they unlock commands and learn error
//! triggers from the Scala source.

use crate::scenario::{cmd, cmd_exit_only, delete, modify, write, Scenario};

/// Every scenario, in the order the suites run them.
pub fn all() -> Vec<Scenario> {
    vec![init_scan_status(), error_paths(), harvest_replay()]
}

/// The smallest useful scenario, and the first one the Rust binary can run:
/// P3 lands `init`, `scan` and `status`.
pub fn init_scan_status() -> Scenario {
    Scenario {
        name: "init-scan-status",
        disc_bytes: None,
        steps: vec![
            write("a.bin", 800),
            write("nested/b.bin", 120),
            cmd(
                "init p",
                &["init", "p", "--source", "{source}", "--work-dir", "{work}"],
            ),
            cmd("scan p", &["scan", "p"]),
            cmd("status p", &["status", "p"]),
            // A second scan with no change: exercises the fingerprint cache's
            // hit path, where a wrong mtime or size comparison would show up as
            // a different "Scanned:" line.
            cmd("scan p again", &["scan", "p"]),
            cmd("status p again", &["status", "p"]),
        ],
    }
}

/// Failure modes, compared by agreement rather than by expectation.
pub fn error_paths() -> Scenario {
    Scenario {
        name: "error-paths",
        disc_bytes: None,
        steps: vec![
            // Missing the required project argument: this fails inside the
            // argument parser, the one place spec §3.2 lets clap and zio-cli
            // word things differently. Exit code only.
            cmd_exit_only("scan with no project", &["scan"]),
            // An unknown subcommand, likewise an argument-parser failure.
            cmd_exit_only("bogus subcommand", &["frobnicate", "p"]),
            // A domain lookup failure, not an argument failure: this one is
            // fully compared, because the exit-code ladder and the message are
            // both contract (CLAUDE.md: 2 = state/lookup).
            cmd("status on an uninitialised project", &["status", "nosuch"]),
        ],
    }
}

/// The P0 harvest flow, replayed live: spill across two discs, supersede and
/// delete, a dead disc, a recovery burn, then `--finalize` to seal.
///
/// It mirrors `scripts/harvest-fixtures.sh` step for step on purpose. Running
/// the same flow through both oracles means a disagreement between the live
/// harness and the committed corpus is itself a signal, rather than something to
/// explain away as "different scenarios".
pub fn harvest_replay() -> Scenario {
    Scenario {
        name: "harvest-replay",
        disc_bytes: Some(1000),
        steps: vec![
            write("a.bin", 800),
            write("b.bin", 800),
            cmd(
                "init p",
                &["init", "p", "--source", "{source}", "--work-dir", "{work}"],
            ),
            // cycle 1: the plan spills across two discs; one disc per burn.
            cmd("scan p", &["scan", "p"]),
            cmd("plan p", &["plan", "p"]),
            cmd("burn p --yes", &["burn", "p", "--yes"]),
            cmd("verify p", &["verify", "p"]),
            // cycle 1b: the spill's remainder lands b@1 on disc 2, left OPEN.
            cmd("scan p", &["scan", "p"]),
            cmd("plan p", &["plan", "p"]),
            cmd("burn p --yes", &["burn", "p", "--yes"]),
            // cycle 2: supersede a, delete b.
            modify("a.bin", 800),
            delete("b.bin"),
            cmd("scan p", &["scan", "p"]),
            cmd("plan p", &["plan", "p"]),
            cmd("burn p --yes", &["burn", "p", "--yes"]),
            // the coaster: disc 3 dies, and a@2's only copy dies with it.
            cmd(
                "disc mark-dead p 3 --yes",
                &["disc", "mark-dead", "p", "3", "--yes"],
            ),
            cmd("scan p", &["scan", "p"]),
            cmd("plan p", &["plan", "p"]),
            cmd("burn p --yes", &["burn", "p", "--yes"]),
            // LOST is per physical copy: a@2 reads LOST on disc 3 and CURRENT
            // on disc 4, sharing one content hash.
            cmd("history p a.bin", &["history", "p", "a.bin"]),
            // seal the non-full disc. Every plan consumes the staged scan, so
            // --finalize needs a fresh one first.
            cmd("scan p", &["scan", "p"]),
            cmd("plan p --finalize", &["plan", "p", "--finalize"]),
            cmd("burn p --yes", &["burn", "p", "--yes"]),
            cmd("status p", &["status", "p"]),
            cmd("verify p", &["verify", "p"]),
            cmd("history p b.bin", &["history", "p", "b.bin"]),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scenario_has_a_unique_name() {
        let mut names: Vec<&str> = all().iter().map(|s| s.name).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "scenario names must be unique");
    }

    #[test]
    fn the_minimal_scenario_uses_only_the_three_commands_p3_lands() {
        let s = init_scan_status();
        let got: Vec<&str> = s.commands().into_iter().collect();
        assert_eq!(got, vec!["init", "scan", "status"]);
    }

    #[test]
    fn the_replay_covers_every_command_the_harvest_scenario_did() {
        let replay = harvest_replay();
        let got = replay.commands();
        for expected in [
            "burn", "disc", "history", "init", "plan", "scan", "status", "verify",
        ] {
            assert!(got.contains(expected), "replay is missing '{expected}'");
        }
    }

    #[test]
    fn the_replay_spills_across_discs() {
        // 1000-byte discs and two 800-byte files: the spill that produces the
        // corpus's disc-1/disc-2 split. Without the override the files would
        // fit one disc and the scenario would test nothing interesting.
        assert_eq!(harvest_replay().disc_bytes, Some(1000));
    }
}
