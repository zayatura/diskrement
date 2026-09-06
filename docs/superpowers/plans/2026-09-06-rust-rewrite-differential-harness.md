# Rust Rewrite — Differential Harness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up `crates/diskrement-difftest`, a dev-only harness that runs a
scenario through the Scala binary, the Rust binary, or the two alternating, and
proves the three runs agree — in their command output and in the end state they
leave on disk.

**Architecture:** Spec §8.1. One scenario definition, a runner parameterized by
*which binary executes step N*, a justified-rule normalizer for volatile values,
and an end-state comparator that reuses the corpus's comparison rules rather than
re-deriving them. The harness bootstraps against **Scala-versus-Scala**, so every
component is proven correct before the Rust binary implements a single command.

**Tech Stack:** Rust 2021. No new project dependencies — `tempfile`,
`serde_json`, `rusqlite`, `sha2`, `reflink-copy` and `proptest` are all already
workspace dependencies.

**Spec:** `docs/superpowers/specs/2026-09-05-rust-rewrite-design.md` — read §4
(the corpus and its comparison rules), §8.1 (this harness), and §9 (phasing)
before starting. Reference implementation for the scenario mechanics:
`scripts/harvest-fixtures.sh`, which already solved scratch layout, env
isolation, the CoW probe, and the content recipe.

---

## Global Constraints

- **Branch:** all work on `feat/rewrite-p3`, forked from `feat/rust-rewrite`.
  This plan is the *leading task* of P3; the fs/scanner work follows it on the
  same branch. Never commit directly to `feat/rust-rewrite`.
- **Commit scope:** `feat(difftest):`, `test(difftest):`, `chore(difftest):`,
  `docs(difftest):`.
- **Do not touch anything under `core/`, `shell/`, `cli/`, `build.sbt`, or
  `project/`.** The Scala tree is the oracle and stays untouched until P10.
- **Do not modify `fixtures/` or `scripts/harvest-fixtures.sh`.** The corpus is
  frozen; re-harvesting is a spec-level event. This plan *reads* the corpus (to
  pin the content recipe) and *reads* the script (as a reference), nothing more.
- After every task: `cargo test --workspace`, `cargo clippy --workspace
  --all-targets -- -D warnings`, and `cargo fmt --check` all clean.
- Every differential test is `#[ignore]`d. Routine `cargo test` must stay
  JVM-free.
- **A skip must say why** (spec §7). There is no `TestAspect.ignore` here; a
  differential run that cannot happen prints the reason and the command that
  fixes it. A run where nothing executed must not look green.

## Running the harness

```bash
sbt cli/stage                                   # build the reference launcher
export DISKREMENT_SCALA_BIN="$PWD/cli/target/universal/stage/bin/diskrement"
export TMPDIR=/path/on/btrfs-or-xfs             # `init` refuses non-CoW
cargo build -p diskrement-cli                   # the Rust binary under test
cargo test -p diskrement-difftest -- --ignored --nocapture
```

## File Structure

| file | responsibility |
|---|---|
| `Cargo.toml` (modify) | add `crates/diskrement-difftest` as a workspace member |
| `crates/diskrement-shell/Cargo.toml` (modify) | add a `testutil` feature and the self dev-dependency that enables it for the crate's own integration tests |
| `crates/diskrement-shell/src/lib.rs` (modify) | expose `pub mod testutil` under that feature |
| `crates/diskrement-shell/src/testutil.rs` (create) | **the** implementation of the §4 logical-dump comparison rules: `CATALOG_TABLES`, `SCANCACHE_TABLES`, `dump_tables`, `assert_dumps_match` |
| `crates/diskrement-shell/tests/support/mod.rs` (modify) | re-export from the crate instead of holding a second copy |
| `crates/diskrement-difftest/Cargo.toml` (create) | dev-only crate manifest |
| `crates/diskrement-difftest/src/lib.rs` (create) | crate docs, `repo_root()`, module wiring |
| `crates/diskrement-difftest/src/content.rs` (create) | the deterministic content recipe, pinned against the committed corpus |
| `crates/diskrement-difftest/src/scenario.rs` (create) | `Scenario`, `Step`, `Expect`, `Which`, `Mode` — data only, no I/O |
| `crates/diskrement-difftest/src/runner.rs` (create) | binary resolution, CoW probe, scratch layout, env isolation, per-step execution |
| `crates/diskrement-difftest/src/normalize.rs` (create) | the named, justified redaction rules |
| `crates/diskrement-difftest/src/state_diff.rs` (create) | `Snapshot` of a finished run and its comparison |
| `crates/diskrement-difftest/src/corpus.rs` (create) | the named scenarios |
| `crates/diskrement-difftest/tests/differential.rs` (create) | the `#[ignore]`d entry points |
| `crates/diskrement-difftest/README.md` (create) | what this crate is, and that it is deleted at P10 |
| `CLAUDE.md` (modify) | how to run the harness; the disposability note |

The split is by responsibility, not by layer: `scenario` is pure data, `runner`
owns process and filesystem effects, `normalize` owns the oracle's holes, and
`state_diff` owns the comparison rules. Only `runner` performs I/O, which is what
lets `scenario`, `normalize` and `content` be unit-tested with no Scala binary
present — and those unit tests are **not** `#[ignore]`d, so the routine suite
still covers most of the crate.

---

### Task 0: Branch

**Files:** none.

- [ ] **Step 1: Fork the phase branch**

```bash
git checkout feat/rust-rewrite
git pull --ff-only origin feat/rust-rewrite 2>/dev/null || true
git checkout -b feat/rewrite-p3
```

Expected: `Switched to a new branch 'feat/rewrite-p3'`, with `git log --oneline -1`
showing the differential-harness spec commit.

---

### Task 1: Promote the comparison rules into `diskrement-shell`

The corpus tests and the harness must share **one** implementation of §4's
logical-dump rules. Today `dump_catalog` lives in
`crates/diskrement-shell/tests/support/mod.rs`, which is an integration-test
module and therefore unreachable from another crate. Move it into the library
behind a `testutil` feature, so nothing ships it and both consumers use it.

**Files:**
- Create: `crates/diskrement-shell/src/testutil.rs`
- Modify: `crates/diskrement-shell/src/lib.rs`
- Modify: `crates/diskrement-shell/Cargo.toml`
- Modify: `crates/diskrement-shell/tests/support/mod.rs:69-134`

- [ ] **Step 1: Write the new module**

`crates/diskrement-shell/src/testutil.rs`:

```rust
//! Logical comparison helpers, shared by the fixture-corpus tests and the
//! differential harness (spec §4, §8.1).
//!
//! Feature-gated behind `testutil` so that nothing ships in the binary. It
//! lives in the library rather than in a test module because two crates need
//! it, and two copies of a comparison rule are two rules that drift.
//!
//! Every comparison here is **logical**, never byte-wise: SQLite file layout is
//! not stable across library builds, and xerial's SQLite is not
//! `libsqlite3-sys`'s.

/// Catalog tables, in schema order.
pub const CATALOG_TABLES: &[&str] = &[
    "schema_version",
    "project",
    "disc",
    "session",
    "file_version",
    "placement",
    "change_record",
    "disc_event",
    "lost_version",
];

/// Fingerprint-cache sidecar tables. The sidecar carries its schema version in
/// `PRAGMA user_version` rather than in a table, so this list has one entry.
pub const SCANCACHE_TABLES: &[&str] = &["fingerprint"];

fn render(value: rusqlite::types::ValueRef<'_>) -> String {
    use rusqlite::types::ValueRef;
    match value {
        ValueRef::Null => "NULL".to_string(),
        ValueRef::Integer(i) => i.to_string(),
        // `{:?}` so 2.0 stays "2.0" rather than collapsing to "2".
        ValueRef::Real(f) => format!("{f:?}"),
        ValueRef::Text(t) => format!("{:?}", String::from_utf8_lossy(t)),
        ValueRef::Blob(b) => format!("<{} bytes>", b.len()),
    }
}

/// Every row of the named tables as `table: col=value …` lines, sorted within
/// each table.
///
/// Callers that normalize the lines afterwards must re-sort: normalization can
/// change a line's sort key, and two runs whose rows differ only in a timestamp
/// would otherwise sort differently and diff as a false positive.
pub fn dump_tables(conn: &rusqlite::Connection, tables: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for table in tables {
        let mut stmt = conn
            .prepare(&format!("select * from {table}"))
            .unwrap_or_else(|e| panic!("preparing dump of {table}: {e}"));
        let columns: Vec<String> = stmt.column_names().iter().map(|c| c.to_string()).collect();
        let mut rows = stmt.query([]).expect("dump query runs");
        let mut lines = Vec::new();
        while let Some(row) = rows.next().expect("readable row") {
            let cells: Vec<String> = columns
                .iter()
                .enumerate()
                .map(|(i, name)| {
                    format!("{name}={}", render(row.get_ref(i).expect("column exists")))
                })
                .collect();
            lines.push(format!("{table}: {}", cells.join(" ")));
        }
        lines.sort();
        out.extend(lines);
    }
    out
}

/// Every row of every catalog table.
pub fn dump_catalog(conn: &rusqlite::Connection) -> Vec<String> {
    dump_tables(conn, CATALOG_TABLES)
}

/// Compare two dumps, reporting the first difference rather than two walls of
/// text.
pub fn assert_dumps_match(expected: &[String], actual: &[String]) {
    for (i, (want, got)) in expected.iter().zip(actual.iter()).enumerate() {
        assert_eq!(want, got, "dumps diverge at line {i}");
    }
    assert_eq!(
        expected.len(),
        actual.len(),
        "row counts differ:\nexpected {expected:#?}\nactual {actual:#?}"
    );
    assert!(!expected.is_empty(), "the dump was empty");
}
```

