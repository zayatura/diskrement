# Burn with verify Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `burn --verify` reads each disc back while it is still in the drive, records it only if the read-back passes, and ejects it only afterwards.

**Architecture:** The pipeline runs the per-disc loop (spec §3.7, §5.1). The burner stops ejecting and gets `Burner::eject()`. The verifier gets a no-prompt `read_back` that returns `Passed`, `Failed` or `Unverifiable`. The expected contents come from the post-burn work-copy catalog, which `burn_disc` already builds. A failed first session is retried through the burner's own blank-disc prompt. A failed append stops with `VerifyFailed` (exit 4).

**Tech Stack:** Rust workspace (`diskrement-core`, `diskrement-shell`, `diskrement-cli`), clap, rusqlite, thiserror, walkdir, insta, assert_cmd.

**Spec:** `docs/superpowers/specs/2026-10-16-burn-verify-design.md`. Read it first.

**Branch:** `feat/burn-verify` (already created; the spec is committed on it).

---

## Ground rules for every task

- Verification is `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check`. Warnings are errors.
- Commit messages are conventional with a scope and end with:

  ```
  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01RkDxvGBgp4hdJznTjDzp3c
  ```
- Never format a disc name by hand: use `DiscKey::name(copies)` or the `disc_name` already computed.
- Do not touch `fixtures/`.
- Record any deviation from this plan in the **Execution deviations** section at the bottom, not only in a commit message.

## Deviations from the spec, decided while planning

1. **`Pipeline::burn` keeps its signature.** A new `Pipeline::burn_with(name, now, burner, read_back: Option<&VerifyAfterBurn<'_>>)` does the work, and `burn` calls it with `None`. This leaves the 15 existing `burn` callers alone. `VerifyAfterBurn` carries the verifier and the operator, because the pipeline has to prompt for a reinsert and the burner doesn't expose its operator.
2. **The scenario checks the burn output, not `status`.** `status` doesn't render session states, so scenario 16 asserts the two `Read back: verified` lines and a clean `verify` afterwards. The VERIFIED rows themselves are asserted in the pipeline tests.

The spec is amended for both in Task 0.

## File map

| File | Change |
|---|---|
| `crates/diskrement-shell/src/burn/testkit.rs` | `ScriptedExec` answers `udisksctl mount`/`unmount` |
| `crates/diskrement-shell/src/burn/verifier.rs` | unreadable files are mismatches; `compare_visible`; `ReadBack`; `read_back` on both verifiers |
| `crates/diskrement-shell/src/burn/xorriso/burner.rs` | `burn` no longer ejects; `eject` is `pub` |
| `crates/diskrement-shell/src/burn/mod.rs` | `Burner::eject` |
| `crates/diskrement-shell/src/error.rs` | `PipelineError::VerifyFailed`; partial-burn tail and advice |
| `crates/diskrement-shell/src/pipeline/reports.rs` | `DiscReadBack`; `DiscBurn::read_back` |
| `crates/diskrement-shell/src/pipeline/read_back.rs` | **new**: `VerifyAfterBurn` and the burn → read-back loop |
| `crates/diskrement-shell/src/pipeline/read_back_tests.rs` | **new**: pipeline tests of every path in spec §5.1 |
| `crates/diskrement-shell/src/pipeline/mod.rs` | `disc_expectation`; `burn_with`; `burn_disc` wiring; eject after commit |
| `crates/diskrement-cli/src/args.rs` | `burn --verify` |
| `crates/diskrement-cli/src/main.rs` | builds the verifier for `--verify` |
| `crates/diskrement-cli/src/render.rs` | `Read back:` line |
| `crates/diskrement-cli/tests/binary.rs` | `--verify` through the binary |
| `crates/diskrement-cli/tests/scenarios/s16_burn_verify.rs` | **new**: scenario 16 |
| `CLAUDE.md`, `docs/smoke-test-real-media.md` | docs |

---

### Task 0: Amend the spec and commit the plan

**Files:**
- Modify: `docs/superpowers/specs/2026-10-16-burn-verify-design.md`

- [x] **Step 1: Amend §4.4**

Replace the code block and the paragraph after it in §4.4 with:

````markdown
```rust
pub struct VerifyAfterBurn<'a> {
    pub verifier: &'a Verifier<'a>,
    pub operator: &'a dyn BurnOperator,
}

pub fn burn_with(&mut self, name: &str, now: DateTime<Utc>, burner: &Burner<'_>,
                 read_back: Option<&VerifyAfterBurn<'_>>) -> Result<BurnReport, PipelineError>;
```

`burn(name, now, burner)` keeps its signature and calls `burn_with(…, None)`.
`VerifyAfterBurn` carries the operator because the pipeline prompts for a
reinsert itself; the burner does not expose its operator.
````

- [x] **Step 2: Amend §7.3's scenario bullet**

Replace "after which `status` shows both discs VERIFIED." with "after which the burn report says both discs read back verified and a `verify` sweep exits 0 (`status` does not render session states; the VERIFIED rows are asserted in the pipeline tests)."

- [x] **Step 3: Commit**

```bash
git add docs/superpowers/specs/2026-10-16-burn-verify-design.md docs/superpowers/plans/2026-10-16-burn-verify.md
git commit -m "docs(plan): burn --verify implementation plan; spec names burn_with and the scenario's evidence"
```

---

### Task 1: `ScriptedExec` answers udisks

The pipeline and verifier tests mount a scripted disc. Today `ScriptedExec` answers every `udisksctl` call with empty output, which `DiscReader::mount` reads as "no mount point".

**Files:**
- Modify: `crates/diskrement-shell/src/burn/testkit.rs`

