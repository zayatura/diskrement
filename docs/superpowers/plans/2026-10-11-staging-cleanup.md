# Staging Cleanup After Real Burns Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** After a real-backend disc is burned and committed, delete its
`<work-dir>/staging/disc-<name>` tree; keep the simulated backend's trees (issue #3).

**Architecture:** `Burner::retains_staging()` says whether a backend's staging tree
outlives the commit. `Pipeline::burn_disc` calls a best-effort `discard_staging` right
after each disc's `apply_update`, and on the resume path for a disc an earlier run already
committed. The removal never returns an error. It is safe for twins because every disc is
staged from `work_dir/snapshot`, never from another disc's tree.

**Tech Stack:** Rust workspace (`diskrement-shell`), `tempfile`, scripted xorriso `Exec`
from `burn::testkit`.

**Spec:** `docs/superpowers/specs/2026-10-11-staging-cleanup-design.md`
**Branch:** `feat/staging-cleanup` (already created; the spec is committed on it)

---

## File map

| File | Change |
|---|---|
| `crates/diskrement-shell/src/burn/testkit.rs` | Gains `ScriptedExec`, `BLANK`, `APPENDABLE` (moved from the burner tests). |
| `crates/diskrement-shell/src/burn/xorriso/burner.rs` | Tests import the moved items instead of defining them. |
| `crates/diskrement-shell/src/burn/mod.rs` | `Burner::retains_staging` + unit test. |
| `crates/diskrement-shell/src/pipeline/mod.rs` | `discard_staging`, `Pipeline::staging_dir`, two call sites in `burn_disc`; real-backend tests in `burn_tests`. |
| `crates/diskrement-shell/src/pipeline/reports.rs` | Doc comment on `DiscBurn::staging_dir`. |
| `README.md`, `CLAUDE.md` | User note and gotcha. |

---

### Task 0: Baseline

- [ ] **Step 1: Record the current test count**

Run:
```bash
DISKREMENT_SCENARIOS_REQUIRED=1 cargo test --workspace 2>&1 | grep '^test result' | awk '{p+=$4; f+=$6; i+=$8} END {print "passed="p, "failed="f, "ignored="i}'
```
Expected: `failed=0`. Write the `passed=` number into the "Execution deviations" section
at the end of this plan as the baseline. `TMPDIR` must be on a CoW filesystem (btrfs here);
if the scenarios fail with "SKIPPED"/required, stop and report.

---

### Task 1: Move the scripted xorriso `Exec` into `testkit`

Pure refactor so pipeline tests can drive `Burner::Real`. No behaviour change.

**Files:**
- Modify: `crates/diskrement-shell/src/burn/testkit.rs`
- Modify: `crates/diskrement-shell/src/burn/xorriso/burner.rs` (test module only)

- [ ] **Step 1: Add the moved items to `testkit.rs`**

Extend the imports at the top of `testkit.rs`:

```rust
use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use diskrement_core::{ContentHash, RelPath, ScannedFile, SessionManifest};
use sha2::{Digest, Sha256};

use crate::burn::cmd::CmdOutput;
use crate::burn::{BurnError, BurnOperator, DiscIdentity, DiscImage, DiscMetadata};
```

(The first `use std::cell::RefCell;` already exists. Do not duplicate it.)

Append after `has_xorriso` (before `sha256`):