- [ ] **Step 2: Gate it in the library root**

Append to `crates/diskrement-shell/src/lib.rs`:

```rust
#[cfg(feature = "testutil")]
pub mod testutil;
```

- [ ] **Step 3: Declare the feature and enable it for this crate's own tests**

In `crates/diskrement-shell/Cargo.toml`, add a `[features]` section after
`[dependencies]` and a self dev-dependency:

```toml
[features]
testutil = []

[dev-dependencies]
diskrement-shell = { path = ".", features = ["testutil"] }
proptest.workspace = true
tempfile.workspace = true
```

The self dev-dependency is the standard way to turn a feature on for a crate's
own integration tests without turning it on for consumers. It does not create a
cycle: Cargo resolves dev-dependencies separately from the normal graph.

- [ ] **Step 4: Rewire the corpus tests to the promoted copy**

In `crates/diskrement-shell/tests/support/mod.rs`, delete `CATALOG_TABLES`,
`render`, `dump_catalog`, and `assert_dumps_match` (lines 69-134) and replace
them with a re-export, so the existing `use support::dump_catalog;` call sites
elsewhere keep compiling unchanged:

```rust
pub use diskrement_shell::testutil::{assert_dumps_match, dump_catalog, CATALOG_TABLES};
```

Leave `fixtures_dir`, `open_fixture`, `fixture_dbs` and `fixture_copy` where they
are — they are corpus-specific, not comparison rules, and the harness does not
want them.

- [ ] **Step 5: Verify nothing regressed**

Run: `cargo test --workspace 2>&1 | tail -25`
Expected: every existing suite passes, with the **same** test count as before the
task. Record the count; a drop here means a corpus test stopped compiling and was
silently dropped rather than fixed.

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`
Expected: no output, exit 0. In particular there must be no `dead_code` warning
from `testutil` — `#![allow(dead_code)]` at the top of `tests/support/mod.rs`
covers that file, not the library.

- [ ] **Step 6: Commit**

```bash
git add crates/diskrement-shell/Cargo.toml crates/diskrement-shell/src/lib.rs \
        crates/diskrement-shell/src/testutil.rs crates/diskrement-shell/tests/support/mod.rs
git commit -m "refactor(shell): promote the logical-dump rules behind a testutil feature

The differential harness needs the same catalog comparison the corpus tests
use, and a test-module copy is unreachable from another crate. One
implementation of a rule beats two that drift."
```

---

### Task 2: Scaffold the crate and pin the content recipe

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/diskrement-difftest/Cargo.toml`
- Create: `crates/diskrement-difftest/src/lib.rs`
- Create: `crates/diskrement-difftest/src/content.rs`

- [ ] **Step 1: Add the workspace member**

In the root `Cargo.toml`, extend `members`:

```toml
members = [
    "crates/diskrement-core",
    "crates/diskrement-shell",
    "crates/diskrement-cli",
    # Dev-only differential harness (spec §8.1). Deleted at P10 with the Scala tree.
    "crates/diskrement-difftest",
]
```

The comment is the point: a workspace member with no shipping consumer is
otherwise indistinguishable from one somebody forgot to wire up.

- [ ] **Step 2: Write the manifest**

`crates/diskrement-difftest/Cargo.toml`:

```toml
[package]
name = "diskrement-difftest"
description = "Dev-only differential harness: runs scenarios through the Scala and Rust binaries and diffs them. Deleted at P10."
version.workspace = true
edition.workspace = true
license.workspace = true
publish.workspace = true
repository.workspace = true

[dependencies]
diskrement-shell = { workspace = true, features = ["testutil"] }
reflink-copy.workspace = true
rusqlite.workspace = true
serde_json.workspace = true
sha2.workspace = true
tempfile.workspace = true

[dev-dependencies]
proptest.workspace = true
```

These are `[dependencies]` rather than `[dev-dependencies]` because the crate's
own `tests/` directory consumes them through the library. Nothing shipping
depends on this crate, so they never reach the binary.

- [ ] **Step 3: Write the crate root**

`crates/diskrement-difftest/src/lib.rs`:

```rust
//! Differential harness: run one scenario through the Scala binary, the Rust
//! binary, or the two alternating, and prove the runs agree (spec §8.1).
//!
//! **This crate is disposable.** It exists only while both implementations do,
//! and P10 deletes it together with the Scala tree. Nothing shipping may depend
//! on it.
//!
//! The harness complements the golden corpus under `fixtures/` rather than
//! replacing it. The corpus is the JVM-free, default-on regression floor; this
//! is the opt-in deep check that also covers the reverse direction — Scala
//! reading what Rust wrote — which a recording structurally cannot.

use std::path::{Path, PathBuf};

pub mod content;
// Each later task adds its own `pub mod` line here as it creates the file.
// Declaring them all up front would break the build until the last one lands.

/// Absolute path of the repository root.
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the crate lives two levels under the repository root")
}
```

- [ ] **Step 4: Write the failing content-recipe test**

`crates/diskrement-difftest/src/content.rs`:

```rust
//! The scenario content recipe, shared with `scripts/harvest-fixtures.sh:74`.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_matches_the_committed_corpus() {
        // `fixtures/source/a.bin` is a.bin at version 1: the P0 scenario
        // modified it in cycle 2. Pinning against the corpus proves this
        // recipe is the harvest script's recipe, not merely a plausible one.
        let expected = std::fs::read(crate::repo_root().join("fixtures/source/a.bin"))
            .expect("the corpus is committed");
        assert_eq!(expected.len(), 800);
        assert_eq!(content("a.bin", 1, 800), expected);
    }

    #[test]
    fn content_is_the_hex_digest_repeated_and_truncated() {
        // sha256("a.bin@0") = aeeb7e5b02f890db450ac5e51ae3bf68…
        assert_eq!(content("a.bin", 0, 16), b"aeeb7e5b02f890db".to_vec());
        // 64 hex chars per cycle, so byte 64 restarts the digest.
        let long = content("a.bin", 0, 70);
        assert_eq!(&long[64..70], b"aeeb7e");
    }
}
```

- [ ] **Step 5: Run it and watch it fail**

Run: `cargo test -p diskrement-difftest`
Expected: FAIL — `cannot find function 'content' in this scope`.

- [ ] **Step 6: Implement the recipe**

Prepend to `crates/diskrement-difftest/src/content.rs`, above the test module:

```rust
use sha2::{Digest, Sha256};

/// Deterministic bytes for one scenario file.
///
/// `sha256("<rel_path>@<version>")` rendered as lowercase hex, repeated and
/// truncated to `size`. Every byte is a function of `(rel_path, version, size)`,
/// so a scenario reproduces on any machine and is directly comparable with the
/// P0 corpus, which the harvest script generated with this same recipe.
pub fn content(rel_path: &str, version: u32, size: usize) -> Vec<u8> {
    let digest = Sha256::digest(format!("{rel_path}@{version}").as_bytes());
    let hex = format!("{digest:x}");
    hex.as_bytes().iter().copied().cycle().take(size).collect()
}
```

- [ ] **Step 7: Run it and watch it pass**

Run: `cargo test -p diskrement-difftest`
Expected: `test result: ok. 2 passed`.

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml crates/diskrement-difftest
git commit -m "feat(difftest): scaffold the harness crate and pin the content recipe

The recipe is pinned against fixtures/source/a.bin rather than against a
hand-computed digest, so the test proves it is the harvest script's recipe
and not merely a plausible one."
```

---

### Task 3: Scenario types

Pure data: a scenario is a name, an optional disc-size override, and a list of
steps. No I/O, so this whole module is unit-testable without a Scala binary and
its tests are **not** `#[ignore]`d.

