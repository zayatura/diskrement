# Rust Rewrite P10 — the parity gate and the cutover Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prove the Rust binary is a drop-in replacement for the Scala one, then
delete the Scala tree and the differential harness and swap `feat/rust-rewrite`
onto `main`.

**Architecture:** Twelve tasks in two halves. The **parity half** (Tasks 1–8)
adds nothing to the product: it ports the eleven Scala integration scenarios to
`crates/diskrement-cli/tests/scenarios/`, re-runs the perf benchmark against
both binaries on one host, runs the full differential corpus and a chunked wide
sweep, and ends with a real-media smoke run you perform by hand. Nothing is
deleted while the evidence that authorizes the deletion is still being
gathered. The **cutover half** (Tasks 9–12) removes `core/`, `shell/`, `cli/`,
`build.sbt`, `project/`, `scripts/harvest-fixtures.sh` and
`crates/diskrement-difftest/`, rewrites `README.md` and `CLAUDE.md` for the
Rust build, and pushes the branch swap. An explicit confirmation gate sits
between the halves, and a second before `main` and `origin` are touched.

**Tech Stack:** Rust (stable, edition 2021), `assert_cmd`, `tempfile`,
`rusqlite`, `sha2`, `walkdir`, `serde_json`, `reflink-copy`, `proptest` (via
the harness); bash for `scripts/perf-bench.sh`; git.

---

## Global Constraints

- **Branch.** All of Tasks 1–11 happen on `feat/rewrite-p10`, forked from
  `feat/rust-rewrite`. Task 11 merges it back with an explicit merge commit.
  Task 12 is the only task that touches `main` or `origin`. Never commit phase
  work directly to `feat/rust-rewrite`; no PRs.
- **Conventional commits with a scope**, as everywhere else in this repo:
  `test(cli):`, `chore:`, `docs:`, `perf:`. Every commit message in this phase
  ends with the two attribution lines, which the `git commit` commands below
  omit only to stay readable:

  ```
  Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_019vNyFqvFhs9JPHi7phvauJ
  ```
- **Verified means** all three of these clean, from the repository root:

  ```
  cargo test --workspace
  cargo clippy --workspace --all-targets -- -D warnings
  cargo fmt --check
  ```

  with no unexplained drop in test count. The P9 landing recorded **527 passed,
  0 failed, 4 ignored**; Task 1 Step 1 re-derives that number so every later
  task compares against a figure measured on this machine rather than a quoted
  one.
- **Nothing is deleted before Gate 1.** If a task in the parity half wants to
  remove something, it is out of scope — write it down and move on.
- **The scenario suite must not skip silently.** CLAUDE.md records that the sbt
  IT suite "silently skips itself when the work area isn't on a CoW
  filesystem", and that a green run may mean "skipped". The port fixes that
  rather than reproducing it: `DISKREMENT_SCENARIOS_REQUIRED=1` turns a skip
  into a failure, and every verification command in this plan sets it.
- **The Scala tree stays buildable through Task 8.** The differential harness
  and the perf A/B both need it. Do not touch `core/`, `shell/`, `cli/`,
  `build.sbt` or `project/` before Gate 1.

---

## What already exists (do not rebuild it)

- `crates/diskrement-cli/tests/binary.rs` — 41 tests driving the binary through
  `assert_cmd`, organized in `mod p3`/`p4`/`p5`/`p6`/`p7` blocks, with
  `project()`, `base()`, `init_scan_plan()` and `burned_project()` helpers. The
  scenario port is a **sibling** of this file, not a replacement: `binary.rs`
  tests one command's contract at a time, the scenarios test multi-cycle flows
  end to end with an on-disc oracle.
- `crates/diskrement-shell/src/burn/json.rs` — `ProjectJson`, `ManifestJson`,
  `ManifestEntryJson`, `PointersJson`, `ArchivePointerJson`, all `pub`, all
  `#[serde(rename_all = "camelCase")]`. The disc oracle deserializes through
  these, exactly as `DiscOracle.scala` decoded through the production zio-json
  codecs.
- `crates/diskrement-shell/tests/` — nine fixture suites plus
  `xorriso_image.rs`, reading the committed `fixtures/` corpus.
- `crates/diskrement-difftest/` — the 13-scenario corpus, the three run modes
  and the `proptest` sweep. Task 6 runs it; Task 9 deletes it.
- `scripts/perf-bench.sh` — already takes the binary as its third argument.
  Task 5 fixes the two places that assume that binary is a JVM.
- `fixtures/perf-baseline.txt` — the P0 Scala baseline, including the
  idempotent recipe for regenerating the 2000-file / 2.072 GB synthetic tree.

---

## Design decisions taken in this plan

Recorded in the spec at §9 ("Five decisions taken when P10 was planned") and
summarized here so this document stands alone.

1. **Two gates, the first before any deletion.** Spec §10 gates only the branch
   swap. This plan gates the deletion too, because the evidence that authorizes
   it is gathered in the same phase.
2. **The perf comparison is an A/B, not a lookup.** Both binaries exist in the
   parity half and the tree recipe is idempotent, so the Rust run and a fresh
   Scala run happen back to back on one host against one tree. The P0 record
   becomes a drift check.
3. **The wide sweep is chunked.** Five sequential `PROPTEST_CASES=200`
   invocations rather than one large one. Failure persistence is off, so each
   picks fresh seeds and the union is ≥1000 fresh sequences, while this host's
   memory guard costs one chunk instead of the run.
4. **The integration scenarios port whole.** All eleven, even where
   `binary.rs`, the fixture suites or the difftest corpus overlap. A coverage
   map claiming equivalence would put the reviewer's trust in the map.
5. **`scripts/harvest-fixtures.sh` goes with the tree it drives.** Deleted in
   Task 9, preserved on the `scala` branch, pointed at from
   `fixtures/README.md`.

---

## File Structure

**Created**

| Path | Responsibility |
|---|---|
| `crates/diskrement-cli/tests/scenarios.rs` | The integration-test target. Declares the modules and holds nothing else. |
| `crates/diskrement-cli/tests/scenarios/support.rs` | Scratch tree, CoW gate, CLI runner, deterministic tree writer. The port of `Scratch.scala`, `CoW.scala`, `Cli.scala`, `Tree.scala`, `Support.scala`. |
| `crates/diskrement-cli/tests/scenarios/disc.rs` | The disc oracle. The port of `DiscOracle.scala`. |
| `crates/diskrement-cli/tests/scenarios/s00_harness.rs` | Harness self-tests. The port of `HarnessSpec.scala` and `BuildSmokeSpec.scala`. |
| `crates/diskrement-cli/tests/scenarios/s01_happy.rs` … `s10_warm_rescan.rs`, `s11_exit_codes.rs` | One file per ported scenario. |
| `fixtures/perf-rust.txt` | The P10 A/B measurement: Rust and a same-host Scala re-run. |

**Modified**

| Path | Change |
|---|---|
| `crates/diskrement-cli/Cargo.toml` | Four dev-dependencies for the oracle and the tree writer. |
| `scripts/perf-bench.sh` | Target-pid resolution and provenance stop assuming a JVM. |
| `fixtures/perf-baseline.txt` | Header says P10, not P9. |
| `docs/smoke-test-real-media.md` | Rust-only; retitled; the sbt fallback removed. |
| `fixtures/README.md` | Points at the `scala` branch for the harvest script. |
| `README.md`, `CLAUDE.md` | Rewritten for the Rust build (Task 10). |
| `Cargo.toml` | The difftest workspace member removed (Task 9). |

**Deleted (Task 9 only, after Gate 1)**

`core/`, `shell/`, `cli/`, `build.sbt`, `project/`, `scripts/harvest-fixtures.sh`,
`crates/diskrement-difftest/`.

---

## Task 1: The scenario harness

Ports `Scratch.scala`, `CoW.scala`, `Cli.scala`, `Tree.scala`, `Support.scala`,
`DiscOracle.scala`, `HarnessSpec.scala` and `BuildSmokeSpec.scala`. No scenario
uses it yet; its own self-tests are what prove it works.

**Files:**
- Modify: `crates/diskrement-cli/Cargo.toml`
- Create: `crates/diskrement-cli/tests/scenarios.rs`
- Create: `crates/diskrement-cli/tests/scenarios/support.rs`
- Create: `crates/diskrement-cli/tests/scenarios/disc.rs`
- Create: `crates/diskrement-cli/tests/scenarios/s00_harness.rs`

- [ ] **Step 1: Fork the branch and record the baseline**

```bash
git checkout feat/rust-rewrite
git status --porcelain          # must be empty
git checkout -b feat/rewrite-p10
cargo test --workspace 2>&1 | tail -20
```

Write the passed/failed/ignored totals into a scratch note; every later
verification compares against them. Expected, from the P9 landing: 527 passed,
0 failed, 4 ignored.

- [ ] **Step 2: Add the dev-dependencies**

The oracle needs `rusqlite` (the embedded catalog) plus `serde` and
`serde_json` (the three metadata files, deserialized through the shipping
DTOs); the tree writer needs `sha2`; walking `data/` needs `walkdir`. All five
are already workspace dependencies.

In `crates/diskrement-cli/Cargo.toml`, replace the `[dev-dependencies]` block
with:

```toml
[dev-dependencies]
assert_cmd.workspace = true
insta.workspace = true
predicates.workspace = true
reflink-copy.workspace = true
rusqlite.workspace = true
serde.workspace = true
serde_json.workspace = true
sha2.workspace = true
tempfile.workspace = true
walkdir.workspace = true
```

- [ ] **Step 3: Write the test target**

Create `crates/diskrement-cli/tests/scenarios.rs`:

```rust
//! The ported integration scenarios (`cli/src/it/scala/diskrement/cli/it/`).
//!
//! `binary.rs` pins one command's contract at a time. These pin multi-cycle
//! flows end to end and read the staged disc back through the production
//! `burn::json` codecs, which is the part no other suite covers.
//!
//! Every scenario needs a CoW work area, because `init` refuses one that is
//! not. Without it a scenario prints `SKIPPED` and passes — unless
//! `DISKREMENT_SCENARIOS_REQUIRED` is set, which turns the skip into a
//! failure. Set it whenever a green run is being used as evidence.

mod disc;
mod support;

mod s00_harness;
```

- [ ] **Step 4: Write the support module**

Create `crates/diskrement-cli/tests/scenarios/support.rs`:

