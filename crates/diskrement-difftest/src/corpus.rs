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
    vec![
        init_scan_status(),
        error_paths(),
        plan_basics(),
        plan_spill_and_seal(),
        plan_oversize(),
        plan_errors(),
        burn_basics(),
        burn_spill_cycles(),
        burn_errors(),
        verify_history_reset(),
        harvest_replay(),
    ]
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
            // A non-numeric `--disc` fails inside the argument parser, the one
            // place spec §3.2 lets clap and zio-cli word things differently.
            cmd_exit_only(
                "verify with a non-numeric disc",
                &["verify", "p", "--disc", "abc"],
            ),
            // `history` needs both arguments; missing one is likewise an
            // argument-parser failure.
            cmd_exit_only("history with no path", &["history", "p"]),
            // A domain lookup failure, not an argument failure: this one is
            // fully compared, because the exit-code ladder and the message are
            // both contract (CLAUDE.md: 2 = state/lookup).
            cmd("status on an uninitialised project", &["status", "nosuch"]),
        ],
    }
}

/// `plan` on a tree that fits one disc: the "completes the set" wording, the
/// `--finalize` seal, and the fact that re-planning is idempotent because it
/// does not consume the staged scan.
pub fn plan_basics() -> Scenario {
    Scenario {
        name: "plan-basics",
        disc_bytes: None,
        steps: vec![
            write("a.bin", 800),
            write("nested/b.bin", 120),
            cmd(
                "init p",
                &["init", "p", "--source", "{source}", "--work-dir", "{work}"],
            ),
            cmd("scan p", &["scan", "p"]),
            cmd("plan p", &["plan", "p"]),
            // Re-plan: same staged scan, same answer, plan.json overwritten.
            cmd("plan p again", &["plan", "p"]),
            cmd("plan p --finalize", &["plan", "p", "--finalize"]),
            // An explicit disc type changes the capacity the plan reports.
            cmd(
                "plan p --disc-type DVD",
                &["plan", "p", "--disc-type", "DVD"],
            ),
            cmd("status p", &["status", "p"]),
        ],
    }
}

/// The spill: two 800-byte files against a 1000-byte disc. This cycle's disc
/// is sealed and the remainder is reported — the shape the corpus's
/// `work/01-after-plan1` recorded.
pub fn plan_spill_and_seal() -> Scenario {
    Scenario {
        name: "plan-spill-and-seal",
        disc_bytes: Some(1000),
        steps: vec![
            write("a.bin", 800),
            write("b.bin", 800),
            cmd(
                "init p",
                &["init", "p", "--source", "{source}", "--work-dir", "{work}"],
            ),
            cmd("scan p", &["scan", "p"]),
            cmd("plan p", &["plan", "p"]),
            cmd("status p", &["status", "p"]),
        ],
    }
}

/// A file no disc can hold. `plan` must list it and still succeed; `burn`
/// refuses it, which is P5's half of the two-stage check.
pub fn plan_oversize() -> Scenario {
    Scenario {
        name: "plan-oversize",
        disc_bytes: Some(500),
        steps: vec![
            write("big.bin", 800),
            write("small.bin", 100),
            cmd(
                "init p",
                &["init", "p", "--source", "{source}", "--work-dir", "{work}"],
            ),
            cmd("scan p", &["scan", "p"]),
            cmd("plan p", &["plan", "p"]),
        ],
    }
}

/// `plan` before `scan`: a state error, not an argument error, so the message
/// and the exit code are both contract and both fully compared.
pub fn plan_errors() -> Scenario {
    Scenario {
        name: "plan-errors",
        disc_bytes: None,
        steps: vec![
            write("a.bin", 100),
            cmd(
                "init p",
                &["init", "p", "--source", "{source}", "--work-dir", "{work}"],
            ),
            cmd("plan before scan", &["plan", "p"]),
            // …and the same failure on an uninitialised project resolves to
            // the project lookup instead.
            cmd("plan on an unknown project", &["plan", "nosuch"]),
            // An unknown disc type fails in the argument parser, the one place
            // spec §3.2 lets clap and zio-cli word things differently.
            cmd_exit_only(
                "plan with a bogus disc type",
                &["plan", "p", "--disc-type", "ZIP"],
            ),
        ],
    }
}

