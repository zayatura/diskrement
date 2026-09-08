//! Generated command sequences: the pool, the lifecycle model, and the value a
//! `proptest` strategy produces (spec §8.1, the `sequence` bullet).
//!
//! **The model steers; it is not the oracle.** Its transition table shapes the
//! distribution of generated sequences so that they reach burns, spills,
//! supersessions and coasters instead of dying on the first error. It is *not*
//! an assertion about the pipeline: if it calls a command legal that is not,
//! both binaries reject it identically and the case still passes. That is what
//! keeps it a hundred lines rather than a second implementation of
//! `pipeline::mod`, and it is why nobody should later "fix" it by making it
//! exact.
//!
//! The one exception is `Op::Delete`, which `Session::apply` performs with
//! `fs::remove_file` — that errors on a missing path and aborts the case. So
//! `live` must be exactly right, and a test pins it against a shadow of the
//! filesystem.
//!
//! Deliberately absent: anything that fails in the argument parser. Spec §3.2
//! lets clap and zio-cli word those differently, and the corpus covers them
//! with `Expect::ExitOnly` steps. Every generated step is `Expect::Full`, so
//! every generated step's stdout is compared.
//!
//! No `proptest` here: this module is plain values, unit-testable by ordinary
//! `cargo test`. The strategy that produces a `Sweep` lives in `tests/sweep.rs`.

use std::collections::{BTreeMap, BTreeSet};

use crate::runner::PROJECT;
use crate::scenario::{self, Scenario, Step};

/// The source-file names a generated sequence may touch.
///
/// A fixed pool rather than generated strings: every name has to be a legal
/// relative path everywhere the harness runs, and the interesting variation is
/// depth, not spelling. Four names is enough to spill a 1000-byte disc,
/// supersede a version and leave a tombstone while keeping the model small.
pub const FILES: &[&str] = &["a.bin", "b.bin", "nested/c.bin", "deep/x/d.bin"];

/// Payload sizes. All smaller than the smallest generated capacity, on
/// purpose: an oversized file is never committed, so it makes every later
/// `burn` exit 3 until something deletes it, and a sequence that writes one
/// early is dead for the rest of its length. The oversize path is worth
/// testing and is covered deliberately, by `corpus::plan_oversize` and
/// `corpus::burn_errors`, rather than by poisoning generated sequences.
pub const SIZES: &[usize] = &[120, 400, 800];

/// `DISKREMENT_DISC_BYTES` values, most constrained first.
///
/// Order matters: `prop::sample::select` shrinks toward the first element, and
/// a counterexample that still reproduces on a 1000-byte disc is the more
/// useful artifact — it is the one that spills, seals and fills.
pub const DISC_BYTES: &[Option<u64>] = &[Some(1000), Some(4000), None];

/// A project name no generated `init` ever creates.
pub const UNKNOWN_PROJECT: &str = "nosuch";

/// A path no generated `write` ever creates.
pub const UNKNOWN_PATH: &str = "nope.bin";

/// The sequence length Task 6's sweep generates. Lives here, not in the
/// sweep, so the reachability test below measures the real distribution
/// rather than a long-walk approximation of it, and so the sweep and this
/// module share one source instead of two numbers that can drift apart.
pub const MAX_LEN: usize = 12;

/// One generated step.
///
/// Command ops carry their project so that the same variant serves both the
/// legal form (`PROJECT`) and the illegal one (`UNKNOWN_PROJECT`). `Init` does
/// not: `init nosuch` would *succeed* and give the model a second project to
/// track, which is churn with nothing behind it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Write {
        rel_path: &'static str,
        version: u32,
        size: usize,
    },
    Delete {
        rel_path: &'static str,
    },
    Init,
    Scan {
        project: &'static str,
    },
    Status {
        project: &'static str,
    },
    Plan {
        project: &'static str,
        finalize: bool,
        disc_type: Option<&'static str>,
    },
    Burn {
        project: &'static str,
    },
    Verify {
        project: &'static str,
        /// `i64`, not `u32`: an ordinal the domain refuses is interesting, and
        /// keeping the width wide leaves room for it without reaching the
        /// values that fail in the argument parser instead (§3.2).
        disc: Option<i64>,
    },
    History {
        project: &'static str,
        path: &'static str,
    },
    Reset {
        project: &'static str,
    },
    MarkDead {
        project: &'static str,
        ordinal: u32,
    },
}