```rust
//! Scratch trees, the CoW gate, the CLI runner and the tree writer.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The binary under test. Cargo builds it for us, which is why
/// `BuildSmokeSpec`'s "is the launcher staged?" test has no counterpart.
pub const BIN: &str = env!("CARGO_BIN_EXE_diskrement");

/// One scenario's isolated world. Dropping it deletes everything.
///
/// The catalog is always passed with `--catalog`, which short-circuits
/// `PathResolver`'s fallback chain; `HOME`, `XDG_DATA_HOME` and
/// `STATE_DIRECTORY` are redirected into the scratch as defense in depth.
pub struct Scratch {
    _dir: tempfile::TempDir,
    pub root: PathBuf,
    pub source: PathBuf,
    pub work: PathBuf,
    pub catalog: PathBuf,
    pub home: PathBuf,
}

impl Scratch {
    /// `<work>/staging/disc-<ordinal>`, the staged disc tree.
    pub fn disc_dir(&self, ordinal: u32) -> PathBuf {
        self.work.join(format!("staging/disc-{ordinal}"))
    }

    /// A runner with the project's default capacity.
    pub fn cli(&self) -> Cli {
        Cli {
            catalog: self.catalog.clone(),
            home: self.home.clone(),
            disc_bytes: None,
        }
    }

    /// A runner with `DISKREMENT_DISC_BYTES` set. Test-only, and it may only
    /// shrink capacity (CLAUDE.md).
    pub fn cli_with_disc_bytes(&self, bytes: u64) -> Cli {
        Cli {
            catalog: self.catalog.clone(),
            home: self.home.clone(),
            disc_bytes: Some(bytes),
        }
    }
}

/// Build a scratch tree, or say why this machine cannot host one.
///
/// The CoW probe is the reason: `init` refuses a work dir whose filesystem
/// cannot reflink, so on ext4 every scenario would fail for an uninteresting
/// reason. `TempDir` follows `TMPDIR`, so pointing that at a btrfs/XFS
/// directory is the fix, and the message says so.
pub fn scratch_or_skip(scenario: &str) -> Option<Scratch> {
    match build_scratch() {
        Ok(s) => Some(s),
        Err(reason) => {
            assert!(
                std::env::var_os("DISKREMENT_SCENARIOS_REQUIRED").is_none(),
                "scenario '{scenario}' cannot run here, and \
                 DISKREMENT_SCENARIOS_REQUIRED is set: {reason}"
            );
            eprintln!("SKIPPED scenario '{scenario}': {reason}");
            None
        }
    }
}

fn build_scratch() -> Result<Scratch, String> {
    let dir = tempfile::TempDir::new().map_err(|e| format!("no writable temp directory: {e}"))?;
    let root = dir
        .path()
        .canonicalize()
        .map_err(|e| format!("canonicalizing the scratch root: {e}"))?;
    if !cow_capable(&root) {
        return Err(format!(
            "{} is not on a CoW filesystem (btrfs/XFS/bcachefs), and `init` \
             refuses one. Set TMPDIR to a CoW directory and re-run.",
            root.display()
        ));
    }
    let source = root.join("source");
    let work = root.join("work");
    let home = root.join("home");
    let catalog = root.join("catalog/cat.sqlite");
    for d in [
        &source,
        &work,
        &home,
        &home.join(".local/share"),
        &home.join("state"),
        &root.join("catalog"),
    ] {
        std::fs::create_dir_all(d).map_err(|e| format!("creating {}: {e}", d.display()))?;
    }
    Ok(Scratch {
        _dir: dir,
        root,
        source,
        work,
        catalog,
        home,
    })
}

/// `CoW.capable`: a real reflink of a real file, no filesystem-name guessing.
fn cow_capable(dir: &Path) -> bool {
    let src = dir.join(".cow-probe-src");
    let dst = dir.join(".cow-probe-dst");
    let ok = std::fs::write(&src, b"abcd").is_ok() && reflink_copy::reflink(&src, &dst).is_ok();
    let _ = std::fs::remove_file(&src);
    let _ = std::fs::remove_file(&dst);
    ok
}

/// One command's result. `CliResult` in `Cli.scala`.
pub struct Output {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn out_has(&self, needle: &str) -> bool {
        self.stdout.contains(needle)
    }

    pub fn err_has(&self, needle: &str) -> bool {
        self.stderr.contains(needle)
    }

    /// True if any stdout line starts with `prefix`. `Scenario09`'s guard
    /// against `disc 1` also matching `disc 10`.
    pub fn line_starts_with(&self, prefix: &str) -> bool {
        self.stdout.lines().any(|l| l.starts_with(prefix))
    }
}

pub struct Cli {
    catalog: PathBuf,
    home: PathBuf,
    disc_bytes: Option<u64>,
}

impl Cli {
    /// Run one command and capture everything.
    ///
    /// stdin is `/dev/null`, so a prompt reads EOF and declines instead of
    /// hanging. That is what makes the Scala runner's 60-second timeout
    /// unnecessary here.
    pub fn run(&self, args: &[&str]) -> Output {
        let mut c = Command::new(BIN);
        c.arg("--catalog")
            .arg(&self.catalog)
            .args(args)
            .env("HOME", &self.home)
            .env("XDG_DATA_HOME", self.home.join(".local/share"))
            .env("STATE_DIRECTORY", self.home.join("state"))
            .env("DISKREMENT_BURNER", "simulated")
            .env("DISKREMENT_HASH_PARALLELISM", "2")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        match self.disc_bytes {
            Some(b) => c.env("DISKREMENT_DISC_BYTES", b.to_string()),
            None => c.env_remove("DISKREMENT_DISC_BYTES"),
        };
        let out = c.output().unwrap_or_else(|e| panic!("running {BIN}: {e}"));
        Output {
            code: out
                .status
                .code()
                .expect("the CLI exits normally, never on a signal"),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    /// `init` with the scratch's own source and work dirs, asserted to
    /// succeed. Every scenario opens with this.
    pub fn init(&self, s: &Scratch, project: &str) -> Output {
        let r = self.run(&[
            "init",
            project,
            "--source",
            &s.source.display().to_string(),
            "--work-dir",
            &s.work.display().to_string(),
        ]);
        assert_eq!(r.code, 0, "init failed: {}{}", r.stdout, r.stderr);
        r
    }
}

/// Deterministic, per-`(rel_path, version)` distinct content, so two
/// same-size files never collide and a run reproduces anywhere.
///
/// `sha256("<rel_path>@<version>")` as lowercase hex, repeated and truncated
/// to `size` — the recipe in `scripts/harvest-fixtures.sh:74`, which is also
/// what the difftest scenarios and the P0 corpus use. `Tree.scala` used a
/// Java `Random` seeded from `String.hashCode`, which Rust cannot reproduce
/// and nothing needs it to: the property that matters is determinism plus
/// version-distinctness.
fn content(rel_path: &str, version: u32, size: usize) -> Vec<u8> {
    let digest = Sha256::digest(format!("{rel_path}@{version}").as_bytes());
    let hex = format!("{digest:x}");
    hex.as_bytes().iter().copied().cycle().take(size).collect()
}

/// `Tree.write`: create or overwrite `rel_path` with version-0 content.
pub fn write(root: &Path, rel_path: &str, size: usize) {
    let p = root.join(rel_path);
    std::fs::create_dir_all(p.parent().expect("a relative path has a parent")).unwrap();
    std::fs::write(&p, content(rel_path, 0, size)).unwrap();
}

/// `Tree.modify`: overwrite with version-1 content, distinct from `write`.
pub fn modify(root: &Path, rel_path: &str, size: usize) {
    let p = root.join(rel_path);
    std::fs::create_dir_all(p.parent().expect("a relative path has a parent")).unwrap();
    std::fs::write(&p, content(rel_path, 1, size)).unwrap();
}

/// `Tree.delete`.
pub fn delete(root: &Path, rel_path: &str) {
    let _ = std::fs::remove_file(root.join(rel_path));
}

/// A `BTreeSet` of `&str`, for comparing file sets in an assertion.
pub fn set(items: &[&str]) -> std::collections::BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}
```

- [ ] **Step 5: Write the disc oracle**

Create `crates/diskrement-cli/tests/scenarios/disc.rs`:

```rust
//! `DiscOracle.scala`: read a staged `disc-N/` back and expose facts.
//!
//! Deserialization goes through the production `burn::json` DTOs rather than
//! ad-hoc structs, so a serde mistake in the shipping codec fails here too.

use diskrement_shell::burn::json::{ManifestJson, PointersJson, ProjectJson};
use std::collections::BTreeSet;
use std::path::Path;

/// Relative paths of every regular file under `disc-N/data`.
pub fn data_files(disc_dir: &Path) -> BTreeSet<String> {
    let data = disc_dir.join("data");
    if !data.is_dir() {
        return BTreeSet::new();
    }
    walkdir::WalkDir::new(&data)
        .into_iter()
        .map(|e| e.expect("walking the staged data dir"))
        .filter(|e| e.file_type().is_file())
        .map(|e| {
            e.path()
                .strip_prefix(&data)
                .expect("walkdir yields paths under its root")
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

pub fn manifest(disc_dir: &Path) -> ManifestJson {
    read_json(disc_dir, "manifest.json")
}

pub fn project_json(disc_dir: &Path) -> ProjectJson {
    read_json(disc_dir, "project.json")
}

pub fn pointers(disc_dir: &Path) -> PointersJson {
    read_json(disc_dir, "pointers.json")
}

/// Files staged on this disc per the manifest: added ++ modified.
pub fn manifest_files(m: &ManifestJson) -> BTreeSet<String> {
    m.added
        .iter()
        .chain(m.modified.iter())
        .map(|e| e.rel_path.clone())
        .collect()
}

pub fn manifest_added(m: &ManifestJson) -> BTreeSet<String> {
    m.added.iter().map(|e| e.rel_path.clone()).collect()
}

pub fn manifest_modified(m: &ManifestJson) -> BTreeSet<String> {
    m.modified.iter().map(|e| e.rel_path.clone()).collect()
}

pub fn manifest_deleted(m: &ManifestJson) -> BTreeSet<String> {
    m.deleted.iter().cloned().collect()
}

/// True if the staged copy of `rel_path` is byte-identical to the source.
pub fn content_matches(disc_dir: &Path, source: &Path, rel_path: &str) -> bool {
    let staged = std::fs::read(disc_dir.join("data").join(rel_path));
    let src = std::fs::read(source.join(rel_path));
    match (staged, src) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Number of tables in the embedded catalog snapshot — proof that it opens.
///
/// Opened `?immutable=1` with `SQLITE_OPEN_URI`: a plain read-only open of a
/// WAL database creates `-wal`/`-shm` beside it, which here would mean
/// writing two files into a staged disc tree that a later `verify` in the
/// same scenario walks.
pub fn catalog_table_count(disc_dir: &Path) -> i64 {
    let path = disc_dir.join(".diskrement/catalog.sqlite");
    let uri = format!("file:{}?immutable=1", path.display());
    let conn = rusqlite::Connection::open_with_flags(
        uri,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .expect("the embedded catalog opens");
    conn.query_row(
        "select count(*) from sqlite_master where type='table'",
        [],
        |r| r.get(0),
    )
    .expect("counting tables")
}

fn read_json<T: serde::de::DeserializeOwned>(disc_dir: &Path, name: &str) -> T {
    let path = disc_dir.join(".diskrement").join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("decoding {}: {e}", path.display()))
}
```

- [ ] **Step 6: Write the harness self-tests**

Create `crates/diskrement-cli/tests/scenarios/s00_harness.rs`:

```rust
//! `HarnessSpec` and `BuildSmokeSpec`: prove the harness before anything
//! depends on it.

use crate::support::{self, BIN};

#[test]
fn the_binary_under_test_exists_and_help_exits_zero() {
    // `BuildSmokeSpec` asked whether `sbt cli/stage` had run. Cargo builds
    // the binary for the test, so all that is left to check is that the
    // path cargo handed us runs.
    assert!(
        std::path::Path::new(BIN).is_file(),
        "CARGO_BIN_EXE_diskrement does not point at a file: {BIN}"
    );
    let out = std::process::Command::new(BIN)
        .arg("--help")
        .output()
        .expect("the binary runs");
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn init_in_an_isolated_scratch_succeeds_and_leaves_nothing_behind() {
    let Some(s) = support::scratch_or_skip("harness") else {
        return;
    };
    let root = s.root.clone();
    let r = s.cli().init(&s, "proj");
    assert!(r.out_has("Created project proj"), "stdout: {}", r.stdout);
    assert!(root.is_dir(), "the scratch exists while the test holds it");
    assert!(s.catalog.is_file(), "the catalog landed at --catalog's path");

    drop(s);
    assert!(!root.exists(), "the scratch is gone once dropped");
}

#[test]
fn the_tree_writer_is_deterministic_and_version_distinct() {
    let Some(s) = support::scratch_or_skip("harness") else {
        return;
    };
    support::write(&s.source, "a.txt", 128);
    let v0 = std::fs::read(s.source.join("a.txt")).unwrap();
    support::write(&s.source, "a.txt", 128);
    assert_eq!(v0, std::fs::read(s.source.join("a.txt")).unwrap());

    support::modify(&s.source, "a.txt", 128);
    let v1 = std::fs::read(s.source.join("a.txt")).unwrap();
    assert_eq!(v1.len(), 128);
    assert_ne!(v0, v1, "modify must produce different bytes at the same size");

    // Same size, different path: distinct content, so a hash collision
    // between two scenario files cannot happen by accident.
    support::write(&s.source, "b.txt", 128);
    assert_ne!(v0, std::fs::read(s.source.join("b.txt")).unwrap());

    support::delete(&s.source, "a.txt");
    assert!(!s.source.join("a.txt").exists());
}

#[test]
fn the_runner_isolates_the_catalog_and_the_home_dir() {
    let Some(s) = support::scratch_or_skip("harness") else {
        return;
    };
    s.cli().init(&s, "p");
    // Nothing was written to the redirected XDG location: --catalog won.
    let xdg = s.home.join(".local/share");
    let stray: Vec<_> = walkdir::WalkDir::new(&xdg)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(|e| e.path().display().to_string())
        .collect();
    assert!(stray.is_empty(), "the CLI wrote outside --catalog: {stray:?}");
}
```