- [ ] **Step 1: Write the failing test** (in `testkit.rs`'s `mod tests`)

```rust
    /// Mounts answer in order: `None` fails the way udisks does (no mount
    /// point to parse), `Some(dir)` mounts there. Unmounts always succeed.
    #[test]
    fn scripted_mounts_answer_in_order() {
        let dir = tempfile::TempDir::new().unwrap();
        let script = ScriptedExec::new().mounts(&[None, Some(dir.path())]);
        let mount: Vec<String> = ["udisksctl", "mount", "-b", "/dev/sr0"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let first = script.respond(&mount, &mut |_| {}).unwrap();
        assert_eq!(crate::burn::reader::parse_mount_point(&first.lines), None);
        let second = script.respond(&mount, &mut |_| {}).unwrap();
        assert_eq!(
            crate::burn::reader::parse_mount_point(&second.lines),
            Some(dir.path().to_path_buf())
        );
        assert!(script.ran("udisksctl mount -b /dev/sr0"));
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p diskrement-shell burn::testkit`
Expected: compile error, `no method named mounts`.

- [ ] **Step 3: Implement**

Add the field to `ScriptedExec`:

```rust
    mounts: RefCell<VecDeque<Option<PathBuf>>>,
```

initialise it in `ScriptedExec::new` with `mounts: RefCell::new(VecDeque::new()),`, and add the builder after `identities`:

```rust
    /// What each `udisksctl mount` answers, in order: `Some(dir)` mounts the
    /// disc at `dir`, `None` fails as udisks does when it cannot mount.
    pub fn mounts(mut self, points: &[Option<&Path>]) -> Self {
        self.mounts = RefCell::new(points.iter().map(|p| p.map(Path::to_path_buf)).collect());
        self
    }
```

In `respond`, right after `let has = …;`, add:

```rust
        if cmd.first().is_some_and(|c| c == "udisksctl") {
            if cmd.get(1).is_some_and(|a| a == "mount") {
                let device = cmd.last().expect("a device");
                let point = self
                    .mounts
                    .borrow_mut()
                    .pop_front()
                    .expect("the script ran out of mounts");
                return Ok(match point {
                    Some(dir) => CmdOutput {
                        exit_code: 0,
                        lines: vec![format!("Mounted {device} at {}.", dir.display())],
                    },
                    None => CmdOutput {
                        exit_code: 1,
                        lines: vec![format!("Error mounting {device}: wrong fs type")],
                    },
                });
            }
            // unmount
            return Ok(CmdOutput {
                exit_code: 0,
                lines: Vec::new(),
            });
        }
```

`Path` is already imported in `testkit.rs` (`use std::path::{Path, PathBuf};`).

- [ ] **Step 4: Run the tests**

Run: `cargo test -p diskrement-shell burn::`
Expected: PASS (the new test plus every existing burn test).

- [ ] **Step 5: Commit**

```bash
git add crates/diskrement-shell/src/burn/testkit.rs
git commit -m "test(burn): the scripted drive answers udisksctl mount and unmount"
```

---

### Task 2: An unreadable file is a mismatch, not an error (spec §5.3)

**Files:**
- Modify: `crates/diskrement-shell/src/burn/verifier.rs`

- [ ] **Step 1: Write the failing tests** (in `verifier.rs`'s outer `mod tests`, after `mismatches_are_reported_in_sorted_order`)

```rust
    /// Make `path` unreadable; `false` when that has no effect (running as
    /// root), and the caller skips.
    fn lock(path: &std::path::Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000)).unwrap();
        let readable = if path.is_dir() {
            std::fs::read_dir(path).is_ok()
        } else {
            std::fs::File::open(path).is_ok()
        };
        !readable
    }

    fn unlock(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A file that cannot be read is a mismatch for that file, and the rest of
    /// the tree is still checked (burn-verify spec §5.3). Before, the read
    /// error stopped `verify` with exit 6.
    #[test]
    fn an_unreadable_file_is_a_mismatch_and_the_rest_is_still_checked() {
        let dir = tempfile::TempDir::new().unwrap();
        let staging = dir.path().join("staging");
        let data = staging.join("disc-1/data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("a.txt"), b"hello").unwrap();
        std::fs::write(data.join("locked.txt"), b"secret").unwrap();
        if !lock(&data.join("locked.txt")) {
            eprintln!("SKIP [an_unreadable_file_is_a_mismatch_and_the_rest_is_still_checked]: running as root");
            return;
        }
        let out = StagingVerifier::new(staging, &NoopSink)
            .verify(&expectation(&[("a.txt", b"hello"), ("locked.txt", b"secret")], 1))
            .unwrap();
        assert_eq!(
            out,
            VerifyOutcome {
                files_checked: 2,
                mismatches: vec![RelPath("locked.txt".to_string())],
                skipped: false,
            }
        );
    }

    /// A directory the walk cannot read is a mismatch named by its own path.
    #[test]
    fn an_unreadable_directory_is_a_mismatch_named_by_its_path() {
        let dir = tempfile::TempDir::new().unwrap();
        let staging = dir.path().join("staging");
        let sub = staging.join("disc-1/data/sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("b.txt"), b"x").unwrap();
        if !lock(&sub) {
            unlock(&sub);
            eprintln!("SKIP [an_unreadable_directory_is_a_mismatch_named_by_its_path]: running as root");
            return;
        }
        let out = StagingVerifier::new(staging, &NoopSink)
            .verify(&expectation(&[("sub/b.txt", b"x")], 1));
        unlock(&sub);
        assert_eq!(out.unwrap().mismatches, vec![RelPath("sub".to_string())]);
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p diskrement-shell burn::verifier`
Expected: both FAIL: `verify` returns `Err(Io(..))`, so the `unwrap` panics.

- [ ] **Step 3: Implement**

Replace `hash_tree` (its doc comment and body) with:

```rust
/// Hash every file under `data_dir`, as one progress phase.
///
/// A file that cannot be read comes back as `None`, and so does an entry the
/// walk cannot read, named by its own path (`.` for `data_dir` itself): an
/// unreadable file on a disc is a finding, not a reason to stop
/// (burn-verify spec §5.3).
///
/// The walk comes first so the phase opens with its totals. The bar advances
/// by bytes during each read, not once per file: from an optical drive at
/// ~13 MB/s a multi-GB video would otherwise hold it still for minutes.
///
/// Sorted, unlike the Scala original: `Files.walk` order is unspecified, so a
/// multi-mismatch report was not reproducible run to run. Sorting can only
/// make the rendered list stable, and every oracle in the corpus has at most
/// one mismatch.
fn hash_tree(
    data_dir: &Path,
    sink: &dyn ProgressSink,
    label: &str,
) -> Vec<(RelPath, Option<ContentHash>)> {
    let mut files = Vec::new();
    let mut unreadable = Vec::new();
    for entry in walkdir::WalkDir::new(data_dir).sort_by_file_name() {
        match entry {
            Ok(entry) if entry.file_type().is_file() => match entry.metadata() {
                Ok(meta) => files.push((entry.into_path(), meta.len())),
                Err(e) => unreadable.push(unreadable_path(data_dir, e.path())),
            },
            Ok(_) => {}
            Err(e) => unreadable.push(unreadable_path(data_dir, e.path())),
        }
    }

    let total_bytes = files.iter().map(|(_, size)| size).sum();
    let mut hashed: Vec<(RelPath, Option<ContentHash>)> =
        progress::phase(sink, label, total_bytes, files.len(), || {
            files
                .iter()
                .map(|(file, _)| {
                    let actual = hasher::hash_with(file, &mut |n| sink.advance(n, 0)).ok();
                    sink.advance(0, 1);
                    (relativize(data_dir, file), actual)
                })
                .collect()
        });
    hashed.extend(unreadable.into_iter().map(|rel| (rel, None)));
    hashed
}

/// Where a walk error happened, relative to `data_dir`: `.` for `data_dir`
/// itself or an error that names no path.
fn unreadable_path(data_dir: &Path, path: Option<&Path>) -> RelPath {
    match path.filter(|p| *p != data_dir) {
        Some(p) => relativize(data_dir, p),
        None => RelPath(".".to_string()),
    }
}

/// Whether one hashed entry is a mismatch against `expected`: unreadable,
/// not expected at all, or carrying another hash.
fn mismatched(
    expected: &BTreeMap<RelPath, ContentHash>,
    rel: &RelPath,
    actual: &Option<ContentHash>,
) -> bool {
    actual
        .as_ref()
        .is_none_or(|a| expected.get(rel) != Some(a))
}

/// Tier 1 against a disc's `/data` tree: every file on it hashed against
/// `visible`, and every expected file missing from it counted as a mismatch.
fn compare_visible(
    data_dir: &Path,
    exp: &DiscExpectation,
    sink: &dyn ProgressSink,
) -> (usize, Vec<RelPath>) {
    let found: BTreeMap<RelPath, Option<ContentHash>> =
        hash_tree(data_dir, sink, &verifying_label(exp))
            .into_iter()
            .collect();
    let mut mismatches: Vec<RelPath> = found
        .iter()
        .filter(|(rel, actual)| mismatched(&exp.visible, rel, actual))
        .map(|(rel, _)| rel.clone())
        .collect();
    let missing: Vec<RelPath> = exp
        .visible
        .keys()
        .filter(|rel| !found.contains_key(*rel))
        .cloned()
        .collect();
    let checked = found.len() + missing.len();
    mismatches.extend(missing);
    (checked, mismatches)
}
```

In `StagingVerifier::verify`, replace the hashing and the filter:

```rust
        let hashed = hash_tree(&data_dir, self.sink, &verifying_label(exp));
        let mut mismatches: Vec<RelPath> = hashed
            .iter()
            .filter(|(rel, actual)| mismatched(&exp.latest_session, rel, actual))
            .map(|(rel, _)| rel.clone())
            .collect();
```

Replace `DiscVerifier::tier1`'s body after the `Mounting` line with:

```rust
        let reader = DiscReader::new(self.device.clone(), self.exec);
        reader.with_data_tree(|data_dir| Ok(compare_visible(data_dir, exp, self.sink)))
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p diskrement-shell burn::verifier && cargo test -p diskrement-shell --test xorriso_image`
Expected: PASS. The existing phase-event assertions (`start verifying disc 1 9 2`) are unchanged, because readable files are still counted the same way.

- [ ] **Step 5: Commit**

```bash
git add crates/diskrement-shell/src/burn/verifier.rs
git commit -m "fix(verify): a file the disc cannot read is a mismatch for that file, not a stopped run"
```

---

### Task 3: The pipeline ejects, after the commit

**Files:**
- Modify: `crates/diskrement-shell/src/burn/xorriso/burner.rs`
- Modify: `crates/diskrement-shell/src/burn/mod.rs`
- Modify: `crates/diskrement-shell/src/pipeline/mod.rs`

- [ ] **Step 1: Change the burner tests to the new contract**

In `burner.rs`'s `session_one_on_blank_media_burns_and_post_checks`, replace the last assertion with:

```rust
        assert!(
            !script.ran("-eject"),
            "the burn leaves the disc in the drive; the pipeline ejects"
        );
        burner.eject();
        assert!(script.ran("-eject"), "eject() ejects a real device");
```

In `an_image_file_is_never_ejected`, bind the burner and eject explicitly:

```rust
        let burner = XorrisoBurner::new(
            BurnDevice::ImageFile(dir.path().join("d.iso")),
            &exec,
            &op,
            &NoopSink,
            2,
        );
        burner
            .burn(&image_of(dir.path(), 1, &[("a.txt", "hello")], false))
            .expect("the burn succeeds");
        burner.eject();
        assert!(!script.ran("-eject"));
```

In `burn/mod.rs`'s `mod tests`, add:

```rust
    /// Only the real backend has a tray to open.
    #[test]
    fn only_the_real_backend_ejects() {
        let script = testkit::ScriptedExec::new();
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
        let op = testkit::ScriptedOperator::new(&[]);
        Burner::Simulated(simulated::SimulatedBurner::new(&NoopSink, 1)).eject();
        assert!(!script.ran("-eject"));
        Burner::Real(xorriso::burner::XorrisoBurner::new(
            BurnDevice::RealDevice("/dev/sr0".to_string()),
            &exec,
            &op,
            &NoopSink,
            1,
        ))
        .eject();
        assert!(script.ran("-eject"));
    }
```

(The module already uses `NoopSink`, `xorriso` and `simulated` in `only_the_simulated_backend_retains_staging`. Add `use crate::burn::testkit;` if it isn't imported there.)

In `pipeline/mod.rs`'s `mod tests`, after `a_real_burn_removes_the_staging_tree_once_the_disc_commits`, add:

```rust
    /// One eject, after the commit: by then the committed disc's tree has
    /// been discarded (burn-verify spec §4.1).
    #[test]
    fn a_real_burn_ejects_once_the_disc_is_committed() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], false);
        let script = ScriptedExec::new().toc(&[BLANK]).identities(&[Some(ID_1)]);
        let tree = env.work.join("staging/disc-1");
        let at_eject = std::cell::RefCell::new(Vec::new());
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| {
            if c.iter().any(|a| a == "-eject") {
                at_eject.borrow_mut().push(tree.exists());
            }
            script.respond(c, l)
        };
        let op = ScriptedOperator::new(&[true]);

        run_real_burn_with(&mut env, &exec, &op).unwrap();

        assert_eq!(*at_eject.borrow(), vec![false]);
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p diskrement-shell`
Expected: compile errors (`no method named eject` on `Burner`; `eject` is private on `XorrisoBurner`).

- [ ] **Step 3: Implement**

`burner.rs`: delete `self.eject();` from `burn`, make `fn eject(&self)` `pub fn eject(&self)`, and change its doc comment and the module doc:

```rust
//! Prompt for the disc, inspect it (strict identity check), stage with
//! re-hash, check capacity, burn, sanity-check the result. The pipeline
//! ejects afterwards (`Burner::eject`), once it has recorded the disc or,
//! under `burn --verify`, read it back. Any failure leaves the catalog
//! uncommitted, because `Pipeline::burn` commits only after this returns.
```

```rust
    /// Open the tray. Best-effort: the burn already succeeded, and an eject
    /// hiccup must not turn a written disc into a failed run.
    pub fn eject(&self) {
```

`burn/mod.rs`, in `impl Burner<'_>`:

```rust
    /// Eject the disc once the pipeline is done with it: after the commit,
    /// or after the read-back under `burn --verify`. Best-effort; the
    /// simulated backend and an image file have no tray.
    pub fn eject(&self) {
        if let Burner::Real(b) = self {
            b.eject();
        }
    }
```

`pipeline/mod.rs`, in `burn_disc`, after the closing brace of `if !burner.retains_staging() { discard_staging(&burned.staging_dir); }`:

```rust
        burner.eject();
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p diskrement-shell`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/diskrement-shell/src/burn crates/diskrement-shell/src/pipeline/mod.rs
git commit -m "refactor(burn): the pipeline ejects the disc after recording it, not the burner"
```

---

### Task 4: `read_back` on both verifiers (spec §4.2)

**Files:**
- Modify: `crates/diskrement-shell/src/burn/verifier.rs`

- [ ] **Step 1: Write the failing tests**

In the outer `mod tests` (staging):

```rust
    /// The simulated backend's read-back is its `verify` against the staged
    /// tree, with an absent tree reported as unverifiable, never a pass.
    #[test]
    fn a_staging_read_back_passes_fails_or_cannot_tell() {
        let dir = tempfile::TempDir::new().unwrap();
        let staging = dir.path().join("staging");
        let data = staging.join("disc-1/data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("a.txt"), b"hello").unwrap();
        let v = StagingVerifier::new(staging, &NoopSink);

        assert_eq!(
            v.read_back(&expectation(&[("a.txt", b"hello")], 1)).unwrap(),
            ReadBack::Passed { files: 1 }
        );
        assert_eq!(
            v.read_back(&expectation(&[("a.txt", b"other")], 1)).unwrap(),
            ReadBack::Failed {
                files: 1,
                mismatches: vec![RelPath("a.txt".to_string())]
            }
        );
        let absent = DiscExpectation {
            disc: DiscKey::new(9, 0),
            disc_name: "9".to_string(),
            ..expectation(&[], 9)
        };
        assert!(matches!(
            v.read_back(&absent).unwrap(),
            ReadBack::Unverifiable(why) if why.contains("disc 9")
        ));
    }
```

In `mod disc`, add the fixtures and tests:

```rust
        use crate::burn::testkit::{ScriptedExec, BLANK};

        const IDENTITY_2: &str = r#"{"projectName":"photos","discOrdinal":1,"sessionOrdinal":2,"createdAt":"2026-08-30T10:00:00Z"}"#;

        /// A disc as its mount shows it: `files` under `data/`, and `id` as
        /// its `/.diskrement/project.json`.
        fn mounted(files: &[(&str, &[u8])], id: &str) -> tempfile::TempDir {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::create_dir_all(dir.path().join("data")).unwrap();
            for (rel, body) in files {
                std::fs::write(dir.path().join("data").join(rel), body).unwrap();
            }
            std::fs::create_dir_all(dir.path().join(".diskrement")).unwrap();
            std::fs::write(dir.path().join(".diskrement/project.json"), id).unwrap();
            dir
        }

        fn visible(pairs: &[(&str, &[u8])]) -> DiscExpectation {
            DiscExpectation {
                visible: pairs
                    .iter()
                    .map(|(rel, body)| (RelPath((*rel).to_string()), sha(body)))
                    .collect(),
                ..expectation()
            }
        }

        /// `read_back` of session `session` on /dev/sr0, with the operator's
        /// transcript. The operator has no answers: a read-back never prompts.
        fn read_back(
            script: &ScriptedExec,
            exp: &DiscExpectation,
            session: u32,
        ) -> (ReadBack, Vec<String>) {
            let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
            let op = ScriptedOperator::new(&[]);
            let out = DiscVerifier::new(
                BurnDevice::RealDevice("/dev/sr0".to_string()),
                &exec,
                &op,
                &NoopSink,
            )
            .read_back(exp, session)
            .unwrap();
            (out, op.log())
        }

        #[test]
        fn a_clean_disc_reads_back_without_a_prompt() {
            let disc = mounted(&[("a.txt", b"hello")], IDENTITY);
            let script = ScriptedExec::new()
                .toc(&[APPENDABLE])
                .identities(&[Some(IDENTITY)])
                .mounts(&[Some(disc.path())]);
            let (out, log) = read_back(&script, &visible(&[("a.txt", b"hello")]), 1);
            assert_eq!(out, ReadBack::Passed { files: 1 });
            assert_eq!(log, vec!["Mounting disc 1…".to_string()]);
            assert!(script.ran("udisksctl unmount -b /dev/sr0"));
        }

        #[test]
        fn a_wrong_or_missing_file_fails_the_read_back() {
            let disc = mounted(&[("a.txt", b"HELLO")], IDENTITY);
            let script = ScriptedExec::new()
                .toc(&[APPENDABLE])
                .identities(&[Some(IDENTITY)])
                .mounts(&[Some(disc.path())]);
            let (out, _) = read_back(
                &script,
                &visible(&[("a.txt", b"hello"), ("b.txt", b"keep")]),
                1,
            );
            assert_eq!(
                out,
                ReadBack::Failed {
                    files: 2,
                    mismatches: vec![
                        RelPath("a.txt".to_string()),
                        RelPath("b.txt".to_string())
                    ],
                }
            );
        }

        #[test]
        fn a_failed_mount_is_unverifiable() {
            let script = ScriptedExec::new()
                .toc(&[APPENDABLE])
                .identities(&[Some(IDENTITY)])
                .mounts(&[None]);
            let (out, _) = read_back(&script, &visible(&[("a.txt", b"hello")]), 1);
            assert!(
                matches!(&out, ReadBack::Unverifiable(why) if why.contains("could not mount it")),
                "{out:?}"
            );
        }

        /// The drive must hold this disc at exactly the session just written;
        /// otherwise nothing is mounted.
        #[test]
        fn another_disc_or_a_blank_is_unverifiable_and_never_mounted() {
            for script in [
                ScriptedExec::new()
                    .toc(&[APPENDABLE])
                    .identities(&[Some(OTHER_DISC)]),
                ScriptedExec::new()
                    .toc(&[APPENDABLE])
                    .identities(&[Some(IDENTITY)]),
                ScriptedExec::new().toc(&[BLANK]),
            ] {
                // Session 2 expected: IDENTITY is session 1.
                let (out, _) = read_back(&script, &visible(&[]), 2);
                assert!(
                    matches!(&out, ReadBack::Unverifiable(why) if why.contains("not disc 1 at session 2")),
                    "{out:?}"
                );
                assert!(!script.ran("udisksctl"));
            }
        }

        /// xorriso already sees session 2, but the mount still shows session
        /// 1: the system has not noticed the new session. Unverifiable, so a
        /// good appended disc is never failed for files it seems to lack.
        #[test]
        fn a_mount_still_showing_the_previous_session_is_unverifiable() {
            let disc = mounted(&[("a.txt", b"hello")], IDENTITY);
            let script = ScriptedExec::new()
                .toc(&[APPENDABLE])
                .identities(&[Some(IDENTITY_2)])
                .mounts(&[Some(disc.path())]);
            let (out, _) = read_back(
                &script,
                &visible(&[("a.txt", b"hello"), ("b.txt", b"new")]),
                2,
            );
            assert!(
                matches!(&out, ReadBack::Unverifiable(why) if why.contains("still shows session 1")),
                "{out:?}"
            );
            assert!(script.ran("udisksctl unmount"), "the stale mount is released");
        }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p diskrement-shell burn::verifier`
Expected: compile errors (`ReadBack` and `read_back` don't exist).

- [ ] **Step 3: Implement**

Imports at the top of `verifier.rs`:

```rust
use crate::burn::json::ProjectJson;
```

After `VerifyOutcome`, add:

```rust
/// What `burn --verify` learns from reading back the disc it just wrote,
/// before it is recorded (burn-verify spec §4.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadBack {
    /// Every file is there and hashes as recorded.
    Passed { files: usize },
    /// Evidence against the disc: mismatched, missing or unreadable files.
    Failed {
        files: usize,
        mismatches: Vec<RelPath>,
    },
    /// Nothing was learned about the disc: it could not be mounted, the drive
    /// holds another disc, or the mount still shows an older session.
    Unverifiable(String),
}

impl ReadBack {
    fn from_outcome(o: VerifyOutcome) -> Self {
        if o.mismatches.is_empty() {
            ReadBack::Passed {
                files: o.files_checked,
            }
        } else {
            ReadBack::Failed {
                files: o.files_checked,
                mismatches: o.mismatches,
            }
        }
    }
}

/// This project's disc `exp.disc`, at exactly session `session`.
fn is_session(exp: &DiscExpectation, session: u32, id: &DiscIdentity) -> bool {
    id.project_name == exp.project_name && id.disc() == exp.disc && id.session_ordinal == session
}
```

In `impl StagingVerifier`:

```rust
    /// `verify` for the disc a simulated burn just staged. An absent tree is
    /// unverifiable, never a pass.
    pub fn read_back(&self, exp: &DiscExpectation) -> Result<ReadBack, BurnError> {
        let out = self.verify(exp)?;
        if out.skipped {
            return Ok(ReadBack::Unverifiable(format!(
                "disc {} has no staged tree",
                exp.disc_name
            )));
        }
        Ok(ReadBack::from_outcome(out))
    }
```

In `impl DiscVerifier`, after `verify`:

```rust
    /// Read back the disc still in the drive after `burn --verify` wrote its
    /// session `session`. There is no prompt. The disc must be this one at
    /// exactly that session, first as xorriso reads the medium and then as the
    /// mount shows it: a mount that still shows an older session means the
    /// system has not noticed the new one, and would otherwise look like
    /// missing files.
    pub fn read_back(&self, exp: &DiscExpectation, session: u32) -> Result<ReadBack, BurnError> {
        let status = MediaInspector::new(self.device.clone(), self.exec).inspect()?;
        let found = match &status {
            MediaStatus::Appendable {
                identity: Some(id), ..
            }
            | MediaStatus::Closed(Some(id)) => Some(id),
            _ => None,
        };
        if !found.is_some_and(|id| is_session(exp, session, id)) {
            return Ok(ReadBack::Unverifiable(format!(
                "the drive holds {status}, not disc {} at session {session}",
                exp.disc_name
            )));
        }

        self.operator
            .progress(&format!("Mounting disc {}…", exp.disc_name));
        let reader = DiscReader::new(self.device.clone(), self.exec);
        let tier1 = reader.with_data_tree(|data_dir| {
            Ok(match self.stale_mount(data_dir, exp, session) {
                Some(stale) => Err(stale),
                None => Ok(compare_visible(data_dir, exp, self.sink)),
            })
        });
        let (checked_1, mut mismatches) = match tier1 {
            Ok(Ok(found)) => found,
            Ok(Err(stale)) => return Ok(ReadBack::Unverifiable(stale)),
            Err(BurnError::MountFailed(why)) => {
                return Ok(ReadBack::Unverifiable(format!("could not mount it: {why}")))
            }
            Err(e) => return Err(e),
        };
        let (checked_2, tier2) = self.tier2(exp)?;
        mismatches.extend(tier2);
        mismatches.sort();
        mismatches.dedup();
        Ok(ReadBack::from_outcome(VerifyOutcome {
            files_checked: checked_1 + checked_2,
            mismatches,
            skipped: false,
        }))
    }

    /// `Some(reason)` when the mounted tree is not session `session` of this
    /// disc. Only a real device can lag; an image file is read by osirrox,
    /// which sees the medium itself.
    fn stale_mount(&self, data_dir: &Path, exp: &DiscExpectation, session: u32) -> Option<String> {
        if !matches!(self.device, BurnDevice::RealDevice(_)) {
            return None;
        }
        let file = data_dir
            .parent()
            .unwrap_or(data_dir)
            .join(".diskrement/project.json");
        let mounted = std::fs::read_to_string(&file)
            .ok()
            .and_then(|text| serde_json::from_str::<ProjectJson>(&text).ok())
            .and_then(|p| p.to_identity().ok());
        match mounted {
            Some(id) if is_session(exp, session, &id) => None,
            Some(id) => Some(format!(
                "the mounted disc still shows session {} of disc {} (the system has not noticed the new session)",
                id.session_ordinal,
                id.disc_name()
            )),
            None => Some("the mounted disc has no readable /.diskrement/project.json".to_string()),
        }
    }
```

In `impl Verifier<'_>`:

```rust
    /// `burn --verify`'s check of the disc just written, before it is
    /// recorded. `session` is the session that burn wrote.
    pub fn read_back(&self, exp: &DiscExpectation, session: u32) -> Result<ReadBack, BurnError> {
        match self {
            Verifier::Staging(v) => v.read_back(exp),
            Verifier::Disc(v) => v.read_back(exp, session),
        }
    }
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p diskrement-shell burn::verifier`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/diskrement-shell/src/burn/verifier.rs
git commit -m "feat(verify): read back the disc still in the drive, telling a bad disc from one that cannot be read yet"
```

---

### Task 5: `PipelineError::VerifyFailed` (spec §5.2)

**Files:**
- Modify: `crates/diskrement-shell/src/error.rs`

- [ ] **Step 1: Write the failing tests** (in `error.rs`'s `mod tests`)

```rust
    #[test]
    fn a_failed_read_back_exits_4_and_says_what_to_do() {
        let first = PipelineError::VerifyFailed {
            disc: "1".into(),
            session: 1,
            bad: 3,
            checked: 9042,
            appended: false,
        };
        assert_eq!(first.exit_code(), 4);
        assert_eq!(
            first.to_string(),
            "disc 1 failed verification: 3 of 9042 files bad, so it was not recorded \
             (re-run burn with a new blank disc)"
        );
        let appended = PipelineError::VerifyFailed {
            disc: "3".into(),
            session: 2,
            bad: 1,
            checked: 40,
            appended: true,
        };
        assert_eq!(appended.exit_code(), 4);
        assert_eq!(
            appended.to_string(),
            "disc 3 failed verification: 1 of 40 files bad, so it was not recorded \
             (disc 3 can no longer be appended to: run disc mark-dead 3, then plan and burn)"
        );
    }

    #[test]
    fn a_partial_burn_ending_in_a_failed_read_back_names_the_failed_disc() {
        let cause = PipelineError::VerifyFailed {
            disc: "1B".into(),
            session: 1,
            bad: 2,
            checked: 10,
            appended: false,
        };
        assert_eq!(
            partial_burn_tail(&["1A".into()], &None, &[], &cause),
            "1A committed; 1B failed verification and was not recorded (re-run burn)"
        );
        let err = PipelineError::PartialBurn {
            committed: vec!["1A".into()],
            unrecorded: None,
            pending: vec![],
            cause: Box::new(cause),
        };
        assert_eq!(err.exit_code(), 4);

        let appended = PipelineError::VerifyFailed {
            disc: "3B".into(),
            session: 2,
            bad: 1,
            checked: 5,
            appended: true,
        };
        let tail = partial_burn_tail(&["3A".into()], &None, &[], &appended);
        assert_eq!(
            tail,
            "3A committed; 3B failed verification and was not recorded \
             (mark 3B dead, then re-run plan and burn)"
        );
        assert!(!tail.contains("reset"));
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p diskrement-shell error`
Expected: compile error, no variant `VerifyFailed`.

- [ ] **Step 3: Implement**

Add the variant before `PartialBurn`:

```rust
    /// `burn --verify` read back a disc it had just written and found it bad,
    /// so the disc was not recorded (burn-verify spec §5.2). A first session
    /// can be burned again on a new blank disc. An appended session is on the
    /// disc for good and the catalog will never match it, so the disc must be
    /// retired. Exit 4, as for a mismatch in `verify`. `disc` is its name.
    #[error(
        "disc {disc} failed verification: {bad} of {checked} files bad, so it was not recorded ({})",
        verify_failed_advice(.disc, .appended)
    )]
    VerifyFailed {
        disc: String,
        session: u32,
        bad: usize,
        checked: usize,
        appended: bool,
    },
```

Add the helper next to `partial_burn_advice`:

```rust
/// What to do after a disc failed `burn --verify`'s read-back.
fn verify_failed_advice(disc: &str, appended: &bool) -> String {
    if *appended {
        format!(
            "disc {disc} can no longer be appended to: run disc mark-dead {disc}, then plan and burn"
        )
    } else {
        "re-run burn with a new blank disc".to_string()
    }
}
```

Change `partial_burn_advice` to return `String` and add the append arm:

```rust
fn partial_burn_advice(cause: &PipelineError) -> String {
    match cause {
        // The staged plan no longer holds, or its snapshot is gone. A fresh
        // `plan` rebuilds both, and turns the committed discs' files into
        // catch-up copies for the lagging sets.
        PipelineError::StalePlan(_) | PipelineError::SnapshotMissing(_) => {
            "re-run plan, then burn".to_string()
        }
        // The failed append is on the disc for good; `mark-dead` re-plans its
        // files. Never `reset`.
        PipelineError::VerifyFailed {
            disc,
            appended: true,
            ..
        } => format!("mark {disc} dead, then re-run plan and burn"),
        _ => "re-run burn".to_string(),
    }
}
```

In `partial_burn_tail`, after `let mut parts = …;`:

```rust
    if let PipelineError::VerifyFailed { disc, .. } = cause {
        parts.push(format!("{disc} failed verification and was not recorded"));
    }
```

and replace the advice selection:

```rust
    let failed_disc = matches!(cause, PipelineError::VerifyFailed { .. });
    let advice = if pending.is_empty() && unrecorded.is_none() && !failed_disc {
        "nothing is left to write".to_string()
    } else {
        partial_burn_advice(cause)
    };
```

In `exit_code`, before `PipelineError::PartialBurn`:

```rust
            PipelineError::VerifyFailed { .. } => 4,
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p diskrement-shell error && cargo test -p diskrement-cli`
Expected: PASS. The CLI's `error_line` falls through to `to_string()` for the new variant.

- [ ] **Step 5: Commit**

```bash
git add crates/diskrement-shell/src/error.rs
git commit -m "feat(burn): VerifyFailed, exit 4, with advice that depends on whether the disc was appended to"
```

---

### Task 6: Expected contents from any catalog; `DiscBurn::read_back`

A refactor with no behaviour change, so the existing `verify` tests are the safety net.

**Files:**
- Modify: `crates/diskrement-shell/src/pipeline/mod.rs`
- Modify: `crates/diskrement-shell/src/pipeline/reports.rs`
- Modify: `crates/diskrement-cli/src/render.rs` (test constructions only)

- [ ] **Step 1: Extract `disc_expectation`**

Add a free function in `pipeline/mod.rs` next to `resolve_disc`:

```rust
/// What one disc should hold, read from `catalog`: the live catalog for
/// `verify`, or for `burn --verify` the post-burn work copy, which already
/// holds the session being checked.
fn disc_expectation(
    catalog: &Catalog,
    p: &Project,
    disc: DiscKey,
) -> Result<DiscExpectation, PipelineError> {
    let latest_session = catalog
        .placements_for_disc(p.id, disc)
        .map_err(Pipeline::failed)?;
    let recorded = catalog
        .disc_recorded_contents(p.id, disc)
        .map_err(Pipeline::failed)?;
    Ok(DiscExpectation {
        project_name: p.name.clone(),
        disc,
        disc_name: disc.name(p.copies),
        latest_session,
        visible: recorded.visible,
        superseded: recorded.superseded,
    })
}
```

In `verify_disc`, replace everything from `let latest_session = …` through the `DiscExpectation { … };` literal with:

```rust
        let expectation = disc_expectation(self.catalog, p, disc)?;
```

- [ ] **Step 2: Add `DiscReadBack`**

In `pipeline/reports.rs`, before `DiscBurn`:

```rust
/// What `burn --verify` concluded about one disc it wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscReadBack {
    /// Read back clean before it was recorded; its session is VERIFIED.
    Verified { files: usize },
    /// Recorded without a passing read-back: it could not be read back, and
    /// the operator declined to reinsert it.
    Unverified,
}
```

and on `DiscBurn`:

```rust
    /// `None` without `burn --verify`.
    pub read_back: Option<DiscReadBack>,
```

Set `read_back: None,` in the `DiscBurn { … }` literal in `burn_disc` and in the four test literals in `crates/diskrement-cli/src/render.rs` (lines ~1157, ~1201, ~1270, ~1861).

- [ ] **Step 3: Run the tests**

Run: `cargo test -p diskrement-shell && cargo test -p diskrement-cli`
Expected: PASS, with the same test count as before this task.

- [ ] **Step 4: Commit**

```bash
git add crates/diskrement-shell/src/pipeline crates/diskrement-cli/src/render.rs
git commit -m "refactor(verify): a disc's expected contents read from any catalog; burn reports carry a read-back slot"
```

---

### Task 7: The burn → read-back loop (spec §5.1)

**Files:**
- Create: `crates/diskrement-shell/src/pipeline/read_back.rs`
- Create: `crates/diskrement-shell/src/pipeline/read_back_tests.rs`
- Modify: `crates/diskrement-shell/src/pipeline/mod.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/diskrement-shell/src/pipeline/read_back_tests.rs`:

```rust
//! `burn --verify` through the pipeline (burn-verify spec §5.1, §7.2): the
//! real burner and the disc verifier on one scripted drive. A first session
//! is "mounted" at the tree the burner staged, which has a disc's layout; an
//! appended disc is mounted at a hand-built merged view.

use std::cell::{Cell, RefCell};

use chrono::TimeZone as _;

use super::*;
use crate::burn::cmd::Exec;
use crate::burn::testkit::{ScriptedExec, ScriptedOperator, APPENDABLE, BLANK};
use crate::burn::verifier::DiscVerifier;
use crate::burn::xorriso::burner::XorrisoBurner;
use crate::burn::{BurnDevice, BurnOperator};
use crate::progress::NoopSink;
use reports::DiscReadBack;

const ID_1: &str = r#"{"projectName":"p","discOrdinal":1,"sessionOrdinal":1,"createdAt":"2026-08-30T10:00:00Z"}"#;
const ID_1_S2: &str = r#"{"projectName":"p","discOrdinal":1,"sessionOrdinal":2,"createdAt":"2026-08-30T10:00:00Z"}"#;
const ID_1A: &str = r#"{"projectName":"p","discOrdinal":1,"copySet":0,"copies":2,"sessionOrdinal":1,"createdAt":"2026-08-30T10:00:00Z"}"#;
const ID_1B: &str = r#"{"projectName":"p","discOrdinal":1,"copySet":1,"copies":2,"sessionOrdinal":1,"createdAt":"2026-08-30T10:00:00Z"}"#;

struct Env {
    _dir: tempfile::TempDir,
    catalog: Catalog,
    work: PathBuf,
    scan_cache: PathBuf,
}

fn now() -> DateTime<Utc> {
    Utc.timestamp_opt(1_757_100_066, 0).unwrap()
}

fn env(copies: u32) -> Env {
    let dir = tempfile::TempDir::new().unwrap();
    let work = dir.path().join("work");
    let source = dir.path().join("source");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(&source).unwrap();
    let mut catalog = Catalog::open(&dir.path().join("catalog.sqlite")).unwrap();
    catalog
        .create_project(&ProjectInit {
            name: "p".to_string(),
            sources: Sources::Single(source),
            work_dir: work.display().to_string(),
            default_disc_type: DiscType::BdR,
            margin_percent: 2.0,
            copies,
            created_at: now(),
        })
        .unwrap();
    Env {
        scan_cache: dir.path().join("scancache.sqlite"),
        _dir: dir,
        catalog,
        work,
    }
}

fn pipeline(env: &mut Env) -> Pipeline<'_> {
    Pipeline::new(
        &mut env.catalog,
        PipelinePaths {
            work_dir: env.work.clone(),
            scan_cache: env.scan_cache.clone(),
        },
        &NoopSink,
    )
}

