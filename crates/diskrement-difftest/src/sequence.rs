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

/// The source-file names a generated sequence may touch.
///
/// A fixed pool rather than generated strings: every name has to be a legal
/// relative path everywhere the harness runs, and the interesting variation is
/// depth, not spelling. Four names is enough to spill a 1000-byte disc,
/// supersede a version and leave a tombstone while keeping the model small.
pub const FILES: &[&str] = &["a.bin", "b.bin", "nested/c.bin", "deep/x/d.bin"];

/// Payload sizes. `3000` exceeds the smallest generated capacity on its own, so
/// the oversize path — `plan` lists it and succeeds, `burn` refuses with exit
/// 3 — is reachable by generation rather than only by hand.
pub const SIZES: &[usize] = &[120, 400, 800, 3000];

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

    /// Ops that should succeed, or at least be refused for a reason the model
    /// does not know about. **Never empty**, and ordered so that index 0 is the
    /// simplest useful op in this state — proptest shrinks the chooser byte
    /// toward zero.
    pub fn legal_ops(&self, payload: u8) -> Vec<Op> {
        let mut ops = Vec::new();
        if !self.initialised {
            ops.push(Op::Init);
        } else {
            ops.push(Op::Scan { project: PROJECT });
            if self.staged_scan {
                ops.push(Op::Plan {
                    project: PROJECT,
                    finalize: false,
                    disc_type: None,
                });
                ops.push(Op::Plan {
                    project: PROJECT,
                    finalize: true,
                    disc_type: None,
                });
                ops.push(Op::Plan {
                    project: PROJECT,
                    finalize: false,
                    disc_type: Some("DVD"),
                });
            }
            if self.staged_plan && self.snapshot {
                ops.push(Op::Burn { project: PROJECT });
            }
            ops.push(Op::Status { project: PROJECT });
            ops.push(Op::Verify {
                project: PROJECT,
                disc: None,
            });
            for ordinal in self.live_discs() {
                ops.push(Op::Verify {
                    project: PROJECT,
                    disc: Some(i64::from(ordinal)),
                });
                ops.push(Op::MarkDead {
                    project: PROJECT,
                    ordinal,
                });
            }
            for path in self.known_paths.iter().copied() {
                ops.push(Op::History {
                    project: PROJECT,
                    path,
                });
            }
            ops.push(Op::Reset { project: PROJECT });
        }
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

    /// Ops the model expects to be refused. **Never empty.**
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
        let ops = if selector % 5 == 4 {
            self.illegal_ops()
        } else {
            self.legal_ops(payload)
        };
        debug_assert!(!ops.is_empty(), "no op is available in state {self:?}");
        ops[chooser as usize % ops.len()].clone()
    }

    /// Advance the model past `op`.
    ///
    /// Only ops aimed at the real project can move it: one aimed at
    /// `UNKNOWN_PROJECT` fails at the project lookup before touching anything.
    /// Every guess here may be wrong without consequence except the `live` set
    /// — see the module doc.
    pub fn apply(&mut self, op: &Op) {
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
            Op::Scan { project } if *project == PROJECT => {
                self.staged_scan = true;
                self.snapshot = true;
            }
            Op::Plan { project, .. } if *project == PROJECT && self.staged_scan => {
                // `plan` does not consume the staged scan (`mod.rs:196`), so
                // re-planning stays legal and idempotent.
                self.staged_plan = true;
            }
            Op::Burn { project } if *project == PROJECT && self.staged_plan && self.snapshot => {
                self.burned_discs += 1;
                self.staged_scan = false;
                self.staged_plan = false;
                self.snapshot = false;
                let live: Vec<&'static str> = self.live.iter().copied().collect();
                self.known_paths.extend(live);
            }
            Op::Reset { project } if *project == PROJECT => {
                self.staged_scan = false;
                self.staged_plan = false;
                self.snapshot = false;
            }
            Op::MarkDead { project, ordinal }
                if *project == PROJECT && *ordinal <= self.burned_discs =>
            {
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
    fn a_fresh_model_offers_only_init_and_writes() {
        let model = Model::default();
        let legal = model.legal_ops(0);
        assert_eq!(legal[0], Op::Init, "shrinking lands on the first entry");
        assert!(
            legal
                .iter()
                .all(|o| matches!(o, Op::Init | Op::Write { .. })),
            "nothing else is legal before init: {legal:?}"
        );
    }

    #[test]
    fn shrinking_toward_zero_lands_on_the_core_cycle() {
        // proptest shrinks each byte toward 0, so (0, 0, 0) must be the
        // simplest useful op in every state: `init` before initialisation and
        // `scan` after it. A shrunk counterexample is then a short, legal,
        // early-command sequence.
        let mut model = Model::default();
        assert_eq!(model.interpret(0, 0, 0), Op::Init);
        model.apply(&Op::Init);
        assert_eq!(model.interpret(0, 0, 0), Op::Scan { project: PROJECT });
    }

    #[test]
    fn neither_op_list_is_ever_empty() {
        // `interpret` indexes with `% len()`, so an empty list would panic.
        let mut model = Model::default();
        for op in walk(1, 400) {
            assert!(
                !model.legal_ops(0).is_empty(),
                "legal list empty at {model:?}"
            );
            assert!(
                !model.illegal_ops().is_empty(),
                "illegal list empty at {model:?}"
            );
            model.apply(&op);
        }
    }

    #[test]
    fn interpretation_is_deterministic() {
        assert_eq!(walk(7, 200), walk(7, 200));
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
    fn illegal_noise_appears_without_dominating() {
        // A fifth of the ops come from the illegal list, and unknown-project
        // ops are about half of that list — so the measured share here is
        // roughly a tenth. It is a distribution property, not a contract; the
        // bound is wide enough to fail only if the selector arithmetic broke.
        let ops = walk(23, 2000);
        let illegal = ops
            .iter()
            .filter(|o| o.project().is_some_and(|p| p == UNKNOWN_PROJECT))
            .count();
        let ratio = illegal as f64 / ops.len() as f64;
        assert!(
            (0.05..0.25).contains(&ratio),
            "unknown-project ops are {ratio:.3} of the sequence"
        );
    }
}