impl Op {
    /// The project this op names, if any. `None` for tree mutations and `Init`.
    pub fn project(&self) -> Option<&'static str> {
        match self {
            Op::Write { .. } | Op::Delete { .. } | Op::Init => None,
            Op::Scan { project }
            | Op::Status { project }
            | Op::Plan { project, .. }
            | Op::Burn { project }
            | Op::Verify { project, .. }
            | Op::History { project, .. }
            | Op::Reset { project }
            | Op::MarkDead { project, .. } => Some(*project),
        }
    }

    /// The argv this op invokes, with `{source}` and `{work}` left for the
    /// runner to substitute. Empty for a tree mutation.
    pub fn to_argv(&self) -> Vec<String> {
        let s = |x: &str| x.to_string();
        match self {
            Op::Write { .. } | Op::Delete { .. } => Vec::new(),
            Op::Init => vec![
                s("init"),
                s(PROJECT),
                s("--source"),
                s("{source}"),
                s("--work-dir"),
                s("{work}"),
            ],
            Op::Scan { project } => vec![s("scan"), s(project)],
            Op::Status { project } => vec![s("status"), s(project)],
            Op::Plan {
                project,
                finalize,
                disc_type,
            } => {
                let mut v = vec![s("plan"), s(project)];
                if *finalize {
                    v.push(s("--finalize"));
                }
                if let Some(t) = disc_type {
                    v.push(s("--disc-type"));
                    v.push(s(t));
                }
                v
            }
            Op::Burn { project } => vec![s("burn"), s(project), s("--yes")],
            Op::Verify { project, disc } => {
                let mut v = vec![s("verify"), s(project)];
                if let Some(n) = disc {
                    v.push(s("--disc"));
                    v.push(n.to_string());
                }
                v
            }
            Op::History { project, path } => vec![s("history"), s(project), s(path)],
            Op::Reset { project } => vec![s("reset"), s(project)],
            Op::MarkDead { project, ordinal } => vec![
                s("disc"),
                s("mark-dead"),
                s(project),
                ordinal.to_string(),
                s("--yes"),
            ],
        }
    }

    /// Lower to a scenario step, so the sweep runs through the same `Session`
    /// the corpus does. Every command step is `Expect::Full` — the generator
    /// stays out of the argument parser (§3.2), so there is nothing to exempt.
    pub fn to_step(&self) -> Step {
        match self {
            // Matching `&self` binds `rel_path` as `&&'static str`, but the
            // compiler's built-in reference coercion (not the generic
            // `Deref` trait) resolves it straight to the concrete inner
            // `&'static str` when it's passed where `write_version`/`delete`
            // expect one — no explicit `*rel_path` needed, and clippy's
            // `explicit_auto_deref` flags one if added. `*version`/`*size`
            // still need theirs: those fields are plain `u32`/`usize`, and
            // there is no implicit reference-to-value coercion for scalars.
            Op::Write {
                rel_path,
                version,
                size,
            } => scenario::write_version(rel_path, *version, *size),
            Op::Delete { rel_path } => scenario::delete(rel_path),
            _ => {
                let argv = self.to_argv();
                let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
                scenario::cmd(argv.join(" "), &refs)
            }
        }
    }

    /// One line of `corpus.rs`, comma included.
    pub fn to_source(&self) -> String {
        match self {
            Op::Write {
                rel_path,
                version: 0,
                size,
            } => format!("write({rel_path:?}, {size}),"),
            Op::Write {
                rel_path,
                version: 1,
                size,
            } => format!("modify({rel_path:?}, {size}),"),
            Op::Write {
                rel_path,
                version,
                size,
            } => format!("write_version({rel_path:?}, {version}, {size}),"),
            Op::Delete { rel_path } => format!("delete({rel_path:?}),"),
            _ => {
                let argv = self.to_argv();
                let args: Vec<String> = argv.iter().map(|a| format!("{a:?}")).collect();
                format!("cmd({:?}, &[{}]),", argv.join(" "), args.join(", "))
            }
        }
    }
}

/// One generated case: a disc capacity and a list of opaque choices.
///
/// The choices are `(selector, chooser, payload)` triples that `Model`
/// interprets in order (spec §8.1, decision 2). Interpretation happens on
/// demand rather than being stored, so a shrunk `Sweep` is re-interpreted
/// against the state its shorter prefix produces — which is what makes plain
/// `Vec`/`u8` shrinking sound here.
#[derive(Clone, PartialEq, Eq)]
pub struct Sweep {
    disc_bytes: Option<u64>,
    choices: Vec<(u8, u8, u8)>,
}

