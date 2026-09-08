//! The randomized differential sweep (spec §8.1's `sequence` bullet, §9's P9
//! row).
//!
//! Generated command sequences, run through all three modes at once and diffed
//! after every command. The `#[ignore]`d entry point needs a staged Scala
//! launcher; the two tests above it do not, and guard the knobs.
//!
//! ```bash
//! sbt cli/stage
//! export DISKREMENT_SCALA_BIN="$PWD/cli/target/universal/stage/bin/diskrement"
//! export TMPDIR=/path/on/btrfs-or-xfs
//! cargo build -p diskrement-cli
//! cargo test -p diskrement-difftest --test sweep -- --ignored --nocapture
//! # the one-time wide run:
//! PROPTEST_CASES=500 cargo test -p diskrement-difftest --test sweep -- --ignored --nocapture
//! ```

use std::cell::Cell;

use diskrement_difftest::normalize::Normalizer;
use diskrement_difftest::runner::{self, Binaries, Session, Skip};
use diskrement_difftest::scenario::{Mode, Scenario, Step};
use diskrement_difftest::sequence::{self, Sweep};
use diskrement_difftest::state_diff::{self, StateSnapshot, StepSnapshot};
use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::{Config, TestCaseError, TestError, TestRunner};

/// Longest generated sequence. From `sequence`, so the reachability test in
/// that module measures the distribution this strategy actually produces.
use diskrement_difftest::sequence::MAX_LEN;

/// Cases per run when `PROPTEST_CASES` is unset.
///
/// Narrow on purpose. Each case is three whole runs, roughly 1.5 JVM starts per
/// generated command, so the default has to stay something you run after a
/// change rather than something you schedule. `PROPTEST_CASES=500` is the
/// one-time wide run (§9: "run wide once and narrow thereafter").
const DEFAULT_CASES: u32 = 8;

/// Shrink attempts before proptest reports the smallest failure it found.
/// Bounded because each attempt is another three runs.
const DEFAULT_MAX_SHRINK_ITERS: u32 = 64;

fn config() -> Config {
    let mut cfg = Config::default();
    // Only override what the environment did not ask for, so `PROPTEST_CASES`
    // and `PROPTEST_MAX_SHRINK_ITERS` still work.
    if std::env::var_os("PROPTEST_CASES").is_none() {
        cfg.cases = DEFAULT_CASES;
    }
    if std::env::var_os("PROPTEST_MAX_SHRINK_ITERS").is_none() {
        cfg.max_shrink_iters = DEFAULT_MAX_SHRINK_ITERS;
    }
    // No `.proptest-regressions`: this crate dies at P10, and the promotion
    // path for a counterexample is `corpus::all()`, which is reviewed and
    // committed (spec §8.1, decision "failure persistence is off").
    cfg.failure_persistence = None;
    cfg
}

fn sweep_strategy(max_len: usize) -> impl Strategy<Value = Sweep> {
    (
        prop::sample::select(sequence::DISC_BYTES.to_vec()),
        prop::collection::vec((any::<u8>(), any::<u8>(), any::<u8>()), 1..=max_len),
    )
        .prop_map(|(disc_bytes, choices)| Sweep::new(disc_bytes, choices))
}

#[test]
fn the_default_case_count_is_narrow_unless_widened() {
    // Guards against a later edit that restores proptest's default of 256 and
    // silently turns the differential suite into an hour-long run.
    if std::env::var_os("PROPTEST_CASES").is_none() {
        assert_eq!(config().cases, DEFAULT_CASES);
    }
    assert!(
        config().failure_persistence.is_none(),
        "a regressions file would outlive the crate it belongs to"
    );
}

#[test]
fn the_strategy_stays_within_its_bounds() {
    let mut runner = TestRunner::deterministic();
    for _ in 0..50 {
        let tree = sweep_strategy(MAX_LEN)
            .new_tree(&mut runner)
            .expect("the strategy produces a value");
        let scenario = tree.current().to_scenario();
        assert!(
            (1..=MAX_LEN).contains(&scenario.steps.len()),
            "{} steps",
            scenario.steps.len()
        );
        assert!(sequence::DISC_BYTES.contains(&scenario.disc_bytes));
    }
}