/// Put `files` in the snapshot and stage a plan for them against the
/// catalog as it stands, the way `plan` would.
fn stage(env: &mut Env, files: &[(&str, &[u8])]) {
    let snapshot = env.work.join("snapshot");
    std::fs::create_dir_all(&snapshot).unwrap();
    let mut scanned = Vec::new();
    for (rel, body) in files {
        let abs = snapshot.join(rel);
        std::fs::write(&abs, body).unwrap();
        scanned.push(ScannedFile {
            rel_path: RelPath((*rel).to_string()),
            content_hash: hasher::hash(&abs).unwrap(),
            size_bytes: body.len() as u64,
            mtime: now(),
        });
    }
    staged::write_scan(&env.work, &scanned).unwrap();
    pipeline(env).plan_staged("p", None, false).unwrap();
}

/// A disc as its mount shows it: `files` under `data/`, identity `id`.
fn mounted(files: &[(&str, &[u8])], id: &str) -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join("data")).unwrap();
    for (rel, body) in files {
        std::fs::write(dir.path().join("data").join(rel), body).unwrap();
    }
    std::fs::create_dir_all(dir.path().join(".diskrement")).unwrap();
    std::fs::write(dir.path().join(".diskrement/project.json"), id).unwrap();
    dir
}

/// `burn --verify` on the real backend at /dev/sr0.
fn burn_verifying(
    env: &mut Env,
    exec: &dyn Exec,
    op: &dyn BurnOperator,
) -> Result<BurnReport, PipelineError> {
    let device = BurnDevice::RealDevice("/dev/sr0".to_string());
    let burner = Burner::Real(XorrisoBurner::new(device.clone(), exec, op, &NoopSink, 2));
    let verifier = Verifier::Disc(DiscVerifier::new(device, exec, op, &NoopSink));
    let step = VerifyAfterBurn {
        verifier: &verifier,
        operator: op,
    };
    pipeline(env).burn_with("p", now(), &burner, Some(&step))
}

