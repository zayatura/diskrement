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
//!
//! A green run takes about 40 seconds. **A red one takes far longer** — see
//! `DEFAULT_MAX_SHRINK_ITERS` — so do not kill a sweep that has started
//! shrinking.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

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
///
/// Measured on a 12-core machine: 8 cases took 48s and 12 took 76s, against
/// the corpus suite's 282s that runs beside it — so 12 costs 27% of what is
/// already being spent, comfortably inside the "no more than about half"
/// budget. The extra four cases buy down the chance of a run that touches no
/// burn at all: at the generator's measured ~30% burn rate that is one run in
/// 17 at eight cases and one in 70 at twelve. The coverage line reports the
/// difference either way, so this is bought insurance rather than a fix.
const DEFAULT_CASES: u32 = 12;

/// Shrink attempts before proptest reports the smallest failure it found.
///
/// Bounded because each attempt is another three runs. Budget the wall clock
/// before you reach for Ctrl-C: at three worlds and a mid-length sequence an
/// attempt is a few seconds of process spawns, so a full 64-attempt shrink is
/// on the order of **ten minutes** before the verdict prints. That is the
/// harness working — a shrunk counterexample is the difference between a
/// twelve-step sequence and a two-step one — not a hang.
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

/// The generator, as proptest sees it: a disc size and a bag of opaque choice
/// triples.
///
/// The triples are `(selector, chooser, payload)` and mean nothing on their
/// own — `Sweep::ops` interprets them against a `Model` threaded through the
/// sequence, **on demand rather than cached**. That is what makes plain
/// `Vec`/`u8` shrinking sound here: proptest may drop or shrink any element,
/// and the survivors are re-interpreted against the state the shorter prefix
/// actually produces, so a shrunk sequence is still a legal sequence rather
/// than a replay of decisions taken against a state that no longer exists.
///
/// The length floor is 1, not 0: an empty sequence runs no command, so it can
/// never diverge and would only spend three scratch trees proving it.
///
/// `select` shrinks toward the first element of `sequence::DISC_BYTES`, which
/// is the 1000-byte disc — the smallest, and the one that spills a plan across
/// discs. A shrunk counterexample therefore lands on the most interesting
/// capacity rather than the roomiest.
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
    } else {
        // Say so rather than pass silently: this test's whole subject is a
        // green result that proves nothing.
        eprintln!(
            "note: PROPTEST_CASES is set, so the case-count assertion was not \
             exercised in this run"
        );
    }
    // The assertion above compares the constant with itself, so it pins the
    // wiring and not the magnitude. This pins the magnitude — in a `const`
    // block, so an edit that raises the default fails to compile rather than
    // waiting for someone to run the test it would fail.
    const {
        assert!(
            DEFAULT_CASES <= 16,
            "the default must stay something you run after a change"
        )
    };
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

/// What a run actually exercised, so a green line cannot mean "nothing ran".
///
/// Keyed on the first word of the step label — the command — and split by exit
/// status, because a `burn` that is refused every time is not burn coverage.
#[derive(Default)]
struct Coverage {
    /// command -> (issued, succeeded)
    commands: BTreeMap<String, (usize, usize)>,
}

impl Coverage {
    /// Fold in one command outcome. Taken from the *reference* world, which is
    /// the one the others are compared against.
    fn record(&mut self, step: &StepSnapshot) {
        let command = step
            .label
            .split_whitespace()
            .next()
            .unwrap_or("?")
            .to_string();
        let entry = self.commands.entry(command).or_insert((0, 0));
        entry.0 += 1;
        if step.exit == 0 {
            entry.1 += 1;
        }
    }

    fn merge(&mut self, other: &Coverage) {
        for (command, (issued, succeeded)) in &other.commands {
            let entry = self.commands.entry(command.clone()).or_insert((0, 0));
            entry.0 += issued;
            entry.1 += succeeded;
        }
    }

    fn succeeded(&self, command: &str) -> usize {
        self.commands.get(command).map_or(0, |(_, ok)| *ok)
    }