/// One scratch tree running the sequence under one mode.
///
/// Each world carries **its own `Normalizer`**: the three live in three
/// different scratch roots, and instant ordinals are assigned in
/// first-appearance order. All three are advanced at the same points, in the
/// same order, so their numbering corresponds.
struct World<'a> {
    session: Session<'a>,
    norm: Normalizer,
    /// The most recent command outcome. `None` before the first command.
    last: Option<StepSnapshot>,
}

impl<'a> World<'a> {
    fn open(bins: &'a Binaries, disc_bytes: Option<u64>, mode: Mode) -> Result<Self, Skip> {
        let session = Session::open(bins, mode, disc_bytes)?;
        let norm = Normalizer::new(&session.scratch().root);
        Ok(World {
            session,
            norm,
            last: None,
        })
    }

    fn mode(&self) -> Mode {
        self.session.mode()
    }

    fn apply(&mut self, step: &Step) -> Result<(), Skip> {
        // The session owns the mode and derives the binary itself, so the
        // read-before-increment ordering is not this caller's to get wrong.
        let outcome = self.session.apply(step)?;
        let snapshot = outcome.map(|o| state_diff::step_snapshot(&o, &mut self.norm));
        self.last = snapshot;
        Ok(())
    }

    /// Capture what is on disk. Safe here and only here: no child is running
    /// between two command invocations, so the SQLite `-wal` has been
    /// checkpointed away and `immutable=1` sees everything.
    fn state(&mut self) -> StateSnapshot {
        state_diff::state_snapshot(self.session.scratch(), &mut self.norm)
    }
}

/// Run one generated sequence through all three modes, diffing after every
/// command. `Err` carries the report for the first divergence.
///
/// A `Skip` from inside here is a panic, not a skip: the environment was probed
/// before the sweep started, so a failure at this point is a harness defect.
fn compare(bins: &Binaries, sweep: &Sweep) -> Result<(), String> {
    let scenario = sweep.to_scenario();
    // Three explicit bindings rather than a closure: `World` borrows `bins`,
    // and a closure returning a value with an inferred lifetime is the kind of
    // thing that compiles only after an argument with the borrow checker.
    let mut reference = World::open(bins, scenario.disc_bytes, Mode::Scala)
        .unwrap_or_else(|Skip(r)| panic!("could not open the all-scala world: {r}"));
    let mut rust = World::open(bins, scenario.disc_bytes, Mode::Rust)
        .unwrap_or_else(|Skip(r)| panic!("could not open the all-rust world: {r}"));
    let mut interleaved = World::open(bins, scenario.disc_bytes, Mode::Interleaved)
        .unwrap_or_else(|Skip(r)| panic!("could not open the interleaved world: {r}"));

    for (i, step) in scenario.steps.iter().enumerate() {
        for world in [&mut reference, &mut rust, &mut interleaved] {
            world
                .apply(step)
                .unwrap_or_else(|Skip(reason)| panic!("step {i} could not be applied: {reason}"));
        }
        if !matches!(step, Step::Cmd { .. }) {
            continue;
        }
        let reference_step = reference
            .last
            .clone()
            .expect("a command step always produces an outcome");
        let reference_state = reference.state();
        for candidate in [&mut rust, &mut interleaved] {
            let candidate_step = candidate
                .last
                .clone()
                .expect("a command step always produces an outcome");
            let mut diffs = Vec::new();
            state_diff::step_differences(i, &reference_step, &candidate_step, &mut diffs);
            let candidate_state = candidate.state();
            state_diff::state_differences(&reference_state, &candidate_state, &mut diffs);
            if !diffs.is_empty() {
                return Err(format!(
                    "step {i} ({}) diverges: all-scala is (a), {} is (b)\n\n{}",
                    reference_step.label,
                    candidate.mode().label(),
                    diffs.join("\n\n")
                ));
            }
        }
    }
    Ok(())
}

