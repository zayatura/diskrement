//! Scenario definitions: pure data, no I/O.
//!
//! A scenario is a reproducible sequence of source-tree mutations and command
//! invocations. `runner` executes one; `state_diff` compares two executions.

use std::collections::BTreeSet;

/// Which implementation runs a step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    Scala,
    Rust,
}

impl Which {
    pub fn label(self) -> &'static str {
        match self {
            Which::Scala => "scala",
            Which::Rust => "rust",
        }
    }
}

/// How a scenario's command steps are distributed across the two binaries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Every step through the Scala binary. The bootstrap: run twice and the
    /// two runs must be identical, which is how the harness proves itself
    /// before the Rust binary implements anything.
    Scala,
    /// Every step through the Rust binary. Forward parity.
    Rust,
    /// Alternating. This is the mode that tests spec §3.1's actual claim —
    /// that a user switching binaries mid-project should not notice.
    Interleaved,
}

impl Mode {
    /// Which binary runs the `n`th *command* step (tree mutations are not
    /// counted, so inserting a file write never shifts the alternation).
    pub fn which(self, command_index: usize) -> Which {
        match self {
            Mode::Scala => Which::Scala,
            Mode::Rust => Which::Rust,
            // Rust first: step 0 then writes state that Scala must read at
            // step 1. Starting with Scala would defer the direction the
            // corpus cannot already test.
            Mode::Interleaved => {
                if command_index.is_multiple_of(2) {
                    Which::Rust
                } else {
                    Which::Scala
                }
            }
        }
    }

    pub fn needs_rust(self) -> bool {
        !matches!(self, Mode::Scala)
    }

    pub fn label(self) -> &'static str {
        match self {
            Mode::Scala => "all-scala",
            Mode::Rust => "all-rust",
            Mode::Interleaved => "interleaved",
        }
    }
}

/// How strictly a command step's output is compared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expect {
    /// Exit code and normalized stdout must both match.
    Full,
    /// Exit code only.
    ///
    /// Reserved for steps that fail in clap's or zio-cli's argument parser,
    /// which spec §3.2 sanctions as the one place wording may drift. Declaring
    /// it per step keeps the exemption visible in the scenario source; a
    /// blanket normalization rule would quietly cover real divergence too.
    /// Every use must carry a comment saying which argument error it hits.
    ExitOnly,
}

#[derive(Clone, Debug)]
pub enum Step {
    /// Write `rel_path` with `content(rel_path, version, size)`.
    Write {
        rel_path: &'static str,
        version: u32,
        size: usize,
    },
    /// Delete `rel_path` from the source tree.
    Delete { rel_path: &'static str },
    /// Invoke the binary. `--catalog <path>` is prepended by the runner.
    Cmd {
        label: &'static str,
        args: Vec<String>,
        expect: Expect,
    },
}

/// Write a file at version 0 (the "added" content).
pub fn write(rel_path: &'static str, size: usize) -> Step {
    Step::Write {
        rel_path,
        version: 0,
        size,
    }
}

/// Rewrite a file at version 1 (the "modified" content), matching the harvest
/// script's `tree_modify`.
pub fn modify(rel_path: &'static str, size: usize) -> Step {
    Step::Write {
        rel_path,
        version: 1,
        size,
    }
}

pub fn delete(rel_path: &'static str) -> Step {
    Step::Delete { rel_path }
}

pub fn cmd(label: &'static str, args: &[&str]) -> Step {
    Step::Cmd {
        label,
        args: args.iter().map(|a| a.to_string()).collect(),
        expect: Expect::Full,
    }
}

pub fn cmd_exit_only(label: &'static str, args: &[&str]) -> Step {
    Step::Cmd {
        label,
        args: args.iter().map(|a| a.to_string()).collect(),
        expect: Expect::ExitOnly,
    }
}

#[derive(Clone, Debug)]
pub struct Scenario {
    pub name: &'static str,
    /// `DISKREMENT_DISC_BYTES` for the run. `None` leaves it unset.
    pub disc_bytes: Option<u64>,
    pub steps: Vec<Step>,
}

impl Scenario {
    /// The distinct command words this scenario invokes, e.g. `{init, scan}`.
    /// `runner` uses it to decide whether the Rust binary can run this scenario
    /// yet.
    pub fn commands(&self) -> BTreeSet<&str> {
        self.steps
            .iter()
            .filter_map(|s| match s {
                Step::Cmd { args, .. } => args.first().map(|a| a.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Positions in `steps` that are command invocations, in order.
    pub fn command_indices(&self) -> Vec<usize> {
        self.steps
            .iter()
            .enumerate()
            .filter(|(_, s)| matches!(s, Step::Cmd { .. }))
            .map(|(i, _)| i)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Scenario {
        Scenario {
            name: "sample",
            disc_bytes: Some(1000),
            steps: vec![
                write("a.bin", 800),
                cmd("init p", &["init", "p"]),
                cmd("scan p", &["scan", "p"]),
                cmd("scan p", &["scan", "p"]),
                cmd_exit_only("bogus", &["scan"]),
            ],
        }
    }

    #[test]
    fn commands_are_the_distinct_leading_words() {
        let s = sample();
        let got: Vec<&str> = s.commands().into_iter().collect();
        assert_eq!(got, vec!["init", "scan"]);
    }

    #[test]
    fn command_steps_are_indexed_independently_of_tree_steps() {
        // The interleave alternates over *command* steps; a Write between two
        // commands must not shift which binary runs the next one.
        let s = sample();
        let indices: Vec<usize> = s.command_indices();
        assert_eq!(indices, vec![1, 2, 3, 4]);
    }

    #[test]
    fn interleaved_alternates_starting_with_rust() {
        // Rust first, so that step 1 already writes state Scala must read.
        // Starting with Scala would defer the interesting direction.
        assert_eq!(Mode::Interleaved.which(0), Which::Rust);
        assert_eq!(Mode::Interleaved.which(1), Which::Scala);
        assert_eq!(Mode::Interleaved.which(2), Which::Rust);
        assert_eq!(Mode::Scala.which(3), Which::Scala);
        assert_eq!(Mode::Rust.which(3), Which::Rust);
    }

    #[test]
    fn modes_needing_rust_are_identified() {
        assert!(!Mode::Scala.needs_rust());
        assert!(Mode::Rust.needs_rust());
        assert!(Mode::Interleaved.needs_rust());
    }
}