    /// `burn 4 (2 refused), disc 1, init 8, …` — the refusal clause only when
    /// there were refusals.
    fn render(&self) -> String {
        if self.commands.is_empty() {
            return "nothing — no command step was generated".to_string();
        }
        self.commands
            .iter()
            .map(|(command, (issued, succeeded))| {
                let refused = issued - succeeded;
                if refused == 0 {
                    format!("{command} {issued}")
                } else {
                    format!("{command} {issued} ({refused} refused)")
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// One scratch tree running the sequence under one mode.
///
/// Each world carries **its own `Normalizer`**: the three live in three
/// different scratch roots, and instant ordinals are assigned in
/// first-appearance order. All three are fed the same sequence of blobs, in the
/// same order, so their numbering corresponds. That is the load-bearing
/// property here — feed one world a blob the others never see and the ordinals
/// stop lining up.
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

/// Run one generated sequence through a reference all-Scala world and one world
/// per candidate mode, diffing after every command. `Err` carries the report for
/// the first divergence; `Ok` carries what the reference world exercised.
///
/// A `Skip` from inside here is a panic, not a skip: the environment was probed
/// before the sweep started, so a failure at this point is a harness defect.
///
/// Two callers, one body. The main comparison passes the two ported modes; the
/// determinism self-check passes a second `Mode::Scala`, which makes the
/// self-check exactly as strong as the thing it adjudicates. A weaker
/// self-check — end state only — would clear the port of a mid-run harness
/// defect whose evidence a later command erases, which is the one verdict that
/// must never be wrong.
fn compare_worlds(bins: &Binaries, sweep: &Sweep, candidates: &[Mode]) -> Result<Coverage, String> {
    let scenario = sweep.to_scenario();
    let open = |mode: Mode| {
        World::open(bins, scenario.disc_bytes, mode).unwrap_or_else(|Skip(reason)| {
            panic!(
                "could not open the {} world: {reason} — the environment was probed \
                 before the sweep started, so this is a harness defect, not a skip",
                mode.label()
            )
        })
    };
    let mut reference = open(Mode::Scala);
    let mut candidates: Vec<World> = candidates.iter().copied().map(open).collect();
    let mut coverage = Coverage::default();

    for (i, step) in scenario.steps.iter().enumerate() {
        for world in std::iter::once(&mut reference).chain(candidates.iter_mut()) {
            // Hoisted past the mutable borrow: a `delete` that fails in one
            // world and not the others is a completely different diagnosis.
            let label = world.mode().label();
            world.apply(step).unwrap_or_else(|Skip(reason)| {
                panic!(
                    "step {i} could not be applied in the {label} world: {reason} — the \
                     environment was probed before the sweep started, so this is a \
                     harness defect, not a skip"
                )
            });
        }
        if !matches!(step, Step::Cmd { .. }) {
            continue;
        }
        let reference_step = reference
            .last
            .clone()
            .expect("a command step always produces an outcome");
        coverage.record(&reference_step);
        // Taken once and reused for every candidate. Re-taking it per candidate
        // would not produce a false red — `Normalizer` looks each instant up by
        // its full text and only records one on first sight, so re-feeding the
        // same blob is idempotent — but it would repeat the SQLite dump and the
        // JSON walk once per candidate, which is a real slowdown for nothing.
        let reference_state = reference.state();
        for candidate in candidates.iter_mut() {
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
                    "step {i} ({}) diverges: the reference all-scala world is (a), \
                     the {} world is (b)\n\n{}",
                    reference_step.label,
                    candidate.mode().label(),
                    diffs.join("\n\n")
                ));
            }
        }
    }
    Ok(coverage)
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
    let coverage = RefCell::new(Coverage::default());
    let mut runner = TestRunner::new(cfg);
    let outcome = runner.run(&sweep_strategy(MAX_LEN), |sweep| {
        sequences.set(sequences.get() + 1);
        match compare_worlds(&bins, &sweep, &[Mode::Rust, Mode::Interleaved]) {
            Ok(exercised) => {
                coverage.borrow_mut().merge(&exercised);
                Ok(())
            }
            Err(report) => Err(TestCaseError::fail(report)),
        }
    });

    match outcome {
        Ok(()) => {
            let coverage = coverage.borrow();
            eprintln!(
                "sweep: {cases} case(s) of up to {MAX_LEN} steps agreed across all three \
                 modes ({} executed)\n  exercised: {}",
                sequences.get(),
                coverage.render()
            );
            if coverage.succeeded("burn") == 0 {
                // A warning and not an assertion: the generator reaches a burn
                // in well under every run at this case count, so asserting
                // would paint a correct run red by design.
                eprintln!(
                    "  WARNING: no case reached a successful burn. This run proved less \
                     than it looks like it did — re-run, or widen it with PROPTEST_CASES."
                );
            }
        }
        Err(TestError::Abort(reason)) => {
            panic!("the sweep aborted before it proved anything: {reason}")
        }
        Err(TestError::Fail(reason, sweep)) => {
            // The determinism guard, and the difference between a night spent
            // on a porting bug and a night spent on an unnormalized timestamp:
            // a second all-Scala world stepped alongside the reference and
            // diffed at every command, exactly as the failing comparison was.
            // If all-Scala cannot reproduce itself, the divergence just
            // reported says nothing about the Rust port.
            let verdict = match compare_worlds(&bins, &sweep, &[Mode::Scala]) {
                Ok(_) => "The all-scala run reproduces itself, so this is a porting bug.\n\
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

    // A run that generated nothing must not read as a proof. `PROPTEST_CASES=0`
    // is the way in, and `differential.rs` guards the same way.
    assert!(
        sequences.get() > 0,
        "no sequence ran; a green result here would be meaningless"
    );
}