- [ ] **Step 7: Run the harness tests**

```bash
DISKREMENT_SCENARIOS_REQUIRED=1 cargo test -p diskrement-cli --test scenarios -- --nocapture
```

Expected: 4 passed, 0 failed, and no `SKIPPED` line. If it skips, `TMPDIR` is
not on a CoW filesystem — set it to one under this repo's own tree
(`TMPDIR=$PWD/target/tmp`, created first) and re-run.

- [ ] **Step 8: Verify and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
git add crates/diskrement-cli/Cargo.toml crates/diskrement-cli/tests/scenarios.rs \
        crates/diskrement-cli/tests/scenarios/
git commit -m "test(cli): the integration-scenario harness — scratch, CoW gate, runner, disc oracle"
```

---

## Task 2: Scenarios 1–4 — the disc-lifecycle flows

The four scenarios that exercise where a session lands: a first disc, a second
session on the same open disc, a fresh disc after `--finalize`, and three
discs under a capacity override.

**Files:**
- Modify: `crates/diskrement-cli/tests/scenarios.rs`
- Create: `crates/diskrement-cli/tests/scenarios/s01_happy.rs`
- Create: `crates/diskrement-cli/tests/scenarios/s02_append_open.rs`
- Create: `crates/diskrement-cli/tests/scenarios/s03_finalized_new_disc.rs`
- Create: `crates/diskrement-cli/tests/scenarios/s04_three_disc.rs`

- [ ] **Step 1: Declare the modules**

In `crates/diskrement-cli/tests/scenarios.rs`, replace `mod s00_harness;` with:

```rust
mod s00_harness;
mod s01_happy;
mod s02_append_open;
mod s03_finalized_new_disc;
mod s04_three_disc;
```

- [ ] **Step 2: Scenario 1 — the single-disc happy path**

Create `crates/diskrement-cli/tests/scenarios/s01_happy.rs`:

```rust
//! Scenario 1: init → scan → plan → burn → verify, with the staged disc read
//! back and compared against the source byte for byte.

use crate::{disc, support};

#[test]
fn init_scan_plan_burn_verify_and_the_disc_contents_equal_the_source() {
    let Some(s) = support::scratch_or_skip("s01 happy path") else {
        return;
    };
    let cli = s.cli();
    support::write(&s.source, "docs/a.txt", 256);
    support::write(&s.source, "img/b.bin", 512);

    cli.init(&s, "p");

    let scan = cli.run(&["scan", "p"]);
    assert_eq!(scan.code, 0);
    assert!(scan.out_has("Scanned: 2 added, 0 modified, 0 deleted, 0 unchanged."));

    let plan = cli.run(&["plan", "p"]);
    assert_eq!(plan.code, 0);
    assert!(plan.out_has("Plan: disc 1"));
    assert!(plan.out_has("new disc"));

    let burn = cli.run(&["burn", "p", "--yes"]);
    assert_eq!(burn.code, 0);
    assert!(burn.out_has("Wrote disc 1"));

    // The oracle: read the freshly staged disc 1 before verify touches it.
    let d1 = s.disc_dir(1);
    let data = disc::data_files(&d1);
    let man = disc::manifest(&d1);
    let proj = disc::project_json(&d1);
    let pts = disc::pointers(&d1);

    assert_eq!(data, support::set(&["docs/a.txt", "img/b.bin"]));
    assert_eq!(disc::manifest_files(&man), data);
    assert_eq!(man.disc_ordinal, 1);
    assert_eq!(man.session_ordinal, 1);
    assert_eq!(
        disc::manifest_added(&man),
        support::set(&["docs/a.txt", "img/b.bin"])
    );
    assert!(man.deleted.is_empty());
    assert_eq!(proj.project_name, "p");
    // pointers.json holds only superseded and deleted versions: empty here.
    assert!(pts.pointers.is_empty());
    assert!(disc::content_matches(&d1, &s.source, "docs/a.txt"));
    assert!(disc::content_matches(&d1, &s.source, "img/b.bin"));
    assert!(disc::catalog_table_count(&d1) > 0);

    let verify = cli.run(&["verify", "p"]);
    assert_eq!(verify.code, 0);
    assert!(verify.out_has("files OK"));

    let status = cli.run(&["status", "p"]);
    assert_eq!(status.code, 0);
    assert!(status.out_has("Current files: 2"));
}
```

- [ ] **Step 3: Scenario 2 — a second session on the open disc**

Create `crates/diskrement-cli/tests/scenarios/s02_append_open.rs`:

```rust
//! Scenario 2: disc 1 is left OPEN after cycle 1, so cycle 2's add, modify
//! and delete are written to the same disc as session 2. Staging then holds
//! only session 2's files; the cumulative view comes from status and history.

use crate::{disc, support};

#[test]
fn the_second_session_lands_on_disc_one_as_session_two() {
    let Some(s) = support::scratch_or_skip("s02 append to open disc") else {
        return;
    };
    let cli = s.cli();
    support::write(&s.source, "a.txt", 128);
    support::write(&s.source, "b.txt", 128);

    cli.init(&s, "p");

    // Cycle 1: both files to disc 1, left open.
    cli.run(&["scan", "p"]);
    cli.run(&["plan", "p"]);
    let b1 = cli.run(&["burn", "p", "--yes"]);
    assert_eq!(b1.code, 0);
    assert!(b1.out_has("Wrote disc 1"));
    assert!(b1.out_has("left OPEN"));

    let man1 = disc::manifest(&s.disc_dir(1));
    assert_eq!(man1.disc_ordinal, 1);
    assert_eq!(man1.session_ordinal, 1);
    assert_eq!(
        disc::data_files(&s.disc_dir(1)),
        support::set(&["a.txt", "b.txt"])
    );

    // Cycle 2: modify a, add c, delete b.
    support::modify(&s.source, "a.txt", 128);
    support::write(&s.source, "c.txt", 64);
    support::delete(&s.source, "b.txt");

    let sc2 = cli.run(&["scan", "p"]);
    assert_eq!(sc2.code, 0);
    assert!(sc2.out_has("Scanned: 1 added, 1 modified, 1 deleted, 0 unchanged."));

    let pl2 = cli.run(&["plan", "p"]);
    assert_eq!(pl2.code, 0);
    assert!(pl2.out_has("appending to open disc 1"));

    let b2 = cli.run(&["burn", "p", "--yes"]);
    assert_eq!(b2.code, 0);
    assert!(b2.out_has("Wrote disc 1"));
    assert!(b2.out_has("left OPEN"));

    // Staging now holds session 2 only.
    let man2 = disc::manifest(&s.disc_dir(1));
    assert_eq!(man2.disc_ordinal, 1);
    assert_eq!(man2.session_ordinal, 2);
    assert_eq!(
        disc::data_files(&s.disc_dir(1)),
        support::set(&["a.txt", "c.txt"])
    );
    assert_eq!(disc::manifest_added(&man2), support::set(&["c.txt"]));
    assert_eq!(disc::manifest_modified(&man2), support::set(&["a.txt"]));
    assert_eq!(disc::manifest_deleted(&man2), support::set(&["b.txt"]));

    let ha = cli.run(&["history", "p", "a.txt"]);
    assert_eq!(ha.code, 0);
    assert!(ha.out_has("SUPERSEDED"));
    assert!(ha.out_has("CURRENT"));

    let hb = cli.run(&["history", "p", "b.txt"]);
    assert_eq!(hb.code, 0);
    assert!(hb.out_has("DELETED"));

    let status = cli.run(&["status", "p"]);
    assert_eq!(status.code, 0);
    assert!(status.out_has("Current files: 2"));
}
```

- [ ] **Step 4: Scenario 3 — `--finalize` forces a fresh disc**

Create `crates/diskrement-cli/tests/scenarios/s03_finalized_new_disc.rs`:

```rust
//! Scenario 3: `plan --finalize` seals disc 1 even though it has room, so
//! the next cycle's change must open disc 2.

use crate::{disc, support};

#[test]
fn after_finalize_new_changes_go_to_disc_two() {
    let Some(s) = support::scratch_or_skip("s03 finalized -> new disc") else {
        return;
    };
    let cli = s.cli();
    support::write(&s.source, "a.txt", 128);

    cli.init(&s, "p");

    cli.run(&["scan", "p"]);
    let pl1 = cli.run(&["plan", "p", "--finalize"]);
    assert_eq!(pl1.code, 0);
    assert!(pl1.out_has("will FINALIZE"));

    let b1 = cli.run(&["burn", "p", "--yes"]);
    assert_eq!(b1.code, 0);
    assert!(b1.out_has("Wrote disc 1"));
    assert!(b1.out_has("FINALIZED"));

    assert_eq!(disc::data_files(&s.disc_dir(1)), support::set(&["a.txt"]));
    assert_eq!(disc::manifest(&s.disc_dir(1)).disc_ordinal, 1);

    // Cycle 2: a new file cannot go on the sealed disc.
    support::write(&s.source, "b.txt", 128);
    cli.run(&["scan", "p"]);
    let pl2 = cli.run(&["plan", "p"]);
    assert_eq!(pl2.code, 0);
    assert!(pl2.out_has("new disc"));

    let b2 = cli.run(&["burn", "p", "--yes"]);
    assert_eq!(b2.code, 0);
    assert!(b2.out_has("Wrote disc 2"));

    assert_eq!(disc::data_files(&s.disc_dir(2)), support::set(&["b.txt"]));
    let man2 = disc::manifest(&s.disc_dir(2));
    assert_eq!(man2.disc_ordinal, 2);
    assert_eq!(man2.session_ordinal, 1);

    let ha = cli.run(&["history", "p", "a.txt"]);
    assert_eq!(ha.code, 0);
    assert!(ha.out_has("CURRENT"));
}
```

- [ ] **Step 5: Scenario 4 — three discs, three cycles**

Create `crates/diskrement-cli/tests/scenarios/s04_three_disc.rs`:

```rust
//! Scenario 4: three cycles under a 1000-byte capacity override, each
//! producing one disc. Cycle 3 modifies `a` and deletes `b`, landing the new
//! `a` on disc 3.

use crate::{disc, support};

#[test]
fn three_cycles_span_discs_one_two_and_three_with_correct_history() {
    let Some(s) = support::scratch_or_skip("s04 three-disc incremental") else {
        return;
    };
    let cli = s.cli_with_disc_bytes(1000);
    support::write(&s.source, "a.bin", 800);

    cli.init(&s, "p");

    // Cycle 1 -> disc 1 [a]
    cli.run(&["scan", "p"]);
    cli.run(&["plan", "p"]);
    cli.run(&["burn", "p", "--yes"]);

    // Cycle 2 -> disc 2 [b]
    support::write(&s.source, "b.bin", 800);
    cli.run(&["scan", "p"]);
    let pl2 = cli.run(&["plan", "p"]);
    assert_eq!(pl2.code, 0);
    assert!(pl2.out_has("new disc"));
    cli.run(&["burn", "p", "--yes"]);

    // Cycle 3 -> disc 3 [a'], plus b's tombstone
    support::modify(&s.source, "a.bin", 800);
    support::delete(&s.source, "b.bin");
    let sc3 = cli.run(&["scan", "p"]);
    assert_eq!(sc3.code, 0);
    assert!(sc3.out_has("Scanned: 0 added, 1 modified, 1 deleted, 0 unchanged."));
    let pl3 = cli.run(&["plan", "p"]);
    assert_eq!(pl3.code, 0);
    assert!(pl3.out_has("new disc"));
    cli.run(&["burn", "p", "--yes"]);

    assert_eq!(disc::data_files(&s.disc_dir(1)), support::set(&["a.bin"]));
    assert_eq!(disc::data_files(&s.disc_dir(2)), support::set(&["b.bin"]));
    assert_eq!(disc::data_files(&s.disc_dir(3)), support::set(&["a.bin"]));

    let m3 = disc::manifest(&s.disc_dir(3));
    assert_eq!(m3.disc_ordinal, 3);
    assert_eq!(disc::manifest_modified(&m3), support::set(&["a.bin"]));
    assert_eq!(disc::manifest_deleted(&m3), support::set(&["b.bin"]));

    let ha = cli.run(&["history", "p", "a.bin"]);
    assert_eq!(ha.code, 0);
    assert!(ha.out_has("SUPERSEDED"));
    assert!(ha.out_has("CURRENT"));

    let hb = cli.run(&["history", "p", "b.bin"]);
    assert_eq!(hb.code, 0);
    assert!(hb.out_has("DELETED"));

    let status = cli.run(&["status", "p"]);
    assert_eq!(status.code, 0);
    assert!(status.out_has("Current files: 1"));

    let verify = cli.run(&["verify", "p"]);
    assert_eq!(verify.code, 0);
    assert!(verify.out_has("files OK"));
    assert!(!verify.out_has("MISMATCH"));
}
```

- [ ] **Step 6: Run them**

```bash
DISKREMENT_SCENARIOS_REQUIRED=1 cargo test -p diskrement-cli --test scenarios -- --nocapture
```

Expected: 8 passed, 0 failed, no `SKIPPED` line.

- [ ] **Step 7: Verify and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
git add crates/diskrement-cli/tests/
git commit -m "test(cli): port integration scenarios 1-4 — the disc-lifecycle flows"
```