**Files:**
- Create: `crates/diskrement-difftest/src/scenario.rs`
- Modify: `crates/diskrement-difftest/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

`crates/diskrement-difftest/src/scenario.rs`, test module only for now:

```rust
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
```

- [ ] **Step 2: Run and watch it fail**

Run: `cargo test -p diskrement-difftest scenario`
Expected: FAIL — `cannot find type 'Scenario' in this scope` and friends.

- [ ] **Step 3: Implement the types**

Prepend to `crates/diskrement-difftest/src/scenario.rs`:

```rust
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
                if command_index % 2 == 0 {
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
```

Add to `crates/diskrement-difftest/src/lib.rs`, after `pub mod content;`:

```rust
pub mod scenario;
```

- [ ] **Step 4: Run and watch it pass**

Run: `cargo test -p diskrement-difftest`
Expected: `test result: ok. 6 passed` (2 from `content`, 4 from `scenario`).

- [ ] **Step 5: Commit**

```bash
git add crates/diskrement-difftest/src/scenario.rs crates/diskrement-difftest/src/lib.rs
git commit -m "feat(difftest): scenario, step and mode types

The interleave alternates over command steps rather than all steps, so
inserting a source-tree mutation never shifts which binary runs the next
command — otherwise editing a scenario would silently change what it tests."
```

---

### Task 4: The runner

The only module that performs I/O. It resolves both binaries, refuses to run on
a non-CoW filesystem, builds the scratch layout the harvest script established,
and executes each step through the binary the mode selects.

**Files:**
- Create: `crates/diskrement-difftest/src/runner.rs`
- Modify: `crates/diskrement-difftest/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

These test the parts that need no binary; the end-to-end path is proven in
Task 7 by the Scala-versus-Scala bootstrap.

`crates/diskrement-difftest/src/runner.rs`, test module only:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_scala_binary_skips_with_the_fix_in_the_message() {
        let skip = resolve_scala(None).expect_err("no path means skip");
        assert!(skip.0.contains("DISKREMENT_SCALA_BIN"), "{}", skip.0);
        assert!(skip.0.contains("sbt cli/stage"), "{}", skip.0);
    }

    #[test]
    fn a_scala_path_that_is_not_a_file_skips_naming_the_path() {
        let skip = resolve_scala(Some("/nonexistent/launcher".into()))
            .expect_err("a missing file means skip");
        assert!(skip.0.contains("/nonexistent/launcher"), "{}", skip.0);
    }

    #[test]
    fn an_explicit_rust_binary_path_wins() {
        assert_eq!(
            resolve_rust(Some("/custom/diskrement".into())),
            Some(PathBuf::from("/custom/diskrement"))
        );
    }

    #[test]
    fn the_scancache_path_follows_the_catalog_stem_and_project() {
        let scratch = Path::new("/tmp/x");
        assert_eq!(
            scancache_path(&scratch.join("catalog/cat.sqlite"), "p"),
            scratch.join("catalog/cat.p.scancache.sqlite")
        );
    }

    #[test]
    fn an_unimplemented_command_blocks_only_the_rust_modes() {
        let uses = ["init", "burn"].into_iter().collect::<BTreeSet<_>>();
        let implemented = ["init"];
        assert!(rust_gap(&uses, &implemented).is_some());
        assert_eq!(rust_gap(&uses, &implemented).unwrap(), vec!["burn"]);
        assert!(rust_gap(&uses, &["init", "burn"]).is_none());
    }
}
```

- [ ] **Step 2: Run and watch it fail**

Run: `cargo test -p diskrement-difftest runner`
Expected: FAIL — `cannot find function 'resolve_scala' in this scope`.

- [ ] **Step 3: Implement the runner**

Prepend to `crates/diskrement-difftest/src/runner.rs`:

```rust
//! Executing a scenario: binary resolution, environment isolation, scratch
//! layout, and per-step invocation.
//!
//! The scratch layout, the environment isolation and the CoW probe all mirror
//! `scripts/harvest-fixtures.sh`, deliberately: a scenario run here must land in
//! the same shape the corpus was harvested from, or the two oracles would not be
//! comparing the same thing.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

use crate::content::content;
use crate::scenario::{Expect, Mode, Scenario, Step, Which};

/// The project name every scenario uses. Short, and it keeps the sidecar
/// filename (`cat.p.scancache.sqlite`) identical to the corpus's.
pub const PROJECT: &str = "p";

/// Commands the Rust binary implements.
///
/// **Each phase from P3 onward appends to this list in the same commit that
/// lands the command.** A mode that needs the Rust binary skips — loudly — any
/// scenario using a command that is not listed here, so the harness never
/// reports a red for work that has not been done yet, and never reports a
/// green for a scenario it quietly declined to run.
pub const RUST_IMPLEMENTED: &[&str] = &[];

/// Why a differential run cannot happen here.
///
/// Always printed, never swallowed: spec §7 replaces the Scala suite's silent
/// `TestAspect.ignore` with a skip that states its reason.
#[derive(Debug)]
pub struct Skip(pub String);

pub struct Binaries {
    pub scala: PathBuf,
    /// `None` when the Rust binary has not been built yet.
    pub rust: Option<PathBuf>,
}

fn resolve_scala(from_env: Option<OsString>) -> Result<PathBuf, Skip> {
    let Some(raw) = from_env else {
        return Err(Skip(
            "DISKREMENT_SCALA_BIN is unset, so there is no reference \
             implementation to diff against. Build it with `sbt cli/stage` and \
             export DISKREMENT_SCALA_BIN=$PWD/cli/target/universal/stage/bin/diskrement."
                .to_string(),
        ));
    };
    let path = PathBuf::from(raw);
    if !path.is_file() {
        return Err(Skip(format!(
            "DISKREMENT_SCALA_BIN={} is not a file. Re-run `sbt cli/stage`.",
            path.display()
        )));
    }
    Ok(path)
}

/// Locate the Rust binary, or `None` if it has not been built.
///
/// Deliberately *not* `assert_cmd::cargo::cargo_bin`, which panics when the
/// binary is absent: the all-Scala bootstrap must run before the Rust binary
/// exists at all, so a missing binary has to be a skip rather than a crash.
/// This resolves it the same way `cargo_bin`'s fallback does — the sibling of
/// the test executable, one level up out of `deps/`.
fn resolve_rust(from_env: Option<OsString>) -> Option<PathBuf> {
    if let Some(raw) = from_env {
        return Some(PathBuf::from(raw));
    }
    let exe = std::env::current_exe().ok()?;
    // …/target/<profile>/deps/differential-<hash> → …/target/<profile>/diskrement
    let dir = exe.parent()?.parent()?;
    let candidate = dir.join(format!("diskrement{}", std::env::consts::EXE_SUFFIX));
    candidate.is_file().then_some(candidate)
}

/// Locate both binaries. Only the Scala one is required here: the modes that
/// need the Rust binary check for it themselves, so the bootstrap can run
/// without it.
pub fn binaries() -> Result<Binaries, Skip> {
    Ok(Binaries {
        scala: resolve_scala(std::env::var_os("DISKREMENT_SCALA_BIN"))?,
        rust: resolve_rust(std::env::var_os("DISKREMENT_RUST_BIN")),
    })
}

/// Commands the scenario needs that the Rust binary does not have yet.
fn rust_gap<'a>(uses: &BTreeSet<&'a str>, implemented: &[&str]) -> Option<Vec<&'a str>> {
    let missing: Vec<&str> = uses
        .iter()
        .copied()
        .filter(|c| !implemented.contains(c))
        .collect();
    (!missing.is_empty()).then_some(missing)
}

/// `<catalog-stem>.<project>.scancache.sqlite`, beside the catalog
/// (`PathResolver.scala:43`).
fn scancache_path(catalog: &Path, project: &str) -> PathBuf {
    let stem = catalog
        .file_stem()
        .expect("the catalog path has a filename")
        .to_string_lossy()
        .into_owned();
    catalog.with_file_name(format!("{stem}.{project}.scancache.sqlite"))
}

/// Can this directory reflink? `init` refuses a non-CoW work area, so a
/// scenario cannot run at all without one.
fn cow_capable(dir: &Path) -> bool {
    let src = dir.join(".cow-probe-src");
    let dst = dir.join(".cow-probe-dst");
    let ok = std::fs::write(&src, b"abcd").is_ok() && reflink_copy::reflink(&src, &dst).is_ok();
    let _ = std::fs::remove_file(&src);
    let _ = std::fs::remove_file(&dst);
    ok
}

/// The scratch tree one run owns. Mirrors the harvest script's layout so that a
/// run and the corpus are the same shape.
pub struct Scratch {
    /// Held so the directory outlives the run; dropping it deletes everything.
    pub dir: TempDir,
    pub root: PathBuf,
    pub source: PathBuf,
    pub work: PathBuf,
    pub catalog: PathBuf,
    pub scancache: PathBuf,
    pub home: PathBuf,
}