impl Sweep {
    pub fn new(disc_bytes: Option<u64>, choices: Vec<(u8, u8, u8)>) -> Self {
        Sweep {
            disc_bytes,
            choices,
        }
    }

    /// Interpret the choices against a model threaded through them.
    pub fn ops(&self) -> Vec<Op> {
        let mut model = Model::default();
        let mut ops = Vec::with_capacity(self.choices.len());
        for (selector, chooser, payload) in &self.choices {
            let op = model.interpret(*selector, *chooser, *payload);
            model.apply(&op);
            ops.push(op);
        }
        ops
    }

    /// Lower to an ordinary scenario, so the sweep and the corpus share one
    /// runner and one step vocabulary.
    pub fn to_scenario(&self) -> Scenario {
        Scenario {
            name: "sweep".to_string(),
            disc_bytes: self.disc_bytes,
            steps: self.ops().iter().map(Op::to_step).collect(),
        }
    }

    /// The sequence as `corpus.rs` source.
    ///
    /// This is the failure report: proptest prints `Debug` of the shrunk input,
    /// so promoting a counterexample to a named scenario is a copy.
    pub fn to_source(&self) -> String {
        let mut out = String::new();
        out.push_str("// generated by the P9 sweep — name it and paste it into corpus.rs\n");
        out.push_str("Scenario {\n");
        out.push_str("    name: \"NAME-ME\".into(),\n");
        out.push_str(&format!("    disc_bytes: {:?},\n", self.disc_bytes));
        out.push_str("    steps: vec![\n");
        for op in self.ops() {
            out.push_str(&format!("        {}\n", op.to_source()));
        }
        out.push_str("    ],\n}\n");
        out
    }
}

impl std::fmt::Debug for Sweep {
    /// Deliberately not derived: proptest prints this on failure, and the
    /// opaque `(u8, u8, u8)` triples would tell a reader nothing.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_source())
    }
}

/// What the generator believes the project looks like.
///
/// Read from `crates/diskrement-shell/src/pipeline/mod.rs`: `plan` reads the
/// staged scan without consuming it (`mod.rs:196`), `burn` requires a staged
/// scan, a staged plan and `work/snapshot` (`mod.rs:231,263`) and clears all
/// three on success (`mod.rs:330-332`), and `reset` clears the same three
/// (`mod.rs:521`).
#[derive(Clone, Debug, Default)]
pub struct Model {
    pub initialised: bool,
    pub staged_scan: bool,
    pub staged_plan: bool,
    /// `work/snapshot` exists. Created by `scan`, destroyed by `burn` and
    /// `reset`.
    pub snapshot: bool,
    /// Discs the model believes were burned; ordinals are `1..=burned_discs`.
    pub burned_discs: u32,
    pub dead: BTreeSet<u32>,
    /// Files currently in the source tree. **Exact**, see the module doc.
    pub live: BTreeSet<&'static str>,
    /// The highest version ever written for a path, remembered across a delete
    /// so a re-added file gets fresh content rather than repeating v0.
    pub versions: BTreeMap<&'static str, u32>,
    /// Paths the catalog has heard of, so `history` can name a real one.
    pub known_paths: BTreeSet<&'static str>,
}

impl Model {
    /// Ordinals of discs that exist and are not dead.
    fn live_discs(&self) -> Vec<u32> {
        (1..=self.burned_discs)
            .filter(|o| !self.dead.contains(o))
            .collect()
    }

    fn next_version(&self, rel_path: &'static str) -> u32 {
        self.versions.get(rel_path).map_or(0, |v| v + 1)
    }

    /// The op that moves the project's lifecycle forward from here.
    ///
    /// Index 0 of the command list, and the chooser is biased toward it,
    /// because a burn needs `init → scan → plan → burn` in order and a
    /// uniform chooser essentially never produces that inside a dozen steps.
    fn advancing_op(&self) -> Op {
        if !self.initialised {
            Op::Init
        } else if !self.staged_scan {
            Op::Scan { project: PROJECT }
        } else if !self.staged_plan {
            Op::Plan {
                project: PROJECT,
                finalize: false,
                disc_type: None,
            }
        } else if self.snapshot {
            Op::Burn { project: PROJECT }
        } else {
            // Unreachable: `staged_scan == snapshot` in every reachable state,
            // because `scan` sets both and `burn` and `reset` clear both. Kept
            // so the match is total rather than because the state occurs.
            Op::Scan { project: PROJECT }
        }
    }