---

## Task 3: Scenarios 5–8 and the exit-code scenario — packing, refusals, no-ops

**Files:**
- Modify: `crates/diskrement-cli/tests/scenarios.rs`
- Create: `crates/diskrement-cli/tests/scenarios/s05_spill.rs`
- Create: `crates/diskrement-cli/tests/scenarios/s06_oversized.rs`
- Create: `crates/diskrement-cli/tests/scenarios/s07_reset.rs`
- Create: `crates/diskrement-cli/tests/scenarios/s08_idempotent.rs`
- Create: `crates/diskrement-cli/tests/scenarios/s11_exit_codes.rs`

- [ ] **Step 1: Declare the modules**

Append to the `mod` list in `crates/diskrement-cli/tests/scenarios.rs`, after
`mod s04_three_disc;`:

```rust
mod s05_spill;
mod s06_oversized;
mod s07_reset;
mod s08_idempotent;
mod s11_exit_codes;
```

- [ ] **Step 2: Scenario 5 — spill across cycles**

Create `crates/diskrement-cli/tests/scenarios/s05_spill.rs`:

```rust
//! Scenario 5: three 400-byte files under a 1000-byte cap. Two fit disc 1,
//! the third spills to disc 2 in the next cycle. The assertion is that the
//! discs *partition* the file set — never which two files the planner chose,
//! which is its business and not a contract.

use crate::{disc, support};

#[test]
fn three_files_over_two_discs_form_a_disjoint_union() {
    let Some(s) = support::scratch_or_skip("s05 spill across cycles") else {
        return;
    };
    let cli = s.cli_with_disc_bytes(1000);
    support::write(&s.source, "f1.bin", 400);
    support::write(&s.source, "f2.bin", 400);
    support::write(&s.source, "f3.bin", 400);

    cli.init(&s, "p");

    // Cycle 1: disc 1 fills with two of the three.
    let sc1 = cli.run(&["scan", "p"]);
    assert_eq!(sc1.code, 0);
    assert!(sc1.out_has("Scanned: 3 added"));
    let pl1 = cli.run(&["plan", "p"]);
    assert_eq!(pl1.code, 0);
    assert!(pl1.out_has("more discs) remain"));
    cli.run(&["burn", "p", "--yes"]);
    let d1 = disc::data_files(&s.disc_dir(1));
    assert_eq!(d1.len(), 2);

    // Cycle 2: the remaining file goes to disc 2 and completes the set.
    let sc2 = cli.run(&["scan", "p"]);
    assert_eq!(sc2.code, 0);
    let pl2 = cli.run(&["plan", "p"]);
    assert_eq!(pl2.code, 0);
    assert!(pl2.out_has("This disc completes the set."));
    cli.run(&["burn", "p", "--yes"]);
    let d2 = disc::data_files(&s.disc_dir(2));
    assert_eq!(d2.len(), 1);

    assert!(
        d1.intersection(&d2).next().is_none(),
        "the two discs overlap: {d1:?} / {d2:?}"
    );
    let union: std::collections::BTreeSet<String> = d1.union(&d2).cloned().collect();
    assert_eq!(union, support::set(&["f1.bin", "f2.bin", "f3.bin"]));

    let status = cli.run(&["status", "p"]);
    assert_eq!(status.code, 0);
    assert!(status.out_has("Current files: 3"));

    assert_eq!(cli.run(&["verify", "p"]).code, 0);
}
```

- [ ] **Step 3: Scenario 6 — an oversized file**

Create `crates/diskrement-cli/tests/scenarios/s06_oversized.rs`:

```rust
//! Scenario 6: a file larger than the disc. `plan` reports it and still
//! exits 0; `burn` refuses with exit 3 (environment/oversize, per the exit
//! contract in `render.rs`); nothing is staged.

use crate::support;

#[test]
fn a_file_larger_than_a_disc_is_reported_and_burn_refuses() {
    let Some(s) = support::scratch_or_skip("s06 oversized file") else {
        return;
    };
    let cli = s.cli_with_disc_bytes(1000);
    support::write(&s.source, "huge.bin", 5000);

    cli.init(&s, "p");
    cli.run(&["scan", "p"]);

    let plan = cli.run(&["plan", "p"]);
    assert_eq!(plan.code, 0);
    assert!(plan.out_has("Oversized (cannot be placed):"));
    assert!(plan.out_has("huge.bin"));

    let burn = cli.run(&["burn", "p", "--yes"]);
    assert_eq!(burn.code, 3);
    assert!(burn.err_has("oversized files cannot be placed"));

    assert!(
        !s.disc_dir(1).exists(),
        "a refused burn must not leave a staged disc behind"
    );
}
```

- [ ] **Step 4: Scenario 7 — reset clears the staged scan**

Create `crates/diskrement-cli/tests/scenarios/s07_reset.rs`:

```rust
//! Scenario 7: `reset` discards the staged scan, so the next `plan` fails
//! with exit 2. This is the scenario that proves the CLI propagates
//! application exit codes rather than collapsing them to 0.

use crate::support;

#[test]
fn reset_clears_the_staged_scan_and_a_following_plan_errors() {
    let Some(s) = support::scratch_or_skip("s07 reset") else {
        return;
    };
    let cli = s.cli();
    support::write(&s.source, "a.txt", 64);

    cli.init(&s, "p");

    assert_eq!(cli.run(&["scan", "p"]).code, 0);

    let reset = cli.run(&["reset", "p"]);
    assert_eq!(reset.code, 0);
    assert!(reset.out_has("Reset p."));

    let plan = cli.run(&["plan", "p"]);
    assert_eq!(plan.code, 2);
    assert!(!plan.stderr.is_empty(), "exit 2 must explain itself");
    // CLAUDE.md: StalePlan/NoStagedScan advice is always "re-run plan",
    // never "run reset" — reset destroys staging the verifier needs.
    assert!(
        !plan.stderr.contains("reset"),
        "the advice must not suggest reset: {}",
        plan.stderr
    );
}
```

- [ ] **Step 5: Scenario 8 — an unchanged re-scan plans nothing**

Create `crates/diskrement-cli/tests/scenarios/s08_idempotent.rs`:

```rust
//! Scenario 8: after a full burn, re-scanning an untouched source reports
//! everything unchanged and there is nothing left to plan.

use crate::support;

#[test]
fn rescanning_an_unchanged_source_reports_all_unchanged_and_plans_nothing() {
    let Some(s) = support::scratch_or_skip("s08 idempotent re-scan") else {
        return;
    };
    let cli = s.cli();
    support::write(&s.source, "a.txt", 64);
    support::write(&s.source, "b.txt", 64);

    cli.init(&s, "p");
    cli.run(&["scan", "p"]);
    cli.run(&["plan", "p"]);
    cli.run(&["burn", "p", "--yes"]);

    // burn cleared the staged state, so this really re-scans.
    let sc2 = cli.run(&["scan", "p"]);
    assert_eq!(sc2.code, 0);
    assert!(sc2.out_has("Scanned: 0 added, 0 modified, 0 deleted, 2 unchanged."));

    let pl2 = cli.run(&["plan", "p"]);
    assert_eq!(pl2.code, 0);
    assert!(pl2.out_has("Nothing to burn: no changes to plan."));
}
```

- [ ] **Step 6: The exit-code scenario**

Create `crates/diskrement-cli/tests/scenarios/s11_exit_codes.rs`:

```rust
//! `ScenarioExitCodes`: application errors propagate as exit 2 rather than
//! collapsing to 0. Exit codes are a stable contract (`render.rs`): 2 =
//! state/lookup, 3 = environment/oversize, 4 = verify mismatch, 5 =
//! unexpected, 6 = media/burner.

use crate::support;

#[test]
fn verify_of_an_unknown_project_and_plan_without_a_scan_both_exit_two() {
    let Some(s) = support::scratch_or_skip("exit codes") else {
        return;
    };
    let cli = s.cli();
    support::write(&s.source, "a.txt", 64);

    cli.init(&s, "p");

    assert_eq!(cli.run(&["verify", "does-not-exist"]).code, 2);
    assert_eq!(cli.run(&["plan", "p"]).code, 2);
}
```

- [ ] **Step 7: Run them**

```bash
DISKREMENT_SCENARIOS_REQUIRED=1 cargo test -p diskrement-cli --test scenarios -- --nocapture
```

Expected: 13 passed, 0 failed, no `SKIPPED` line.

- [ ] **Step 8: Verify and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
git add crates/diskrement-cli/tests/
git commit -m "test(cli): port integration scenarios 5-8 and the exit-code scenario"
```

---

## Task 4: Scenarios 9–10 — the coaster flow and the warm cache

The two heaviest scenarios. Scenario 9 has two tests; scenario 10 pins the
fingerprint cache with a step that can only be explained by the sidecar.

**Files:**
- Modify: `crates/diskrement-cli/tests/scenarios.rs`
- Create: `crates/diskrement-cli/tests/scenarios/s09_mark_dead.rs`
- Create: `crates/diskrement-cli/tests/scenarios/s10_warm_rescan.rs`

- [ ] **Step 1: Declare the modules**

Insert these two into `crates/diskrement-cli/tests/scenarios.rs` **before**
`mod s11_exit_codes;`, so the final list reads in order from `s00_harness` to
`s11_exit_codes`:

```rust
mod s09_mark_dead;
mod s10_warm_rescan;
```

- [ ] **Step 2: Scenario 9 — a dead disc, and a declined one**

Create `crates/diskrement-cli/tests/scenarios/s09_mark_dead.rs`:

```rust
//! Scenario 9: the coaster recovery flow. A topped-off disc dies; its live
//! content re-plans onto a fresh disc, the version that was superseded
//! *on that disc* is permanently lost, and verify stops looking at the dead
//! disc.
//!
//! CLAUDE.md: `LOST` is per physical copy, not per content hash, and `DEAD`
//! is derived from `disc_event` rather than stored.

use crate::{disc, support};