```rust
/// `-toc` output for a blank disc with room to spare.
pub const BLANK: &[&str] = &["Media status : is blank", "Media space  : 500000s"];

/// `-toc` output for a written disc that can take another session.
pub const APPENDABLE: &[&str] = &[
    "Media status : is written , is appendable",
    "Media summary: 1 session, 57 data blocks, 114k data, 23g free",
    "Media space  : 500000s",
];

/// A scripted `Exec` standing in for xorriso.
///
/// `toc` and `identities` are queues consumed per matching invocation,
/// which is how one script expresses "a wrong disc, then the right one"
/// and "the medium says session 1, the burned disc says session 2".
/// Callers wrap it as `|c, l| script.respond(c, l)`.
pub struct ScriptedExec {
    version: String,
    toc: RefCell<VecDeque<Vec<String>>>,
    identities: RefCell<VecDeque<Option<&'static str>>>,
    burn_exit: i32,
    burn_lines: Vec<String>,
    calls: RefCell<Vec<String>>,
}

impl Default for ScriptedExec {
    fn default() -> Self {
        Self::new()
    }
}

impl ScriptedExec {
    pub fn new() -> Self {
        ScriptedExec {
            version: "xorriso 1.5.8.pl02".to_string(),
            toc: RefCell::new(VecDeque::new()),
            identities: RefCell::new(VecDeque::new()),
            burn_exit: 0,
            burn_lines: vec!["xorriso : UPDATE : 100% done".to_string()],
            calls: RefCell::new(Vec::new()),
        }
    }

    pub fn toc(mut self, states: &[&[&str]]) -> Self {
        self.toc = RefCell::new(
            states
                .iter()
                .map(|s| s.iter().map(|l| (*l).to_string()).collect())
                .collect(),
        );
        self
    }

    pub fn identities(mut self, ids: &[Option<&'static str>]) -> Self {
        self.identities = RefCell::new(ids.iter().copied().collect());
        self
    }

    pub fn version(mut self, v: &str) -> Self {
        self.version = v.to_string();
        self
    }

    pub fn burn_fails(mut self) -> Self {
        self.burn_exit = 5;
        self.burn_lines = vec!["FAILURE : write error".to_string()];
        self
    }

    pub fn ran(&self, needle: &str) -> bool {
        self.calls.borrow().iter().any(|c| c.contains(needle))
    }

    pub fn respond(
        &self,
        cmd: &[String],
        on_line: &mut dyn FnMut(&str),
    ) -> Result<CmdOutput, BurnError> {
        self.calls.borrow_mut().push(cmd.join(" "));
        let has = |flag: &str| cmd.iter().any(|a| a == flag);
        if has("--version") {
            return Ok(CmdOutput {
                exit_code: 0,
                lines: vec![self.version.clone()],
            });
        }
        if has("-toc") {
            let lines = self
                .toc
                .borrow_mut()
                .pop_front()
                .expect("the script ran out of media states");
            return Ok(CmdOutput {
                exit_code: 0,
                lines,
            });
        }
        if has("-extract") {
            return match self.identities.borrow_mut().pop_front().flatten() {
                Some(json) => {
                    std::fs::write(cmd.last().expect("a destination"), json)
                        .expect("writable destination");
                    Ok(CmdOutput {
                        exit_code: 0,
                        lines: Vec::new(),
                    })
                }
                None => Ok(CmdOutput {
                    exit_code: 5,
                    lines: vec!["FAILURE : cannot find path".to_string()],
                }),
            };
        }
        if has("-commit") {
            for line in &self.burn_lines {
                on_line(line);
            }
            return Ok(CmdOutput {
                exit_code: self.burn_exit,
                lines: self.burn_lines.clone(),
            });
        }
        // -eject and anything else
        Ok(CmdOutput {
            exit_code: 0,
            lines: Vec::new(),
        })
    }
}
```

This is `Script::respond` from `burner.rs`, unchanged.

- [ ] **Step 2: Strip the originals from the burner tests**

In `crates/diskrement-shell/src/burn/xorriso/burner.rs`'s `mod tests`:
- delete `const BLANK`, `const APPENDABLE`, `struct Script` with its doc comment, and
  `impl Script`;
- delete the now-unused `use std::cell::RefCell;`, `use std::collections::VecDeque;`,
  `use crate::burn::cmd::CmdOutput;`. If the compiler says `CmdOutput` is still used by
  a later test, keep that one import;
- replace `use crate::burn::testkit::{image_of, ScriptedOperator};` with

```rust
    use crate::burn::testkit::{
        image_of, ScriptedExec as Script, ScriptedOperator, APPENDABLE, BLANK,
    };
```