fn scratch() -> Result<Scratch, Skip> {
    let dir = TempDir::new().map_err(|e| Skip(format!("no writable temp directory: {e}")))?;
    let root = dir
        .path()
        .canonicalize()
        .map_err(|e| Skip(format!("canonicalizing the scratch root: {e}")))?;
    if !cow_capable(&root) {
        return Err(Skip(format!(
            "{} is not on a CoW filesystem (btrfs/XFS/bcachefs), and `init` \
             refuses one. Set TMPDIR to a CoW directory and re-run.",
            root.display()
        )));
    }
    let source = root.join("source");
    let work = root.join("work");
    let home = root.join("home");
    let catalog = root.join("catalog/cat.sqlite");
    let scancache = scancache_path(&catalog, PROJECT);
    for d in [
        &source,
        &work,
        &home,
        &home.join(".local/share"),
        &home.join("state"),
        &root.join("catalog"),
    ] {
        std::fs::create_dir_all(d).map_err(|e| Skip(format!("creating {}: {e}", d.display())))?;
    }
    Ok(Scratch {
        dir,
        root,
        source,
        work,
        catalog,
        scancache,
        home,
    })
}

#[derive(Debug)]
pub struct StepOutcome {
    pub label: String,
    pub which: Which,
    pub argv: Vec<String>,
    pub exit: i32,
    pub stdout: String,
    /// Captured so it does not leak into the test output, and so it is on hand
    /// when debugging a diff. **Not compared**: spec §4 records that stderr
    /// carries progress lines and is not an oracle.
    pub stderr: String,
    pub expect: Expect,
}

pub struct RunOutcome {
    pub scenario: &'static str,
    pub mode: Mode,
    pub steps: Vec<StepOutcome>,
    pub scratch: Scratch,
}

/// Run one scenario end to end.
///
/// Returns `Skip` — never a panic — when the environment cannot support the
/// run, so the caller can report it as a skip with its reason.
pub fn run(bins: &Binaries, scenario: &Scenario, mode: Mode) -> Result<RunOutcome, Skip> {
    if mode.needs_rust() {
        if let Some(missing) = rust_gap(&scenario.commands(), RUST_IMPLEMENTED) {
            return Err(Skip(format!(
                "scenario '{}' in {} mode needs commands the Rust binary does not \
                 implement yet: {}. Add them to runner::RUST_IMPLEMENTED in the \
                 commit that lands them.",
                scenario.name,
                mode.label(),
                missing.join(", ")
            )));
        }
        if bins.rust.is_none() {
            return Err(Skip(
                "the Rust binary has not been built. Run \
                 `cargo build -p diskrement-cli`, or set DISKREMENT_RUST_BIN."
                    .to_string(),
            ));
        }
    }

    let scratch = scratch()?;
    let mut steps = Vec::new();
    let mut command_index = 0usize;

    for step in &scenario.steps {
        match step {
            Step::Write {
                rel_path,
                version,
                size,
            } => {
                let path = scratch.source.join(rel_path);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| Skip(format!("creating {}: {e}", parent.display())))?;
                }
                std::fs::write(&path, content(rel_path, *version, *size))
                    .map_err(|e| Skip(format!("writing {}: {e}", path.display())))?;
            }
            Step::Delete { rel_path } => {
                let path = scratch.source.join(rel_path);
                std::fs::remove_file(&path)
                    .map_err(|e| Skip(format!("deleting {}: {e}", path.display())))?;
            }
            Step::Cmd {
                label,
                args,
                expect,
            } => {
                let which = mode.which(command_index);
                command_index += 1;
                let bin = match which {
                    Which::Scala => &bins.scala,
                    // Checked above: a Rust mode returns Skip before reaching
                    // here when the binary is absent.
                    Which::Rust => bins.rust.as_ref().expect("checked above"),
                };
                // Scenarios cannot know the scratch path, so they write the
                // placeholders `{source}` and `{work}` and the runner fills
                // them in. Substituting here keeps a scenario a pure,
                // machine-independent value that two runs can share.
                let source = scratch.source.display().to_string();
                let work = scratch.work.display().to_string();
                let mut argv: Vec<String> = vec![
                    "--catalog".to_string(),
                    scratch.catalog.display().to_string(),
                ];
                argv.extend(
                    args.iter()
                        .map(|a| a.replace("{source}", &source).replace("{work}", &work)),
                );

                let mut cmd = Command::new(bin);
                cmd.args(&argv)
                    .current_dir(&scratch.root)
                    .env("HOME", &scratch.home)
                    .env("XDG_DATA_HOME", scratch.home.join(".local/share"))
                    .env("STATE_DIRECTORY", scratch.home.join("state"))
                    .env("DISKREMENT_BURNER", "simulated")
                    .env_remove("DISKREMENT_DEVICE")
                    .env_remove("DISKREMENT_HASH_PARALLELISM");
                match scenario.disc_bytes {
                    Some(n) => {
                        cmd.env("DISKREMENT_DISC_BYTES", n.to_string());
                    }
                    None => {
                        cmd.env_remove("DISKREMENT_DISC_BYTES");
                    }
                }
                let out = cmd
                    .output()
                    .map_err(|e| Skip(format!("spawning {}: {e}", bin.display())))?;
                steps.push(StepOutcome {
                    label: (*label).to_string(),
                    which,
                    argv,
                    // A signal-killed child has no code; -1 is not a valid exit
                    // code, so it can never collide with a real one.
                    exit: out.status.code().unwrap_or(-1),
                    stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
                    expect: *expect,
                });
            }
        }
    }

    Ok(RunOutcome {
        scenario: scenario.name,
        mode,
        steps,
        scratch,
    })
}
```

Add to `crates/diskrement-difftest/src/lib.rs`:

```rust
pub mod runner;
```

- [ ] **Step 4: Run and watch the unit tests pass**

Run: `cargo test -p diskrement-difftest`
Expected: `test result: ok. 11 passed` (2 content, 4 scenario, 5 runner).

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`
Expected: exit 0.

- [ ] **Step 5: Commit**

```bash
git add crates/diskrement-difftest/src/runner.rs crates/diskrement-difftest/src/lib.rs
git commit -m "feat(difftest): scenario runner with loud skips and a Rust-coverage gate

Scratch layout, env isolation and the CoW probe mirror harvest-fixtures.sh so
a run and the corpus are the same shape. RUST_IMPLEMENTED is empty until P3's
fs task lands init/scan/status, so the Rust modes skip with a reason rather
than failing for work not yet done."
```

---

### Task 5: The normalizer

Every rule here is a hole punched in the oracle. The module is written so that
adding one is deliberate: rules are named methods with a doc comment stating what
varies and why erasing it is safe.

The instant rule deserves its reasoning up front. Wall-clock values differ
between two runs by nature, so they must go — but replacing them all with a
single `<timestamp>` would also erase the *relationships* between them. Two
fingerprint rows written in one batch share an instant; a session's `burned_at`
and its disc's are ordered. Replacing the `n`th **distinct** instant with `<tn>`
in first-appearance order erases the absolute values and keeps every equality and
distinctness relation, so the harness still notices if one implementation stops
batching its writes.

**Files:**
- Create: `crates/diskrement-difftest/src/normalize.rs`
- Modify: `crates/diskrement-difftest/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

`crates/diskrement-difftest/src/normalize.rs`, test module only:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn norm() -> Normalizer {
        Normalizer::new(Path::new("/tmp/scratch-abc"))
    }

    #[test]
    fn the_scratch_root_becomes_a_placeholder() {
        let mut n = norm();
        assert_eq!(
            n.text("staged at /tmp/scratch-abc/work/staging/disc-1"),
            "staged at <scratch>/work/staging/disc-1"
        );
    }

    #[test]
    fn distinct_instants_get_ordinals_in_first_appearance_order() {
        let mut n = norm();
        let input = "a=2026-09-05T20:01:03.260Z b=2026-09-05T20:01:04.900788637Z \
                     c=2026-09-05T20:01:03.260Z";
        assert_eq!(n.text(input), "a=<t0> b=<t1> c=<t0>");
    }

    #[test]
    fn ordinals_continue_across_calls_so_a_whole_run_shares_one_numbering() {
        let mut n = norm();
        assert_eq!(n.text("2026-01-01T00:00:00Z"), "<t0>");
        assert_eq!(n.text("2026-01-02T00:00:00Z"), "<t1>");
        assert_eq!(n.text("2026-01-01T00:00:00Z"), "<t0>");
    }

    #[test]
    fn whole_second_and_fractional_instants_are_both_recognised() {
        // java.time.Instant.toString() omits the fraction when it is zero and
        // otherwise emits 3, 6 or 9 digits (spec §3.1).
        let mut n = norm();
        assert_eq!(
            n.text("2026-01-01T00:00:00Z 2026-01-01T00:00:00.123Z \
                    2026-01-01T00:00:00.123456Z 2026-01-01T00:00:00.123456789Z"),
            "<t0> <t1> <t2> <t3>"
        );
    }

    #[test]
    fn near_misses_are_left_alone() {
        let mut n = norm();
        // No trailing Z: a local timestamp is not an instant, and silently
        // erasing one would hide a real format difference.
        assert_eq!(n.text("2026-01-01T00:00:00"), "2026-01-01T00:00:00");
        // Not a date at all.
        assert_eq!(n.text("version 2026-01"), "version 2026-01");
        assert_eq!(n.text("12345678901234567890"), "12345678901234567890");
    }

    #[test]
    fn json_strings_and_keys_are_normalised_but_numbers_are_not() {
        let mut n = norm();
        let v: serde_json::Value = serde_json::from_str(
            r#"{"burnedAt":"2026-01-01T00:00:00Z","sizeBytes":800,
                "paths":["/tmp/scratch-abc/source/a.bin"]}"#,
        )
        .unwrap();
        let got = n.json(&v);
        assert_eq!(got["burnedAt"], serde_json::json!("<t0>"));
        assert_eq!(got["sizeBytes"], serde_json::json!(800));
        assert_eq!(got["paths"][0], serde_json::json!("<scratch>/source/a.bin"));
    }
}
```