/// One full cycle: scan, plan, burn, and the status that reports it. The
/// simplest proof that a Rust burn and a Scala burn leave the same catalog,
/// the same staged disc tree and the same embedded snapshot.
pub fn burn_basics() -> Scenario {
    Scenario {
        name: "burn-basics",
        disc_bytes: None,
        steps: vec![
            write("a.bin", 800),
            write("nested/b.bin", 120),
            cmd(
                "init p",
                &["init", "p", "--source", "{source}", "--work-dir", "{work}"],
            ),
            cmd("scan p", &["scan", "p"]),
            cmd("plan p", &["plan", "p"]),
            cmd("burn p --yes", &["burn", "p", "--yes"]),
            cmd("status p", &["status", "p"]),
            // Nothing changed, so this second cycle has nothing to plan: the
            // "Nothing to burn" answer must agree on both sides.
            cmd("scan p", &["scan", "p"]),
            cmd("plan p", &["plan", "p"]),
        ],
    }
}

/// The spill, burned: one disc per cycle, a second session's ordinal, a
/// superseded file and a tombstone. This is the shape that produced the
/// corpus's disc-1 through disc-3, minus the coaster (P7's `disc mark-dead`).
pub fn burn_spill_cycles() -> Scenario {
    Scenario {
        name: "burn-spill-cycles",
        disc_bytes: Some(1000),
        steps: vec![
            write("a.bin", 800),
            write("b.bin", 800),
            cmd(
                "init p",
                &["init", "p", "--source", "{source}", "--work-dir", "{work}"],
            ),
            // cycle 1: a.bin fills disc 1, which is sealed; b.bin spills.
            cmd("scan p", &["scan", "p"]),
            cmd("plan p", &["plan", "p"]),
            cmd("burn p --yes", &["burn", "p", "--yes"]),
            // cycle 1b: the remainder lands on disc 2, left OPEN.
            cmd("scan p", &["scan", "p"]),
            cmd("plan p", &["plan", "p"]),
            cmd("burn p --yes", &["burn", "p", "--yes"]),
            // cycle 2: supersede a, delete b — the modify+delete manifest.
            modify("a.bin", 800),
            delete("b.bin"),
            cmd("scan p", &["scan", "p"]),
            cmd("plan p", &["plan", "p"]),
            cmd("burn p --yes", &["burn", "p", "--yes"]),
            cmd("status p", &["status", "p"]),
            // Seal the open disc with a file-less session.
            cmd("scan p", &["scan", "p"]),
            cmd("plan p --finalize", &["plan", "p", "--finalize"]),
            cmd("burn p --yes", &["burn", "p", "--yes"]),
            cmd("status p", &["status", "p"]),
        ],
    }
}

/// `burn`'s refusals, compared by agreement. Each is a state or environment
/// error, so both the message and the exit code are contract.
pub fn burn_errors() -> Scenario {
    Scenario {
        name: "burn-errors",
        disc_bytes: Some(500),
        steps: vec![
            write("big.bin", 800),
            write("small.bin", 100),
            cmd(
                "init p",
                &["init", "p", "--source", "{source}", "--work-dir", "{work}"],
            ),
            // Nothing staged yet.
            cmd("burn before scan", &["burn", "p", "--yes"]),
            cmd("scan p", &["scan", "p"]),
            // Scanned but not planned.
            cmd("burn before plan", &["burn", "p", "--yes"]),
            // `plan` records the oversized file and succeeds; `burn` is the
            // half that refuses it (exit 3).
            cmd("plan p", &["plan", "p"]),
            cmd("burn with an oversized file", &["burn", "p", "--yes"]),
            cmd("burn on an unknown project", &["burn", "nosuch", "--yes"]),
        ],
    }
}