    /// Command ops that should succeed, or at least be refused for a reason
    /// the model does not know about — everything except the tree mutations,
    /// which `tree_ops` generates separately so they cannot crowd this list
    /// out. **Never empty**: index 0 is always `advancing_op`, and `interpret`
    /// biases the chooser toward it.
    ///
    /// Ordered lifecycle-first — `Scan`, `Plan`, `Burn` before the
    /// inspection ops — with `Reset` last, so that shrinking a failing case
    /// strips noise (status, verify, history) before it strips the
    /// structural ops a reproduced sequence still needs.
    pub fn legal_commands(&self) -> Vec<Op> {
        let advancing = self.advancing_op();
        let mut ops = vec![advancing.clone()];
        let mut rest = Vec::new();
        if self.initialised {
            rest.push(Op::Scan { project: PROJECT });
            if self.staged_scan {
                rest.push(Op::Plan {
                    project: PROJECT,
                    finalize: false,
                    disc_type: None,
                });
                rest.push(Op::Plan {
                    project: PROJECT,
                    finalize: true,
                    disc_type: None,
                });
                rest.push(Op::Plan {
                    project: PROJECT,
                    finalize: false,
                    disc_type: Some("DVD"),
                });
            }
            if self.staged_plan && self.snapshot {
                rest.push(Op::Burn { project: PROJECT });
            }
            rest.push(Op::Status { project: PROJECT });
            rest.push(Op::Verify {
                project: PROJECT,
                disc: None,
            });
            for ordinal in self.live_discs() {
                rest.push(Op::Verify {
                    project: PROJECT,
                    disc: Some(i64::from(ordinal)),
                });
                rest.push(Op::MarkDead {
                    project: PROJECT,
                    ordinal,
                });
            }
            for path in self.known_paths.iter().copied() {
                rest.push(Op::History {
                    project: PROJECT,
                    path,
                });
            }
            rest.push(Op::Reset { project: PROJECT });
        }
        rest.retain(|o| *o != advancing);
        ops.extend(rest);
        ops
    }

    /// The four `Write`s (one per `FILES` entry, each superseding its current
    /// version) plus one `Delete` per file the model believes is live.
    /// **Never empty**: `FILES` always contributes at least one `Write`, so
    /// this list cannot be starved the way it used to starve `legal_commands`
    /// — see the module doc and `interpret`.
    pub fn tree_ops(&self, payload: u8) -> Vec<Op> {
        let mut ops = Vec::new();
        let size = SIZES[payload as usize % SIZES.len()];
        // `.copied()` throughout: iterating `&[&str]` or `&BTreeSet<&str>`
        // yields `&&'static str`, and the op fields are `&'static str`.
        for rel_path in FILES.iter().copied() {
            ops.push(Op::Write {
                rel_path,
                version: self.next_version(rel_path),
                size,
            });
        }
        for rel_path in self.live.iter().copied() {
            ops.push(Op::Delete { rel_path });
        }
        ops
    }

    /// Ops the model expects to be refused, or answered with a not-found
    /// sentence. **Never empty**: it opens with a seven-element literal
    /// (below) that this function only ever pushes onto, so it cannot become
    /// empty regardless of state.
    ///
    /// These are as valuable as the legal ones: the oracle is agreement, not
    /// success, and error wording plus the exit-code ladder are both contract
    /// (CLAUDE.md: 2 = state/lookup, 3 = environment/oversize). No entry here
    /// fails in the argument parser — see the module doc.
    ///
    /// Takes no payload: an illegal op has nothing to size.
    pub fn illegal_ops(&self) -> Vec<Op> {
        let mut ops = vec![
            Op::Status {
                project: UNKNOWN_PROJECT,
            },
            Op::Plan {
                project: UNKNOWN_PROJECT,
                finalize: false,
                disc_type: None,
            },
            Op::Burn {
                project: UNKNOWN_PROJECT,
            },
            Op::Reset {
                project: UNKNOWN_PROJECT,
            },
            Op::Verify {
                project: UNKNOWN_PROJECT,
                disc: None,
            },
            Op::History {
                project: UNKNOWN_PROJECT,
                path: UNKNOWN_PATH,
            },
            Op::MarkDead {
                project: UNKNOWN_PROJECT,
                ordinal: 1,
            },
        ];
        if self.initialised {
            // A second `init` on a project that exists.
            ops.push(Op::Init);
            // A path the catalog never heard of: exit 0 with a "no history"
            // sentence, which is still a full stdout comparison.
            ops.push(Op::History {
                project: PROJECT,
                path: UNKNOWN_PATH,
            });
            // Ordinals no disc has. `+ 9` rather than `+ 1` so that a burn the
            // model failed to predict cannot make these accidentally valid.
            ops.push(Op::Verify {
                project: PROJECT,
                disc: Some(i64::from(self.burned_discs) + 9),
            });
            ops.push(Op::MarkDead {
                project: PROJECT,
                ordinal: self.burned_discs + 9,
            });
            if !self.staged_scan {
                ops.push(Op::Plan {
                    project: PROJECT,
                    finalize: false,
                    disc_type: None,
                });
            }
            if !self.staged_plan {
                ops.push(Op::Burn { project: PROJECT });
            }
            for ordinal in self.dead.iter().copied() {
                // Naming a dead disc is an error; a sweep skips it silently.
                ops.push(Op::Verify {
                    project: PROJECT,
                    disc: Some(i64::from(ordinal)),
                });
                // Marking one dead twice is refused, and must append no second
                // event — which the state diff checks, not just the exit code.
                ops.push(Op::MarkDead {
                    project: PROJECT,
                    ordinal,
                });
            }
        }
        ops
    }