/// Run the sequence twice through the Scala binary alone and diff the two.
///
/// Only called on failure, and it is the difference between a night spent on a
/// porting bug and a night spent on an unnormalized timestamp: if all-Scala
/// does not reproduce itself, the divergence just reported says nothing about
/// the Rust port. This is the corpus bootstrap's guarantee, applied to a
/// sequence the corpus never saw.
fn scala_reproduces_itself(bins: &Binaries, sweep: &Sweep) -> Result<(), String> {
    let scenario = sweep.to_scenario();
    let once = |label: &str| -> Result<state_diff::Snapshot, String> {
        let run = runner::run(bins, &scenario, Mode::Scala)
            .map_err(|Skip(reason)| format!("the {label} reference run skipped: {reason}"))?;
        let mut norm = Normalizer::new(&run.scratch.root);
        Ok(state_diff::snapshot(&run, &mut norm))
    };
    let first = once("first")?;
    let second = once("second")?;
    let diffs = state_diff::differences(&first, &second);
    if diffs.is_empty() {
        Ok(())
    } else {
        Err(diffs.join("\n\n"))
    }
}

/// Generated sequences must agree across all three modes (spec §9, the P9 row).
#[test]
#[ignore = "needs DISKREMENT_SCALA_BIN; run with --ignored"]
fn generated_sequences_agree_across_all_three_modes() {
    let bins = match runner::binaries() {
        Ok(b) => b,
        Err(Skip(reason)) => {
            eprintln!("SKIP: {reason}");
            return;
        }
    };
    // Preflight an empty scenario rather than re-typing `runner`'s skip
    // message here. With no command steps the port gate is vacuous, so what
    // this actually checks is that the Rust binary exists — in `runner`'s
    // wording. The port gate itself is pinned at compile time by
    // `sequence`'s command-surface test, not re-checked per run.
    let probe = Scenario {
        name: "sweep-probe".to_string(),
        disc_bytes: None,
        steps: Vec::new(),
    };
    if let Err(Skip(reason)) = runner::preflight(&bins, &probe, Mode::Interleaved) {
        eprintln!("SKIP: {reason}");
        return;
    }
    // Probe the scratch environment once. A TMPDIR that cannot reflink is a
    // skip, not the same failure repeated `cases` times — and after this
    // probe, a Skip from inside a case is a harness defect and panics.
    if let Err(Skip(reason)) = Session::open(&bins, Mode::Scala, None) {
        eprintln!("SKIP: {reason}");
        return;
    }

    let cfg = config();
    let cases = cfg.cases;
    let sequences = Cell::new(0u32);
    let mut runner = TestRunner::new(cfg);
    let outcome = runner.run(&sweep_strategy(MAX_LEN), |sweep| {
        sequences.set(sequences.get() + 1);
        compare(&bins, &sweep).map_err(TestCaseError::fail)
    });

    match outcome {
        Ok(()) => eprintln!(
            "sweep: {cases} generated case(s) of up to {MAX_LEN} steps agreed across \
             all three modes ({} sequence(s) executed)",
            sequences.get()
        ),
        Err(TestError::Abort(reason)) => {
            panic!("the sweep aborted before it proved anything: {reason}")
        }
        Err(TestError::Fail(reason, sweep)) => {
            let verdict = match scala_reproduces_itself(&bins, &sweep) {
                Ok(()) => "The all-scala run reproduces itself, so this is a porting bug.\n\
                           Fix it, then promote the sequence below to a named scenario in \
                           corpus.rs."
                    .to_string(),
                Err(diffs) => format!(
                    "The all-scala run does NOT reproduce itself, so this is a harness \
                     defect — an unnormalized volatile value — and says nothing about the \
                     port:\n\n{diffs}"
                ),
            };
            panic!("{reason}\n\n{verdict}\n\n{sweep:?}");
        }
    }
}