/// `(disc ordinal, copy set, session ordinal)` of every session in `state`.
fn sessions(env: &Env, state: SessionState) -> Vec<(u32, u32, u32)> {
    let mut out: Vec<(u32, u32, u32)> = env
        .catalog
        .sessions_by_state(ProjectId(1), state)
        .unwrap()
        .iter()
        .map(|s| (s.disc.ordinal, s.disc.copy_set, s.ordinal))
        .collect();
    out.sort_unstable();
    out
}

/// Records every command in order, then answers from `script`.
fn recording<'a>(
    script: &'a ScriptedExec,
    calls: &'a RefCell<Vec<String>>,
) -> impl Fn(&[String], &mut dyn FnMut(&str)) -> Result<crate::burn::cmd::CmdOutput, crate::burn::BurnError> + 'a
{
    move |c, l| {
        calls.borrow_mut().push(c.join(" "));
        script.respond(c, l)
    }
}

/// Session 1 of disc 1, burned and read back clean.
fn burn_first_session(env: &mut Env) {
    stage(env, &[("a.bin", b"aaaa")]);
    let tree = env.work.join("staging/disc-1");
    let script = ScriptedExec::new()
        .toc(&[BLANK, APPENDABLE])
        .identities(&[Some(ID_1), Some(ID_1)])
        .mounts(&[Some(&tree)]);
    let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
    burn_verifying(env, &exec, &ScriptedOperator::new(&[true])).unwrap();
}