#[test]
fn a_dead_discs_live_content_replans_and_the_superseded_version_is_lost() {
    let Some(s) = support::scratch_or_skip("s09 mark-dead -> re-plan") else {
        return;
    };
    let cli = s.cli();
    support::write(&s.source, "a.txt", 128);
    support::write(&s.source, "b.txt", 128);

    cli.init(&s, "p");

    // Cycle 1: a + b to disc 1, left open.
    cli.run(&["scan", "p"]);
    cli.run(&["plan", "p"]);
    cli.run(&["burn", "p", "--yes"]);

    // Cycle 2: modify b. Session 2 tops off disc 1, superseding b@v0 in
    // place — which is what makes the loss below unrecoverable.
    support::modify(&s.source, "b.txt", 128);
    cli.run(&["scan", "p"]);
    cli.run(&["plan", "p"]);
    cli.run(&["burn", "p", "--yes"]);

    // The coaster event.
    let dead = cli.run(&["disc", "mark-dead", "p", "1", "--yes"]);
    assert_eq!(dead.code, 0);
    assert!(dead.out_has("Disc 1 marked DEAD."));
    assert!(dead.out_has("Permanently lost (1):"));
    // Specifically b.txt's superseded version, not merely "b.txt appears
    // somewhere" — it also appears in the re-burn list below.
    assert!(
        dead.stdout
            .lines()
            .any(|l| l.trim_start().starts_with("b.txt") && l.contains("(was session 1)")),
        "the lost entry is not b.txt's session-1 version: {}",
        dead.stdout
    );
    assert!(dead.out_has("Re-burn on next scan/plan/burn (2):"));

    let status = cli.run(&["status", "p"]);
    assert!(status.out_has("DEAD"));

    // Recovery cycle: everything live re-plans onto a fresh disc 2. The
    // dead disc's content is gone from the burned view, so both files read
    // as added rather than unchanged.
    let sc = cli.run(&["scan", "p"]);
    assert_eq!(sc.code, 0);
    assert!(sc.out_has("Scanned: 2 added, 0 modified, 0 deleted, 0 unchanged."));

    let pl = cli.run(&["plan", "p"]);
    assert_eq!(pl.code, 0);
    assert!(pl.out_has("new disc"));

    let b2 = cli.run(&["burn", "p", "--yes"]);
    assert_eq!(b2.code, 0);
    assert!(b2.out_has("Wrote disc 2"));
    assert_eq!(
        disc::data_files(&s.disc_dir(2)),
        support::set(&["a.txt", "b.txt"])
    );

    let verify = cli.run(&["verify", "p"]);
    assert_eq!(verify.code, 0);
    // Matched on the line prefix: a bare "disc 1" also matches "disc 10".
    assert!(
        !verify.line_starts_with("disc 1:"),
        "verify must skip the dead disc: {}",
        verify.stdout
    );
    assert!(verify.line_starts_with("disc 2:"));
    assert!(verify.out_has("files OK"));

    let hb = cli.run(&["history", "p", "b.txt"]);
    assert_eq!(hb.code, 0);
    assert!(hb.out_has("LOST"));
}

#[test]
fn a_declined_confirmation_aborts_cleanly_and_marks_nothing() {
    let Some(s) = support::scratch_or_skip("s09 declined mark-dead") else {
        return;
    };
    let cli = s.cli();
    support::write(&s.source, "a.txt", 128);

    cli.init(&s, "p");
    cli.run(&["scan", "p"]);
    cli.run(&["plan", "p"]);
    cli.run(&["burn", "p", "--yes"]);

    // No --yes: stdin is /dev/null, so the prompt reads EOF and declines.
    let dead = cli.run(&["disc", "mark-dead", "p", "1"]);
    assert_eq!(dead.code, 0);
    assert!(dead.out_has("Aborted."));

    let status = cli.run(&["status", "p"]);
    assert!(!status.out_has("DEAD"));
}
```

- [ ] **Step 3: Scenario 10 — the warm fingerprint cache**

The discriminating step is `scan --force` taken *before* any burn: the
catalog's fingerprint table is provably empty there, so a fast-path hit can
only come from the sidecar cache. Do not move it after the burn and do not
drop `--force` — either change makes the scenario unable to tell the sidecar
and the catalog fast path apart, which is exactly the defect the Scala
version was rewritten to fix.

Create `crates/diskrement-cli/tests/scenarios/s10_warm_rescan.rs`:

```rust
//! Scenario 10: the fingerprint cache, end to end.
//!
//! `freshScan` merges two sources: fingerprints read back from the catalog
//! (populated once a session is burned) and the sidecar cache (written after
//! every scan, burn or no burn). A rescan taken after a burn is explained by
//! either, so it alone proves nothing about the sidecar. Three rescans are
//! pinned instead: one before any burn, one after, and one after a file's
//! size changes.

use crate::support;

#[test]
fn scans_reuse_fingerprints_before_and_after_a_burn_and_a_touched_file_rehashes() {
    let Some(s) = support::scratch_or_skip("s10 warm-cache rescan") else {
        return;
    };
    let cli = s.cli();
    support::write(&s.source, "a.txt", 100);
    support::write(&s.source, "b.txt", 200);
    support::write(&s.source, "c.txt", 300);

    cli.init(&s, "p");

    let scan1 = cli.run(&["scan", "p"]);
    assert_eq!(scan1.code, 0);
    assert!(scan1.out_has("Hashed 3 of 3 files (0 fingerprints reused)."));

    // The discriminating step. --force is required: without it this reuses
    // the staged scan from scan1, omits the "Hashed" line entirely, and
    // proves nothing.
    let scan1b = cli.run(&["scan", "p", "--force"]);
    assert_eq!(scan1b.code, 0);
    assert!(scan1b.out_has("Hashed 0 of 3 files (3 fingerprints reused)."));

    cli.run(&["plan", "p"]);
    cli.run(&["burn", "p", "--yes"]);

    let scan2 = cli.run(&["scan", "p"]);
    assert_eq!(scan2.code, 0);
    assert!(scan2.out_has("Scanned: 0 added, 0 modified, 0 deleted, 3 unchanged."));
    assert!(scan2.out_has("Hashed 0 of 3 files (3 fingerprints reused)."));

    // A size change is a fingerprint miss whichever source answered.
    support::modify(&s.source, "b.txt", 999);
    let scan3 = cli.run(&["scan", "p", "--force"]);
    assert_eq!(scan3.code, 0);
    assert!(scan3.out_has("Scanned: 0 added, 1 modified, 0 deleted, 2 unchanged."));
    assert!(scan3.out_has("Hashed 1 of 3 files (2 fingerprints reused)."));

    // Pins the sidecar naming convention the CLI resolves. Mere existence
    // proves nothing about persistence — the file is created on demand by
    // any cache call, a plain read included — so this is a path assertion
    // and nothing more.
    assert!(
        s.catalog
            .parent()
            .unwrap()
            .join("cat.p.scancache.sqlite")
            .exists(),
        "the sidecar is not where the CLI resolves it"
    );
}
```

- [ ] **Step 4: Run the whole ported suite**

```bash
DISKREMENT_SCENARIOS_REQUIRED=1 cargo test -p diskrement-cli --test scenarios -- --nocapture
```

Expected: 16 passed, 0 failed, no `SKIPPED` line. That is 4 harness tests plus
the 12 test cases of the 11 ported scenarios (scenario 9 contributes two).

- [ ] **Step 5: Full verification**

```bash
cargo test --workspace 2>&1 | tail -20
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

Expected: the Step 1 Task 1 baseline plus 16, still 0 failed.

- [ ] **Step 6: Commit**

```bash
git add crates/diskrement-cli/tests/
git commit -m "test(cli): port integration scenarios 9-10 — the coaster flow and the warm cache"
```

---

## Task 5: The perf comparison

Spec §1 asks whether the Rust binary is at least as fast as the Scala one.
Both still exist here, so this is a same-host A/B against one regenerated
tree rather than a comparison with numbers taken on a different day under a
different page-cache state.

**Files:**
- Modify: `scripts/perf-bench.sh`
- Modify: `fixtures/perf-baseline.txt`
- Create: `fixtures/perf-rust.txt`

- [ ] **Step 1: Teach the sampler about a non-JVM binary**

`resolve_jvm_pid` matches the process's cmdline against `diskrement.cli.Main`.
For the Rust binary that never matches, `jvm_pid` stays empty, the sampler
never records, and every I/O column reads 0.0 GB — a wrong number, not a
missing one.

In `scripts/perf-bench.sh`, replace the whole `resolve_jvm_pid` function
(currently at lines 185–199, ending at the `}` before the `# sample_io`
comment) with:

```bash
# Identify the process to sample among $1 (the launcher's pid) and its
# process tree. Two shapes: the Scala launcher execs java, so root_pid
# usually *is* the JVM and its cmdline names the main class; the Rust
# binary is root_pid itself and its cmdline names $BIN. Match either.
resolve_target_pid() {
  local root_pid="$1" cmdline
  if [[ -r "/proc/$root_pid/cmdline" ]]; then
    cmdline="$(tr '\0' ' ' < "/proc/$root_pid/cmdline" 2>/dev/null)" || cmdline=""
    if grep -qF -e 'diskrement.cli.Main' -e "$BIN" <<< "$cmdline"; then
      echo "$root_pid"
      return 0
    fi
  fi
  local found
  found="$(pgrep -f 'diskrement\.cli\.Main' 2>/dev/null | head -n1)" || true
  if [[ -n "$found" ]]; then
    echo "$found"
    return 0
  fi
  return 1
}
```

Then, in `sample_io`, rename both the variable and the call: `local pid="$1"
outfile="$2" target_pid="" snap`, `if [[ -z "$target_pid" ]]; then
target_pid="$(resolve_target_pid "$pid" 2>/dev/null)" || target_pid=""; fi`,
and `snap="$(get_io_snapshot "$target_pid")"`.

- [ ] **Step 2: Default to the Rust binary, and record the toolchain**

In `scripts/perf-bench.sh`, change the default binary and its error hint
(lines 54 and 64):

```bash
DEFAULT_BIN="$SCRIPT_DIR/../target/release/diskrement"
```

```bash
  echo "error: BIN '$BIN_ARG' is not an executable file (did you run 'cargo build --release -p diskrement-cli'?)" >&2
```

In the provenance block, after the `jvm:` line, add a `rustc:` line so a
recorded run says which toolchain produced the binary:

```bash
  if command -v rustc >/dev/null 2>&1; then
    rustc_ver="$(rustc --version 2>&1 | head -n1)"
  else
    rustc_ver="unknown (no 'rustc' on PATH)"
  fi
  echo "  rustc:        $rustc_ver"
```

Declare `rustc_ver` alongside `jvm_ver` in that function's `local` list.

- [ ] **Step 3: Check the script still parses**

```bash
bash -n scripts/perf-bench.sh
```

Expected: no output.

- [ ] **Step 4: Regenerate the benchmark tree**

The recipe is recorded in `fixtures/perf-baseline.txt` and is idempotent in
structure; the content is random, and only sizes and structure affect the
timings.

```bash
rm -rf /tmp/diskrement-bench-src && mkdir -p /tmp/diskrement-bench-src
for i in $(seq 0 1999); do
  d=$(( i % 8 )); sz=$(( 512 + (i % 1024) ))
  mkdir -p /tmp/diskrement-bench-src/dir$d
  head -c $(( sz * 1024 )) /dev/urandom > /tmp/diskrement-bench-src/dir$d/file$i.bin
done
mkdir -p /tmp/diskrement-bench-work
du -sh /tmp/diskrement-bench-src
```

Expected: about 2.0 GB across 2000 files. If `/tmp` is not on a CoW
filesystem on this machine, use a directory that is and record the paths in
the output file — `init` refuses anything else.

- [ ] **Step 5: Build both binaries**

```bash
cargo build --release -p diskrement-cli
sbt cli/stage
ls -l target/release/diskrement cli/target/universal/stage/bin/diskrement
```

- [ ] **Step 6: Run the A/B, Rust first, then Scala, then Rust again**

Three runs, not two. The first Rust run warms the page cache for the Scala
run that follows it, so a second Rust run at the end shows how much of any
difference is cache order rather than the binary. `DROP_CACHES` stays unset,
matching the P0 baseline, which was also taken warm.

```bash
scripts/perf-bench.sh /tmp/diskrement-bench-src /tmp/diskrement-bench-work \
  target/release/diskrement 2>&1 | tee /tmp/p10-rust-1.txt
scripts/perf-bench.sh /tmp/diskrement-bench-src /tmp/diskrement-bench-work \
  cli/target/universal/stage/bin/diskrement 2>&1 | tee /tmp/p10-scala.txt
scripts/perf-bench.sh /tmp/diskrement-bench-src /tmp/diskrement-bench-work \
  target/release/diskrement 2>&1 | tee /tmp/p10-rust-2.txt
```