- [ ] **Step 2: Run and watch it fail**

Run: `cargo test -p diskrement-difftest normalize`
Expected: FAIL — `cannot find type 'Normalizer' in this scope`.

- [ ] **Step 3: Implement the normalizer**

Prepend to `crates/diskrement-difftest/src/normalize.rs`:

```rust
//! Volatile-value normalization.
//!
//! **Every rule in this module is a hole punched in the differential oracle.**
//! A rule that erases more than it must silently converts a real difference into
//! a pass, which is the one failure mode this harness cannot detect in itself.
//! So each rule is a named method carrying a doc comment that says exactly what
//! varies between two runs and why erasing it is safe. Adding a rule without
//! that justification is a review failure, not a style preference.
//!
//! One `Normalizer` serves one run, and every value from that run — command
//! stdout, catalog dumps, staged JSON — passes through the same instance, so
//! instant ordinals are consistent across the whole run.
//!
//! Deliberately *not* normalized: the wording of clap's and zio-cli's argument
//! errors, which spec §3.2 sanctions as free to drift. That exemption is
//! declared per step, as `Expect::ExitOnly` in the scenario, so it is visible
//! where it applies instead of being a pattern that quietly covers everything.

use std::path::Path;

use serde_json::{Map, Value};

pub struct Normalizer {
    scratch_root: String,
    seen_instants: Vec<String>,
}

impl Normalizer {
    pub fn new(scratch_root: &Path) -> Self {
        Self {
            scratch_root: scratch_root.display().to_string(),
            seen_instants: Vec::new(),
        }
    }

    /// Apply every rule to one blob of text.
    pub fn text(&mut self, input: &str) -> String {
        let stage = self.rule_scratch_root(input);
        self.rule_instant_ordinals(&stage)
    }

    /// Recursively normalize every string in a JSON value — keys included,
    /// since a key is as capable of carrying a path as a value is. Numbers and
    /// booleans pass through: nothing volatile is encoded as one.
    pub fn json(&mut self, value: &Value) -> Value {
        match value {
            Value::String(s) => Value::String(self.text(s)),
            Value::Array(items) => Value::Array(items.iter().map(|v| self.json(v)).collect()),
            Value::Object(map) => {
                let mut out = Map::new();
                for (k, v) in map {
                    let key = self.text(k);
                    let val = self.json(v);
                    out.insert(key, val);
                }
                Value::Object(out)
            }
            other => other.clone(),
        }
    }

    /// **Rule: scratch root.** The two runs being compared live in different
    /// temp directories by construction, and the path appears both in recorded
    /// argv and in `status` output. Safe because the *relative* structure below
    /// the root is preserved verbatim, so a real layout difference still shows.
    fn rule_scratch_root(&self, input: &str) -> String {
        input.replace(&self.scratch_root, "<scratch>")
    }

    /// **Rule: instant ordinals.** Wall-clock values differ between runs by
    /// nature. The `n`th distinct instant becomes `<tn>` in first-appearance
    /// order, which erases absolute values while preserving every equality and
    /// distinctness relation between them — so two rows written in one batch
    /// still compare equal in both runs, and would still diverge if one
    /// implementation stopped batching.
    fn rule_instant_ordinals(&mut self, input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        let mut i = 0usize;
        while i < input.len() {
            if let Some(end) = instant_at(input, i) {
                let text = &input[i..end];
                let n = match self.seen_instants.iter().position(|s| s == text) {
                    Some(n) => n,
                    None => {
                        self.seen_instants.push(text.to_string());
                        self.seen_instants.len() - 1
                    }
                };
                out.push_str(&format!("<t{n}>"));
                i = end;
                continue;
            }
            // `i` is always on a char boundary: it starts at 0 and advances
            // either by a whole char or to `end`, which follows an ASCII 'Z'.
            let ch = input[i..].chars().next().expect("i is a char boundary");
            out.push(ch);
            i += ch.len_utf8();
        }
        out
    }
}

/// If a `java.time.Instant.toString()` value starts at byte `i`, its exclusive
/// end index.
///
/// Shape: `YYYY-MM-DDTHH:MM:SS`, an optional `.` plus one or more digits, then
/// `Z`. Hand-written rather than a regex so the harness adds no dependency, and
/// so the accepted shape is legible next to spec §3.1's description of it.
fn instant_at(s: &str, i: usize) -> Option<usize> {
    let b = s.as_bytes();
    let digit = |k: usize| b.get(k).is_some_and(|c| c.is_ascii_digit());
    let lit = |k: usize, c: u8| b.get(k) == Some(&c);

    for k in [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18] {
        if !digit(i + k) {
            return None;
        }
    }
    if !lit(i + 4, b'-') || !lit(i + 7, b'-') || !lit(i + 10, b'T') {
        return None;
    }
    if !lit(i + 13, b':') || !lit(i + 16, b':') {
        return None;
    }
    let mut end = i + 19;
    if lit(end, b'.') {
        let mut j = end + 1;
        while digit(j) {
            j += 1;
        }
        // A lone '.' with no digits is not a fraction; leave `end` where it is
        // and let the 'Z' check below reject the whole match.
        if j > end + 1 {
            end = j;
        }
    }
    lit(end, b'Z').then_some(end + 1)
}
```

Add to `crates/diskrement-difftest/src/lib.rs`:

```rust
pub mod normalize;
```

- [ ] **Step 4: Run and watch it pass**

Run: `cargo test -p diskrement-difftest`
Expected: `test result: ok. 17 passed` (2 content, 4 scenario, 5 runner, 6 normalize).

- [ ] **Step 5: Commit**

```bash
git add crates/diskrement-difftest/src/normalize.rs crates/diskrement-difftest/src/lib.rs
git commit -m "feat(difftest): the justified-rule normalizer

Instants become ordinals rather than a single placeholder, so absolute times
are erased but equality between them survives — the harness still notices if
one implementation stops writing a batch under one timestamp."
```

---

### Task 6: End-state comparison

Command output is only half the oracle. Two runs can print the same thing and
leave different bytes on disk, and on-disc format drift is the risk spec §11
calls the most expensive to discover late. This module snapshots what a finished
run left behind, under §4's comparison rules, and diffs two snapshots.

**Files:**
- Create: `crates/diskrement-difftest/src/state_diff.rs`
- Modify: `crates/diskrement-difftest/src/lib.rs`

- [ ] **Step 1: Write the failing test**

The interesting behavior needs a real run, which Task 7 supplies; what is
unit-testable here is that the comparison reports the first real difference and
tolerates the volatile ones.