#[test]
fn a_disc_that_reads_back_clean_is_committed_verified_and_ejected_after() {
    let mut env = env(1);
    stage(&mut env, &[("a.bin", b"aaaa")]);
    let tree = env.work.join("staging/disc-1");
    let script = ScriptedExec::new()
        .toc(&[BLANK, APPENDABLE])
        .identities(&[Some(ID_1), Some(ID_1)])
        .mounts(&[Some(&tree)]);
    let calls = RefCell::new(Vec::new());
    let exec = recording(&script, &calls);
    let op = ScriptedOperator::new(&[true]);

    let report = burn_verifying(&mut env, &exec, &op).unwrap();

    assert_eq!(
        report.discs[0].read_back,
        Some(DiscReadBack::Verified { files: 1 })
    );
    assert_eq!(sessions(&env, SessionState::Verified), vec![(1, 0, 1)]);
    assert!(sessions(&env, SessionState::Burned).is_empty());
    let calls = calls.borrow();
    let mount = calls
        .iter()
        .position(|c| c.starts_with("udisksctl mount"))
        .expect("a read-back");
    let ejects: Vec<usize> = calls
        .iter()
        .enumerate()
        .filter(|(_, c)| c.contains("-eject"))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(ejects.len(), 1, "{calls:?}");
    assert!(ejects[0] > mount, "ejected only after the read-back: {calls:?}");
    assert!(!op.said("Reinsert"), "{:?}", op.log());
}

#[test]
fn a_failed_first_session_is_not_recorded_and_a_retry_on_a_new_blank_passes() {
    let mut env = env(1);
    stage(&mut env, &[("a.bin", b"aaaa")]);
    let tree = env.work.join("staging/disc-1");
    let script = ScriptedExec::new()
        .toc(&[BLANK, APPENDABLE, BLANK, APPENDABLE])
        .identities(&[Some(ID_1), Some(ID_1), Some(ID_1), Some(ID_1)])
        .mounts(&[Some(&tree), Some(&tree)]);
    let burns = Cell::new(0);
    // The first disc comes out bad; the retry, restaged from the snapshot, is good.
    let exec = |c: &[String], l: &mut dyn FnMut(&str)| {
        if c.iter().any(|a| a == "-commit") {
            burns.set(burns.get() + 1);
            if burns.get() == 1 {
                std::fs::write(tree.join("data/a.bin"), b"rot!").unwrap();
            }
        }
        script.respond(c, l)
    };
    // The first blank, then the new blank for the retry.
    let op = ScriptedOperator::new(&[true, true]);

    let report = burn_verifying(&mut env, &exec, &op).unwrap();

    assert_eq!(burns.get(), 2);
    assert!(
        op.said("Disc 1 FAILED verification: 1 of 1 files bad (a.bin). It is not recorded; discard it."),
        "{:?}",
        op.log()
    );
    assert!(op.said("To retry disc 1, insert a new blank disc when asked; decline to stop."));
    assert_eq!(
        report.discs[0].read_back,
        Some(DiscReadBack::Verified { files: 1 })
    );
    assert_eq!(sessions(&env, SessionState::Verified), vec![(1, 0, 1)]);
    assert!(
        sessions(&env, SessionState::Burned).is_empty(),
        "exactly one session, the retry's"
    );
}