Keep the `IDENTITY_*` constants where they are. Test bodies stay unchanged.

- [ ] **Step 3: Run the burner and testkit tests**

Run: `cargo test -p diskrement-shell burn::`
Expected: PASS, the same count as before the move.

- [ ] **Step 4: Lint**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`
Expected: clean. If `cargo fmt --check` complains, run `cargo fmt` and re-check.

- [ ] **Step 5: Commit**

```bash
git add crates/diskrement-shell/src/burn/testkit.rs crates/diskrement-shell/src/burn/xorriso/burner.rs
git commit -m "test(burn): move the scripted xorriso exec into testkit

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: `Burner::retains_staging`

**Files:**
- Modify: `crates/diskrement-shell/src/burn/mod.rs`

- [ ] **Step 1: Write the failing test**

Add to `mod tests` in `burn/mod.rs`:

```rust
    /// The simulated tree is the disc `StagingVerifier` reads back; a real
    /// disc carries its own record, so its tree is scratch space.
    #[test]
    fn only_the_simulated_backend_retains_staging() {
        use crate::burn::cmd::CmdOutput;
        use crate::burn::testkit::ScriptedOperator;
        use crate::progress::NoopSink;

        let exec = |_: &[String], _: &mut dyn FnMut(&str)| -> Result<CmdOutput, BurnError> {
            unreachable!("asking the policy runs nothing")
        };
        let op = ScriptedOperator::new(&[]);
        let real = Burner::Real(xorriso::burner::XorrisoBurner::new(
            BurnDevice::RealDevice("/dev/sr0".to_string()),
            &exec,
            &op,
            &NoopSink,
            1,
        ));
        let simulated = Burner::Simulated(simulated::SimulatedBurner::new(&NoopSink, 1));

        assert!(simulated.retains_staging());
        assert!(!real.retains_staging());
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p diskrement-shell burn::tests::only_the_simulated_backend_retains_staging`
Expected: compile error, `no method named retains_staging`.

- [ ] **Step 3: Implement**

In `impl Burner<'_>` in `burn/mod.rs`, after `burn`:

```rust
    /// Whether a disc's staging tree must outlive its commit. The simulated
    /// backend's tree *is* the disc `StagingVerifier` reads back; a real disc
    /// carries its own record, so its tree is only scratch space, and a kept
    /// tree pins source extents the source has since deleted (issue #3).
    pub fn retains_staging(&self) -> bool {
        matches!(self, Burner::Simulated(_))
    }
```

- [ ] **Step 4: Run it to verify it passes**

Run: `cargo test -p diskrement-shell burn::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/diskrement-shell/src/burn/mod.rs
git commit -m "feat(burn): say which backends keep staging after a commit

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Remove a real disc's staging tree after its commit

**Files:**
- Modify: `crates/diskrement-shell/src/pipeline/mod.rs` (`burn_disc`, a new free function, `mod burn_tests`)

- [ ] **Step 1: Add the real-backend test harness to `mod burn_tests`**

Extend `mod burn_tests`'s imports:

```rust
    use crate::burn::testkit::{ScriptedExec, ScriptedOperator, BLANK};
    use crate::burn::xorriso::burner::XorrisoBurner;
    use crate::burn::{BurnDevice, BurnError, BurnOperator};
