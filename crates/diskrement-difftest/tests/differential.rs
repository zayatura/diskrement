//! The differential entry points. All `#[ignore]`d: they need a staged Scala
//! launcher, so routine `cargo test` must not run them.
//!
//! ```bash
//! sbt cli/stage
//! export DISKREMENT_SCALA_BIN="$PWD/cli/target/universal/stage/bin/diskrement"
//! export TMPDIR=/path/on/btrfs-or-xfs
//! cargo build -p diskrement-cli
//! cargo test -p diskrement-difftest -- --ignored --nocapture
//! ```

use diskrement_difftest::corpus;
use diskrement_difftest::normalize::Normalizer;
use diskrement_difftest::runner::{self, Binaries, Skip};
use diskrement_difftest::scenario::{Mode, Scenario};
use diskrement_difftest::state_diff::{assert_equivalent, snapshot, Snapshot};

/// Resolve both binaries, or explain why this run cannot happen and return
/// `None`. A skip that says nothing is the failure mode spec §7 exists to
/// prevent.
fn binaries_or_skip() -> Option<Binaries> {
    match runner::binaries() {
        Ok(b) => Some(b),
        Err(Skip(reason)) => {
            eprintln!("SKIP: {reason}");
            None
        }
    }
}

/// Run one scenario in one mode and snapshot it. `None` means skipped, with the
/// reason printed.
fn run_and_snapshot(bins: &Binaries, scenario: &Scenario, mode: Mode) -> Option<Snapshot> {
    match runner::run(bins, scenario, mode) {
        Ok(outcome) => {
            let mut norm = Normalizer::new(&outcome.scratch.root);
            Some(snapshot(&outcome, &mut norm))
        }
        Err(Skip(reason)) => {
            eprintln!("SKIP [{} / {}]: {reason}", scenario.name, mode.label());
            None
        }
    }
}

/// The bootstrap. One scenario, twice through the Scala binary, must be
/// identical.
///
/// This proves the harness itself: the runner is deterministic, the normalizer
/// erases every value that legitimately varies between two runs, and the
/// snapshot captures a stable view of the end state. It needs no Rust
/// implementation at all, which is the point — running it first means every
/// later red is a porting bug rather than a harness bug.
#[test]
#[ignore = "needs DISKREMENT_SCALA_BIN; run with --ignored"]
fn the_harness_is_deterministic_against_itself() {
    let Some(bins) = binaries_or_skip() else {
        return;
    };
    let mut ran = 0;
    for scenario in corpus::all() {
        let (Some(a), Some(b)) = (
            run_and_snapshot(&bins, &scenario, Mode::Scala),
            run_and_snapshot(&bins, &scenario, Mode::Scala),
        ) else {
            continue;
        };
        assert_equivalent(
            &a,
            &b,
            &format!(
                "scenario '{}' is not reproducible through one binary — this is a \
                 harness defect (an unnormalized volatile value), not a porting bug",
                scenario.name
            ),
        );
        ran += 1;
    }
    assert!(
        ran > 0,
        "no scenario ran; a green result here would be meaningless"
    );
    eprintln!("bootstrap: {ran} scenario(s) reproduced identically");
}

/// Every scenario the Rust binary can run must match the Scala binary.
///
/// Forward parity: what the corpus under `fixtures/` also checks, but for any
/// scenario rather than the single harvested one.
#[test]
#[ignore = "needs DISKREMENT_SCALA_BIN; run with --ignored"]
fn rust_matches_scala() {
    compare_against_scala(Mode::Rust);
}

/// Alternating the binaries mid-scenario must land where all-Scala lands.
///
/// This is the only test of spec §3.1's actual claim — that a user switching
/// from the Scala binary to the Rust binary mid-project should not notice. It
/// subsumes the reverse direction: half these steps have Scala reading a
/// catalog, a staged plan and a disc tree that Rust wrote.
#[test]
#[ignore = "needs DISKREMENT_SCALA_BIN; run with --ignored"]
fn interleaved_matches_scala() {
    compare_against_scala(Mode::Interleaved);
}

fn compare_against_scala(mode: Mode) {
    let Some(bins) = binaries_or_skip() else {
        return;
    };
    let mut ran = 0;
    let mut skipped = 0;
    for scenario in corpus::all() {
        let Some(candidate) = run_and_snapshot(&bins, &scenario, mode) else {
            skipped += 1;
            continue;
        };
        let Some(reference) = run_and_snapshot(&bins, &scenario, Mode::Scala) else {
            skipped += 1;
            continue;
        };
        assert_equivalent(
            &reference,
            &candidate,
            &format!(
                "scenario '{}': all-scala (a) and {} (b) disagree",
                scenario.name,
                mode.label()
            ),
        );
        ran += 1;
    }
    // Deliberately not an assertion that `ran > 0`. Until P3's fs task appends
    // to RUST_IMPLEMENTED, every scenario legitimately skips, and failing here
    // would mean the harness cannot land before the commands it tests. The
    // printed counts are what keeps a fully-skipped run from reading as green.
    eprintln!(
        "{}: {ran} scenario(s) compared, {skipped} skipped",
        mode.label()
    );
}