Each run must end with `exit=0` on all five steps and the cache-evidence
block must show scan2 reusing 2000 fingerprints. A run whose I/O columns are
all `0.000 GB` means Step 1 did not take effect — fix it before recording
anything.

- [ ] **Step 7: Record the measurement**

Create `fixtures/perf-rust.txt`, header first, then the three runs
concatenated verbatim:

```
# P10 A/B: the Rust binary against the Scala binary, same host, same tree,
# same page-cache state, in one sitting. Compare with fixtures/perf-baseline.txt
# (the P0 Scala record) only as a drift check: that run was taken on a
# different day, and the caveat below applies to it too.
#
# date: <fill in, ISO-8601 UTC>
# commit: <fill in, `git rev-parse --short HEAD`>
# tree: the same synthetic 2000-file / ~2.07 GB recipe recorded in
#   fixtures/perf-baseline.txt, regenerated for this run.
# order: rust, scala, rust — the third run shows how much of the difference
#   is page-cache order rather than the binary.
# caveat: 1–3 s per step. Wall times are the measurement; the per-step I/O
#   columns are approximate, because the 0.2 s sampler cannot see I/O after
#   its last poll (see the notes in scripts/perf-bench.sh).
#
# summary: <one line per step: rust vs scala wall time, and the verdict>
```

Then append the three captured files. Write the `summary` line only after
reading the numbers: say plainly whether Rust is faster, slower, or level per
step, and do not round a regression away.

- [ ] **Step 8: Fix the stale reference in the baseline**

`fixtures/perf-baseline.txt` line 1 says the comparison happens in P9. Change
`(spec §1, compared in P9)` to `(spec §1, compared in P10 — see perf-rust.txt)`,
and in the header comment change `P9 must re-run against this same pair of
paths` to `P10 must re-run against this same pair of paths`.

- [ ] **Step 9: Refresh the integrity manifest**

`fixtures/SHA256SUMS` pins every committed fixture file, and this task added
one and edited another. Regenerate it the way `scripts/harvest-fixtures.sh`
does (that script is still present until Task 9, but do not run the whole
harvest — only its manifest step):

```bash
sums=$( cd fixtures && find . -type f ! -name SHA256SUMS -print0 \
    | sort -z | xargs -0 sha256sum )
printf '%s\n' "$sums" > fixtures/SHA256SUMS
git diff --stat fixtures/SHA256SUMS
```

Expected: the `perf-baseline.txt` line changes and a `perf-rust.txt` line
appears. Nothing else may move — a changed sum on a catalog, disc or source
file means something dirtied the corpus, which is a bug to chase, not a sum to
re-record.

- [ ] **Step 10: Commit**

```bash
git add scripts/perf-bench.sh fixtures/perf-baseline.txt fixtures/perf-rust.txt fixtures/SHA256SUMS
git commit -m "perf: A/B the Rust binary against the Scala one on one host and one tree"
```

---

## Task 6: The full differential run and the chunked wide sweep

The last time both binaries can be compared. Everything the harness can prove
must be proven here, because Task 9 deletes it.

**Files:**
- Modify (only if the sweep finds something): `crates/diskrement-difftest/src/corpus.rs`, plus whatever Rust source the counterexample indicts.

- [ ] **Step 1: Point the harness at both binaries**

```bash
sbt cli/stage
export DISKREMENT_SCALA_BIN="$PWD/cli/target/universal/stage/bin/diskrement"
cargo build --release -p diskrement-cli
export DISKREMENT_RUST_BIN="$PWD/target/release/diskrement"
```

- [ ] **Step 2: Confirm every command is claimed as implemented**

```bash
grep -n "RUST_IMPLEMENTED" -A 20 crates/diskrement-difftest/src/runner.rs | head -30
```

Every command the CLI ships — `init`, `scan`, `status`, `plan`, `burn`,
`verify`, `history`, `reset`, `disc mark-dead` — must appear. A command
missing from that list makes the Rust and interleaved modes skip its
scenarios, and a fully-skipped run still exits green. If one is missing, add
it and commit that with `feat(difftest):`.

- [ ] **Step 3: Run the whole corpus in all three modes**

```bash
cargo test -p diskrement-difftest --test differential -- --ignored --nocapture 2>&1 | tee /tmp/p10-corpus.txt
grep -E "scenarios|skip" /tmp/p10-corpus.txt | tail -20
```

Expected: 13 scenarios × 3 modes, **0 skipped**. A skip is a result to
investigate, not a pass: read the printed reason. `real_backend` needs
`xorriso` — install it rather than accepting the skip, since this is the last
chance to compare that path.

- [ ] **Step 4: Run the narrow sweep once, to check the machinery**

```bash
cargo test -p diskrement-difftest --test sweep -- --ignored --nocapture 2>&1 | tail -30
```

Expected: green, with a coverage line naming the commands it exercised and a
non-zero successful-burn count.

- [ ] **Step 5: The wide sweep, in five chunks**

Failure persistence is off, so each invocation draws fresh seeds and the five
chunks are ≥1000 distinct sequences. Chunking is what makes this survivable
on this host: P9's three attempts at a single large run were all killed by
the memory guard, never by a failure, and a kill now costs one chunk.

```bash
for i in 1 2 3 4 5; do
  echo "=== chunk $i ==="
  PROPTEST_CASES=200 cargo test -p diskrement-difftest --test sweep -- \
    --ignored --nocapture 2>&1 | tee "/tmp/p10-sweep-$i.txt" | tail -12
done
```

Between chunks, check that nothing is accumulating:

```bash
ls -d /tmp/.tmp* 2>/dev/null | wc -l
```

A killed chunk leaves its scratch tree behind; remove the strays and re-run
that chunk. If a chunk is killed twice in a row, drop to
`PROPTEST_CASES=100` and run ten chunks instead — the bar is the cumulative
count, not the chunk size.

- [ ] **Step 6: Read the coverage lines, not the exit statuses**

```bash
grep -h -A 6 "coverage" /tmp/p10-sweep-*.txt
```

Record, per chunk: cases run, commands exercised, successful burns, dead
discs. The bar is **≥1000 cases cumulatively, with burns reached in every
chunk**. A chunk that reached no successful burn has proved very little and
does not count toward the total — re-run it.

- [ ] **Step 7: Triage any counterexample**

If a chunk fails, the printed value is paste-ready `corpus.rs` source.

1. Paste the printed `Scenario` into `crates/diskrement-difftest/src/corpus.rs`
   under a descriptive name and add it to `corpus::all()`.
2. Run just that scenario in all three modes and confirm it reproduces.
3. Decide what it is. A **porting bug** is fixed in the Rust tree, with the
   scenario kept as its regression test. A **harness defect** is fixed in the
   harness — and never by widening a normalization rule to make the diff pass;
   each rule is a hole in the oracle and widening one is how this harness
   stops working without anyone noticing.
4. Re-run the full corpus and restart the chunk count from the fix.

There is no regressions file, on purpose: `corpus::all()` is the promotion
path, and it is reviewable.

- [ ] **Step 8: Record the outcome in this plan**

Append an "Execution deviations" entry with the cumulative case count, the
per-chunk coverage figures, and either the counterexamples found and what
each turned out to be, or an explicit statement that none were found. §9's
bar is satisfied vacuously when the sweep finds nothing — record it as such
rather than leaving it blank.

- [ ] **Step 9: Commit anything the triage changed**

```bash
git status --porcelain
# if corpus.rs or any Rust source changed:
git add -A && git commit -m "fix(<scope>): <what the sweep found>"
```

If nothing changed, there is nothing to commit — the evidence lives in this
plan document, which Task 8 commits.

---

## Task 7: Rewrite the real-media checklist for the Rust binary

`docs/smoke-test-real-media.md` still offers the sbt launcher as a fallback
and is titled after a Scala-era plan. It is rewritten *before* the run in
Task 8 so that the run also checks the rewritten checklist.

**Files:**
- Modify: `docs/smoke-test-real-media.md`

- [ ] **Step 1: Retitle and replace the preamble**

Replace lines 1–10 (the title and the "Since the Rust rewrite's P8"
paragraph) with:

```markdown
# Real-media smoke test

Run this with the Rust binary: `cargo build --release -p diskrement-cli`, then
`target/release/diskrement`. Item 5 (Tier-2 verification) and item 2 (udisks
mount) are the two the automated image-backed suite cannot reach at all —
`tests/xorriso_image.rs` covers osirrox extraction, but never a mount.
```

- [ ] **Step 2: Replace the closing line**

Line 45 reads `Findings feed parser fixes; re-run `sbt shell/test` after any
parser change.` Replace it with:

```markdown
Findings feed parser fixes; re-run `cargo test -p diskrement-shell` after any
parser change.
```

- [ ] **Step 3: Reset the checkboxes that were ticked against the Scala binary**

Items 6 and 8 are `[x]`, captured on 2026-08-30 against the Scala
implementation. Their *recorded findings* stay — they are what the parser was
built against — but the boxes go back to `[ ]`, because nothing has yet
confirmed those observations against the Rust binary. Keep the parenthetical
notes, adding "(Scala, 2026-08-30 — re-confirm with the Rust binary)" to each.
Item 8's reference to `MediaParse.parse` and `classifyFailure` should name the
Rust module instead: `burn::media`'s parser and `burn::cmd::classify_failure`.

- [ ] **Step 4: Read the file end to end**

```bash
cat docs/smoke-test-real-media.md
```

No `sbt`, no `Scala launcher`, no `cli/target/universal`. The Pioneer/`/dev/sr0`
hardware notes, the optical-group prereq and the unformatted-BD-R warning all
stay — they are about the drive, not the implementation.

- [ ] **Step 5: Commit**

```bash
git add docs/smoke-test-real-media.md
git commit -m "docs: the real-media checklist targets the Rust binary"
```

---

## Task 8: The real-media smoke run — **this task needs the user at the drive**

Every preceding task is agent-actionable; this one is not. It requires
inserting, swapping and removing physical discs. Do not attempt to work
around it, and do not treat a skip as a pass.

**Files:**
- Modify: `docs/smoke-test-real-media.md` (the checkboxes and captured strings)
- Modify: `docs/superpowers/plans/2026-09-10-rust-rewrite-p10-parity-cutover.md` (the record)

- [ ] **Step 1: Prepare and hand over**

```bash
cargo build --release -p diskrement-cli
groups | tr ' ' '\n' | grep -x optical || echo "NOT in the optical group — item 0 applies"
ls -l /dev/sr0
```

Then tell the user, in one message: the binary is at
`target/release/diskrement`, the checklist is `docs/smoke-test-real-media.md`,
the media requirement is one expendable unformatted BD-R (or a BD-RE for
repeatable drills), and which items need a disc swap (4) versus a fresh disc.

- [ ] **Step 2: Walk the checklist with the user**

Work item by item. For each, run the command, show the output, and let the
user confirm what the drive did. Tick the box and record any exact string the
item asks for.

- [ ] **Step 3: Record the result honestly**

If the run happens: tick the boxes, paste the captured strings, and note the
date and the binary's commit in the file.

If it cannot happen — no drive, no media, no time — record it as **not
performed**, and name what stays unproven: sealing (`-close on` has no
observable effect on `stdio:` images), capacity refusal (free space on an
image file is a fake huge number), the real device open path, the udisks
mount in item 2, and Tier-2 verification in item 5. Everything else is
already covered by `tests/xorriso_image.rs` and the differential
`real_backend` scenario. A skip is a stated gap, never a silent one.

- [ ] **Step 4: Commit the record**

```bash
git add docs/smoke-test-real-media.md docs/superpowers/plans/2026-09-10-rust-rewrite-p10-parity-cutover.md
git commit -m "docs: record the P10 real-media smoke run"
```

---

## GATE 1 — the parity review

**Stop here. Do not proceed to Task 9 without the user's explicit
confirmation.**

Present, in one message:

1. `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D
   warnings`, `cargo fmt --check` — the actual output tails, with the test
   count next to the Task 1 baseline.
2. The scenario suite's count from a `DISKREMENT_SCENARIOS_REQUIRED=1` run,
   stated as "ran", not "passed".
3. The differential corpus result: scenarios × modes, and the number skipped.
4. The sweep: cumulative cases, per-chunk coverage, burns reached, and what
   was found.