```

(`Burner` and `simulated::SimulatedBurner` are already imported. Merge the
`crate::burn` lines if `cargo fmt` prefers that.)

Add after `run_burn`:

```rust
    /// `project.json` as the scripted drive reads it back after each burn.
    /// Only project, disc and session are compared, not `createdAt`.
    const ID_1: &str = r#"{"projectName":"p","discOrdinal":1,"sessionOrdinal":1,"createdAt":"2026-08-30T10:00:00Z"}"#;
    const ID_1A: &str = r#"{"projectName":"p","discOrdinal":1,"copySet":0,"copies":2,"sessionOrdinal":1,"createdAt":"2026-08-30T10:00:00Z"}"#;
    const ID_1B: &str = r#"{"projectName":"p","discOrdinal":1,"copySet":1,"copies":2,"sessionOrdinal":1,"createdAt":"2026-08-30T10:00:00Z"}"#;

    /// `run_burn` with the real backend, xorriso scripted by `script`.
    fn run_real_burn(
        env: &mut Env,
        script: &ScriptedExec,
        operator: &dyn BurnOperator,
    ) -> Result<BurnReport, PipelineError> {
        let sink = NoopSink;
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
        let burner = Burner::Real(XorrisoBurner::new(
            BurnDevice::RealDevice("/dev/sr0".to_string()),
            &exec,
            operator,
            &sink,
            2,
        ));
        let mut pipeline = Pipeline::new(
            &mut env.catalog,
            PipelinePaths {
                work_dir: env.work.clone(),
                scan_cache: env.scan_cache.clone(),
            },
            &sink,
        );
        pipeline.burn("p", now(), &burner)
    }

    /// Confirms every insert, recording whether `probe` exists at each prompt.
    struct ProbeOperator {
        probe: PathBuf,
        seen: std::cell::RefCell<Vec<bool>>,
    }

    impl BurnOperator for ProbeOperator {
        fn confirm_insert(&self, _message: &str) -> bool {
            self.seen.borrow_mut().push(self.probe.exists());
            true
        }

        fn progress(&self, _line: &str) {}
    }

    fn burned_sessions(env: &Env) -> usize {
        env.catalog
            .sessions_by_state(ProjectId(1), SessionState::Burned)
            .unwrap()
            .len()
    }