`crates/diskrement-difftest/src/state_diff.rs`, test module only:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn snap(lines: &[&str]) -> Snapshot {
        Snapshot {
            scenario: "t",
            steps: vec![StepSnapshot {
                label: "scan p".into(),
                exit: 0,
                stdout: Some("Scanned: 1 added".into()),
            }],
            catalog: lines.iter().map(|s| s.to_string()).collect(),
            scancache: Vec::new(),
            staged: Default::default(),
            disc_json: Default::default(),
            disc_data: Default::default(),
        }
    }

    #[test]
    fn identical_snapshots_compare_equal() {
        let a = snap(&["disc: id=1 status=\"OPEN\""]);
        let b = snap(&["disc: id=1 status=\"OPEN\""]);
        assert_eq!(differences(&a, &b), Vec::<String>::new());
    }

    #[test]
    fn a_row_difference_is_reported_with_both_sides() {
        let a = snap(&["disc: id=1 status=\"OPEN\""]);
        let b = snap(&["disc: id=1 status=\"FINALIZED\""]);
        let diffs = differences(&a, &b);
        assert_eq!(diffs.len(), 1);
        assert!(diffs[0].contains("catalog"), "{}", diffs[0]);
        assert!(diffs[0].contains("OPEN"), "{}", diffs[0]);
        assert!(diffs[0].contains("FINALIZED"), "{}", diffs[0]);
    }

    #[test]
    fn a_row_count_difference_is_reported() {
        let a = snap(&["disc: id=1 status=\"OPEN\""]);
        let b = snap(&["disc: id=1 status=\"OPEN\"", "disc: id=2 status=\"OPEN\""]);
        assert_eq!(differences(&a, &b).len(), 1);
        assert!(differences(&a, &b)[0].contains("1 vs 2"));
    }

    #[test]
    fn an_exit_only_step_ignores_stdout_but_not_the_code() {
        let mut a = snap(&[]);
        let mut b = snap(&[]);
        a.steps[0].stdout = None;
        b.steps[0].stdout = None;
        assert!(differences(&a, &b).is_empty());
        b.steps[0].exit = 2;
        assert_eq!(differences(&a, &b).len(), 1);
    }
}
```

- [ ] **Step 2: Run and watch it fail**

Run: `cargo test -p diskrement-difftest state_diff`
Expected: FAIL — `cannot find type 'Snapshot' in this scope`.

- [ ] **Step 3: Implement the snapshot and the comparison**

Prepend to `crates/diskrement-difftest/src/state_diff.rs`:

```rust
//! What a finished run left behind, and whether two runs left the same thing.
//!
//! Comparison rules are spec §4's, unchanged and deliberately not re-derived:
//! catalog and scancache **logically** (row dumps, never bytes, because SQLite
//! file layout is not stable across library builds); staged plans, manifests and
//! pointers **semantically** (parsed-value equality, because field order is not
//! an oracle); `data/` payloads **byte-for-byte**, since those bytes are the
//! whole point of the tool.

use std::collections::BTreeMap;
use std::path::Path;

use diskrement_shell::testutil::{dump_tables, CATALOG_TABLES, SCANCACHE_TABLES};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::normalize::Normalizer;
use crate::runner::RunOutcome;
use crate::scenario::Expect;

#[derive(Debug, Clone)]
pub struct StepSnapshot {
    pub label: String,
    pub exit: i32,
    /// `None` for `Expect::ExitOnly` steps, where spec §3.2 lets clap and
    /// zio-cli phrase argument errors differently.
    pub stdout: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub scenario: &'static str,
    pub steps: Vec<StepSnapshot>,
    pub catalog: Vec<String>,
    pub scancache: Vec<String>,
    /// `staged/*.json` in the work dir, by filename.
    pub staged: BTreeMap<String, Value>,
    /// `.diskrement/*.json` from every staged disc, keyed `disc-N/<file>`.
    pub disc_json: BTreeMap<String, Value>,
    /// `data/` payloads from every staged disc, keyed `disc-N/<relpath>`, as
    /// sha256 hex — the bytes must match exactly, but printing them on failure
    /// would be useless.
    pub disc_data: BTreeMap<String, String>,
}

/// Open a database that the run just wrote, read-only and without disturbing it.
///
/// `immutable=1` is what the corpus tests use to avoid creating `-wal`/`-shm`
/// files. Here the file is in a temp dir that is about to be deleted, so the
/// motive is different — a read must not change what the *other* comparison
/// steps will see — but the mechanism is the same one.
fn open_ro(path: &Path) -> Option<Connection> {
    if !path.is_file() {
        return None;
    }
    let uri = format!("file:{}?immutable=1", path.display());
    Connection::open_with_flags(
        &uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .ok()
}

fn dump_normalized(path: &Path, tables: &[&str], norm: &mut Normalizer) -> Vec<String> {
    let Some(conn) = open_ro(path) else {
        return Vec::new();
    };
    let mut lines: Vec<String> = dump_tables(&conn, tables)
        .iter()
        .map(|l| norm.text(l))
        .collect();
    // Re-sort *after* normalizing: normalization changes a line's sort key, and
    // two runs whose rows differ only in a timestamp would otherwise sort
    // differently and diff as a false positive.
    lines.sort();
    lines
}

fn read_json(path: &Path, norm: &mut Normalizer) -> Option<Value> {
    let raw = std::fs::read_to_string(path).ok()?;
    let parsed: Value = serde_json::from_str(&raw).ok()?;
    Some(norm.json(&parsed))
}

fn walk_files(root: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// Capture everything a finished run left on disk.
///
/// Takes the run's own `Normalizer`, already advanced by the command stdout, so
/// that instant ordinals are consistent across the whole snapshot.
pub fn snapshot(run: &RunOutcome, norm: &mut Normalizer) -> Snapshot {
    let steps = run
        .steps
        .iter()
        .map(|s| StepSnapshot {
            label: s.label.clone(),
            exit: s.exit,
            stdout: match s.expect {
                Expect::Full => Some(norm.text(&s.stdout)),
                Expect::ExitOnly => None,
            },
        })
        .collect();

    let catalog = dump_normalized(&run.scratch.catalog, CATALOG_TABLES, norm);
    let scancache = dump_normalized(&run.scratch.scancache, SCANCACHE_TABLES, norm);

    let mut staged = BTreeMap::new();
    let staged_dir = run.scratch.work.join("staged");
    for path in walk_files(&staged_dir) {
        if path.extension().is_some_and(|e| e == "json") {
            let name = path
                .strip_prefix(&staged_dir)
                .expect("walked from staged_dir")
                .display()
                .to_string();
            if let Some(v) = read_json(&path, norm) {
                staged.insert(name, v);
            }
        }
    }

    let mut disc_json = BTreeMap::new();
    let mut disc_data = BTreeMap::new();
    let staging = run.scratch.work.join("staging");
    let mut discs: Vec<std::path::PathBuf> = std::fs::read_dir(&staging)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect())
        .unwrap_or_default();
    discs.sort();
    for disc in discs {
        let disc_name = disc
            .file_name()
            .expect("a directory has a name")
            .to_string_lossy()
            .into_owned();
        let meta = disc.join(".diskrement");
        for path in walk_files(&meta) {
            if path.extension().is_some_and(|e| e == "json") {
                let name = path.file_name().expect("a file has a name").to_string_lossy();
                if let Some(v) = read_json(&path, norm) {
                    disc_json.insert(format!("{disc_name}/{name}"), v);
                }
            }
        }
        // The embedded catalog.sqlite is deliberately not compared here: it is a
        // *copy* of the catalog already compared above, and comparing it twice
        // would report one drift as two failures. P5's differential task adds it
        // once `burn` writes it.
        let data = disc.join("data");
        for path in walk_files(&data) {
            let rel = path
                .strip_prefix(&data)
                .expect("walked from data")
                .display()
                .to_string();
            let bytes = std::fs::read(&path).unwrap_or_default();
            let hex = format!("{:x}", Sha256::digest(&bytes));
            disc_data.insert(format!("{disc_name}/{rel}"), hex);
        }
    }

    Snapshot {
        scenario: run.scenario,
        steps,
        catalog,
        scancache,
        staged,
        disc_json,
        disc_data,
    }
}

fn diff_lines(what: &str, a: &[String], b: &[String], out: &mut Vec<String>) {
    for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
        if x != y {
            out.push(format!("{what} diverges at line {i}:\n  a: {x}\n  b: {y}"));
            return;
        }
    }
    if a.len() != b.len() {
        out.push(format!("{what} row counts differ: {} vs {}", a.len(), b.len()));
    }
}

fn diff_maps<T: PartialEq + std::fmt::Debug>(
    what: &str,
    a: &BTreeMap<String, T>,
    b: &BTreeMap<String, T>,
    out: &mut Vec<String>,
) {
    for key in a.keys().chain(b.keys()).collect::<std::collections::BTreeSet<_>>() {
        match (a.get(key), b.get(key)) {
            (Some(x), Some(y)) if x != y => {
                out.push(format!("{what} '{key}' differs:\n  a: {x:?}\n  b: {y:?}"))
            }
            (None, Some(_)) => out.push(format!("{what} '{key}' exists only in run b")),
            (Some(_), None) => out.push(format!("{what} '{key}' exists only in run a")),
            _ => {}
        }
    }
}