5. The perf A/B: per-step wall times for both binaries, and the verdict.
6. The smoke run: performed and what it showed, or not performed and what
   stays unproven.

Then ask whether to proceed with the cutover. The next task deletes the only
implementation that any of this evidence was compared against.

---

## Task 9: Delete the Scala tree, the harness and the harvest script

Only after Gate 1. `fixtures/` stays — it is the Scala-written compatibility
floor that nine shell suites read on every `cargo test`, and it cannot be
regenerated once this task lands. That is the point of the `scala` branch.

**Files:**
- Delete: `core/`, `shell/`, `cli/`, `build.sbt`, `project/`, `scripts/harvest-fixtures.sh`, `crates/diskrement-difftest/`
- Modify: `Cargo.toml`, `.gitignore`, `fixtures/README.md`, `fixtures/SHA256SUMS`

- [ ] **Step 1: Confirm nothing in the Rust tree depends on what is going**

```bash
grep -rn "difftest" --include="*.rs" --include="*.toml" crates/diskrement-core crates/diskrement-shell crates/diskrement-cli Cargo.toml
grep -rn "DISKREMENT_SCALA_BIN" --include="*.rs" crates/diskrement-core crates/diskrement-shell crates/diskrement-cli
```

Expected: the only hit is the workspace-member line in `Cargo.toml`. Anything
else must be resolved before deleting, not after.

- [ ] **Step 2: Delete**

```bash
git rm -r --quiet core shell cli project crates/diskrement-difftest
git rm --quiet build.sbt scripts/harvest-fixtures.sh
```

- [ ] **Step 3: Drop the workspace member**

In `Cargo.toml`, remove both the comment and the member line:

```toml
    # Dev-only differential harness (spec §8.1). Deleted at P10 with the Scala tree.
    "crates/diskrement-difftest",
```

leaving:

```toml
[workspace]
resolver = "2"
members = [
    "crates/diskrement-core",
    "crates/diskrement-shell",
    "crates/diskrement-cli",
]
```

`proptest` stays in `[workspace.dependencies]`: the core's property suites use
it.

- [ ] **Step 4: Drop the Scala tooling's ignore entries**

In `.gitignore`, remove `.bsp/`, `.metals/` and `.bloop/` — Metals and BSP
artifacts of a build that no longer exists. Keep `.superpowers/`, `target/`
and `*.snap.new`.

- [ ] **Step 5: Say where the harvest script went**

In `fixtures/README.md`, replace the second line:

```markdown
Harvested from the **Scala** implementation by `scripts/harvest-fixtures.sh`.
```

with:

```markdown
Harvested from the **Scala** implementation by `scripts/harvest-fixtures.sh`,
which lived in this repository until P10 and now exists only on the `scala`
branch, together with the sbt tree it drives. This corpus cannot be
regenerated from `main`; treat it as frozen evidence, and `git checkout scala`
if you ever need to re-derive it.
```

In the same file's table, the `perf-baseline.txt` row says "P9 compares
against it". Change it to "the P0 Scala baseline; `perf-rust.txt` is the P10
A/B against it", and add a row for `perf-rust.txt`:

```markdown
| `perf-rust.txt` | the P10 same-host A/B: Rust and Scala on one tree | no (timings) |
```

- [ ] **Step 6: Regenerate the integrity manifest**

Task 5 already refreshed this after its own edits; `fixtures/README.md`
changed in Step 5, so refresh it again. Regenerate the sums the way the
(now deleted) harvest script did:

```bash
sums=$( cd fixtures && find . -type f ! -name SHA256SUMS -print0 \
    | sort -z | xargs -0 sha256sum )
printf '%s\n' "$sums" > fixtures/SHA256SUMS
git diff --stat fixtures/SHA256SUMS
```

Expected: only the `README.md` line changes.

- [ ] **Step 7: Verify the workspace still builds and passes**

```bash
cargo test --workspace 2>&1 | tail -20
DISKREMENT_SCENARIOS_REQUIRED=1 cargo test -p diskrement-cli --test scenarios -- --nocapture 2>&1 | tail -5
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

Expected: the Task 4 count **minus the difftest crate's own unit tests**, which
went with it. Compute the expected drop before running — `cargo test -p
diskrement-difftest --lib` counted them, and the P9 landing recorded 4 ignored
tests that all belonged to the harness — and state it as an explained drop
rather than discovering it afterwards. The nine fixture suites in
`crates/diskrement-shell/tests/` must still pass: they are the proof that
deleting the Scala tree did not delete the compatibility evidence.

- [ ] **Step 8: Confirm the tree is clean of the old build**

```bash
ls
grep -rln "sbt\|scala-3\|zio" --include="*.rs" --include="*.toml" --include="*.sh" . | grep -v "^./docs/" | grep -v "^./target/"
```

Expected listing: `Cargo.lock  Cargo.toml  CLAUDE.md  crates  docs  fixtures
LICENSE  README.md  rust-toolchain.toml  scripts  target`. The grep should
return nothing outside `docs/` — the plan and spec documents keep their
Scala references, because they are the history of how this was done.

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "chore: delete the Scala tree, the differential harness and the harvest script"
```

---

## Task 10: Rewrite README.md and CLAUDE.md for the Rust build

After the deletion, so both documents describe what is actually in the tree.

**Files:**
- Modify: `README.md`
- Modify: `CLAUDE.md`

- [ ] **Step 1: Rewrite `README.md`'s implementation-specific sections**

Four places name the old stack. Keep everything else — the problem statement,
what it does, the status narrative and the license are all
implementation-independent.

`## Design`, the module list around line 57:

```markdown
- **`crates/diskrement-core/`** — pure Rust with no I/O. `diff` computes a change
  set between the last burned state and a fresh scan; `planner` packs it onto
  discs; `manifest` builds what gets written. One dependency, `chrono` with
  `default-features = false`, which excludes the `clock` feature — `Utc::now()`
  does not compile here, and a test proves it.
- **`crates/diskrement-shell/`** — every effect: the SQLite catalog, the
  scanner and hasher, the CoW snapshot, staging, and the burners.
- **`crates/diskrement-cli/`** — the `diskrement` binary: argument parsing,
  path resolution, rendering and the exit-code contract.
```

The **Tech stack** line (line 73):

```markdown
**Tech stack:** Rust (stable, edition 2021), `clap` 4, `rusqlite` (bundled
SQLite), `serde`/`serde_json`, `sha2`, `rayon`, `walkdir`, `reflink-copy`.
```

`## Building` requirements (line 202):

```markdown
**Requirements:** a stable Rust toolchain (pinned by `rust-toolchain.toml`),
Linux, and a CoW filesystem (btrfs, XFS with reflink, or bcachefs) for the
work area. `rusqlite`'s bundled SQLite is compiled from source, so a C
compiler is needed for the first build.
```

and the command block:

```markdown
```
cargo build --release          # -> target/release/diskrement
cargo test --workspace         # all tests
cargo run -p diskrement-cli -- scan photos
```
```

`### Producing an executable` (lines 214–237): replace the `sbt cli/stage` and
`sbt cli/universal:packageBin` instructions with `cargo build --release`,
naming `target/release/diskrement` as the single self-contained binary, and
drop the zip-distribution paragraph — a Rust binary needs no launcher script
and no JVM on the target machine, which is worth saying in one sentence.

- [ ] **Step 2: Rewrite `CLAUDE.md`**

This is the file that governs every future session in this repository, so it
gets a real rewrite rather than a search-and-replace.

- **Build & test.** Replace the sbt block with:

  ```
  cargo test --workspace                       # all tests
  cargo test -p diskrement-core                # pure core, incl. proptest suites
  cargo test -p diskrement-shell               # catalog, fs, burn; reads fixtures/
  cargo test -p diskrement-cli                 # CLI contract + integration scenarios
  cargo clippy --workspace --all-targets -- -D warnings
  cargo fmt --check
  cargo build --release                        # -> target/release/diskrement
  ./scripts/perf-bench.sh SRC WORK             # DROP_CACHES=1 for cold cache
  ```

  Single test: `cargo test -p diskrement-shell catalog::store`. Warnings are
  failures via `.cargo/config.toml`, so `-Wunused:all`'s "by convention" note
  goes away — it is enforced now.

- **What "verified" means.** `cargo test --workspace`, `cargo clippy
  --workspace --all-targets -- -D warnings` and `cargo fmt --check`, all
  clean, with no unexplained drop in test count. Replace the sbt IT caveat
  with the scenario suite's: the ported integration scenarios in
  `crates/diskrement-cli/tests/scenarios/` need a CoW `TMPDIR` and print
  `SKIPPED` without one; `DISKREMENT_SCENARIOS_REQUIRED=1` turns that into a
  failure, and any run used as evidence must set it. `tests/xorriso_image.rs`
  still skips when `xorriso` is absent.

- **Gotchas that will bite.** Delete every Scala/ZIO/zio-jdbc entry — forked
  tests, `setAutoCommit`, `Db.poolLayer`'s live clock, `wrapConn`,
  for-comprehension destructuring, `Files` statics under sbt's classloader,
  `finalize`. Promote the Rust gotchas that currently live under "Rust rewrite
  gotchas (P2+)" into this section verbatim, dropping that heading and its
  "these apply to `crates/` only" preamble, which is no longer true of
  anything.

- **The differential harness (P3+).** Delete the whole section. Add one
  sentence to the `fixtures/` description instead: the harness that used to
  compare against the Scala binary was deleted with it at P10, and `fixtures/`
  is now a frozen corpus that cannot be regenerated from `main`.

- **Environment, invariants, deferred debt.** Unchanged — every one of them is
  a property of the design, not of the language. Check each line for a path or
  a filename that moved: the exit-code contract now lives at
  `crates/diskrement-cli/src/render.rs`, and `ExitMappingSpec`'s assertions are
  now in that file's tests and in `crates/diskrement-cli/tests/binary.rs`.

- **Repo etiquette.** Delete the "While the Rust rewrite is in flight" bullet —
  it stops being true at Task 12. The `feat/<topic>` off `main` convention and
  the conventional-commit scopes stay.

- **In-flight work.** Unchanged: `docs/superpowers/specs/` and
  `docs/superpowers/plans/` remain the source of truth.

- [ ] **Step 3: Check both documents against the tree**

```bash
grep -n "sbt\|Scala\|scala\|ZIO\|zio\|JVM\|JDK" README.md CLAUDE.md
```

Every remaining hit must be deliberate — the `scala` preservation branch, and
the history sentence in the fixtures note. Anything describing how to build or
test this repository today is a defect.

Then run every command the two documents now claim works:

```bash
cargo test --workspace 2>&1 | tail -5
cargo build --release && ls -l target/release/diskrement
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

- [ ] **Step 4: Commit**

```bash
git add README.md CLAUDE.md
git commit -m "docs: rewrite README and CLAUDE.md for the Rust build"
```

---

## Task 11: Land the phase branch

**Files:** none — git only.

- [ ] **Step 1: Final verification on the phase branch**

```bash
cargo test --workspace 2>&1 | tail -20
DISKREMENT_SCENARIOS_REQUIRED=1 cargo test -p diskrement-cli --test scenarios -- --nocapture 2>&1 | tail -5
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
git status --porcelain
```

All clean; the working tree empty.

- [ ] **Step 2: Append the execution record to this plan**

Fill in the "Execution deviations" and "Execution verification" sections at
the bottom of this document: the measured counts, the sweep figures, the perf
verdict, the smoke-run outcome, and anything that went differently from what
is written above. Deviations belong here, not in commit messages.

```bash
git add docs/superpowers/plans/2026-09-10-rust-rewrite-p10-parity-cutover.md
git commit -m "docs(plan): record the P10 execution"
```

- [ ] **Step 3: Merge into the integration branch**

```bash
git checkout feat/rust-rewrite
git merge --no-ff feat/rewrite-p10 -m "Merge feat/rewrite-p10: the parity gate and the cutover — the eleven Scala integration scenarios ported with an on-disc oracle, a same-host perf A/B, the full differential corpus and a chunked wide sweep, then the Scala tree, the differential harness and the harvest script deleted and the docs rewritten for the Rust build

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_019vNyFqvFhs9JPHi7phvauJ"
git branch -d feat/rewrite-p10
```

- [ ] **Step 4: Verify from a clean build on the integration branch**

```bash
cargo clean
cargo test --workspace 2>&1 | tail -20
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