```

- [ ] **Step 2: Write the failing tests**

Add to `mod burn_tests`:

```rust
    #[test]
    fn a_real_burn_removes_the_staging_tree_once_the_disc_commits() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], false);
        let script = ScriptedExec::new().toc(&[BLANK]).identities(&[Some(ID_1)]);
        let op = ScriptedOperator::new(&[true]);

        let report = run_real_burn(&mut env, &script, &op).unwrap();

        assert_eq!(report.discs.len(), 1);
        assert!(script.ran("-commit"), "the disc was written");
        assert_eq!(burned_sessions(&env), 1);
        assert!(
            !env.work.join("staging/disc-1").exists(),
            "the committed disc's tree is gone"
        );
    }

    /// Twins are staged from the snapshot, never from each other: 1A's tree
    /// is already gone when the operator is asked for 1B, and 1B still stages
    /// (the stager re-hashes every file) and commits.
    #[test]
    fn a_two_copy_real_burn_removes_each_tree_before_the_next_disc() {
        let mut env = staged_env_with(&[("a.bin", b"aaaa")], false, 2);
        let script = ScriptedExec::new()
            .toc(&[BLANK, BLANK])
            .identities(&[Some(ID_1A), Some(ID_1B)]);
        let op = ProbeOperator {
            probe: env.work.join("staging/disc-1A"),
            seen: Default::default(),
        };

        let report = run_real_burn(&mut env, &script, &op).unwrap();

        assert_eq!(
            report.discs.iter().map(|d| d.disc).collect::<Vec<_>>(),
            vec![DiscKey::new(1, 0), DiscKey::new(1, 1)]
        );
        assert_eq!(burned_sessions(&env), 2);
        assert_eq!(
            *op.seen.borrow(),
            vec![false, false],
            "1A's tree was removed before 1B's prompt"
        );
        assert!(!env.work.join("staging/disc-1A").exists());
        assert!(!env.work.join("staging/disc-1B").exists());
    }

    /// Commit after success: a disc whose commit failed keeps its tree; the
    /// twin committed before it has already lost its own.
    #[test]
    fn a_real_burn_keeps_the_tree_of_a_disc_whose_commit_failed() {
        let mut env = staged_env_with(&[("a.bin", b"aaaa")], false, 2);
        env.catalog
            .raw_connection_for_tests()
            .execute_batch(
                "create temp trigger refuse_1b before insert on disc
                   when new.copy_set = 1
                 begin select raise(abort, 'refused for the test'); end;",
            )
            .unwrap();
        let script = ScriptedExec::new()
            .toc(&[BLANK, BLANK])
            .identities(&[Some(ID_1A), Some(ID_1B)]);
        let op = ScriptedOperator::new(&[true, true]);

        let err = run_real_burn(&mut env, &script, &op).unwrap_err();

        assert_eq!(
            partial_parts(&err),
            (
                vec!["1A".to_string()],
                Some(UnrecordedDisc::written("1B")),
                vec![]
            )
        );
        assert!(!env.work.join("staging/disc-1A").exists());
        assert!(env.work.join("staging/disc-1B/data/a.bin").is_file());
    }

    /// Pins existing behaviour: a declined prompt stages and commits nothing.
    #[test]
    fn a_declined_real_burn_stages_nothing_and_commits_nothing() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], false);
        let script = ScriptedExec::new().toc(&[BLANK]);
        let op = ScriptedOperator::new(&[false]);

        let err = run_real_burn(&mut env, &script, &op).unwrap_err();

        assert!(
            matches!(err, PipelineError::Burn(BurnError::Aborted)),
            "{err}"
        );
        assert_eq!(err.exit_code(), 0);
        assert!(!env.work.join("staging").exists());
        assert_eq!(burned_sessions(&env), 0);
    }

    #[test]
    fn discarding_a_staging_tree_removes_all_of_it() {
        let dir = tempfile::TempDir::new().unwrap();
        let tree = dir.path().join("staging/disc-1");
        std::fs::create_dir_all(tree.join("data/n")).unwrap();
        std::fs::write(tree.join("data/n/a.bin"), b"a").unwrap();

        discard_staging(&tree);

        assert!(!tree.exists());
        assert!(dir.path().join("staging").is_dir(), "only the disc's tree");
    }

    #[test]
    fn discarding_a_missing_staging_tree_is_quiet() {
        let dir = tempfile::TempDir::new().unwrap();
        discard_staging(&dir.path().join("staging/disc-9"));
        assert!(dir.path().is_dir());
    }

    /// A tree that cannot be removed is left in place, and the call still
    /// returns: the disc is already committed, so this costs space, not data.
    #[test]
    fn a_staging_tree_that_cannot_be_removed_is_left_without_failing() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::TempDir::new().unwrap();
        let tree = dir.path().join("staging/disc-1");
        let locked = tree.join("data/locked");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::write(locked.join("a.bin"), b"a").unwrap();
        std::fs::write(locked.join("probe"), b"p").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        if std::fs::remove_file(locked.join("probe")).is_ok() {
            eprintln!("SKIP: running as root, a read-only directory is still writable");
            return;
        }

        discard_staging(&tree);

        assert!(locked.join("a.bin").exists(), "the tree is left in place");
        // Let the TempDir clean up after itself.
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
```

- [ ] **Step 3: Run them to verify they fail**

Run: `cargo test -p diskrement-shell pipeline::burn_tests`
Expected: compile error, `cannot find function discard_staging`. Without the
`discard_staging` tests, `a_real_burn_removes_the_staging_tree_once_the_disc_commits`
fails on "the committed disc's tree is gone", and the probe test sees `[false, true]`.

- [ ] **Step 4: Implement**

In `crates/diskrement-shell/src/pipeline/mod.rs`, add a free function next to
`burner_failure`:

```rust
/// Remove a committed disc's staging tree, best-effort. The burned disc and
/// its commit are the record, so a failure here costs only space: it is
/// reported with the path and never fails the burn. Absent is fine.
fn discard_staging(dir: &Path) {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => eprintln!(
            "could not remove staging tree {} ({e}); it is safe to delete by hand",
            dir.display()
        ),
    }
}
```

Add to `impl Pipeline` (next to `next_session_ordinal`):

```rust
    /// `work_dir/staging/disc-<name>`, one tree per disc (`DiscKey::name`).
    fn staging_dir(&self, disc_name: &str) -> PathBuf {
        self.paths
            .work_dir
            .join("staging")
            .join(format!("disc-{disc_name}"))
    }