/// `verify`, `history` and `reset`, and the state each leaves behind.
///
/// The interesting assertions are not the renders alone — they are the catalog
/// rows underneath. A `verify` advances a session `BURNED -> VERIFIED`, and the
/// state diff compares session states, so a port that marked the wrong session
/// (or marked one it should have skipped) fails here even though both binaries
/// printed the same line.
pub fn verify_history_reset() -> Scenario {
    Scenario {
        name: "verify-history-reset",
        disc_bytes: Some(1000),
        steps: vec![
            write("a.bin", 800),
            write("b.bin", 800),
            cmd(
                "init p",
                &["init", "p", "--source", "{source}", "--work-dir", "{work}"],
            ),
            // Nothing burned: the sweep has nothing to check, and the path has
            // no history. Both are exit-0 sentences, not errors.
            cmd("verify before any burn", &["verify", "p"]),
            cmd("history before any burn", &["history", "p", "a.bin"]),
            // cycle 1: a.bin fills disc 1, which is sealed; b.bin spills.
            cmd("scan p", &["scan", "p"]),
            cmd("plan p", &["plan", "p"]),
            cmd("burn p --yes", &["burn", "p", "--yes"]),
            cmd("verify p", &["verify", "p"]),
            // The second sweep finds the session already VERIFIED. It must be
            // a clean no-op on both sides, not an illegal transition.
            cmd("verify p again", &["verify", "p"]),
            cmd("verify p --disc 1", &["verify", "p", "--disc", "1"]),
            // A named ordinal that does not exist is an error, not a silent
            // exit 0 — including the negative one zio-cli let through.
            cmd("verify p --disc 99", &["verify", "p", "--disc", "99"]),
            cmd("verify p --disc -1", &["verify", "p", "--disc", "-1"]),
            cmd("history p a.bin", &["history", "p", "a.bin"]),
            cmd("history p nope.bin", &["history", "p", "nope.bin"]),
            // cycle 1b: b.bin lands on disc 2, so history spans two discs and
            // the sweep has two discs to report.
            cmd("scan p", &["scan", "p"]),
            cmd("plan p", &["plan", "p"]),
            cmd("burn p --yes", &["burn", "p", "--yes"]),
            cmd("verify p", &["verify", "p"]),
            cmd("history p b.bin", &["history", "p", "b.bin"]),
            // cycle 2: supersede a, delete b — history then shows SUPERSEDED
            // and DELETED alongside CURRENT.
            modify("a.bin", 800),
            delete("b.bin"),
            cmd("scan p", &["scan", "p"]),
            cmd("plan p", &["plan", "p"]),
            cmd("burn p --yes", &["burn", "p", "--yes"]),
            cmd("history p a.bin", &["history", "p", "a.bin"]),
            cmd("history p b.bin", &["history", "p", "b.bin"]),
            cmd("verify p", &["verify", "p"]),
            cmd("status p", &["status", "p"]),
            // reset drops the staged state *and* the staging trees, so every
            // disc then skips and `plan` needs a fresh scan.
            cmd("scan p", &["scan", "p"]),
            cmd("reset p", &["reset", "p"]),
            cmd("verify after reset", &["verify", "p"]),
            cmd("plan after reset", &["plan", "p"]),
            cmd("reset p again", &["reset", "p"]),
            cmd("reset on an unknown project", &["reset", "nosuch"]),
            cmd("status p", &["status", "p"]),
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
    fn the_plan_scenarios_use_only_commands_p4_lands() {
        for s in [
            plan_basics(),
            plan_spill_and_seal(),
            plan_oversize(),
            plan_errors(),
        ] {
            for c in s.commands() {
                assert!(
                    ["init", "scan", "status", "plan"].contains(&c),
                    "scenario '{}' uses '{c}', which P4 does not land",
                    s.name
                );
            }
        }
    }

    #[test]
    fn the_burn_scenarios_use_only_commands_p5_lands() {
        for s in [burn_basics(), burn_spill_cycles(), burn_errors()] {
            for c in s.commands() {
                assert!(
                    ["init", "scan", "status", "plan", "burn"].contains(&c),
                    "scenario '{}' uses '{c}', which P5 does not land",
                    s.name
                );
            }
        }
    }

    /// The oversize scenario must actually reach the refusal: a 800-byte file
    /// against a 500-byte disc, with a 100-byte file that still plans.
    #[test]
    fn the_burn_error_scenario_stages_a_plan_that_burn_must_refuse() {
        assert_eq!(burn_errors().disc_bytes, Some(500));
    }

    #[test]
    fn the_spill_scenario_shrinks_the_disc_enough_to_spill() {
        // Two 800-byte files, a 1000-byte disc: without the override they
        // would share one disc and the scenario would test nothing.
        assert_eq!(plan_spill_and_seal().disc_bytes, Some(1000));
    }

    #[test]
    fn the_oversize_scenario_puts_one_file_beyond_any_disc() {
        // 800 bytes against a 500-byte disc, with a 100-byte file that still
        // fits: the plan must list the first and place the second.
        assert_eq!(plan_oversize().disc_bytes, Some(500));
    }

    #[test]
    fn the_replay_spills_across_discs() {
        // 1000-byte discs and two 800-byte files: the spill that produces the
        // corpus's disc-1/disc-2 split. Without the override the files would
        // fit one disc and the scenario would test nothing interesting.
        assert_eq!(harvest_replay().disc_bytes, Some(1000));
    }
}