/// Every way two snapshots disagree. Empty means they are equivalent.
pub fn differences(a: &Snapshot, b: &Snapshot) -> Vec<String> {
    let mut out = Vec::new();
    for (i, (x, y)) in a.steps.iter().zip(b.steps.iter()).enumerate() {
        if x.exit != y.exit {
            out.push(format!(
                "step {i} ({}) exit code: {} vs {}",
                x.label, x.exit, y.exit
            ));
        }
        if x.stdout != y.stdout {
            out.push(format!(
                "step {i} ({}) stdout:\n--- a ---\n{}\n--- b ---\n{}",
                x.label,
                x.stdout.as_deref().unwrap_or("<not compared>"),
                y.stdout.as_deref().unwrap_or("<not compared>")
            ));
        }
    }
    if a.steps.len() != b.steps.len() {
        out.push(format!(
            "step counts differ: {} vs {}",
            a.steps.len(),
            b.steps.len()
        ));
    }
    diff_lines("catalog", &a.catalog, &b.catalog, &mut out);
    diff_lines("scancache", &a.scancache, &b.scancache, &mut out);
    diff_maps("staged json", &a.staged, &b.staged, &mut out);
    diff_maps("disc json", &a.disc_json, &b.disc_json, &mut out);
    diff_maps("disc data", &a.disc_data, &b.disc_data, &mut out);
    out
}

/// Assert two snapshots agree, printing every difference at once.
pub fn assert_equivalent(a: &Snapshot, b: &Snapshot, context: &str) {
    let diffs = differences(a, b);
    assert!(
        diffs.is_empty(),
        "{context}: {} difference(s)\n\n{}",
        diffs.len(),
        diffs.join("\n\n")
    );
}
```

Add to `crates/diskrement-difftest/src/lib.rs`:

```rust
pub mod state_diff;
```

- [ ] **Step 4: Run and watch it pass**

Run: `cargo test -p diskrement-difftest`
Expected: `test result: ok. 21 passed` (the 17 above plus 4 in `state_diff`).

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`
Expected: exit 0.

- [ ] **Step 5: Commit**

```bash
git add crates/diskrement-difftest/src/state_diff.rs crates/diskrement-difftest/src/lib.rs
git commit -m "feat(difftest): end-state snapshot and comparison

Two runs can print the same thing and leave different bytes on disk, which is
the drift spec §11 calls the most expensive to find late. Comparison rules are
§4's, reused from diskrement-shell rather than re-derived."
```

---

### Task 7: The scenario corpus

Three scenarios at scaffold time. Deliberately not more: an error-path scenario
whose trigger has not been ported yet would be a guess, and the later phases add
theirs as they learn the triggers from the Scala source.

Note what differential testing does *not* require: the corpus never hard-codes an
expected exit code or an expected line of output. Both binaries run the same
step, and the assertion is that they agree. That is why an error-path scenario is
safe to add before the error is ported — until then only the Scala-versus-Scala
mode runs it, and it passes.

**Files:**
- Create: `crates/diskrement-difftest/src/corpus.rs`
- Modify: `crates/diskrement-difftest/src/lib.rs`

- [ ] **Step 1: Write the failing test**

`crates/diskrement-difftest/src/corpus.rs`, test module only:

```rust
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
        let got = harvest_replay().commands();
        for expected in ["burn", "disc", "history", "init", "plan", "scan", "status", "verify"] {
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
```

- [ ] **Step 2: Run and watch it fail**

Run: `cargo test -p diskrement-difftest corpus`
Expected: FAIL — `cannot find function 'all' in this scope`.

- [ ] **Step 3: Write the scenarios**

Prepend to `crates/diskrement-difftest/src/corpus.rs`:

```rust
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
            cmd("disc mark-dead p 3 --yes", &["disc", "mark-dead", "p", "3", "--yes"]),
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
```

Add to `crates/diskrement-difftest/src/lib.rs`:

```rust
pub mod corpus;
```

- [ ] **Step 4: Run and watch it pass**

Run: `cargo test -p diskrement-difftest`
Expected: `test result: ok. 25 passed` (the 21 above plus 4 in `corpus`).

- [ ] **Step 5: Commit**

```bash
git add crates/diskrement-difftest/src/corpus.rs crates/diskrement-difftest/src/lib.rs
git commit -m "feat(difftest): the scenario corpus

Three scenarios: the minimal flow P3 unlocks, the error paths, and a live
replay of the P0 harvest flow. No scenario hard-codes an expected exit code
or output line — the assertion is that the two binaries agree."
```

---

### Task 8: The bootstrap — Scala against Scala

This is the task that proves the harness. Running one scenario twice through the
*same* binary must produce identical snapshots; anything else is a volatile value
the normalizer does not yet handle. Doing this before the Rust binary implements
anything means a later red is unambiguously a porting bug, never a harness bug.

**Files:**
- Create: `crates/diskrement-difftest/tests/differential.rs`

- [ ] **Step 1: Write the bootstrap test**

`crates/diskrement-difftest/tests/differential.rs`:

```rust
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
```

- [ ] **Step 2: Build the reference launcher**

```bash
sbt cli/stage
export DISKREMENT_SCALA_BIN="$PWD/cli/target/universal/stage/bin/diskrement"
```

Expected: `cli/target/universal/stage/bin/diskrement` exists and is executable.

- [ ] **Step 3: Confirm the skip path first**

Run: `env -u DISKREMENT_SCALA_BIN cargo test -p diskrement-difftest -- --ignored --nocapture`
Expected: the test passes, printing
`SKIP: DISKREMENT_SCALA_BIN is unset, so there is no reference implementation …`
and nothing else. A skip that produces silence is the defect; confirm the message
appears before trusting any later green.

- [ ] **Step 4: Run the bootstrap for real**

Ensure `TMPDIR` points at a CoW filesystem, then run:

```bash
cargo test -p diskrement-difftest -- --ignored --nocapture
```

Expected on the first attempt: **likely a failure**, naming a value that differs
between two runs of the same binary. That is the discovery loop working as
designed.

- [ ] **Step 5: Close each reported difference with a justified rule**

For every difference the bootstrap reports, decide which it is:

1. *A volatile value* — a temp path, a wall-clock time, a pid, an inode. Add a
   rule to `normalize.rs` as a named method with a doc comment stating what
   varies and why erasing it is safe, plus a unit test in that module's test
   block. Never widen an existing rule to swallow a new case; a rule that covers
   two unrelated things cannot be reviewed.
2. *A non-deterministic ordering* — two rows or two files whose order is not
   fixed. Sort at the point of capture in `state_diff.rs` and say why in a
   comment. Do not normalize the values to hide it.
3. *A genuine defect in the harness* — the runner leaking state between runs, or
   the snapshot reading a file mid-write. Fix the cause.

Re-run after each addition. Repeat until the bootstrap is green.

- [ ] **Step 6: Record what the loop found**

Append an `## Execution deviations` section to this plan file listing every rule
that had to be added beyond the two written in Task 5, with the value it erases.
That list is the honest inventory of the oracle's holes, and P10's parity gate
reads it.

- [ ] **Step 7: Commit**

```bash
git add crates/diskrement-difftest docs/superpowers/plans/2026-09-06-rust-rewrite-differential-harness.md
git commit -m "test(difftest): bootstrap the harness against Scala-versus-Scala

One scenario twice through the same binary must be identical. Proving that
before any Rust command exists means every later red is a porting bug rather
than a harness bug."
```

---

### Task 9: The three-mode entry points

With the bootstrap green, add the two modes that actually compare
implementations. Both skip entirely at scaffold time, because
`RUST_IMPLEMENTED` is empty — and that skip is the deliverable: it proves the
gate works before any command depends on it.

**Files:**
- Modify: `crates/diskrement-difftest/tests/differential.rs`

- [ ] **Step 1: Write the two tests**

Append to `crates/diskrement-difftest/tests/differential.rs`:

```rust
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
```

- [ ] **Step 2: Run and confirm both skip, loudly**

Run: `cargo test -p diskrement-difftest -- --ignored --nocapture`
Expected: three tests pass. `the_harness_is_deterministic_against_itself` reports
the scenarios it reproduced; the other two print, for every scenario, a line like

```
SKIP [init-scan-status / all-rust]: scenario 'init-scan-status' in all-rust mode
needs commands the Rust binary does not implement yet: init, scan, status. Add
them to runner::RUST_IMPLEMENTED in the commit that lands them.
```

followed by `all-rust: 0 scenario(s) compared, 3 skipped`.

- [ ] **Step 3: Prove the gate opens**

Temporarily set `RUST_IMPLEMENTED` to `&["init", "scan", "status"]` and re-run.
Expected: `init-scan-status` no longer skips for a missing command; it now skips
on the missing binary, or fails comparing against the P3 stub. Either outcome
proves the gate is a gate and not a permanent off switch. **Revert the edit**
before committing — the real change belongs to the commit that lands those
commands.

Run: `git diff --stat crates/diskrement-difftest/src/runner.rs`
Expected: no output. If there is any, the temporary edit was not reverted.

- [ ] **Step 4: Commit**