    /// Turn one generated triple into an op against the current state.
    ///
    /// `selector % 5 == 4` is the illegal fifth. The comparison is against 4
    /// rather than 0 deliberately: proptest shrinks bytes toward zero, and a
    /// shrunk sequence should be a *legal* one.
    pub fn interpret(&self, selector: u8, chooser: u8, payload: u8) -> Op {
        // One residue in five for deliberate illegal noise, one for tree
        // mutations, three for commands. Tree ops used to share the legal
        // list, where they outnumbered the command ops and starved the
        // lifecycle: measured, 0.8% of sequences at the sweep's length
        // reached a single burn and 0.001% reached two, so spills,
        // supersessions and coasters were unreachable in practice.
        let (ops, bias_to_first) = match selector % 5 {
            4 => (self.illegal_ops(), false),
            3 => (self.tree_ops(payload), false),
            _ => (self.legal_commands(), true),
        };
        debug_assert!(!ops.is_empty(), "no op is available in state {self:?}");
        // Half the chooser range goes to index 0 — the op that advances the
        // lifecycle. Shrinking is unaffected: chooser 0 still selects index 0
        // on every branch.
        let index = if bias_to_first && chooser < 128 {
            0
        } else {
            chooser as usize % ops.len()
        };
        ops[index].clone()
    }