```

In `burn_disc`, build the `DiscImage`'s `staging_dir` with it:

```rust
        let image = DiscImage {
            staging_dir: self.staging_dir(&disc_name),
```

and, right after the commit's `let _ = std::fs::remove_file(&tmp_catalog);`:

```rust
        // Only after the commit: until then the tree is part of what a retry
        // or an Unrecorded disc leaves behind.
        if !burner.retains_staging() {
            discard_staging(&burned.staging_dir);
        }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p diskrement-shell pipeline::`
Expected: PASS, including the existing simulated
`a_successful_burn_clears_the_staged_state` ("staging survives") and
`a_commit_failure_after_the_disc_was_written_says_so`.

- [ ] **Step 6: Lint**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`
Expected: clean (run `cargo fmt` if needed).

- [ ] **Step 7: Commit**

```bash
git add crates/diskrement-shell/src/pipeline/mod.rs
git commit -m "fix(pipeline): remove a real disc's staging tree once it commits

Closes the unbounded work-dir growth in #3: a kept tree's reflinks pin
extents the source has since deleted. The simulated backend keeps its
trees, which StagingVerifier reads back.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Clean up on the resume path

An earlier run that crashed between a commit and its cleanup leaves the tree behind.
The re-run sees the disc as already committed (`Ok(None)`) and must remove it.

**Files:**
- Modify: `crates/diskrement-shell/src/pipeline/mod.rs`

- [ ] **Step 1: Write the failing test**

Add to `mod burn_tests`:

```rust
    #[test]
    fn resuming_a_real_burn_removes_a_committed_discs_leftover_tree() {
        let mut env = staged_env_with(&[("a.bin", b"aaaa")], false, 2);
        // A directory where the rewrite's temp file goes makes it fail, so
        // the staged plan still lists 1A after 1A commits.
        let tmp = env.work.join("staged/plan.json.tmp");
        std::fs::create_dir_all(&tmp).unwrap();
        let script = ScriptedExec::new().toc(&[BLANK]).identities(&[Some(ID_1A)]);
        let err = run_real_burn(&mut env, &script, &ScriptedOperator::new(&[true])).unwrap_err();
        assert_eq!(
            partial_parts(&err),
            (vec!["1A".to_string()], None, vec!["1B".to_string()])
        );

        // What a crash between 1A's commit and its cleanup leaves behind.
        let leftover = env.work.join("staging/disc-1A/data");
        std::fs::create_dir_all(&leftover).unwrap();
        std::fs::write(leftover.join("a.bin"), b"aaaa").unwrap();
        std::fs::remove_dir(&tmp).unwrap();

        let script = ScriptedExec::new().toc(&[BLANK]).identities(&[Some(ID_1B)]);
        let report =
            run_real_burn(&mut env, &script, &ScriptedOperator::new(&[true])).unwrap();

        assert_eq!(
            report.discs.iter().map(|d| d.disc).collect::<Vec<_>>(),
            vec![DiscKey::new(1, 1)],
            "1A is not burned twice"
        );
        assert!(
            !env.work.join("staging/disc-1A").exists(),
            "the resume path cleans up 1A"
        );
        assert!(!env.work.join("staging/disc-1B").exists());
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p diskrement-shell resuming_a_real_burn_removes`
Expected: FAIL on "the resume path cleans up 1A".

- [ ] **Step 3: Implement**

In `burn_disc`, replace

```rust
        if sess_ord == sp.session_ordinal + 1 {
            return Ok(None);
        }
```

with

```rust
        if sess_ord == sp.session_ordinal + 1 {
            // Committed by an earlier run that stopped before its cleanup or
            // the plan rewrite; finish the cleanup it owed.
            if !burner.retains_staging() {
                discard_staging(&self.staging_dir(&disc_name));
            }
            return Ok(None);
        }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p diskrement-shell pipeline::`
Expected: PASS (including the simulated
`a_plan_rewrite_failure_after_a_commit_is_a_partial_burn_that_resumes`).

- [ ] **Step 5: Lint and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/diskrement-shell/src/pipeline/mod.rs
git commit -m "fix(pipeline): finish a committed disc's staging cleanup on resume

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Docs

**Files:**
- Modify: `crates/diskrement-shell/src/pipeline/reports.rs` (`DiscBurn`)
- Modify: `README.md` (after the paragraph ending "…a quiet \"nothing to verify\".", ~line 163)
- Modify: `CLAUDE.md` ("Gotchas that will bite")

- [ ] **Step 1: `DiscBurn::staging_dir` doc comment**

```rust
    /// Where this disc was staged. With the real backend the tree has already
    /// been removed by the time the report is returned (issue #3); with the
    /// simulated backend it stays, as the disc `verify` reads back.
    pub staging_dir: PathBuf,
```

- [ ] **Step 2: README paragraph** (new paragraph after the dead-disc/`reset` paragraph)

```markdown
A real burn deletes each disc's staging tree (`<work-dir>/staging/disc-<name>`) as soon
as that disc is committed. The burned disc and the catalog are the record, and a kept
tree would pin the space of source files you have since deleted, through its reflinks.
The simulated backend keeps its trees, because they are what `verify` reads back. Trees
left by older versions of diskrement stay until you remove them. On a real-backend
project with nothing staged, for example right after a burn, `reset <name>` does that
safely. `btrfs filesystem du -s <work-dir>/staging/*` shows how much space they hold
exclusively.
```

- [ ] **Step 3: CLAUDE.md gotcha** (append to "Gotchas that will bite")

```markdown
- **A real burn deletes each disc's staging tree right after its commit**
  (`Burner::retains_staging`, `discard_staging` in `pipeline/mod.rs`); the simulated
  backend keeps them because `StagingVerifier` reads them. This is safe only because
  every disc, twins included, is staged from `work_dir/snapshot`: never stage a disc
  from another disc's tree, and never hardlink into staging.
```

- [ ] **Step 4: Commit**

```bash
git add crates/diskrement-shell/src/pipeline/reports.rs README.md CLAUDE.md
git commit -m "docs: staging trees are removed after a real burn commits

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Verify

- [ ] **Step 1: Full suite with scenarios required**

```bash
DISKREMENT_SCENARIOS_REQUIRED=1 cargo test --workspace 2>&1 | grep '^test result' | awk '{p+=$4; f+=$6; i+=$8} END {print "passed="p, "failed="f, "ignored="i}'
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

Expected: `failed=0`; `passed` = baseline + 9 (1 in Task 2, 7 in Task 3, 1 in Task 4);
clippy and fmt are clean. Check that `xorriso_image.rs` ran rather than skipped
(`cargo test -p diskrement-shell --test xorriso_image -- --nocapture 2>&1 | grep -i skip`
prints nothing when xorriso is installed).

- [ ] **Step 2: Record deviations** in the section below and commit them with
  `docs(plan): record staging-cleanup execution deviations`.

The merge into `main` (`Merge feat/staging-cleanup: …`, branch deleted locally and on
`origin`) and closing #3 happen only after the user approves.

---

## Execution deviations

- Baseline test count (Task 0): 572 passed, 0 failed, with
  `DISKREMENT_SCENARIOS_REQUIRED=1`. Final (Task 6): 581 (+9, as planned). xorriso was
  installed, so `xorriso_image.rs` ran (5/5).
- Task 3: to watch the behavioural tests fail before wiring, `discard_staging` was added
  first under a temporary `#[allow(dead_code)]`, because `-D warnings` rejects an unused
  function. The red run showed the expected failures (the probe saw `[false, true]`).
  The `allow` was removed in the same commit that wired the call sites.
- Executed inline rather than subagent-driven, at the user's choice.
- Spec D4 wording was aligned with the codebase's unprefixed stderr warnings before the
  plan was written. Spec test 6 was moved to `discard_staging` directly, because no fault
  can be injected between staging and commit without a burner hook.