---

## GATE 2 — the swap

**Stop here. Task 12 is the only task that touches `main` or `origin`, and
`main` is where the Scala implementation still lives. Do not proceed without
the user's explicit confirmation.**

Present: the clean-build result from Task 11 Step 4, and exactly what Task 12
will do — push `main`'s current tip to a new `scala` branch on `origin`, merge
`feat/rust-rewrite` into `main`, and delete `feat/rust-rewrite` locally and on
`origin`.

---

## Task 12: The branch swap

**Files:** none — git only.

- [ ] **Step 1: Preserve the Scala implementation first**

Before anything is merged. If this step fails, stop: nothing later is safe
without it.

```bash
git fetch origin
git rev-parse main origin/main       # must agree; if not, ask before continuing
git push origin main:refs/heads/scala
git ls-remote --heads origin scala
```

Expected: the remote now has `scala` at `main`'s current tip. Confirm the
Scala tree is really on it:

```bash
git ls-tree --name-only origin/scala | sort
```

Expected to include `build.sbt`, `cli`, `core`, `project`, `shell`, and
`scripts/harvest-fixtures.sh` under `git ls-tree --name-only origin/scala scripts/`.

- [ ] **Step 2: Merge the rewrite into `main`**

```bash
git checkout main
git merge --no-ff feat/rust-rewrite -m "Merge feat/rust-rewrite: diskrement is a Rust program

Ten phases: the golden fixture corpus and perf baseline harvested from the
Scala build, the pure core, the append-only catalog, the scanner and
fingerprint cache, the planner and staging, the burn pipeline, verify/history/
reset, the coaster workflow, the real xorriso backend, a randomized
differential sweep across both implementations, and the parity gate. The Scala
tree is preserved on the 'scala' branch; fixtures/ is what it wrote, and the
shell suites still read it on every build.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_019vNyFqvFhs9JPHi7phvauJ"
```

- [ ] **Step 3: Verify `main` from a clean build**

The last chance to catch a bad merge before it is pushed.

```bash
cargo clean
cargo test --workspace 2>&1 | tail -20
DISKREMENT_SCENARIOS_REQUIRED=1 cargo test -p diskrement-cli --test scenarios -- --nocapture 2>&1 | tail -5
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

- [ ] **Step 4: Push, then delete the integration branch**

```bash
git push origin main
git push origin --delete feat/rust-rewrite
git branch -d feat/rust-rewrite
git branch -a
```

Expected: `main` and `scala` on `origin`, no `feat/rust-rewrite` anywhere, no
`feat/rewrite-p*` anywhere.

- [ ] **Step 5: Tell the user it is done**

State plainly: `main` is Rust, `scala` preserves the previous implementation
at commit `<sha>`, and the fixture corpus on `main` is what that
implementation wrote.

---

## Phase exit bar (spec §9, P10 row)

The phase is done when all of these hold and each has output to show for it:

1. `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D
   warnings` and `cargo fmt --check` are clean on `main`, from a clean build,
   with any drop in test count explained (the difftest crate's own tests went
   with it).
2. All eleven Scala integration scenarios have a Rust counterpart, and the
   suite **ran** — proven by a `DISKREMENT_SCENARIOS_REQUIRED=1` run, not by
   an exit status.
3. The differential corpus passed 13 scenarios in all three modes with **0
   skipped**, recorded before the harness was deleted.
4. The wide sweep reached **≥1000 cumulative generated sequences** agreeing
   across all three modes, with per-chunk coverage lines recorded and burns
   reached in every counted chunk; every counterexample either fixed or
   promoted to `corpus::all()`.
5. The perf A/B is recorded in `fixtures/perf-rust.txt` with a stated verdict.
6. The real-media smoke run is recorded as performed, or as not performed
   with what stays unproven named explicitly.
7. `core/`, `shell/`, `cli/`, `build.sbt`, `project/`,
   `scripts/harvest-fixtures.sh` and `crates/diskrement-difftest/` are gone
   from `main`; `fixtures/` is intact and still read by the shell suites.
8. `README.md` and `CLAUDE.md` describe the Rust build, and every command
   they claim works has been run.
9. `origin/scala` exists at the pre-merge `main`, `feat/rust-rewrite` is
   deleted locally and on `origin`.

---

## Spec coverage

| Spec | Where it lands |
|---|---|
| §9 P10 row: port the integration scenarios | Tasks 1–4 |
| §9 P10 row: perf comparison vs the P0 baseline | Task 5 |
| §9 P10 row: full differential sweep | Task 6 |
| §9 P10 row: real-media smoke run | Tasks 7–8 |
| §9 P10 row: docs rewrite | Tasks 7 and 10 |
| §9 P10 row: harness and Scala tree deleted | Task 9 |
| §9 P10 row: branch swap | Task 12 |
| §9 decision 1 (two gates, the first before any deletion) | Gate 1, Gate 2 |
| §9 decision 2 (the perf comparison is an A/B) | Task 5 Steps 4–7 |
| §9 decision 3 (the wide sweep is chunked) | Task 6 Steps 5–6 |
| §9 decision 4 (the scenarios port whole) | Tasks 2–4 |
| §9 decision 5 (the harvest script goes with the tree) | Task 9 Steps 2 and 5 |
| §7 (a skip must be loud) | `DISKREMENT_SCENARIOS_REQUIRED`, Task 1 Step 4 |
| §8 (every existing assertion gets a counterpart) | Tasks 1–4 |
| §8.1 (disposability; the harness dies with the tree) | Task 9 |
| §10 (branch strategy, preservation branch) | Tasks 11–12 |
| §11 (silent format drift) | `fixtures/` kept and still read; Task 9 Steps 5–7 |

**Not in scope.** `RunOutcome.scenario` and `Snapshot.scenario`, the write-only
fields P9 recorded as P10 deletion candidates, need no task: they are deleted
with `crates/diskrement-difftest/` in Task 9.

---

## Execution deviations

*(Append during execution. Anything done differently from the plan, and why,
goes here rather than in a commit message.)*

### P10 Task 6 — differential corpus and chunked wide sweep record (2026-09-10)

Run against the Task 5 binaries: `cli/target/universal/stage/bin/diskrement`
(Scala, re-staged) and `target/release/diskrement` (Rust, rebuilt); xorriso
1.5.8.pl02 present. Evidence: `/tmp/p10-corpus.txt`, `/tmp/p10-sweep-{1..5}.txt`.

**Corpus (Task 6 Step 3):** 13 scenarios × 3 modes, **0 skipped** —
`bootstrap: 13 reproduced identically`, `all-rust: 13 compared, 0 skipped`,
`interleaved: 13 compared, 0 skipped`. `real-backend` is one of the 13 in
`corpus::all()` and ran (the log prints totals only, not names). §9's
differential bar is satisfied vacuously: no counterexample was found.

**Narrow sweep (Step 4):** green, 12 cases (12 executed) — `burn 6 (2
refused)` ⇒ 4 burns at exit 0; every shipped command exercised. *(Figures are
from the live console output — Step 4's command had no `tee`, so this run is
not captured; the wide-sweep chunks below are.)*

**Wide sweep (Steps 5–6):** five chunks, `PROPTEST_CASES=200`, fresh seeds each
(failure persistence off). Coverage figures are from each chunk's `exercised:`
line; "burns ok" / "mark-dead ok" are commands with exit 0 in the reference
world (the coverage's own success definition). Every chunk reached successful
burns; all five count.

| Chunk | Cases | burn issued (refused / ok) | mark-dead issued (refused / ok) | Other commands issued | Stray `/tmp/.tmp*` (live) |
|---|---|---|---|---|---|
| 1 | 200 | 103 (37 / 66) | 52 (46 / 6) | init 208, plan 200, scan 210, status 81, verify 110, history 54, reset 79 | 410 (0 new) |
| 2 | 200 | 99 (34 / 65) | 48 (41 / 7) | init 204, plan 194, scan 207, status 91, verify 92, history 43, reset 73 | 410 (0 new) |
| 3 | 200 | 120 (44 / 76) | 44 (38 / 6) | init 203, plan 186, scan 210, status 86, verify 129, history 47, reset 81 | 410 (0 new) |
| 4 | 200 | 107 (39 / 68) | 52 (49 / 3) | init 204, plan 162, scan 204, status 75, verify 95, history 59, reset 86 | 410 (0 new) |
| 5 | 200 | 108 (43 / 65) | 48 (40 / 8) | init 205, plan 175, scan 207, status 75, verify 107, history 38, reset 83 | 410 (0 new) |
| **Total** | **1000** | **537 (197 / 340)** | **244 (214 / 30)** | | |

**Cumulative: 1000 generated cases, burns reached in 5/5 chunks (340 successful
burn commands, 30 successful `disc mark-dead` commands)** — the ≥1000-cases bar
is met with margin 0, exactly as planned (5 × 200).

Notes: the `/tmp/.tmp*` count was a stable 410 across all chunk boundaries —
no chunk was killed and nothing accumulated. These are live `ls` observations
taken between chunks (as the brief's Step 5 directs), not captured output —
the column above is the only record of them. The 410 are pre-existing strays
from Sep 6–8 and from that morning's Task 5 runs (dated 14:32–15:02, before
this task started at ~15:34), left untouched. No chunk needed the
`PROPTEST_CASES=100` fallback. **No counterexample was found** — the sweep and
corpus agreed across all three modes throughout, so Step 7 triage never
engaged and no commit was made for this task.

### P10 Task 7 — the smoke checklist's Rust paths (2026-09-10)

This plan's Task 7 Step 3 said item 8 should name "`burn::media`'s parser and
`burn::cmd::classify_failure`". `burn::cmd::classify_failure` does not exist —
the function lives at `crates/diskrement-shell/src/burn/xorriso/parse.rs:119`
(`pub fn classify_failure`), called from `burn/media.rs:68`. The checklist was
written with the real path (`burn::xorriso::parse::classify_failure`);
`burn::media` remains named as the parser's caller, per the plan's intent.

### P10 Task 8 — the real-media smoke run (2026-09-11)

Performed on the Pioneer BD-RW BDR-UD04 — at `/dev/sr1` on this machine, not
`/dev/sr0` as the checklist's hardware note said (`/dev/sr0` now hosts the
PiKVM virtual boot drive; the checklist was corrected). One blank unformatted
CMC BD-R, Rust binary at commit b81b342. Items 1–8 performed and passed with
the operator at the drive; every captured string is recorded inline in
`docs/smoke-test-real-media.md`. Highlights: the two-prompt burn (plan + insert),
auto-eject after every burn, 2- then 3-session TOC growth, Tier-2 verify proven
arithmetically (4 visible + 1 superseded = "5 files OK"), the finalize seal
reproducing `Media status : is written , is closed` verbatim, and the identity
check refusing a wrong disc, the sealed disc and the empty tray with exit 0.

Two items were not performed and say so in the file: item 0 (arranging a
process without the optical group needs root; the mapping is pinned by unit
tests) and item 9 (optional bit-rot drill; no second machine). What stays
unproven is narrower than the plan's worst case, because the automated suites
plus the difftest `real_backend` scenario already covered the sealing,
capacity-refusal and device-open paths — the run's unique evidence was the
udisks mount (item 2, exercised), the real eject behavior (item 1, confirmed
by the operator), and Tier-2 extraction from real media (item 5, confirmed).

Deviation from this plan's Task 7: the checklist's item 7 said "confirm
DiscClosed refusal", but `DiscClosed` was deliberately not ported to Rust
(`burn/mod.rs`: four Scala variants nothing constructed and nothing pinned).
The live equivalent was captured instead: the planner targets a new disc and
the identity check refuses the sealed disc ("found a finalized (closed)
disc"), which is item 7's intent. The checklist text was updated accordingly.

---

## Execution verification

*(Append at Task 11 Step 2: the measured test counts, the scenario suite's ran
count, the differential corpus result, the per-chunk sweep coverage, the perf
verdict, and the smoke-run outcome.)*