    /// Advance the model past `op`.
    ///
    /// Only ops aimed at the real project can move it: one aimed at
    /// `UNKNOWN_PROJECT` fails at the project lookup before touching anything.
    /// Every guess here may be wrong without consequence except the `live` set
    /// — see the module doc.
    pub fn apply(&mut self, op: &Op) {
        // An op aimed at another project fails at the project lookup before
        // it touches anything, so it cannot move the model. Ops with no
        // project (`Write`, `Delete`, `Init`) fall through: `project()` is
        // `None` for them, and `None.is_some_and(..)` is `false`.
        if op.project().is_some_and(|p| p != PROJECT) {
            return;
        }
        match op {
            Op::Write {
                rel_path, version, ..
            } => {
                self.versions.insert(*rel_path, *version);
                self.live.insert(*rel_path);
            }
            Op::Delete { rel_path } => {
                self.live.remove(*rel_path);
            }
            Op::Init => self.initialised = true,
            Op::Scan { .. } => {
                self.staged_scan = true;
                self.snapshot = true;
            }
            Op::Plan { .. } if self.staged_scan => {
                // `plan` does not consume the staged scan (`mod.rs:196`), so
                // re-planning stays legal and idempotent.
                self.staged_plan = true;
            }
            Op::Burn { .. } if self.staged_plan && self.snapshot => {
                self.burned_discs += 1;
                self.staged_scan = false;
                self.staged_plan = false;
                self.snapshot = false;
                let live: Vec<&'static str> = self.live.iter().copied().collect();
                self.known_paths.extend(live);
            }
            Op::Reset { .. } => {
                self.staged_scan = false;
                self.staged_plan = false;
                self.snapshot = false;
            }
            Op::MarkDead { ordinal, .. } if *ordinal <= self.burned_discs => {
                self.dead.insert(*ordinal);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// A deterministic byte source. Not a quality generator — it only has to
    /// walk the model through many states reproducibly, so a failure here is
    /// re-runnable without a seed file.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> u8 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (self.0 >> 33) as u8
        }
    }

    /// Walk the model for `steps`, returning every op it emitted.
    fn walk(seed: u64, steps: usize) -> Vec<Op> {
        let mut lcg = Lcg(seed);
        let mut model = Model::default();
        let mut ops = Vec::with_capacity(steps);
        for _ in 0..steps {
            let op = model.interpret(lcg.next(), lcg.next(), lcg.next());
            model.apply(&op);
            ops.push(op);
        }
        ops
    }

    #[test]
    fn a_fresh_model_offers_only_init_as_a_command() {
        let model = Model::default();
        assert_eq!(
            model.legal_commands(),
            vec![Op::Init],
            "nothing else is legal before init"
        );
        let tree = model.tree_ops(0);
        assert!(
            tree.iter().all(|o| matches!(o, Op::Write { .. })),
            "no deletes before anything is written: {tree:?}"
        );
        assert_eq!(tree.len(), FILES.len());
    }

    #[test]
    fn shrinking_toward_zero_lands_on_the_core_cycle() {
        // proptest shrinks each byte toward 0, so (0, 0, 0) must be the
        // simplest useful op in every state along the happy path — and now
        // that index 0 is `advancing_op` rather than always `Scan`, it must
        // walk the *whole* cycle: init, scan, plan, burn, in order. A shrunk
        // counterexample is then a short, legal sequence that actually
        // reaches a burn.
        let mut model = Model::default();
        for expected in [
            Op::Init,
            Op::Scan { project: PROJECT },
            Op::Plan {
                project: PROJECT,
                finalize: false,
                disc_type: None,
            },
            Op::Burn { project: PROJECT },
        ] {
            let op = model.interpret(0, 0, 0);
            assert_eq!(op, expected);
            model.apply(&op);
        }
    }

    #[test]
    fn no_op_list_is_ever_empty() {
        // `interpret` indexes with `% len()`, so an empty list would panic.
        // `illegal_ops` opens with a seven-element literal and only ever
        // pushes onto it (see its doc), so that half cannot fail by
        // construction; this pins the two halves that can, at every state
        // the walk visits *and* at the final state, which the loop-then-apply
        // shape below would otherwise never check.
        let mut model = Model::default();
        let assert_nonempty = |model: &Model| {
            assert!(
                !model.legal_commands().is_empty(),
                "legal_commands empty at {model:?}"
            );
            assert!(!model.tree_ops(0).is_empty(), "tree_ops empty at {model:?}");
            assert!(
                !model.illegal_ops().is_empty(),
                "illegal_ops empty at {model:?}"
            );
        };
        for op in walk(1, 400) {
            assert_nonempty(&model);
            model.apply(&op);
        }
        assert_nonempty(&model);
    }

    #[test]
    fn interpretation_is_a_golden_sequence() {
        // A pure function equalling itself twice would not catch a swap from
        // `BTreeSet` to `HashSet`: within one process a `HashSet` iterates
        // identically run to run, and only cross-process reproducibility
        // would break. Pinning a literal output instead catches that, and
        // gives the file its only example of what generated output looks
        // like.
        assert_eq!(
            walk(7, 8),
            vec![
                Op::Status {
                    project: UNKNOWN_PROJECT
                },
                Op::Init,
                Op::Scan { project: PROJECT },
                Op::Write {
                    rel_path: "deep/x/d.bin",
                    version: 0,
                    size: 800
                },
                Op::Verify {
                    project: UNKNOWN_PROJECT,
                    disc: None
                },
                Op::Scan { project: PROJECT },
                Op::Reset {
                    project: UNKNOWN_PROJECT
                },
                Op::Plan {
                    project: PROJECT,
                    finalize: false,
                    disc_type: None
                },
            ]
        );
    }

    #[test]
    fn a_delete_is_only_ever_emitted_for_a_file_that_exists() {
        // The one place model correctness is load-bearing: `Session::apply`
        // deletes with `fs::remove_file`, which *errors* on a missing path and
        // aborts the case with a Skip. Everything else the model gets wrong is
        // harmless — both binaries reject it identically — but this would look
        // like a harness defect. Checked against a shadow of the filesystem
        // rather than against the model's own bookkeeping, so a bug in that
        // bookkeeping cannot hide here.
        for seed in [1u64, 2, 3, 11, 101] {
            let mut on_disk: BTreeSet<&'static str> = BTreeSet::new();
            for op in walk(seed, 500) {
                match op {
                    Op::Write { rel_path, .. } => {
                        on_disk.insert(rel_path);
                    }
                    Op::Delete { rel_path } => {
                        assert!(
                            on_disk.remove(rel_path),
                            "seed {seed}: the model asked to delete {rel_path}, \
                             which is not in the source tree"
                        );
                    }
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn illegal_noise_is_about_a_fifth_of_the_sequence() {
        // Classify each op by membership in the illegal list *at the state it
        // was emitted from*. That is independent of the selector arithmetic
        // this is checking, and the two lists are disjoint for any given
        // state — an op legal there is never also illegal there — so
        // membership is a sound classifier.
        //
        // Deliberately not measured via the unknown-project ops: their share
        // decays with walk length, because the illegal list grows two entries
        // per dead disc while the unknown-project entries stay at seven. This
        // measures the branch itself, so the expected value is the selector's
        // one-residue-in-five regardless of how long the walk is.
        let mut lcg = Lcg(23);
        let mut model = Model::default();
        let total = 2000usize;
        let mut illegal = 0usize;
        for _ in 0..total {
            let op = model.interpret(lcg.next(), lcg.next(), lcg.next());
            if model.illegal_ops().contains(&op) {
                illegal += 1;
            }
            model.apply(&op);
        }
        let ratio = illegal as f64 / total as f64;
        assert!(
            (0.12..0.30).contains(&ratio),
            "illegal ops are {ratio:.3} of the sequence; the selector reserves \
             one residue in five for them"
        );
    }

    #[test]
    fn generation_reaches_the_states_the_model_exists_for() {
        // The model exists to steer into burns, spills, supersessions and
        // coasters. Nothing else in this file would notice if it stopped —
        // the sweep would simply pass, having exercised nothing the corpus
        // does not already cover. Measured at the length the sweep actually
        // generates, so this is the real distribution and not a long-walk
        // approximation of it.
        //
        // Two honest limits. This measures the *model's* belief that a burn
        // happened, not the binaries' — a model whose burn precondition drifts
        // from `pipeline::mod` would satisfy this while the real sweep burned
        // nothing, and only the differential suite would notice. And the write
        // share is asserted alongside the reached states because the residue
        // split's real contribution is the op mix: without it the mix nearly
        // doubles in writes while the reached-state thresholds still scrape
        // through.
        const CASES: usize = 2000;
        let (mut burned, mut twice, mut dead, mut superseded) = (0, 0, 0, 0);
        let (mut writes, mut ops_total) = (0usize, 0usize);
        for seed in 0..CASES as u64 {
            let mut lcg = Lcg(seed);
            let mut model = Model::default();
            let len = 1 + (lcg.next() as usize % MAX_LEN);
            let mut burns_seen = 0;
            let mut superseded_here = false;
            for _ in 0..len {
                let op = model.interpret(lcg.next(), lcg.next(), lcg.next());
                ops_total += 1;
                if matches!(op, Op::Write { .. }) {
                    writes += 1;
                }
                let before = model.burned_discs;
                model.apply(&op);
                if model.burned_discs > before {
                    burns_seen += 1;
                }
                if burns_seen > 0 && matches!(op, Op::Write { version, .. } if version >= 1) {
                    superseded_here = true;
                }
            }
            if burns_seen >= 1 {
                burned += 1;
            }
            if burns_seen >= 2 {
                twice += 1;
            }
            if !model.dead.is_empty() {
                dead += 1;
            }
            if superseded_here {
                superseded += 1;
            }
        }
        let share = |n: usize| n as f64 / CASES as f64;
        assert!(
            share(burned) > 0.15,
            "burn reached in {:.3} of sequences",
            share(burned)
        );
        assert!(
            share(twice) > 0.01,
            "two burns reached in {:.3}",
            share(twice)
        );
        assert!(
            share(dead) > 0.01,
            "a dead disc reached in {:.3}",
            share(dead)
        );
        assert!(
            share(superseded) > 0.005,
            "a post-burn supersede reached in {:.3}",
            share(superseded)
        );
        // Guards the residue split specifically: folding the tree ops back
        // into the command list takes this from ~0.18 to ~0.32, while the
        // thresholds above would still pass.
        let write_share = writes as f64 / ops_total as f64;
        assert!(
            write_share < 0.25,
            "writes are {write_share:.3} of generated ops; tree mutations have \
             their own selector residue so they cannot crowd out the commands"
        );
    }

    /// Build a `Sweep` from a deterministic byte source.
    fn sweep(seed: u64, steps: usize, disc_bytes: Option<u64>) -> Sweep {
        let mut lcg = Lcg(seed);
        let choices = (0..steps)
            .map(|_| (lcg.next(), lcg.next(), lcg.next()))
            .collect();
        Sweep::new(disc_bytes, choices)
    }

    #[test]
    fn every_generated_command_is_one_the_rust_binary_implements() {
        // The port gate would otherwise skip generated scenarios silently, and
        // a fully-skipped sweep exits green. Pinned here rather than
        // rediscovered at 2am.
        let scenario = sweep(5, 300, Some(1000)).to_scenario();
        for command in scenario.commands() {
            assert!(
                crate::runner::RUST_IMPLEMENTED.contains(&command),
                "the generator emits `{command}`, which is not in RUST_IMPLEMENTED"
            );
        }
    }

    #[test]
    fn generation_reaches_the_whole_command_surface() {
        // A sweep that only ever emits `scan` would pass forever and prove
        // nothing. This asserts the generator actually gets there.
        let ops: Vec<Op> = [1u64, 2, 3, 5, 8]
            .iter()
            .flat_map(|s| walk(*s, 1500))
            .collect();
        let commands: BTreeSet<String> = ops
            .iter()
            .filter_map(|o| o.to_argv().first().cloned())
            .collect();
        for expected in [
            "burn", "disc", "history", "init", "plan", "reset", "scan", "status", "verify",
        ] {
            assert!(commands.contains(expected), "never generated `{expected}`");
        }
        assert!(
            ops.iter()
                .any(|o| matches!(o, Op::Plan { finalize: true, .. })),
            "never generated a finalizing plan"
        );
        assert!(
            ops.iter().any(|o| matches!(
                o,
                Op::Plan {
                    disc_type: Some(_),
                    ..
                }
            )),
            "never generated an explicit disc type"
        );
        assert!(
            ops.iter().any(|o| matches!(o, Op::Delete { .. })),
            "never generated a delete"
        );
        assert!(
            ops.iter()
                .any(|o| matches!(o, Op::Write { version, .. } if *version >= 2)),
            "never generated a third version of a file"
        );
    }

    #[test]
    fn the_debug_output_is_paste_ready_corpus_source() {
        // proptest prints Debug of the shrunk input, so this *is* the failure
        // report. It has to be the body of the scenario it gets promoted to.
        let s = sweep(9, 6, Some(1000));
        let text = format!("{s:?}");
        assert!(text.contains("Scenario {"), "{text}");
        assert!(text.contains("disc_bytes: Some(1000)"), "{text}");
        assert!(text.contains("steps: vec!["), "{text}");
        // One rendered line per choice. Filtered on the step indentation,
        // because `disc_bytes: Some(1000),` also ends in `),`.
        let rendered = text
            .lines()
            .filter(|l| l.starts_with("        ") && l.trim_end().ends_with("),"))
            .count();
        assert_eq!(rendered, 6, "{text}");
    }

    #[test]
    fn a_write_renders_as_the_constructor_matching_its_version() {
        assert_eq!(
            Op::Write {
                rel_path: "a.bin",
                version: 0,
                size: 800
            }
            .to_source(),
            r#"write("a.bin", 800),"#
        );
        assert_eq!(
            Op::Write {
                rel_path: "a.bin",
                version: 1,
                size: 800
            }
            .to_source(),
            r#"modify("a.bin", 800),"#
        );
        assert_eq!(
            Op::Write {
                rel_path: "a.bin",
                version: 4,
                size: 800
            }
            .to_source(),
            r#"write_version("a.bin", 4, 800),"#
        );
        assert_eq!(
            Op::Delete { rel_path: "a.bin" }.to_source(),
            r#"delete("a.bin"),"#
        );
        assert_eq!(
            Op::MarkDead {
                project: "p",
                ordinal: 3
            }
            .to_source(),
            r#"cmd("disc mark-dead p 3 --yes", &["disc", "mark-dead", "p", "3", "--yes"]),"#
        );
    }

    #[test]
    fn a_sweep_lowers_to_one_step_per_choice() {
        let s = sweep(4, 11, None);
        let scenario = s.to_scenario();
        assert_eq!(scenario.steps.len(), 11);
        assert_eq!(scenario.disc_bytes, None);
    }
}