#[test]
fn declining_the_retry_stops_with_verify_failed_and_records_nothing() {
    let mut env = env(1);
    stage(&mut env, &[("a.bin", b"aaaa")]);
    let tree = env.work.join("staging/disc-1");
    let script = ScriptedExec::new()
        .toc(&[BLANK, APPENDABLE])
        .identities(&[Some(ID_1), Some(ID_1)])
        .mounts(&[Some(&tree)]);
    let exec = |c: &[String], l: &mut dyn FnMut(&str)| {
        if c.iter().any(|a| a == "-commit") {
            std::fs::write(tree.join("data/a.bin"), b"rot!").unwrap();
        }
        script.respond(c, l)
    };
    let op = ScriptedOperator::new(&[true, false]);

    let err = burn_verifying(&mut env, &exec, &op).unwrap_err();

    assert!(
        matches!(
            &err,
            PipelineError::VerifyFailed { disc, session: 1, bad: 1, checked: 1, appended: false }
                if disc == "1"
        ),
        "{err:?}"
    );
    assert_eq!(err.exit_code(), 4);
    assert!(sessions(&env, SessionState::Burned).is_empty());
    assert!(sessions(&env, SessionState::Verified).is_empty());
    assert!(script.ran("-eject"), "the failed disc is ejected");
    assert!(
        env.work.join("staged/plan.json").is_file(),
        "the plan survives for the next attempt"
    );
}

#[test]
fn a_failed_append_stops_without_a_retry_and_says_to_mark_the_disc_dead() {
    let mut env = env(1);
    burn_first_session(&mut env);
    stage(&mut env, &[("a.bin", b"aaaa"), ("b.bin", b"bbbb")]);
    let disc = mounted(&[("a.bin", b"aaaa"), ("b.bin", b"rot!")], ID_1_S2);
    let script = ScriptedExec::new()
        .toc(&[APPENDABLE, APPENDABLE])
        .identities(&[Some(ID_1), Some(ID_1_S2), Some(ID_1_S2)])
        .mounts(&[Some(disc.path())]);
    let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
    let op = ScriptedOperator::new(&[true]);

    let err = burn_verifying(&mut env, &exec, &op).unwrap_err();

    assert!(
        matches!(
            err,
            PipelineError::VerifyFailed { session: 2, bad: 1, checked: 2, appended: true, .. }
        ),
        "{err:?}"
    );
    assert!(err.to_string().contains("run disc mark-dead 1"), "{err}");
    assert!(!op.said("To retry"), "{:?}", op.log());
    assert_eq!(
        sessions(&env, SessionState::Verified),
        vec![(1, 0, 1)],
        "session 2 was not recorded"
    );
    assert!(sessions(&env, SessionState::Burned).is_empty());
}

#[test]
fn a_mount_still_showing_the_last_session_asks_for_the_disc_again() {
    let mut env = env(1);
    burn_first_session(&mut env);
    stage(&mut env, &[("a.bin", b"aaaa"), ("b.bin", b"bbbb")]);
    let stale = mounted(&[("a.bin", b"aaaa")], ID_1);
    let fresh = mounted(&[("a.bin", b"aaaa"), ("b.bin", b"bbbb")], ID_1_S2);
    let script = ScriptedExec::new()
        .toc(&[APPENDABLE, APPENDABLE, APPENDABLE])
        .identities(&[Some(ID_1), Some(ID_1_S2), Some(ID_1_S2), Some(ID_1_S2)])
        .mounts(&[Some(stale.path()), Some(fresh.path())]);
    let calls = RefCell::new(Vec::new());
    let exec = recording(&script, &calls);
    // Insert for the burn, then reinsert after the stale mount.
    let op = ScriptedOperator::new(&[true, true]);

    let report = burn_verifying(&mut env, &exec, &op).unwrap();

    assert!(
        op.said("Couldn't verify disc 1: the mounted disc still shows session 1"),
        "{:?}",
        op.log()
    );
    assert_eq!(
        report.discs[0].read_back,
        Some(DiscReadBack::Verified { files: 2 })
    );
    assert_eq!(
        sessions(&env, SessionState::Verified),
        vec![(1, 0, 1), (1, 0, 2)]
    );
    let ejects = calls.borrow().iter().filter(|c| c.contains("-eject")).count();
    assert_eq!(ejects, 2, "one before the reinsert, one when done");
}

#[test]
fn declining_to_reinsert_records_the_disc_unverified() {
    let mut env = env(1);
    stage(&mut env, &[("a.bin", b"aaaa")]);
    let script = ScriptedExec::new()
        .toc(&[BLANK, APPENDABLE])
        .identities(&[Some(ID_1), Some(ID_1)])
        .mounts(&[None]);
    let calls = RefCell::new(Vec::new());
    let exec = recording(&script, &calls);
    let op = ScriptedOperator::new(&[true, false]);

    let report = burn_verifying(&mut env, &exec, &op).unwrap();

    assert!(
        op.said("Couldn't verify disc 1: could not mount it"),
        "{:?}",
        op.log()
    );
    assert_eq!(report.discs[0].read_back, Some(DiscReadBack::Unverified));
    assert_eq!(sessions(&env, SessionState::Burned), vec![(1, 0, 1)]);
    assert!(sessions(&env, SessionState::Verified).is_empty());
    let ejects = calls.borrow().iter().filter(|c| c.contains("-eject")).count();
    assert_eq!(ejects, 1, "ejected for the reinsert, and not again");
}

#[test]
fn a_twin_that_fails_after_its_sibling_committed_is_a_partial_burn() {
    let mut env = env(2);
    stage(&mut env, &[("a.bin", b"aaaa")]);
    let tree_a = env.work.join("staging/disc-1A");
    let tree_b = env.work.join("staging/disc-1B");
    let script = ScriptedExec::new()
        .toc(&[BLANK, APPENDABLE, BLANK, APPENDABLE])
        .identities(&[Some(ID_1A), Some(ID_1A), Some(ID_1B), Some(ID_1B)])
        .mounts(&[Some(&tree_a), Some(&tree_b)]);
    let burns = Cell::new(0);
    let exec = |c: &[String], l: &mut dyn FnMut(&str)| {
        if c.iter().any(|a| a == "-commit") {
            burns.set(burns.get() + 1);
            if burns.get() == 2 {
                std::fs::write(tree_b.join("data/a.bin"), b"rot!").unwrap();
            }
        }
        script.respond(c, l)
    };
    // 1A's blank, 1B's blank, then decline 1B's retry.
    let op = ScriptedOperator::new(&[true, true, false]);

    let err = burn_verifying(&mut env, &exec, &op).unwrap_err();

    assert_eq!(err.exit_code(), 4);
    let text = err.to_string();
    assert!(
        text.ends_with("1A committed; 1B failed verification and was not recorded (re-run burn)"),
        "{text}"
    );
    assert!(!text.contains("reset") && !text.contains("not written"), "{text}");
    assert_eq!(sessions(&env, SessionState::Verified), vec![(1, 0, 1)]);
    assert!(sessions(&env, SessionState::Burned).is_empty());
}
```

Register the module in `pipeline/mod.rs`, next to `mod roots_tests;` (match its `#[cfg(test)]` attribute):