```bash
git add crates/diskrement-difftest/tests/differential.rs
git commit -m "test(difftest): all-rust and interleaved entry points

Both skip until RUST_IMPLEMENTED is populated, and say so per scenario. The
interleaved mode is the only test of the spec's claim that switching binaries
mid-project is invisible."
```

---

### Task 10: Documentation and the per-phase handoff

The harness is only useful if the next eight phases know they must extend it, and
only safe if P10 knows to delete it.

**Files:**
- Create: `crates/diskrement-difftest/README.md`
- Modify: `CLAUDE.md`

- [ ] **Step 1: Write the crate README**

`crates/diskrement-difftest/README.md`:

```markdown
# diskrement-difftest

Dev-only differential harness for the Rust rewrite (spec §8.1). It runs a
scenario through the Scala binary, the Rust binary, or the two alternating, and
proves the runs agree — in command output and in the end state left on disk.

**This crate is disposable.** It exists only while both implementations do, and
P10 deletes it together with the Scala tree. Nothing shipping may depend on it.

## Running

```bash
sbt cli/stage
export DISKREMENT_SCALA_BIN="$PWD/cli/target/universal/stage/bin/diskrement"
export TMPDIR=/path/on/btrfs-or-xfs      # `init` refuses a non-CoW work area
cargo build -p diskrement-cli
cargo test -p diskrement-difftest -- --ignored --nocapture
```

Every differential test is `#[ignore]`d, so routine `cargo test` stays JVM-free.
The unit tests for `content`, `scenario`, `normalize` and `state_diff` are not
ignored and run normally.

## The three modes

| mode | what it proves |
|---|---|
| `Mode::Scala` | **the harness itself** — one scenario run twice through one binary must be identical. Run this first; a failure here is an unnormalized volatile value, not a porting bug. |
| `Mode::Rust` | forward parity, for any scenario rather than the corpus's single harvested one |
| `Mode::Interleaved` | that switching binaries mid-project is invisible (spec §3.1). Subsumes the reverse direction: Scala reads what Rust wrote. |

## Extending it

- **New command ported?** Append it to `runner::RUST_IMPLEMENTED` *in the same
  commit*. Until then, every scenario using it skips with that reason printed.
- **New scenario?** Add it to `corpus::all()`. Never hard-code an expected exit
  code or output line — both binaries run the step and the assertion is that
  they agree, which is why an error-path scenario is safe to add before the
  error is ported.
- **New normalization rule?** It is a hole punched in the oracle. Give it a named
  method, a doc comment saying what varies and why erasing it is safe, and a unit
  test. Never widen an existing rule to swallow an unrelated case.

## Relationship to `fixtures/`

The corpus is the JVM-free, default-on regression floor: it proves Rust reads
what Scala wrote, for one frozen scenario, on every `cargo test`. This harness is
the opt-in deep check covering what a recording structurally cannot — the reverse
direction, arbitrary scenarios, and generated command sequences (P9). Neither
replaces the other.
```

- [ ] **Step 2: Add the CLAUDE.md section**

In `CLAUDE.md`, immediately after the `## Rust rewrite gotchas (P2+)` section,
insert:

```markdown
## The differential harness (P3+)

`crates/diskrement-difftest` runs a scenario through the Scala binary, the Rust
binary, or the two alternating, and diffs command output and end state. It
complements `fixtures/`, which is one-directional and frozen; the harness covers
the reverse direction and arbitrary scenarios. See its README to run it.

- **It is disposable.** P10 deletes it with the Scala tree. Nothing shipping may
  depend on it.
- **Every differential test is `#[ignore]`d.** Routine `cargo test` stays
  JVM-free. A run with no `DISKREMENT_SCALA_BIN` skips and says so — check the
  printed scenario counts, exactly as with the sbt IT suite.
- **Landing a command means appending it to `runner::RUST_IMPLEMENTED`** in the
  same commit. Otherwise the Rust and interleaved modes keep skipping that
  scenario, and a fully-skipped run still exits green.
- **A normalization rule is a hole in the oracle.** Each one carries a doc
  comment saying what varies and why erasing it is safe. Widening a rule to make
  a failing diff pass is how this harness stops working without anyone noticing.
```

- [ ] **Step 3: Verify the whole suite**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
cargo test -p diskrement-difftest -- --ignored --nocapture
```

Expected: all clean; the ignored run reports the bootstrap green and the two
comparison modes fully skipped with reasons. Confirm the Scala build is
untouched: `git status --porcelain -- core shell cli build.sbt project` must
print nothing.

- [ ] **Step 4: Commit**

```bash
git add crates/diskrement-difftest/README.md CLAUDE.md
git commit -m "docs(difftest): document the harness and its per-phase obligations

Records the three things a later phase must not get wrong: append to
RUST_IMPLEMENTED when a command lands, never hard-code an expected value in a
scenario, and never widen a normalization rule to make a diff pass."
```

---

## Execution deviations

- **Task 1, Step 4:** the re-export in `tests/support/mod.rs` is an
  *unused import* (not dead code) in the three test binaries that never touch
  it, and `-D warnings` makes that fatal. The file-level allow became
  `#![allow(dead_code, unused_imports)]` with a comment saying why. The plan's
  note anticipated a `dead_code` warning from `testutil` itself; this is the
  consumer side of the same problem.
- **Task 3/4:** the tree's clippy (1.98) flags `command_index % 2 == 0` as
  `manual_is_multiple_of`, which postdates the plan. Replaced with
  `command_index.is_multiple_of(2)` in `scenario.rs`.
- **Task 7:** the test `the_replay_covers_every_command_the_harvest_scenario_did`
  does not compile as written — `harvest_replay().commands()` borrows a
  `BTreeSet<&str>` from a temporary that drops at the end of the statement
  (E0716). The scenario is bound to a `let` first.

### Bootstrap outcome (Task 8, Step 6)

The bootstrap was **green on the first attempt**: all three scenarios
(`init-scan-status`, `error-paths`, `harvest-replay`) reproduced identically
through Scala-versus-Scala with no edits. **No normalization rules were added
beyond the two written in Task 5** (scratch root, instant ordinals), and no
capture point needed an extra sort. The oracle's holes are exactly:
`<scratch>` path substitution and `<tn>` instant ordinals.

---

## What later phases must add

This plan builds the scaffold. Each phase from P4 to P8 carries its own
differential task in its own plan document; the pattern is the same every time,
and is written here once so those plans can reference it rather than repeat it.

| phase | append to `RUST_IMPLEMENTED` | add to the corpus | extend `state_diff` |
|---|---|---|---|
| **P3** (this branch, after the fs task) | `init`, `scan`, `status` | — the corpus already has `init-scan-status` | scancache dump already covered |
| **P4** | `plan` | a `StalePlan` scenario and an oversize scenario, once the Scala triggers are known | staged `plan.json` already covered |
| **P5** | `burn` | a multi-cycle spill scenario | the embedded `.diskrement/catalog.sqlite`, which Task 6 deliberately left out because nothing writes it yet |
| **P6** | `verify`, `history`, `reset` | a `reset` scenario | — |
| **P7** | `disc` | the coaster flow is already in `harvest-replay`; add a `copies>1` scenario | — |
| **P8** | — (backend selection, not a new command) | a `DISKREMENT_BURNER` permutation scenario | — |
| **P9** | — | the generated sequences replace hand-written additions | — |

At **P10**, delete `crates/diskrement-difftest/`, its workspace-member line, the
`testutil` feature and self dev-dependency in `crates/diskrement-shell/Cargo.toml`
(the corpus tests can take `src/testutil.rs` back into `tests/support/mod.rs`, or
the feature can simply stay — decide then), and the `CLAUDE.md` section added in
Task 10.

---

## Spec coverage

| spec §8.1 requirement | task |
|---|---|
| dev-only crate `crates/diskrement-difftest` | 2 |
| `scenario` module, content recipe shared with the harvest script | 2, 3 |
| `runner` parameterized by which binary runs step N | 3 (`Mode::which`), 4 |
| `DISKREMENT_SCALA_BIN` / `DISKREMENT_RUST_BIN` resolution, without panicking | 4 |
| loud skip naming the reason and the fix | 4, 8 (step 3 verifies it) |
| `normalize` with named, justified rules | 5 |
| per-step `ExitOnly` for the sanctioned clap divergence | 3, 6, 7 |
| `state_diff` under §4's rules | 6 |
| `dump_catalog` promoted so corpus and harness share one implementation | 1 |
| interleaved mode as the test of §3.1's claim | 3, 9 |
| corpus stays the default-on floor; harness is opt-in | 8, 9 (all `#[ignore]`d), 10 |
| disposability recorded so P10 removes it | 2 (manifest), 10 (README, CLAUDE.md), "What later phases must add" |
| `sequence` / randomized generation | **not this plan** — P9 has its own |