```rust
#[cfg(test)]
mod read_back_tests;
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p diskrement-shell pipeline::read_back_tests`
Expected: compile errors (`VerifyAfterBurn` and `burn_with` don't exist).

- [ ] **Step 3: Implement the loop**

Create `crates/diskrement-shell/src/pipeline/read_back.rs`:

```rust
//! `burn --verify`: read each disc back while it is still in the drive,
//! before it is recorded (burn-verify spec §5.1).

use diskrement_core::RelPath;

use crate::burn::verifier::{DiscExpectation, ReadBack, Verifier};
use crate::burn::{BurnError, BurnOperator, BurnResult, Burner, DiscImage};
use crate::error::PipelineError;

use super::{burner_failure, DiscFailure};

/// `burn --verify`'s collaborators: the verifier that matches the burner,
/// and the operator who reinserts or replaces a disc.
pub struct VerifyAfterBurn<'a> {
    pub verifier: &'a Verifier<'a>,
    pub operator: &'a dyn BurnOperator,
}

/// A disc written and read back, ready to be recorded.
pub(super) struct ReadBackBurn {
    pub burned: BurnResult,
    /// `Some(files)` when the read-back passed; `None` when the operator
    /// chose to record the disc unverified.
    pub verified_files: Option<usize>,
    /// Whether the disc has already left the drive.
    pub ejected: bool,
}

enum Settled {
    Passed(usize),
    Failed {
        files: usize,
        mismatches: Vec<RelPath>,
    },
    Unverified,
}

/// How many bad paths a failure line names before `…`.
const SHOWN_MISMATCHES: usize = 5;

/// Burn `image` and read it back until it passes, is left unverified, or
/// fails. A failed first session is offered again on a new blank disc,
/// through the burner's own prompt (declining stops with `VerifyFailed`). A
/// failed append stops at once: its session is on the disc for good.
pub(super) fn burn_and_read_back(
    burner: &Burner<'_>,
    step: &VerifyAfterBurn<'_>,
    image: &DiscImage,
    exp: &DiscExpectation,
) -> Result<ReadBackBurn, DiscFailure> {
    let session = image.metadata.identity.session_ordinal;
    let name = &exp.disc_name;
    let mut failed: Option<PipelineError> = None;
    loop {
        let burned = match (burner.burn(image), failed.take()) {
            (Ok(burned), _) => burned,
            // Declining the retry: the failed disc is the outcome.
            (Err(BurnError::Aborted), Some(err)) => return Err(DiscFailure::Unwritten(err)),
            (Err(e), _) => return Err(burner_failure(e)),
        };
        step.operator
            .progress(&format!("Reading disc {name} back before it is recorded."));
        match settle(burner, step, exp, session) {
            Settled::Passed(files) => {
                return Ok(ReadBackBurn {
                    burned,
                    verified_files: Some(files),
                    ejected: false,
                })
            }
            Settled::Unverified => {
                return Ok(ReadBackBurn {
                    burned,
                    verified_files: None,
                    ejected: true,
                })
            }
            Settled::Failed { files, mismatches } => {
                burner.eject();
                step.operator
                    .progress(&failure_line(name, files, &mismatches));
                let err = PipelineError::VerifyFailed {
                    disc: name.clone(),
                    session,
                    bad: mismatches.len(),
                    checked: files,
                    appended: session > 1,
                };
                if session > 1 {
                    return Err(DiscFailure::Unwritten(err));
                }
                step.operator.progress(&format!(
                    "To retry disc {name}, insert a new blank disc when asked; decline to stop."
                ));
                failed = Some(err);
            }
        }
    }
}

/// Read the disc back, asking for it again for as long as nothing can be
/// learned from it. It is ejected before every such request.
fn settle(
    burner: &Burner<'_>,
    step: &VerifyAfterBurn<'_>,
    exp: &DiscExpectation,
    session: u32,
) -> Settled {
    let name = &exp.disc_name;
    loop {
        let reason = match step.verifier.read_back(exp, session) {
            Ok(ReadBack::Passed { files }) => return Settled::Passed(files),
            Ok(ReadBack::Failed { files, mismatches }) => {
                return Settled::Failed { files, mismatches }
            }
            Ok(ReadBack::Unverifiable(reason)) => reason,
            // Not about the disc (spec §4.2): never a reason to discard it.
            Err(e) => crate::error::with_causes(&e),
        };
        burner.eject();
        if !step.operator.confirm_insert(&format!(
            "Couldn't verify disc {name}: {reason}. Reinsert disc {name} to verify it, \
             or decline to leave it unverified."
        )) {
            return Settled::Unverified;
        }
    }
}

/// `Disc 1A FAILED verification: 3 of 9042 files bad (a.jpg, b.mp4, …). It is
/// not recorded; discard it.`
fn failure_line(name: &str, files: usize, mismatches: &[RelPath]) -> String {
    let mut shown: Vec<&str> = mismatches
        .iter()
        .take(SHOWN_MISMATCHES)
        .map(RelPath::as_str)
        .collect();
    if mismatches.len() > SHOWN_MISMATCHES {
        shown.push("…");
    }
    format!(
        "Disc {name} FAILED verification: {} of {files} files bad ({}). \
         It is not recorded; discard it.",
        mismatches.len(),
        shown.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_line_names_at_most_five_paths() {
        let paths: Vec<RelPath> = (1..=7).map(|i| RelPath(format!("f{i}"))).collect();
        assert_eq!(
            failure_line("1A", 9042, &paths),
            "Disc 1A FAILED verification: 7 of 9042 files bad (f1, f2, f3, f4, f5, …). \
             It is not recorded; discard it."
        );
        assert_eq!(
            failure_line("2", 3, &paths[..1]),
            "Disc 2 FAILED verification: 1 of 3 files bad (f1). It is not recorded; discard it."
        );
    }
}
```

- [ ] **Step 4: Wire it into the pipeline**

In `pipeline/mod.rs`:

1. Module and re-export, next to the other `mod` lines:

```rust
mod read_back;

pub use read_back::VerifyAfterBurn;
```

2. Import `DiscReadBack` in the `use reports::{…}` list.

3. Rename `pub fn burn(` to `pub fn burn_with(`, add the parameter `read_back: Option<&VerifyAfterBurn<'_>>` after `burner`, pass `read_back` into the `self.burn_disc(…)` call, and add a new `burn` above it:

```rust
    /// `burn_with` without a read-back: plain `burn`.
    pub fn burn(
        &mut self,
        name: &str,
        now: DateTime<Utc>,
        burner: &Burner<'_>,
    ) -> Result<BurnReport, PipelineError> {
        self.burn_with(name, now, burner, None)
    }
```

Add to `burn_with`'s doc comment: "With `read_back`, each disc is read back before it is recorded and ejected after (burn-verify spec §5.1)."

4. In `burn_with`'s loop, replace the `Err(DiscFailure::Unwritten(e))` arm:

```rust
                Err(DiscFailure::Unwritten(e)) => {
                    // A disc that failed its read-back is named by the cause,
                    // so it is not also listed as unwritten.
                    let pending = if matches!(e, PipelineError::VerifyFailed { .. }) {
                        &report.sets[1..]
                    } else {
                        &report.sets[..]
                    };
                    return Err(partial(&done, None, pending, e));
                }
```

5. `burn_disc` gets the parameter `read_back: Option<&VerifyAfterBurn<'_>>`. Replace

```rust
        let burned = burner.burn(&image).map_err(burner_failure)?;
```

with:

```rust
        let (burned, read_back_result, ejected) = match read_back {
            None => (burner.burn(&image).map_err(burner_failure)?, None, false),
            Some(step) => {
                // Read before the burn, while nothing else holds the work
                // copy; it already includes this disc's session.
                let exp = {
                    let work = Catalog::open(&tmp_catalog).map_err(DiscFailure::unwritten)?;
                    disc_expectation(&work, p, key).map_err(DiscFailure::Unwritten)?
                };
                let done = read_back::burn_and_read_back(burner, step, &image, &exp)?;
                let status = match done.verified_files {
                    Some(files) => DiscReadBack::Verified { files },
                    None => DiscReadBack::Unverified,
                };
                (done.burned, Some(status), done.ejected)
            }
        };
```

After the commit and `let _ = std::fs::remove_file(&tmp_catalog);`, add:

```rust
        // A clean read-back marks the session VERIFIED, the same forward-only
        // transition `verify` makes. If that write fails, the disc is still
        // recorded, just unverified.
        let read_back_result = match read_back_result {
            Some(DiscReadBack::Verified { files }) => {
                match self.catalog.mark_verified(p.id, key, sess_ord) {
                    Ok(()) => Some(DiscReadBack::Verified { files }),
                    Err(e) => {
                        eprintln!(
                            "disc {disc_name} read back clean but could not be marked verified ({e}); \
                             run verify --disc {disc_name}"
                        );
                        Some(DiscReadBack::Unverified)
                    }
                }
            }
            other => other,
        };
```

Change the eject added in Task 3 to:

```rust
        if !ejected {
            burner.eject();
        }
```

and set `read_back: read_back_result,` in the returned `DiscBurn`.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p diskrement-shell`
Expected: PASS: the eight new pipeline tests, the `failure_line` test, and every existing test.

If `recording`'s return type fails to infer against `&dyn Exec`, replace its uses with an inline closure: `let exec = |c: &[String], l: &mut dyn FnMut(&str)| { calls.borrow_mut().push(c.join(" ")); script.respond(c, l) };`. Record that in Execution deviations.

- [ ] **Step 6: Clippy**

Run: `cargo clippy -p diskrement-shell --all-targets -- -D warnings`
Expected: clean. If `too_many_arguments` fires on `burn_disc`, add `#[allow(clippy::too_many_arguments)]` with a one-line reason comment, and record it.

- [ ] **Step 7: Commit**

```bash
git add crates/diskrement-shell/src/pipeline
git commit -m "feat(burn): burn_with reads each disc back before recording it, retrying a failed first session"
```

---

### Task 8: The CLI flag and the report line

**Files:**
- Modify: `crates/diskrement-cli/src/args.rs`
- Modify: `crates/diskrement-cli/src/main.rs`
- Modify: `crates/diskrement-cli/src/render.rs`
- Modify: `crates/diskrement-cli/tests/binary.rs`

- [ ] **Step 1: Write the failing tests**

`args.rs` `mod tests`: change `let Command::Burn { name, yes } = cli.command else {` to `let Command::Burn { name, yes, .. } = cli.command else {`, and add:

```rust
    #[test]
    fn burn_verify_is_off_unless_asked_for() {
        let verify = |argv: &[&str]| match Cli::try_parse_from(argv).unwrap().command {
            Command::Burn { verify, .. } => verify,
            _ => panic!("not burn"),
        };
        assert!(!verify(&["diskrement", "burn", "p"]));
        assert!(verify(&["diskrement", "burn", "p", "--verify"]));
    }
```

`render.rs` `mod tests`, next to the other `burn_report` tests (the `report(finalized, more_discs)` helper is at ~line 1268, inside a nested module; put the test in the same module):

```rust
    #[test]
    fn a_read_back_disc_says_whether_it_was_verified() {
        use diskrement_shell::pipeline::reports::DiscReadBack;
        let mut r = report(false, 0);
        r.discs[0].read_back = Some(DiscReadBack::Verified { files: 9042 });
        assert!(burn_report(&r, 1).contains(
            "Status: left OPEN — may receive more sessions later.\nRead back: verified (9042 files)."
        ));
        r.discs[0].read_back = Some(DiscReadBack::Unverified);
        assert!(burn_report(&r, 1)
            .contains("Read back: recorded unverified — run verify --disc 1."));
        r.discs[0].read_back = None;
        assert!(!burn_report(&r, 1).contains("Read back"));
    }
```

`binary.rs`, in `mod burn`:

```rust
    /// `burn --verify` on the simulated backend reads the staged disc back
    /// and says so; a later sweep agrees.
    #[test]
    fn burn_verify_reports_the_disc_read_back() {
        let p = project();
        init_and_plan(&p);

        base(&p)
            .args(["burn", "p", "--yes", "--verify"])
            .assert()
            .success()
            .stdout(contains("Reading disc 1 back before it is recorded."))
            .stdout(contains("Read back: verified ("));
        base(&p).args(["verify", "p"]).assert().success();
    }
```

`binary.rs`, in `mod real_backend`, after `a_real_burn_asks_for_the_disc_and_an_unanswered_prompt_aborts`:

```rust
    /// `--verify` changes nothing before the disc prompt: with no stdin the
    /// burn still aborts, exit 0, before the device is touched.
    #[test]
    fn a_real_burn_with_verify_still_asks_for_the_disc_first() {
        if !has_xorriso() {
            eprintln!(
                "SKIP [a_real_burn_with_verify_still_asks_for_the_disc_first]: \
                 xorriso is not installed; the burn path preflights it."
            );
            return;
        }
        let p = project();
        init_and_plan(&p);

        Command::cargo_bin("diskrement")
            .expect("binary builds")
            .arg("--catalog")
            .arg(&p.catalog)
            .args([
                "--burner", "real", "--device", "/dev/sr0", "burn", "p", "--yes", "--verify",
            ])
            .env_remove("DISKREMENT_BURNER")
            .env_remove("DISKREMENT_DEVICE")
            .assert()
            .code(0)
            .stdout(contains(
                "Insert a blank disc for 'p' disc 1 into /dev/sr0 and confirm. [y/N]",
            ))
            .stderr(contains("Aborted."));
        assert!(p.work.join("staged/plan.json").is_file());
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p diskrement-cli`
Expected: compile errors (`verify` field, `read_back` rendering), then the binary test fails on `--verify` as an unknown argument.

- [ ] **Step 3: Implement**

`args.rs`, the `Burn` variant:

```rust
    /// Burn the staged plan (with confirmation).
    Burn {
        name: String,
        #[arg(long, short = 'y', default_value_t = false)]
        yes: bool,
        /// Read each disc back before recording it, and eject it only after.
        /// A disc that reads back bad is not recorded.
        #[arg(long, default_value_t = false)]
        verify: bool,
    },
```

`main.rs`: add `VerifyAfterBurn` to the `diskrement_shell::pipeline` import, change the arm head to `Command::Burn { name, yes, verify } => {`, and replace the `pipeline.burn(…)` line with:

```rust
            // The verifier is chosen by the same decision as the burner, so a
            // simulated burn is read back from its staging tree.
            let verifier = verify.then(|| {
                resolve_verifier(
                    cli.burner,
                    cli.device.as_deref(),
                    std::path::Path::new(&p.work_dir),
                    &exec,
                    &operator,
                    &sink,
                )
            });
            let step = verifier.as_ref().map(|v| VerifyAfterBurn {
                verifier: v,
                operator: &operator,
            });
            let report = pipeline.burn_with(&name, chrono::Utc::now(), &burner, step.as_ref())?;
```

The `verifier` and `step` bindings must be declared before `let mut pipeline = …`, or anywhere before the call, as long as they outlive it. Put them right after the `prompt::confirm` block.

`render.rs` `burn_report`: import `DiscReadBack` in the top-level `use diskrement_shell::pipeline::reports::{…}` and, inside the `for d in &r.discs` loop after the status line push:

```rust
        match d.read_back {
            Some(DiscReadBack::Verified { files }) => {
                lines.push(format!("Read back: verified ({files} files)."));
            }
            Some(DiscReadBack::Unverified) => lines.push(format!(
                "Read back: recorded unverified — run verify --disc {}.",
                d.disc.name(copies)
            )),
            None => {}
        }
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p diskrement-cli`
Expected: PASS. Snapshots are unchanged, because no snapshot case sets `read_back`.

- [ ] **Step 5: Commit**

```bash
git add crates/diskrement-cli
git commit -m "feat(cli): burn --verify reads each disc back before recording it"
```

---

### Task 9: Scenario 16

**Files:**
- Create: `crates/diskrement-cli/tests/scenarios/s16_burn_verify.rs`
- Modify: `crates/diskrement-cli/tests/scenarios/main.rs`

- [ ] **Step 1: Write the scenario**

```rust
//! Scenario 16: `burn --verify` at two copies reads each twin back before
//! recording it. Both discs report verified, and a later sweep agrees.

use crate::support;

#[test]
fn burn_verify_reads_back_both_twins_before_recording_them() {
    let Some(s) = support::scratch_or_skip("s16 burn --verify") else {
        return;
    };
    let cli = s.cli();
    support::write(&s.source, "a.txt", 128);
    support::write(&s.source, "b.txt", 128);
    let init = cli.run(&[
        "init",
        "p",
        "--source",
        &s.source.display().to_string(),
        "--work-dir",
        &s.work.display().to_string(),
        "--copies",
        "2",
    ]);
    assert_eq!(init.code, 0, "init: {}{}", init.stdout, init.stderr);
    let plan = cli.run(&["plan", "p"]);
    assert_eq!(plan.code, 0, "plan: {}{}", plan.stdout, plan.stderr);

    let burn = cli.run(&["burn", "p", "--yes", "--verify"]);
    assert_eq!(burn.code, 0, "{}{}", burn.stdout, burn.stderr);
    assert!(burn.out_has("Wrote disc 1A"), "{}", burn.stdout);
    assert!(burn.out_has("Wrote disc 1B"), "{}", burn.stdout);
    assert_eq!(
        burn.stdout.matches("Read back: verified (2 files).").count(),
        2,
        "{}",
        burn.stdout
    );

    let verify = cli.run(&["verify", "p"]);
    assert_eq!(verify.code, 0, "{}{}", verify.stdout, verify.stderr);
}
```

Add `mod s16_burn_verify;` after `mod s15_roots;` in `main.rs`.

- [ ] **Step 2: Run it with scenarios required**

Run: `DISKREMENT_SCENARIOS_REQUIRED=1 cargo test -p diskrement-cli --test scenarios s16`
Expected: PASS. If it fails with a CoW error, set `TMPDIR` to a btrfs/XFS directory and re-run; never report a `SKIPPED` run as a pass.

- [ ] **Step 3: Commit**

```bash
git add crates/diskrement-cli/tests/scenarios
git commit -m "test(cli): scenario 16, burn --verify reads back both twins"
```

---

### Task 10: Docs and the full verification

**Files:**
- Modify: `CLAUDE.md`
- Modify: `docs/smoke-test-real-media.md`
- Modify: this plan (Execution deviations)

- [ ] **Step 1: CLAUDE.md**

Under "Gotchas that will bite", after the "A real burn deletes each disc's staging tree" bullet, add:

```markdown
- **The pipeline ejects, not the burner.** `XorrisoBurner::burn` leaves the disc
  in the drive; `burn_disc` calls `Burner::eject` after the commit, or under
  `burn --verify` after the read-back (`pipeline/read_back.rs`). The read-back
  happens before the commit, against expected contents read from the post-burn
  work copy (`disc_expectation`), and a failed disc is never recorded.
- **A read-back trusts the mount only if it shows the new session.** Without an
  eject the kernel may still show the previous session; `DiscVerifier::read_back`
  compares the mounted `/.diskrement/project.json` with the session just written
  and calls a mismatch `Unverifiable` (reinsert), never `Failed`. Otherwise a
  good appended disc would read as missing its new files.
```

- [ ] **Step 2: Smoke test item 12**

Append after item 11 in `docs/smoke-test-real-media.md`:

```markdown
- [ ] 12. `burn --verify` on real media, before relying on the flag (burn-verify
      spec §2, §7.4). (a) A blank BD-R: after `Writing done`, `Reading disc <name>
      back before it is recorded.`, `Mounting disc <name>…` and the verifying
      line appear with no eject, then `Read back: verified (N files).`; the
      drive ejects only then. (b) An append to an open disc: the same, and the
      mount shows the new session. Record whether `udisksctl` mounted the new
      session without a reload. If it did not, confirm that the `Couldn't verify
      … Reinsert` prompt appeared and that the reinserted disc verified.
```

- [ ] **Step 3: Full verification**

Run each and read the counts:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
DISKREMENT_SCENARIOS_REQUIRED=1 cargo test --workspace
```

Expected: all pass. The test count is higher than on `main` by the tests added here (about 20), and no scenario prints `SKIPPED`. Note whether `xorriso_image.rs` and the `real_backend` binary tests ran or skipped (they need `xorriso`).

- [ ] **Step 4: Fill in Execution deviations below** (or write "None").

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md docs/smoke-test-real-media.md docs/superpowers/plans/2026-10-16-burn-verify.md
git commit -m "docs: burn --verify in the gotchas and the real-media smoke test"
```

- [ ] **Step 6: Merge** (repo etiquette, after the user approves)

```bash
git switch main
git merge --no-ff feat/burn-verify -m "Merge feat/burn-verify: burn --verify reads each disc back before recording and ejecting it"
git branch -d feat/burn-verify
git push origin main
git push origin --delete feat/burn-verify
```

(The branch was never pushed. If `--delete` reports it is absent on `origin`, that is fine.)

---

## Execution deviations

Executed inline on 2026-10-16 (not subagent-driven, at the user's choice), with one review subagent at the end.

1. **`recording` returns `impl Exec + 'a`.** The planned `impl Fn(…) -> Result<…>` return type, and the same with a type alias, both fail `clippy::type_complexity`. `Exec` is the blanket-implemented trait the closure stands for anyway.
2. **Seven read-back pipeline tests, not eight.** The plan counted Task 3's `a_real_burn_ejects_once_the_disc_is_committed` (the no-verify path) among them.
3. **Tasks 1, 6 and 9 were not seen failing first.** Task 1's test is for test infrastructure, Task 6 is a refactor guarded by the existing tests, and scenario 16 was written after the feature it drives. Every other task's tests were run red before the implementation.
4. **Test count:** 698 on `main` → 724 (`DISKREMENT_SCENARIOS_REQUIRED=1`, xorriso present, nothing skipped).
