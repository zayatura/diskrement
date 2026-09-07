# Rust Rewrite P8 — the real xorriso backend: media inspector, disc reader, read-back verify, backend resolution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land P8 — `burn::cmd` (the process edge), `burn::xorriso::{args,parse,burner}`,
`burn::media` (the media inspector), `burn::reader` (udisks mount / osirrox
extraction), the two-tier read-back `DiscVerifier`, and the CLI wiring that
finally makes `--burner real` do something — ending with a Rust binary that can
burn to and verify physical media, proven end to end against real `xorriso`
through `stdio:` image files, with the burner/verifier environment semantics
pinned in all three differential modes.

**Architecture:** The Scala build expressed this subsystem as five injected
services (`CmdRunner`, `MediaInspector`, `BurnOperator`, `DiscReader`,
`DiscVerifier`) plus two pure objects (`Xorriso`, `MediaParse`). Spec §5.2 says
a port becomes *a pure function plus a thin, boring edge*, and that is the
shape here: everything that decides anything moves into `xorriso::args` (what we
send) and `xorriso::parse` (what we read), both testable with zero I/O; what
remains is a ~70-line `burn::cmd` that spawns a process and streams its combined
output. Two seams survive, because two implementations genuinely ship: the
`Burner` enum gains its `Real` arm and the `Verifier` enum gains its `Disc` arm,
selected together by the one backend decision the CLI already makes. A third,
smaller seam is added deliberately and argued for below (`burn::cmd::Exec`),
without which the entire error-classification surface — `ToolTooOld`,
`DevicePermissionDenied`, `InsufficientCapacity`, `PostBurnCheckFailed` — would
have no test at all.

**Tech Stack:** Rust 2021; `serde`, `serde_json`, `rusqlite` (bundled), `sha2`,
`rayon`, `reflink-copy`, `walkdir`, `chrono`, `clap` 4 derive, `assert_cmd`,
`predicates`, `insta`, `proptest`. **One dependency move, no new dependency:**
`tempfile` becomes a runtime dependency of `diskrement-shell` (it is already a
dev-dependency and already in the lockfile) — see *Design decisions*, D2.

**Spec:** `docs/superpowers/specs/2026-09-05-rust-rewrite-design.md` — §1 (both
burner backends and the real xorriso path are in scope; the deferred debt is
ported as-is), §3.1 (`/.diskrement/project.json` is the disc's identity and the
timestamp rule governs it), §3.2 (`DISKREMENT_DEVICE` / `DISKREMENT_BURNER`,
exit 6 = media/burner, an aborted burn exits 0), §5.2 (a port becomes a pure
function plus a thin edge; the two closed backend enums), §5.3 (the file layout
this phase fills in: `burn/{reader,media,cmd}.rs` and `burn/xorriso/{args,parse,burner}.rs`),
§5.4 (the three external commands that survive: `xorriso`, `udisksctl`, `mount`),
§7 (a skip must state its reason), §8.1 (the differential task every phase from
P3 carries), §9 (the P8 row: *"Real xorriso backend, media inspector, disc
reader, udisks read-back verify, backend resolution"*, ending with *"Image IT
green where `xorriso` is installed; burner env semantics pinned"*), §11 (this is
the hardware-dependent phase; its hardware half is a P10 bar item).
The Scala sources are the behavioral oracle: **read the file named in each task
before porting it.**

---

## Global Constraints

- **Branch:** `feat/rewrite-p8`, forked from `feat/rust-rewrite`. Never commit
  directly to `feat/rust-rewrite`. Merge back at the end with an explicit
  `Merge feat/rewrite-p8: <summary>` commit and delete the phase branch.

```bash
git switch feat/rust-rewrite && git switch -c feat/rewrite-p8
```

- **Do not touch** `core/`, `shell/`, `cli/`, `build.sbt`, `project/`,
  `fixtures/`, `scripts/harvest-fixtures.sh`. The Scala tree must stay
  buildable until P10, and the corpus is a frozen, spec-level artifact.
- **After every task:**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

  All clean, with no drop in test count. Baseline at plan start, measured on
  `feat/rust-rewrite` at `5dcaf83`: **443 passed, 0 failed, 3 ignored**
  (the three `#[ignore]`d differential entry points).
- **Commit scopes:** `feat(burn):`, `test(burn):`, `feat(cli):`, `test(cli):`,
  `feat(difftest):`, `docs:`, `docs(plan):`.
- **User-visible strings and exit codes are contract** (§3.2). Copy them
  verbatim from the Scala source named in each task. Every Scala `toString`
  shape quoted in this plan was captured by running a throwaway `main` through
  `sbt shell/Test/runMain` against the real types — they are transcripts, not
  guesses (see *Design decisions*, D3). The only sanctioned divergence is
  clap's own usage/argument-error wording.
- **`-D warnings` is on for the whole workspace** (`.cargo/config.toml`). In
  `diskrement-shell` (a library) a `pub` item is public API and never trips
  `dead_code`, so the intermediate tasks compile clean without attributes. In
  `diskrement-cli` (a binary) nothing is public API: a function added before it
  is wired needs `#[allow(dead_code)]` **removed again in the commit that wires
  it**, exactly as P5–P7 did. Leave no stale attribute behind.
- **No new external process is introduced.** `xorriso` and `udisksctl` are the
  two commands this phase spawns; §5.4 already accounts for them. Argument
  vectors are built by `xorriso::args` and passed to `Exec` — never assembled
  as a shell string, never passed through `sh -c`.
- **`Pipeline` returns data and does no I/O path resolution** (CLAUDE.md).
  P8 adds no pipeline method and changes no pipeline logic: the burner and the
  verifier are values the CLI constructs and passes in, exactly as in P5/P6.
  The only pipeline edit in this phase is one lifetime on `verify`'s parameter.
- **Commit after success stays intact.** `Pipeline::burn` commits the catalog
  only after `burner.burn(&image)` returns `Ok`. Every new failure mode in this
  phase (wrong disc, insufficient capacity, xorriso failure, post-burn check)
  therefore leaves the catalog untouched, and a test in Task 12 proves it
  through the binary.
- **A skipped test says so out loud** (§7). Tests that need `xorriso` print
  the reason and the fix and return; the suite prints how many scenarios
  actually executed. A green run that quietly skipped everything is the failure
  mode this rule exists to prevent — the same one CLAUDE.md records for the sbt
  IT suite.
- **Deferred debt is ported as-is** (§1). In this phase specifically:
  `writeDisc` does not refuse a dead disc, and `archivePointers` has no dead
  flag. Do not repair either.

---

## What already exists (do not rebuild it)

Read this table before writing anything, and delete nothing it names.

| Already landed | Where | Pinned by |
|---|---|---|
| `BurnDevice` (both variants) and `xorriso_address()` — `stdio:` prefixing included | `crates/diskrement-shell/src/burn/mod.rs:66-83` | used by every task here |
| `DiscIdentity`, `DiscMetadata`, `DiscImage`, `BurnResult` | `crates/diskrement-shell/src/burn/mod.rs:20-63` | P5 |
| `BurnError::{HashMismatch, SourceFileMissing, Io, Aborted}` and the exit ladder (`Burn(_) => 6`, `Burn(Aborted) => 0`) | `crates/diskrement-shell/src/burn/mod.rs:85-105`, `src/error.rs:135-147` | `error.rs` test `burn_errors_are_exit_6_and_an_abort_is_exit_0` |
| `stager::stage` — the staged tree both backends write and xorriso masters | `crates/diskrement-shell/src/burn/stager.rs:24` | `tests/fixture_burn.rs` |
| `snapshot_writer::write` — `/.diskrement/{project,manifest,pointers}.json` + `catalog.sqlite` | `crates/diskrement-shell/src/burn/snapshot_writer.rs:13` | `tests/fixture_*.rs` |
| `json::ProjectJson::{from,to_identity}` — the identity the inspector reads back off a disc | `crates/diskrement-shell/src/burn/json.rs:111-140` | `tests/fixture_pointers.rs`, `timestamps.rs` |
| `Burner` enum with its `Simulated` arm, and `Pipeline::burn(name, now, &burner)` | `burn/mod.rs:107-118`, `pipeline/mod.rs:224` | P5 |
| `Verifier` enum with its `Staging` arm, `DiscExpectation` (`visible` + `superseded` already populated by the catalog), `VerifyOutcome` | `burn/verifier.rs` | P6 |
| `DiscRecordedContents` / `SupersededVersion` — what tier 1 and tier 2 check against | `catalog/model.rs:204-219`, `catalog/read.rs` | P4/P6 |
| `backend::resolve` — flag > env > `/dev/sr0`, and the empty-`DISKREMENT_DEVICE` rule | `crates/diskrement-cli/src/backend.rs` | six unit tests in that file |
| `prompt::confirm{,_from}` — `[y/N] `, EOF is a refusal | `crates/diskrement-cli/src/prompt.rs` | three unit tests there |
| `progress::ConsoleProgress`, `NoopSink`, `progress::phase` | `cli/src/progress.rs`, `shell/src/progress.rs` | P3 |
| `fs::hasher::hash`, `fs::scanner::relativize`, `fs::scanner::parallelism_from_env` | `shell/src/fs/` | P3 |
| The differential harness: scenarios, runner, normalizer, state diff, `RUST_IMPLEMENTED` | `crates/diskrement-difftest/` | 12 scenarios, 3 modes, 0 skipped since P7 |

**What is missing, and is the whole of P8:** the process edge (Task 1), the
media/burn error variants (Task 2), the xorriso argument builders (Task 3), the
output parsers (Task 4), the media inspector (Task 5), the disc reader (Task 6),
the operator port and the shared test kit (Task 7), the xorriso burner (Task 8),
the read-back disc verifier (Task 9), the image-backed integration suite against
real `xorriso` (Task 10), the CLI wiring and the `Aborted.` render (Task 11),
the binary-level tests (Task 12), the differential task (Task 13), and landing
(Task 14).

---

## Scala oracle files (read before porting each area)

| Area | File |
|---|---|
| `CmdRunner`, `CmdResult.tail` | `shell/src/main/scala/diskrement/shell/burn/CmdRunner.scala` |
| The `BurnError` sealed hierarchy and its exact messages | `shell/src/main/scala/diskrement/shell/burn/Burn.scala` |
| Command builders, `volid`, `parseVersion` | `shell/src/main/scala/diskrement/shell/burn/Xorriso.scala` |
| `-toc` parsing and failure classification | `shell/src/main/scala/diskrement/shell/burn/MediaParse.scala` |
| `MediaStatus`, `inspect`, `readIdentity` | `shell/src/main/scala/diskrement/shell/burn/MediaInspector.scala` |
| The operator port | `shell/src/main/scala/diskrement/shell/burn/BurnOperator.scala` |
| udisks mount / osirrox extraction, mount-point parsing | `shell/src/main/scala/diskrement/shell/burn/DiscReader.scala` |
| The real burner: preflight, media wait loop, capacity, burn, post-burn check, eject | `shell/src/main/scala/diskrement/shell/burn/XorrisoBurner.scala` |
| Two-tier read-back verification | `shell/src/main/scala/diskrement/shell/burn/DiscVerifier.scala` |
| Wiring the real backend, the console operator | `cli/src/main/scala/diskrement/cli/Runner.scala` (`ConsoleBurnOperator`, `withPipeline`'s `Backend.Real` arm) |
| `Render.handle`'s `Aborted.` special case | `cli/src/main/scala/diskrement/cli/Render.scala:95-117` |
| The test shapes being ported | `shell/src/test/scala/diskrement/shell/burn/{XorrisoSpec,MediaParseSpec,MediaInspectorSpec,DiscReaderSpec,XorrisoBurnerSpec,RealDiscVerifierSpec,XorrisoImageITSpec,FakeCmdRunner}.scala`, `cli/src/test/scala/diskrement/cli/ExitMappingSpec.scala` |
| Validated xorriso command shapes (do not re-derive) | `docs/superpowers/plans/2026-08-30-diskrement-plan-4a-real-burning.md`, §"Validated xorriso facts" |

---

## Design decisions taken in this plan

Four decisions are made here rather than left to the implementer, because each
is a deliberate, arguable deviation and the reason must travel with it.

**D1 — `burn::cmd::Exec` is a seam, and spec §5.2 said `CmdRunner` would not
be.** §5.2's instruction is honoured where it pays: everything that *decides*
anything is now a pure function in `xorriso::args` / `xorriso::parse`, unit
tested with no process at all. What is left is genuinely thin. But the burner
and the inspector still have branch logic driven by what the process *said* —
`ToolTooOld`, `DevicePermissionDenied`, `DeviceNotFound`, `InsufficientCapacity`,
`PostBurnCheckFailed`, the wrong-disc re-prompt loop — and none of those states
can be produced by running real `xorriso` against an image file. Dropping the
seam would mean deleting six of `XorrisoBurnerSpec`'s eight cases and four of
`MediaInspectorSpec`'s six, with the real-media checklist as their only
remaining cover. So a one-method `Exec` trait stays, with a blanket impl for
closures, so a test substitutes a closure and never a hand-written struct. It
is smaller than `CmdRunner` was: no `ZLayer`, no live/fake pair, no service
lookup — a function type with a name.

**D2 — `tempfile` moves from dev-dependencies to dependencies of
`diskrement-shell`.** Three production paths need a temporary file: the
inspector extracting `/.diskrement/project.json` to read a disc's identity, the
verifier extracting a superseded version for tier 2, and the reader extracting
`/data` from an image file. The Scala build used `Files.createTempFile`. The
alternative — composing a name from the pid in `std::env::temp_dir()` — is a
predictable path in a world-writable directory, i.e. a symlink-attack surface,
and re-implementing `O_EXCL` naming is exactly the wheel `tempfile` is. The
crate is already in the lockfile as a dev-dependency of this same crate, so the
dependency *tree* does not change; only the section it is declared in does.
Spec §5.4 lists it under Dev, so this is a recorded deviation, not an oversight.

**D3 — `DiscIdentity` and `MediaStatus` get `Display` impls that reproduce
Scala's `toString` exactly.** Two user-visible strings interpolate them:
`PostBurnCheckFailed`'s detail (`…found: $other`, an `Option[DiscIdentity]`) and
the verifier's re-prompt line (`Not the expected disc (found: $found)`, a
`MediaStatus`). Rust's `Debug` would print `Appendable { free_bytes: 123, .. }`
where Scala prints `Appendable(123,2,Some(DiscIdentity(photos,1,2,2026-08-30T10:00:00Z)))`,
and §3.2 lets wording drift only in clap's argument errors. The exact shapes
below were captured from the Scala types themselves:

```
DiscIdentity(photos,1,2,2026-08-30T10:00:00Z)
Some(DiscIdentity(photos,1,2,2026-08-30T10:00:00Z))
None
Blank(24220008448)
Appendable(123,2,Some(DiscIdentity(photos,1,2,2026-08-30T10:00:00Z)))
Appendable(123,2,None)
Closed(Some(DiscIdentity(photos,1,2,2026-08-30T10:00:00Z)))
NoMedia
Unsuitable
```

**D4 — four `BurnError` variants are deliberately not ported.**
`NotBlank`, `DiscClosed`, `ForeignDisc` and `WrongDisc` exist in
`Burn.scala` and are constructed **nowhere** in the Scala tree (verified by
grep across `core/`, `shell/`, `cli/` including tests): the wrong-media cases
they read like are handled by the burner's re-prompt loop, which reports them
as operator text and never as an error value. Porting them would add four
`Display` strings nothing can reach and nothing can test. `NoMedia` *is*
ported, because `ExitMappingSpec` pins it. If a real-media finding ever needs
one of the four, it is a one-line addition; record it in the plan's deviations
if that happens.

---

## File Structure

**Created (shell):**

| File | Responsibility |
|---|---|
| `crates/diskrement-shell/src/burn/cmd.rs` | `CmdOutput`, `tail`, the `Exec` seam, and `system` — spawn one command, merge stderr into stdout, stream lines |
| `crates/diskrement-shell/src/burn/xorriso/mod.rs` | the xorriso submodule root: `args`, `parse`, `burner` |
| `crates/diskrement-shell/src/burn/xorriso/args.rs` | pure argument vectors + `volid` |
| `crates/diskrement-shell/src/burn/xorriso/parse.rs` | pure parsers: version, `-toc` media state, failure classification |
| `crates/diskrement-shell/src/burn/xorriso/burner.rs` | `XorrisoBurner`: preflight → media wait → stage → capacity → burn → post-burn check → eject |
| `crates/diskrement-shell/src/burn/media.rs` | `MediaStatus` (+`Display`) and `MediaInspector` |
| `crates/diskrement-shell/src/burn/reader.rs` | `DiscReader`: udisks mount for real devices, osirrox extraction for images |
| `crates/diskrement-shell/src/burn/testkit.rs` | `#[cfg(any(test, feature = "testutil"))]` — `ScriptedOperator`, `has_xorriso`, the image-fixture builders |
| `crates/diskrement-shell/tests/xorriso_image.rs` | the image-backed integration suite against real `xorriso` |

**Modified (shell):**

| File | Change |
|---|---|
| `crates/diskrement-shell/src/burn/mod.rs` | new modules; the `BurnOperator` trait; the new `BurnError` variants; `DiscIdentity: Display`; `Burner::Real` |
| `crates/diskrement-shell/src/burn/verifier.rs` | `DiscVerifier` and `Verifier::Disc`; `Verifier` gains a lifetime |
| `crates/diskrement-shell/src/pipeline/mod.rs` | one signature: `verifier: &Verifier<'_>` |
| `crates/diskrement-shell/Cargo.toml` | `tempfile` moves into `[dependencies]` (D2) |

**Modified (cli):**

| File | Change |
|---|---|
| `crates/diskrement-cli/src/main.rs` | the real burner and real verifier arms; the error line goes through `render` |
| `crates/diskrement-cli/src/prompt.rs` | `ConsoleOperator` — the `BurnOperator` the CLI supplies |
| `crates/diskrement-cli/src/render.rs` | `error_line`: `Burn(Aborted)` renders `Aborted.` |
| `crates/diskrement-cli/tests/binary.rs` | the two "not implemented yet" tests are replaced by real-backend tests |

**Modified (difftest, deleted at P10):**

| File | Change |
|---|---|
| `crates/diskrement-difftest/src/scenario.rs` | per-step `Backend`; per-scenario `requires` |
| `crates/diskrement-difftest/src/runner.rs` | apply the step's backend env; probe required tools and skip loudly |
| `crates/diskrement-difftest/src/corpus.rs` | the `real-backend` scenario |

**Modified (docs):** `docs/smoke-test-real-media.md`, `CLAUDE.md`.

---

### Task 1: `burn::cmd` — the process edge

**Read first:** `shell/src/main/scala/diskrement/shell/burn/CmdRunner.scala`.

**Files:**
- Create: `crates/diskrement-shell/src/burn/cmd.rs`
- Modify: `crates/diskrement-shell/src/burn/mod.rs` (add `pub mod cmd;`)

The Scala runner does four things: launch, classify a launch failure as
`ToolMissing`, stream every combined stdout/stderr line through a callback as it
appears, and return `CmdResult(exitCode, lines)` — a nonzero exit is a normal
result, never an error. `redirectErrorStream(true)` has no `std::process`
equivalent, so the port reads both pipes on their own threads and appends into
one shared buffer, which reproduces the interleaving closely enough for the two
consumers that care: `tail(10)` (an error message) and `classify_failure` (which
lowercases and concatenates everything, so order cannot matter).

- [ ] **Step 1: Write the failing tests**

Create `crates/diskrement-shell/src/burn/cmd.rs` with only the test module for
now (the code below compiles once Step 3 lands):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Both streams are captured, a nonzero exit is a value and not an error,
    /// and every line reaches the callback as well as the buffer
    /// (`redirectErrorStream(true)` + the `onLine` hook).
    #[test]
    fn captures_both_streams_and_returns_a_nonzero_exit_as_a_value() {
        let mut seen = Vec::new();
        let out = system(
            &[
                "/bin/sh".to_string(),
                "-c".to_string(),
                "echo to-stdout; echo to-stderr 1>&2; exit 3".to_string(),
            ],
            &mut |line| seen.push(line.to_string()),
        )
        .expect("the process launched");

        assert_eq!(out.exit_code, 3);
        let mut lines = out.lines.clone();
        lines.sort();
        assert_eq!(lines, vec!["to-stderr".to_string(), "to-stdout".to_string()]);
        seen.sort();
        assert_eq!(seen, lines, "every captured line also reached the callback");
    }

    /// A missing binary is `ToolMissing`, not an I/O error: the message tells
    /// the operator what to install (`BurnError.ToolMissing` in Burn.scala).
    #[test]
    fn a_missing_binary_is_tool_missing_and_names_the_command() {
        let err = system(
            &["definitely-not-a-real-binary-xyz".to_string()],
            &mut |_| {},
        )
        .expect_err("cannot launch");
        let msg = err.to_string();
        assert!(msg.contains("xorriso is not available"), "{msg}");
        assert!(
            msg.contains("cannot launch definitely-not-a-real-binary-xyz"),
            "{msg}"
        );
        assert!(msg.contains("libisoburn/xorriso"), "{msg}");
    }

    /// `CmdResult.tail(10)`: the last ten lines, or all of them when there are
    /// fewer. It is what every `BurnFailed` message quotes.
    #[test]
    fn tail_returns_the_last_lines_in_order() {
        let out = CmdOutput {
            exit_code: 5,
            lines: (0..12).map(|i| format!("line {i}")).collect(),
        };
        assert_eq!(out.tail(10).len(), 10);
        assert_eq!(out.tail(10)[0], "line 2");
        assert_eq!(out.tail(10)[9], "line 11");
        let short = CmdOutput {
            exit_code: 0,
            lines: vec!["only".to_string()],
        };
        assert_eq!(short.tail(10), vec!["only".to_string()]);
    }

    /// The blanket impl: any closure of the right shape is an `Exec`, which is
    /// what lets a test substitute canned output without a hand-written type.
    #[test]
    fn a_closure_is_an_exec() {
        let fake = |cmd: &[String], _: &mut dyn FnMut(&str)| {
            Ok(CmdOutput {
                exit_code: 0,
                lines: vec![cmd.join(" ")],
            })
        };
        let exec: &dyn Exec = &fake;
        let out = exec.run_quiet(&["xorriso".to_string(), "--version".to_string()]);
        assert_eq!(out.unwrap().lines, vec!["xorriso --version".to_string()]);
    }
}
```

- [ ] **Step 2: Run them and watch them fail to compile**

```bash
cargo test -p diskrement-shell burn::cmd
```

Expected: compile errors — `cannot find function 'system'`, `cannot find type
'CmdOutput'`, `cannot find trait 'Exec'`. That is the failing state; a test that
cannot name its subject has not passed by accident.

- [ ] **Step 3: Write the implementation**

Put this **above** the test module in `crates/diskrement-shell/src/burn/cmd.rs`:

```rust
//! The one place a child process is spawned (`CmdRunner.scala`).
//!
//! Everything that decides anything lives in `xorriso::args` and
//! `xorriso::parse`; what is left here is the edge (spec §5.2). A nonzero exit
//! is a normal `CmdOutput`, never an error — only a failure to *launch* is an
//! error, because only that means the operator has something to install.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use crate::burn::BurnError;

/// One finished command: its exit code and its combined output lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmdOutput {
    pub exit_code: i32,
    pub lines: Vec<String>,
}

impl CmdOutput {
    /// The last `n` lines — what a `BurnFailed` message quotes
    /// (`CmdResult.tail`, whose Scala default is 10).
    pub fn tail(&self, n: usize) -> Vec<String> {
        self.lines[self.lines.len().saturating_sub(n)..].to_vec()
    }
}

/// Running one external command.
///
/// A trait rather than a bare function so unit tests can substitute canned
/// output for states real `xorriso` cannot be talked into producing — an old
/// version, a permission denial, a post-burn identity mismatch. The blanket
/// impl below means a test writes a closure, never a struct.
pub trait Exec {
    fn run(
        &self,
        command: &[String],
        on_line: &mut dyn FnMut(&str),
    ) -> Result<CmdOutput, BurnError>;

    /// `run` for the callers that have no progress to report.
    fn run_quiet(&self, command: &[String]) -> Result<CmdOutput, BurnError> {
        self.run(command, &mut |_| {})
    }
}

impl<F> Exec for F
where
    F: Fn(&[String], &mut dyn FnMut(&str)) -> Result<CmdOutput, BurnError>,
{
    fn run(
        &self,
        command: &[String],
        on_line: &mut dyn FnMut(&str),
    ) -> Result<CmdOutput, BurnError> {
        self(command, on_line)
    }
}

/// The production `Exec`: spawn, stream, wait.
///
/// stdout and stderr are read on separate threads and appended to one buffer,
/// which is what `ProcessBuilder.redirectErrorStream(true)` did in one call.
/// The two consumers of the order — `tail` in an error message and
/// `classify_failure`, which concatenates everything — are both insensitive to
/// how the two streams interleave, so approximating it is enough.
pub fn system(
    command: &[String],
    on_line: &mut dyn FnMut(&str),
) -> Result<CmdOutput, BurnError> {
    let (program, args) = command
        .split_first()
        .expect("an argument vector always names its program");

    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| match e.kind() {
            // `BurnError.ToolMissing`: the Scala matched the IOException's
            // English text for "no such file"; an ErrorKind cannot drift with
            // a locale or a JDK message change.
            std::io::ErrorKind::NotFound => {
                BurnError::ToolMissing(format!("cannot launch {program}: {e}"))
            }
            _ => BurnError::Io(e),
        })?;

    let collected: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");

    // stderr goes to the shared buffer from its own thread; stdout is read on
    // this one, so `on_line` (which the caller may borrow mutably) never
    // crosses a thread boundary.
    let err_sink = Arc::clone(&collected);
    let err_thread = std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            err_sink.lock().expect("no panic holds this lock").push(line);
        }
    });

    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        on_line(&line);
        collected
            .lock()
            .expect("no panic holds this lock")
            .push(line);
    }
    let _ = err_thread.join();

    let status = child.wait().map_err(BurnError::Io)?;
    let lines = collected.lock().expect("no panic holds this lock").clone();
    // A signal-killed child has no code. -1 is not a valid exit code, so it
    // can never collide with one xorriso actually returned.
    Ok(CmdOutput {
        exit_code: status.code().unwrap_or(-1),
        lines,
    })
}
```

Then, in `crates/diskrement-shell/src/burn/mod.rs`, add to the module list at
the top (keep it alphabetical):

```rust
pub mod cmd;
```

**Note:** stderr lines do not reach `on_line`. The only `on_line` consumer is
the burner's progress hook, which filters for xorriso's `UPDATE` lines; those
are on stdout. Recording this here so a later reader does not "fix" it.

- [ ] **Step 4: Add the error variants this task's tests need**

The `ToolMissing` variant does not exist yet. Task 2 adds the full set; to keep
this task's tests runnable, add just this one now, to `BurnError` in
`crates/diskrement-shell/src/burn/mod.rs`, immediately after `Io`:

```rust
    #[error("xorriso is not available: {0} (install the libisoburn/xorriso package)")]
    ToolMissing(String),
```

- [ ] **Step 5: Run the tests**

```bash
cargo test -p diskrement-shell burn::cmd
```

Expected: 4 passed.

- [ ] **Step 6: Full verification and commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/diskrement-shell/src/burn/cmd.rs crates/diskrement-shell/src/burn/mod.rs
git commit -m "feat(burn): spawn one external command and stream its output"
```

---

### Task 2: The media and burner error variants

**Read first:** `shell/src/main/scala/diskrement/shell/burn/Burn.scala` (the
`BurnError` object) and `cli/src/test/scala/diskrement/cli/ExitMappingSpec.scala`.

**Files:**
- Modify: `crates/diskrement-shell/src/burn/mod.rs`
- Modify: `crates/diskrement-shell/src/error.rs` (tests only)

Every message is contract: it is what the CLI prints on stderr before exiting 6.
Copy each string from `Burn.scala` character for character. Four variants are
deliberately omitted — see *Design decisions*, D4.

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `crates/diskrement-shell/src/error.rs`:

```rust
    /// Every media/burner failure is exit 6, and every message is the Scala
    /// one verbatim (`Burn.scala`). The abort is the single exception, and it
    /// is already pinned above.
    #[test]
    fn the_media_and_tool_errors_are_exit_6_with_their_scala_messages() {
        use crate::burn::BurnError;

        let cases: Vec<(BurnError, &str)> = vec![
            (
                BurnError::ToolMissing("cannot launch xorriso: nope".into()),
                "xorriso is not available: cannot launch xorriso: nope \
                 (install the libisoburn/xorriso package)",
            ),
            (
                BurnError::ToolTooOld("1.4.2".into()),
                "xorriso 1.4.2 is too old (need 1.5 or newer)",
            ),
            (
                BurnError::DeviceNotFound("/dev/sr9".into()),
                "burner device not found: /dev/sr9",
            ),
            (
                BurnError::DevicePermissionDenied("/dev/sr0".into()),
                "no permission to open /dev/sr0 — add your user to the 'optical' group \
                 (sudo usermod -aG optical <user>) and log in again",
            ),
            (
                BurnError::NoMedia("/dev/sr0".into()),
                "no disc in /dev/sr0",
            ),
            (
                BurnError::InsufficientCapacity {
                    needed_bytes: 1_000,
                    free_bytes: 10,
                },
                "insufficient media capacity: need 1000 bytes, only 10 free",
            ),
            (
                BurnError::BurnFailed {
                    exit_code: 5,
                    output_tail: vec!["FAILURE : write error".into(), "aborting".into()],
                },
                "xorriso failed (exit 5): FAILURE : write error | aborting",
            ),
            (
                BurnError::PostBurnCheckFailed("expected …, found: None".into()),
                "post-burn sanity check failed: expected …, found: None",
            ),
            (
                BurnError::MountFailed("udisksctl gave no mount point".into()),
                "could not mount the disc for verification: udisksctl gave no mount point",
            ),
        ];

        for (err, message) in cases {
            assert_eq!(err.to_string(), message);
            assert_eq!(
                PipelineError::Burn(err).exit_code(),
                6,
                "every burn failure but an abort is 6"
            );
        }
    }
```

- [ ] **Step 2: Run and watch it fail**

```bash
cargo test -p diskrement-shell error::tests::the_media_and_tool_errors
```

Expected: compile errors naming each missing variant.

- [ ] **Step 3: Add the variants**

In `crates/diskrement-shell/src/burn/mod.rs`, replace the `BurnError` enum's
trailing comment and `Aborted` variant so the enum reads (keeping
`HashMismatch`, `SourceFileMissing`, `Io` and `ToolMissing` exactly as they
are):

```rust
    #[error("xorriso {0} is too old (need 1.5 or newer)")]
    ToolTooOld(String),
    #[error("burner device not found: {0}")]
    DeviceNotFound(String),
    #[error(
        "no permission to open {0} — add your user to the 'optical' group \
         (sudo usermod -aG optical <user>) and log in again"
    )]
    DevicePermissionDenied(String),
    /// Constructed nowhere in this crate, exactly as in the Scala build — it
    /// is pinned by `ExitMappingSpec` as the representative exit-6 failure and
    /// is kept for that reason alone. `NotBlank`, `DiscClosed`, `ForeignDisc`
    /// and `WrongDisc` are the four Scala variants nothing constructs *and*
    /// nothing pins; they are deliberately not ported.
    #[error("no disc in {0}")]
    NoMedia(String),
    #[error("insufficient media capacity: need {needed_bytes} bytes, only {free_bytes} free")]
    InsufficientCapacity { needed_bytes: u64, free_bytes: u64 },
    #[error("xorriso failed (exit {exit_code}): {}", output_tail.join(" | "))]
    BurnFailed {
        exit_code: i32,
        output_tail: Vec<String>,
    },
    #[error("post-burn sanity check failed: {0}")]
    PostBurnCheckFailed(String),
    #[error("could not mount the disc for verification: {0}")]
    MountFailed(String),
    #[error("aborted by operator")]
    Aborted,
```

The doc comment on the enum itself still says "P8 adds the xorriso and media
variants with the code that raises them" — update it to say what it now is:

```rust
/// Expected burn failures.
///
/// One variant per failure the operator can act on. The exit ladder maps every
/// one of them to 6 except `Aborted`, which is a successful run that wrote
/// nothing and exits 0 (spec §3.2).
```

- [ ] **Step 4: Run the tests**

```bash
cargo test -p diskrement-shell error::
```

Expected: all green, including the new case table.

- [ ] **Step 5: Commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/diskrement-shell/src/burn/mod.rs crates/diskrement-shell/src/error.rs
git commit -m "feat(burn): the media and tool failures and their exit-6 rung"
```

---

### Task 3: `xorriso::args` — the command vectors

**Read first:** `shell/src/main/scala/diskrement/shell/burn/Xorriso.scala` and
`shell/src/test/scala/diskrement/shell/burn/XorrisoSpec.scala`. The command
shapes are validated facts (`docs/superpowers/plans/2026-08-30-diskrement-plan-4a-real-burning.md`,
§"Validated xorriso facts") — transcribe them, do not re-derive them.

**Files:**
- Create: `crates/diskrement-shell/src/burn/xorriso/mod.rs`
- Create: `crates/diskrement-shell/src/burn/xorriso/args.rs`
- Modify: `crates/diskrement-shell/src/burn/mod.rs` (`pub mod xorriso;`)

- [ ] **Step 1: Write the failing tests**

Create `crates/diskrement-shell/src/burn/xorriso/args.rs` with the test module
(the `XorrisoSpec` port):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn image() -> BurnDevice {
        BurnDevice::ImageFile(PathBuf::from("/tmp/d.iso"))
    }

    #[test]
    fn the_first_session_writes_to_outdev_and_leaves_the_disc_open() {
        assert_eq!(
            burn(
                &image(),
                Path::new("/w/staging/disc-1"),
                true,
                false,
                "PHOTOS_DISC_1"
            ),
            vec![
                "xorriso",
                "-abort_on",
                "FAILURE",
                "-outdev",
                "stdio:/tmp/d.iso",
                "-compliance",
                "iso_9660_level=3",
                "-volid",
                "PHOTOS_DISC_1",
                "-map",
                "/w/staging/disc-1",
                "/",
                "-close",
                "off",
                "-commit",
            ]
        );
    }

    /// An append uses `-dev` (which merges) rather than `-outdev` (which
    /// starts a new image), and a sealing session closes the disc.
    #[test]
    fn an_appending_session_uses_dev_and_can_close() {
        let args = burn(
            &BurnDevice::RealDevice("/dev/sr0".to_string()),
            Path::new("/w/s"),
            false,
            true,
            "V",
        );
        assert!(args.contains(&"-dev".to_string()), "{args:?}");
        assert!(args.contains(&"/dev/sr0".to_string()), "{args:?}");
        assert!(!args.contains(&"-outdev".to_string()), "{args:?}");
        assert!(
            args.windows(2)
                .any(|w| w == ["-close".to_string(), "on".to_string()]),
            "{args:?}"
        );
    }

    #[test]
    fn inspect_reads_the_toc_and_the_media_space() {
        assert_eq!(
            inspect(&image()),
            vec![
                "xorriso",
                "-abort_on",
                "FAILURE",
                "-outdev",
                "stdio:/tmp/d.iso",
                "-toc",
                "-tell_media_space",
            ]
        );
    }

    #[test]
    fn extract_without_a_session_reads_the_merged_view() {
        assert_eq!(
            extract(
                &image(),
                "/.diskrement/project.json",
                Path::new("/tmp/p.json"),
                None
            ),
            vec![
                "xorriso",
                "-abort_on",
                "FAILURE",
                "-indev",
                "stdio:/tmp/d.iso",
                "-osirrox",
                "on",
                "-extract",
                "/.diskrement/project.json",
                "/tmp/p.json",
            ]
        );
    }

    /// Tier-2 verification reads a version a later session hid, so the session
    /// is loaded before osirrox is switched on.
    #[test]
    fn extract_with_a_session_loads_that_session_first() {
        let args = extract(&image(), "/data/a.txt", Path::new("/tmp/a"), Some(1));
        let expected: Vec<String> = [
            "-indev",
            "stdio:/tmp/d.iso",
            "-load",
            "session",
            "1",
            "-osirrox",
            "on",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert!(
            args.windows(expected.len()).any(|w| w == expected),
            "{args:?}"
        );
    }

    #[test]
    fn eject_targets_the_device() {
        assert_eq!(
            eject(&BurnDevice::RealDevice("/dev/sr0".to_string())),
            vec!["xorriso", "-outdev", "/dev/sr0", "-eject", "all"]
        );
    }

    /// An ISO volume id is uppercase `[A-Z0-9_]`, at most 32 characters.
    #[test]
    fn volid_uppercases_replaces_and_truncates() {
        assert_eq!(volid("my photos!", 3), "MY_PHOTOS__DISC_3");
        assert_eq!(volid(&"x".repeat(40), 1).len(), 32);
    }
}
```

- [ ] **Step 2: Run and watch it fail**

```bash
cargo test -p diskrement-shell burn::xorriso::args
```

Expected: `failed to resolve: use of undeclared crate or module` / unresolved
functions.

- [ ] **Step 3: Write the implementation**

Above the tests in `crates/diskrement-shell/src/burn/xorriso/args.rs`:

```rust
//! Pure xorriso argument vectors (`Xorriso.scala`).
//!
//! Every vector starts with the binary name; `cmd::Exec` runs it. The shapes
//! are validated facts from Plan 4a's "Validated xorriso facts" (xorriso
//! 1.5.8): `-compliance iso_9660_level=3` is the native-mode spelling —
//! `-iso_level` is mkisofs-emulation syntax and silently does nothing here —
//! and `-dev` merges a new session where `-outdev` starts a fresh image.

use std::path::{Path, PathBuf};

use crate::burn::BurnDevice;

fn words(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

/// Master `staged_dir` onto the medium as one session.
pub fn burn(
    device: &BurnDevice,
    staged_dir: &Path,
    first_session: bool,
    close_disc: bool,
    volid: &str,
) -> Vec<String> {
    let mut out = words(&["xorriso", "-abort_on", "FAILURE"]);
    out.push(if first_session { "-outdev" } else { "-dev" }.to_string());
    out.push(device.xorriso_address());
    out.extend(words(&["-compliance", "iso_9660_level=3", "-volid"]));
    out.push(volid.to_string());
    out.push("-map".to_string());
    out.push(staged_dir.display().to_string());
    out.push("/".to_string());
    out.push("-close".to_string());
    out.push(if close_disc { "on" } else { "off" }.to_string());
    out.push("-commit".to_string());
    out
}

/// Report the medium's state and its free space.
pub fn inspect(device: &BurnDevice) -> Vec<String> {
    let mut out = words(&["xorriso", "-abort_on", "FAILURE", "-outdev"]);
    out.push(device.xorriso_address());
    out.extend(words(&["-toc", "-tell_media_space"]));
    out
}

/// Copy `disc_path` off the medium. `session` picks one recorded session
/// instead of the merged view — the tier-2 read.
pub fn extract(
    device: &BurnDevice,
    disc_path: &str,
    dest: &Path,
    session: Option<u32>,
) -> Vec<String> {
    let mut out = words(&["xorriso", "-abort_on", "FAILURE", "-indev"]);
    out.push(device.xorriso_address());
    if let Some(n) = session {
        out.extend(words(&["-load", "session"]));
        out.push(n.to_string());
    }
    out.extend(words(&["-osirrox", "on", "-extract"]));
    out.push(disc_path.to_string());
    out.push(dest.display().to_string());
    out
}

pub fn eject(device: &BurnDevice) -> Vec<String> {
    let mut out = words(&["xorriso", "-outdev"]);
    out.push(device.xorriso_address());
    out.extend(words(&["-eject", "all"]));
    out
}

/// `xorriso --version`, whose output the preflight parses.
pub fn version() -> Vec<String> {
    words(&["xorriso", "--version"])
}

/// An ISO volume id: uppercase, `[A-Z0-9_]`, at most 32 characters.
pub fn volid(project_name: &str, disc_ordinal: u32) -> String {
    format!("{project_name}_DISC_{disc_ordinal}")
        .to_uppercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .take(32)
        .collect()
}
```

**Watch the `PathBuf` import:** it is used by the test module only. If clippy
objects to an unused import in the non-test build, move `use std::path::PathBuf;`
into `mod tests` — do not delete the test's use of it.

Create `crates/diskrement-shell/src/burn/xorriso/mod.rs`:

```rust
//! The xorriso backend: what we send (`args`), what we read (`parse`), and the
//! burner that sequences them (`burner`).

pub mod args;
```

And add to `crates/diskrement-shell/src/burn/mod.rs`:

```rust
pub mod xorriso;
```

- [ ] **Step 4: Run the tests**

```bash
cargo test -p diskrement-shell burn::xorriso::args
```

Expected: 7 passed.

Sanity-check `volid` against Scala's while you are here: Scala's
`c.isLetterOrDigit` is Unicode-aware and so is Rust's `char::is_alphanumeric`,
and both apply `take(32)` *after* the mapping, so a multi-byte name truncates
identically by *character*, not by byte.

- [ ] **Step 5: Commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/diskrement-shell/src/burn/xorriso crates/diskrement-shell/src/burn/mod.rs
git commit -m "feat(burn): the xorriso argument vectors and the volume id"
```

---

### Task 4: `xorriso::parse` — version, media state, failure kind

**Read first:** `shell/src/main/scala/diskrement/shell/burn/MediaParse.scala`,
`Xorriso.scala`'s `parseVersion`, and
`shell/src/test/scala/diskrement/shell/burn/MediaParseSpec.scala`. The fixtures
in that spec are **captured xorriso output** — two of them from a real Pioneer
BDR-UD04 on 2026-08-30. Copy them verbatim; they are evidence, not examples.

**Files:**
- Create: `crates/diskrement-shell/src/burn/xorriso/parse.rs`
- Modify: `crates/diskrement-shell/src/burn/xorriso/mod.rs`

No `regex` crate: the workspace has none (§5.4) and these three patterns are
substring-and-digits work. Each parser below states the Scala regex it replaces
so the two can be compared line by line.

- [ ] **Step 1: Write the failing tests**

Create `crates/diskrement-shell/src/burn/xorriso/parse.rs` with the test module
(the `MediaParseSpec` port plus the `parseVersion` cases from `XorrisoSpec`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|s| (*s).to_string()).collect()
    }

    const BLANK: &[&str] = &[
        "xorriso 1.5.8.pl02 : RockRidge filesystem manipulator, libburnia project.",
        "Drive current: -outdev 'stdio:empty.iso'",
        "Media current: stdio file, overwriteable",
        "Media status : is blank",
        "Media summary: 0 sessions, 0 data blocks, 0 data,  660g free",
        "Media space  : 346016270s",
        "After commit : 346016270s",
    ];

    const APPENDABLE: &[&str] = &[
        "Media current: stdio file, overwriteable",
        "Media status : is written , is appendable",
        "ISO session  :   1 ,        32 ,        29s , ISOIMAGE",
        "ISO session  :   2 ,        64 ,        28s , ISOIMAGE",
        "Media summary: 2 sessions, 57 data blocks,  114k data,  660g free",
        "Media space  : 346016430s",
    ];

    const CLOSED: &[&str] = &[
        "Media current: BD-R sequential recording",
        "Media status : is written , is closed",
        "Media summary: 3 sessions, 57 data blocks,  114k data, 0 free",
        "Media space  : 0s",
    ];

    // Real captured output (2026-08-30, Pioneer BDR-UD04, a POW-formatted
    // BD-R): such media cannot take sequential multisession burns.
    const UNSUITABLE: &[&str] = &[
        "Media current: BD-R sequential recording, Pseudo Overwrite formatted",
        "Media status : is unsuitable , is POW formatted",
        "Media summary: unsuitable Pseudo Overwrite formatted BD-R",
        "Media space  : 248768s",
    ];

    // Real captured output (2026-08-30, Pioneer BDR-UD04, empty tray): xorriso
    // exits 0 — an absent disc is a media status, not a failure.
    const EMPTY_TRAY: &[&str] = &[
        "xorriso : NOTE : Disc status unsuitable for writing",
        "Drive current: -outdev '/dev/sr0'",
        "Media current: is not recognizable",
        "Media status : is not present",
        "Media space  : 0s",
    ];

    #[test]
    fn blank_media() {
        assert_eq!(
            media(&lines(BLANK)).unwrap(),
            RawMediaInfo {
                status: RawStatus::Blank,
                sessions: 0,
                free_bytes: 346_016_270 * 2048,
            }
        );
    }

    #[test]
    fn appendable_media_carries_its_session_count() {
        assert_eq!(
            media(&lines(APPENDABLE)).unwrap(),
            RawMediaInfo {
                status: RawStatus::Appendable,
                sessions: 2,
                free_bytes: 346_016_430 * 2048,
            }
        );
    }

    #[test]
    fn written_but_not_appendable_is_closed() {
        assert_eq!(
            media(&lines(CLOSED)).unwrap(),
            RawMediaInfo {
                status: RawStatus::Closed,
                sessions: 3,
                free_bytes: 0,
            }
        );
    }

    #[test]
    fn a_pow_formatted_bd_r_is_unsuitable() {
        assert_eq!(
            media(&lines(UNSUITABLE)).unwrap(),
            RawMediaInfo {
                status: RawStatus::Unsuitable,
                sessions: 0,
                free_bytes: 248_768 * 2048,
            }
        );
    }

    #[test]
    fn an_empty_tray_exits_zero_and_parses_as_not_present() {
        assert_eq!(
            media(&lines(EMPTY_TRAY)).unwrap(),
            RawMediaInfo {
                status: RawStatus::NotPresent,
                sessions: 0,
                free_bytes: 0,
            }
        );
    }

    /// No recognisable status line is an error carrying the first few lines,
    /// so the operator sees what xorriso actually said.
    #[test]
    fn output_with_no_status_line_is_an_error_quoting_it() {
        let err = media(&lines(&["Drive current: ..."])).unwrap_err();
        assert!(err.starts_with("no Media status line in xorriso output: "), "{err}");
        assert!(err.contains("Drive current: ..."), "{err}");
    }

    #[test]
    fn failures_are_classified_by_their_libburn_wording() {
        assert_eq!(
            classify_failure(&lines(&["libburn : SORRY : No disc loaded, not ready"])),
            FailureKind::NoMedia
        );
        assert_eq!(
            classify_failure(&lines(&[
                "libburn : NOTE : Failed to open device '/dev/sr0' : Permission denied"
            ])),
            FailureKind::Permission
        );
        assert_eq!(
            classify_failure(&lines(&["stdio: could not open, No such file or directory"])),
            FailureKind::NotFound
        );
        assert_eq!(
            classify_failure(&lines(&["xorriso : FAILURE : something else"])),
            FailureKind::Other
        );
    }

    #[test]
    fn the_version_line_is_parsed_and_an_old_one_still_parses() {
        assert_eq!(
            version(&lines(&["xorriso 1.5.8.pl02", "ISO 9660 ..."])),
            Some((1, 5, "1.5.8.pl02".to_string()))
        );
        assert_eq!(
            version(&lines(&["xorriso 1.4.2"])),
            Some((1, 4, "1.4.2".to_string()))
        );
        assert_eq!(version(&lines(&["garbage"])), None);
    }

    /// The real banner: the version is followed by the rest of the line, and
    /// only the version token belongs to the raw string.
    #[test]
    fn the_real_banner_line_parses() {
        assert_eq!(
            version(&lines(&[
                "xorriso 1.5.8.pl02 : RockRidge filesystem manipulator, libburnia project."
            ])),
            Some((1, 5, "1.5.8.pl02".to_string()))
        );
    }
}
```

- [ ] **Step 2: Run and watch it fail**

```bash
cargo test -p diskrement-shell burn::xorriso::parse
```

Expected: unresolved names `media`, `RawMediaInfo`, `RawStatus`,
`classify_failure`, `FailureKind`, `version`.

- [ ] **Step 3: Write the implementation**

Above the tests in `crates/diskrement-shell/src/burn/xorriso/parse.rs`:

```rust
//! Pure parsers for what xorriso prints (`MediaParse.scala`, plus
//! `Xorriso.parseVersion`).
//!
//! Line shapes were validated against xorriso 1.5.8; the "closed" and
//! empty-tray shapes were captured from a real drive on 2026-08-30 and are
//! recorded verbatim in this module's tests. No regex engine: the workspace
//! has none (spec §5.4), and each function below names the Scala pattern it
//! replaces so the two can be compared directly.

/// The medium's state, before the identity read-back turns it into a
/// `MediaStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawStatus {
    Blank,
    Appendable,
    Closed,
    Unsuitable,
    NotPresent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawMediaInfo {
    pub status: RawStatus,
    pub sessions: u32,
    pub free_bytes: u64,
}

/// Why an inspection or a burn failed. Substrings chosen from observed and
/// documented libburn messages; unknown text falls through to `Other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    NoMedia,
    Permission,
    NotFound,
    Other,
}

/// The leading run of ASCII digits of `s` and its length, if `s` starts with
/// one. The length is returned rather than recomputed from the value, so a
/// zero-padded count cannot desynchronise the caller's slice.
fn leading_number(s: &str) -> Option<(u64, usize)> {
    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let value = digits.parse().ok()?;
    Some((value, digits.len()))
}

/// Parse `xorriso -toc -tell_media_space` output.
///
/// Replaces three Scala patterns: the `Media status` substring match, the
/// `Media summary:\s*(\d+)\s+session` count, and the
/// `Media space\s*:\s*(\d+)s` block count (2048 bytes per block). A missing
/// count or space line is 0, exactly as `getOrElse(0)` had it.
pub fn media(lines: &[String]) -> Result<RawMediaInfo, String> {
    let status = lines.iter().find_map(|l| {
        if !l.contains("Media status") {
            return None;
        }
        // Order matters: "is written , is appendable" contains both, and
        // appendable must win — the Scala arms are in this same order.
        if l.contains("is not present") {
            Some(RawStatus::NotPresent)
        } else if l.contains("is blank") {
            Some(RawStatus::Blank)
        } else if l.contains("is unsuitable") {
            Some(RawStatus::Unsuitable)
        } else if l.contains("is appendable") {
            Some(RawStatus::Appendable)
        } else if l.contains("is written") {
            Some(RawStatus::Closed)
        } else {
            None
        }
    });

    let sessions = lines
        .iter()
        .find_map(|l| {
            let rest = l.split_once("Media summary:")?.1.trim_start();
            let (n, len) = leading_number(rest)?;
            rest[len..]
                .trim_start()
                .starts_with("session")
                .then_some(n as u32)
        })
        .unwrap_or(0);

    let free_blocks = lines
        .iter()
        .find_map(|l| {
            let rest = l.split_once("Media space")?.1;
            let rest = rest.trim_start().strip_prefix(':')?.trim_start();
            let (n, len) = leading_number(rest)?;
            rest[len..].starts_with('s').then_some(n)
        })
        .unwrap_or(0);

    match status {
        Some(status) => Ok(RawMediaInfo {
            status,
            sessions,
            free_bytes: free_blocks * 2048,
        }),
        None => Err(format!(
            "no Media status line in xorriso output: {}",
            lines
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join(" | ")
        )),
    }
}

/// Why an invocation failed, from its output.
pub fn classify_failure(lines: &[String]) -> FailureKind {
    let text = lines.join("\n").to_lowercase();
    if text.contains("permission denied") {
        FailureKind::Permission
    } else if text.contains("no such file") || text.contains("cannot find device") {
        FailureKind::NotFound
    } else if ["no disc", "not ready", "no medium", "not present"]
        .iter()
        .any(|s| text.contains(s))
    {
        FailureKind::NoMedia
    } else {
        FailureKind::Other
    }
}

/// `(major, minor, raw)` from `xorriso --version` output.
///
/// Replaces `xorriso\s+((\d+)\.(\d+)\S*).*`, which Scala matched against the
/// whole line: the line must *start* with `xorriso`, and the raw version is
/// the token after it — `1.5.8.pl02`, not `1.5`.
pub fn version(lines: &[String]) -> Option<(u32, u32, String)> {
    lines.iter().find_map(|line| {
        let rest = line.strip_prefix("xorriso")?;
        if !rest.starts_with(char::is_whitespace) {
            return None;
        }
        let raw = rest.split_whitespace().next()?;
        let (major, tail) = raw.split_once('.')?;
        let minor: String = tail.chars().take_while(char::is_ascii_digit).collect();
        Some((
            major.parse().ok()?,
            minor.parse().ok()?,
            raw.to_string(),
        ))
    })
}
```

Add to `crates/diskrement-shell/src/burn/xorriso/mod.rs`:

```rust
pub mod parse;
```

- [ ] **Step 4: Run the tests**

```bash
cargo test -p diskrement-shell burn::xorriso::parse
```

Expected: 9 passed.

- [ ] **Step 5: Commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/diskrement-shell/src/burn/xorriso
git commit -m "feat(burn): parse the version banner, the media table and the failure kind"
```

---

### Task 5: `burn::media` — the media inspector

**Read first:** `shell/src/main/scala/diskrement/shell/burn/MediaInspector.scala`
and `shell/src/test/scala/diskrement/shell/burn/MediaInspectorSpec.scala`.

**Files:**
- Create: `crates/diskrement-shell/src/burn/media.rs`
- Modify: `crates/diskrement-shell/src/burn/mod.rs` (`pub mod media;`, `DiscIdentity: Display`)
- Modify: `crates/diskrement-shell/Cargo.toml` (`tempfile` into `[dependencies]`, D2)

Two operations: `inspect` classifies the medium (and, for written media, reads
the identity back off it), and `read_identity` extracts
`/.diskrement/project.json` from the merged view. A failed extraction is
`Ok(None)`, never an error: a blank-but-written non-diskrement disc is a normal
answer, not a fault.

**Validated on 2026-09-07, xorriso 1.5.8.pl02** (re-confirming Plan 4a's facts
for this port): extracting to a path that does not exist yet creates it;
extracting a *directory* (`-extract /data <dir>`) recreates the tree; and
`-toc -tell_media_space` prints the `Media status` line **twice** (once per
sub-command), which is why the parser takes the first match rather than
asserting there is exactly one.

- [ ] **Step 1: Write the failing tests**

Create `crates/diskrement-shell/src/burn/media.rs` with the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::burn::cmd::CmdOutput;

    fn device() -> BurnDevice {
        BurnDevice::RealDevice("/dev/sr0".to_string())
    }

    fn out(code: i32, lines: &[&str]) -> Result<CmdOutput, BurnError> {
        Ok(CmdOutput {
            exit_code: code,
            lines: lines.iter().map(|s| (*s).to_string()).collect(),
        })
    }

    const BLANK_TOC: &[&str] = &[
        "Media status : is blank",
        "Media summary: 0 sessions, 0 data blocks, 0 data, 23g free",
        "Media space  : 11826176s",
    ];

    const APPENDABLE_TOC: &[&str] = &[
        "Media status : is written , is appendable",
        "Media summary: 2 sessions, 57 data blocks, 114k data, 23g free",
        "Media space  : 11826000s",
    ];

    const IDENTITY_JSON: &str = r#"{"projectName":"photos","discOrdinal":1,"sessionOrdinal":2,"createdAt":"2026-08-30T10:00:00Z"}"#;

    #[test]
    fn blank_media_reports_its_free_space() {
        let exec = |_: &[String], _: &mut dyn FnMut(&str)| out(0, BLANK_TOC);
        assert_eq!(
            MediaInspector::new(device(), &exec).inspect().unwrap(),
            MediaStatus::Blank(11_826_176 * 2048)
        );
    }

    /// Written diskrement media: the `-toc` gives the state, a second
    /// invocation extracts `/.diskrement/project.json`, and the identity comes
    /// back parsed.
    #[test]
    fn an_appendable_diskrement_disc_yields_its_identity() {
        let exec = |cmd: &[String], _: &mut dyn FnMut(&str)| {
            if cmd.iter().any(|a| a == "-toc") {
                out(0, APPENDABLE_TOC)
            } else if cmd.iter().any(|a| a == "-extract") {
                std::fs::write(cmd.last().expect("a destination"), IDENTITY_JSON)
                    .expect("the destination is writable");
                out(0, &[])
            } else {
                out(1, &["unexpected"])
            }
        };
        let status = MediaInspector::new(device(), &exec).inspect().unwrap();
        match status {
            MediaStatus::Appendable {
                free_bytes,
                sessions,
                identity: Some(id),
            } => {
                assert_eq!(free_bytes, 11_826_000 * 2048);
                assert_eq!(sessions, 2);
                assert_eq!(id.project_name, "photos");
                assert_eq!(id.disc_ordinal, 1);
                assert_eq!(id.session_ordinal, 2);
            }
            other => panic!("unexpected: {other}"),
        }
    }

    /// A written disc that is not ours: the extraction fails, and that is an
    /// answer (`None`), not an error.
    #[test]
    fn an_appendable_foreign_disc_yields_no_identity() {
        let exec = |cmd: &[String], _: &mut dyn FnMut(&str)| {
            if cmd.iter().any(|a| a == "-toc") {
                out(0, APPENDABLE_TOC)
            } else {
                out(5, &["FAILURE : cannot find path"])
            }
        };
        assert!(matches!(
            MediaInspector::new(device(), &exec).inspect().unwrap(),
            MediaStatus::Appendable {
                identity: None,
                ..
            }
        ));
    }

    /// The empty tray as the drive really reports it: exit 0 with
    /// "is not present" (captured 2026-08-30).
    #[test]
    fn an_empty_tray_at_exit_zero_is_no_media() {
        let exec = |_: &[String], _: &mut dyn FnMut(&str)| {
            out(
                0,
                &[
                    "Media current: is not recognizable",
                    "Media status : is not present",
                    "Media space  : 0s",
                ],
            )
        };
        assert_eq!(
            MediaInspector::new(device(), &exec).inspect().unwrap(),
            MediaStatus::NoMedia
        );
    }

    /// A drive that fails with a no-disc message is also just "no media".
    #[test]
    fn a_no_disc_failure_is_no_media() {
        let exec = |_: &[String], _: &mut dyn FnMut(&str)| {
            out(5, &["libburn : SORRY : No disc loaded, not ready"])
        };
        assert_eq!(
            MediaInspector::new(device(), &exec).inspect().unwrap(),
            MediaStatus::NoMedia
        );
    }

    #[test]
    fn a_permission_failure_names_the_optical_group() {
        let exec = |_: &[String], _: &mut dyn FnMut(&str)| {
            out(5, &["Failed to open device '/dev/sr0' : Permission denied"])
        };
        let err = MediaInspector::new(device(), &exec).inspect().unwrap_err();
        assert!(matches!(err, BurnError::DevicePermissionDenied(ref d) if d == "/dev/sr0"));
        assert!(err.to_string().contains("usermod -aG optical"), "{err}");
    }

    #[test]
    fn a_missing_device_is_device_not_found() {
        let exec = |_: &[String], _: &mut dyn FnMut(&str)| {
            out(5, &["stdio: could not open, No such file or directory"])
        };
        assert!(matches!(
            MediaInspector::new(device(), &exec).inspect().unwrap_err(),
            BurnError::DeviceNotFound(_)
        ));
    }

    /// An unrecognised failure keeps xorriso's own words, so the operator can
    /// act on something.
    #[test]
    fn an_unclassifiable_failure_surfaces_the_output_tail() {
        let exec = |_: &[String], _: &mut dyn FnMut(&str)| {
            out(5, &["xorriso : FAILURE : something else"])
        };
        let err = MediaInspector::new(device(), &exec).inspect().unwrap_err();
        assert_eq!(
            err.to_string(),
            "xorriso failed (exit 5): xorriso : FAILURE : something else"
        );
    }

    /// Output with no status line at all is a `BurnFailed` quoting the parse
    /// complaint (`MediaParse.parse`'s `Left`).
    #[test]
    fn unparsable_output_is_a_burn_failure_quoting_the_complaint() {
        let exec = |_: &[String], _: &mut dyn FnMut(&str)| out(0, &["Drive current: ..."]);
        let err = MediaInspector::new(device(), &exec).inspect().unwrap_err();
        assert!(err.to_string().contains("no Media status line"), "{err}");
    }

    /// D3: these strings are interpolated into user-visible messages, so they
    /// reproduce Scala's `toString` exactly.
    #[test]
    fn the_display_shapes_match_the_scala_tostring() {
        let id = DiscIdentity {
            project_name: "photos".to_string(),
            disc_ordinal: 1,
            session_ordinal: 2,
            created_at: crate::catalog::time::parse("2026-08-30T10:00:00Z").unwrap(),
        };
        assert_eq!(id.to_string(), "DiscIdentity(photos,1,2,2026-08-30T10:00:00Z)");
        assert_eq!(MediaStatus::NoMedia.to_string(), "NoMedia");
        assert_eq!(MediaStatus::Unsuitable.to_string(), "Unsuitable");
        assert_eq!(
            MediaStatus::Blank(24_220_008_448).to_string(),
            "Blank(24220008448)"
        );
        assert_eq!(
            MediaStatus::Appendable {
                free_bytes: 123,
                sessions: 2,
                identity: Some(id.clone()),
            }
            .to_string(),
            "Appendable(123,2,Some(DiscIdentity(photos,1,2,2026-08-30T10:00:00Z)))"
        );
        assert_eq!(
            MediaStatus::Appendable {
                free_bytes: 123,
                sessions: 2,
                identity: None,
            }
            .to_string(),
            "Appendable(123,2,None)"
        );
        assert_eq!(
            MediaStatus::Closed(Some(id)).to_string(),
            "Closed(Some(DiscIdentity(photos,1,2,2026-08-30T10:00:00Z)))"
        );
        assert_eq!(MediaStatus::Closed(None).to_string(), "Closed(None)");
    }
}
```

- [ ] **Step 2: Run and watch it fail**

```bash
cargo test -p diskrement-shell burn::media
```

Expected: unresolved `MediaInspector`, `MediaStatus`.

- [ ] **Step 3: Move `tempfile` into runtime dependencies**

In `crates/diskrement-shell/Cargo.toml`, add to `[dependencies]` (keeping the
list alphabetical) and leave the `[dev-dependencies]` entry alone — Cargo
accepts both, and dropping the dev entry would change nothing:

```toml
tempfile.workspace = true
```

Add the reason above it, since a dependency move against the spec's §5.4 list
needs one in the file people actually read:

```toml
# Runtime, not just dev (spec §5.4 lists it under Dev): the media inspector,
# the disc reader and the tier-2 verifier all extract to a temporary file, and
# hand-rolling O_EXCL naming in a world-writable directory is a wheel with a
# symlink-attack surface. Already in the lockfile as a dev-dependency, so the
# dependency tree is unchanged.
```

- [ ] **Step 4: Write the implementation**

Above the tests in `crates/diskrement-shell/src/burn/media.rs`:

```rust
//! What is in the drive (`MediaInspector.scala`).
//!
//! `inspect` classifies the medium; for written media it also reads the
//! identity the last session left in `/.diskrement/project.json`, which is what
//! lets the burner refuse the wrong disc before writing to it.

use std::fmt;

use crate::burn::cmd::Exec;
use crate::burn::json::ProjectJson;
use crate::burn::xorriso::{args, parse};
use crate::burn::{BurnDevice, BurnError, DiscIdentity};

/// The state of the medium in a device.
///
/// `Display` reproduces the Scala enum's `toString` because two user-visible
/// messages interpolate it (see the plan's D3); do not swap it for `Debug`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaStatus {
    NoMedia,
    Blank(u64),
    Appendable {
        free_bytes: u64,
        sessions: u32,
        identity: Option<DiscIdentity>,
    },
    Closed(Option<DiscIdentity>),
    /// Media libburn refuses to write sequentially, e.g. a POW-formatted BD-R.
    Unsuitable,
}

fn show(identity: &Option<DiscIdentity>) -> String {
    match identity {
        Some(id) => format!("Some({id})"),
        None => "None".to_string(),
    }
}

impl fmt::Display for MediaStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MediaStatus::NoMedia => f.write_str("NoMedia"),
            MediaStatus::Blank(free) => write!(f, "Blank({free})"),
            MediaStatus::Appendable {
                free_bytes,
                sessions,
                identity,
            } => write!(
                f,
                "Appendable({free_bytes},{sessions},{})",
                show(identity)
            ),
            MediaStatus::Closed(identity) => write!(f, "Closed({})", show(identity)),
            MediaStatus::Unsuitable => f.write_str("Unsuitable"),
        }
    }
}

pub struct MediaInspector<'a> {
    device: BurnDevice,
    exec: &'a dyn Exec,
}

impl<'a> MediaInspector<'a> {
    pub fn new(device: BurnDevice, exec: &'a dyn Exec) -> Self {
        MediaInspector { device, exec }
    }

    pub fn inspect(&self) -> Result<MediaStatus, BurnError> {
        let result = self.exec.run_quiet(&args::inspect(&self.device))?;
        if result.exit_code != 0 {
            return match parse::classify_failure(&result.lines) {
                // An absent disc is a state, not a failure — the drive says so
                // both ways, at exit 0 and at exit 5.
                parse::FailureKind::NoMedia => Ok(MediaStatus::NoMedia),
                parse::FailureKind::Permission => Err(BurnError::DevicePermissionDenied(
                    self.device.xorriso_address(),
                )),
                parse::FailureKind::NotFound => {
                    Err(BurnError::DeviceNotFound(self.device.xorriso_address()))
                }
                parse::FailureKind::Other => Err(BurnError::BurnFailed {
                    exit_code: result.exit_code,
                    output_tail: result.tail(10),
                }),
            };
        }

        let info = parse::media(&result.lines).map_err(|complaint| BurnError::BurnFailed {
            exit_code: result.exit_code,
            output_tail: vec![complaint],
        })?;
        Ok(match info.status {
            parse::RawStatus::Blank => MediaStatus::Blank(info.free_bytes),
            parse::RawStatus::Appendable => MediaStatus::Appendable {
                free_bytes: info.free_bytes,
                sessions: info.sessions,
                identity: self.read_identity()?,
            },
            parse::RawStatus::Closed => MediaStatus::Closed(self.read_identity()?),
            parse::RawStatus::Unsuitable => MediaStatus::Unsuitable,
            parse::RawStatus::NotPresent => MediaStatus::NoMedia,
        })
    }

    /// The `/.diskrement/project.json` identity of the disc's merged view.
    ///
    /// `Ok(None)` covers every "this is not one of ours" case: the file is
    /// absent, the extraction failed, or the JSON does not parse. Only the
    /// temp-directory creation can fail here.
    pub fn read_identity(&self) -> Result<Option<DiscIdentity>, BurnError> {
        let dir = tempfile::TempDir::new().map_err(BurnError::Io)?;
        // A path inside a fresh directory, deliberately not yet created:
        // xorriso creates it, and nothing else can be holding it.
        let dest = dir.path().join("project.json");
        let result = self.exec.run_quiet(&args::extract(
            &self.device,
            "/.diskrement/project.json",
            &dest,
            None,
        ))?;
        if result.exit_code != 0 {
            return Ok(None);
        }
        let Ok(text) = std::fs::read_to_string(&dest) else {
            return Ok(None);
        };
        Ok(serde_json::from_str::<ProjectJson>(&text)
            .ok()
            .and_then(|p| p.to_identity().ok()))
    }
}
```

Add to `crates/diskrement-shell/src/burn/mod.rs` the module declaration and the
`Display` impl (put the impl immediately after the `DiscIdentity` struct):

```rust
pub mod media;
```

```rust
/// Scala's case-class `toString`, reproduced because it is interpolated into
/// `PostBurnCheckFailed`'s detail and into the verifier's re-prompt line
/// (spec §3.2: output wording is contract outside clap's argument errors).
impl std::fmt::Display for DiscIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "DiscIdentity({},{},{},{})",
            self.project_name,
            self.disc_ordinal,
            self.session_ordinal,
            crate::catalog::time::format(&self.created_at)
        )
    }
}
```

- [ ] **Step 5: Run the tests**

```bash
cargo test -p diskrement-shell burn::media
```

Expected: 10 passed.

- [ ] **Step 6: Commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/diskrement-shell/src/burn/media.rs crates/diskrement-shell/src/burn/mod.rs crates/diskrement-shell/Cargo.toml Cargo.lock
git commit -m "feat(burn): inspect the medium and read a disc's identity back"
```

---

### Task 6: `burn::reader` — mounting and extracting a burned disc

**Read first:** `shell/src/main/scala/diskrement/shell/burn/DiscReader.scala` and
the parser suite in `shell/src/test/scala/diskrement/shell/burn/DiscReaderSpec.scala`.

**Files:**
- Create: `crates/diskrement-shell/src/burn/reader.rs`
- Modify: `crates/diskrement-shell/src/burn/mod.rs` (`pub mod reader;`)

Two substrates, one interface. A real device is mounted through `udisksctl`
(polkit lets a desktop session mount removable media without root) and unmounted
afterwards whatever happened; an image file is extracted with osirrox into a
temporary directory that drops with the call. `extract_from_session` is the
tier-2 read and is substrate-independent.

- [ ] **Step 1: Write the failing tests**

Create `crates/diskrement-shell/src/burn/reader.rs` with the test module. The
image-backed half of `DiscReaderSpec` needs real `xorriso` and lands in Task 10;
these are the parser and the unmount-ordering cases.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::burn::cmd::CmdOutput;
    use std::cell::RefCell;

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn a_mount_line_gives_the_mount_point() {
        assert_eq!(
            parse_mount_point(&lines(&["Mounted /dev/sr0 at /run/media/z/PHOTOS_DISC_1"])),
            Some(PathBuf::from("/run/media/z/PHOTOS_DISC_1"))
        );
    }

    /// udisksctl ends the sentence with a period on some versions; it is not
    /// part of the path.
    #[test]
    fn a_trailing_period_is_not_part_of_the_path() {
        assert_eq!(
            parse_mount_point(&lines(&["Mounted /dev/sr0 at /run/media/z/D."])),
            Some(PathBuf::from("/run/media/z/D"))
        );
    }

    /// An already-mounted disc is not a failure: the error message carries the
    /// mount point we wanted.
    #[test]
    fn an_already_mounted_disc_reports_where_it_is() {
        assert_eq!(
            parse_mount_point(&lines(&[
                "Error mounting: GDBus.Error:org.freedesktop.UDisks2.Error.AlreadyMounted: \
                 Device /dev/sr0 is already mounted at `/run/media/z/OLD'."
            ])),
            Some(PathBuf::from("/run/media/z/OLD"))
        );
    }

    #[test]
    fn unrecognised_output_gives_no_mount_point() {
        assert_eq!(parse_mount_point(&lines(&["garbage"])), None);
    }

    /// Unmount runs even when the caller's closure failed — otherwise a failed
    /// verify would leave the disc mounted and the next one would hit the
    /// AlreadyMounted path.
    #[test]
    fn the_disc_is_unmounted_even_when_the_body_fails() {
        let seen: RefCell<Vec<String>> = RefCell::new(Vec::new());
        let exec = |cmd: &[String], _: &mut dyn FnMut(&str)| {
            seen.borrow_mut().push(cmd.join(" "));
            Ok(CmdOutput {
                exit_code: 0,
                lines: lines(&["Mounted /dev/sr0 at /mnt/disc"]),
            })
        };
        let reader = DiscReader::new(BurnDevice::RealDevice("/dev/sr0".to_string()), &exec);
        let err = reader
            .with_data_tree(|data| {
                assert_eq!(data, std::path::Path::new("/mnt/disc/data"));
                Err::<(), _>(BurnError::MountFailed("body failed".to_string()))
            })
            .unwrap_err();
        assert!(matches!(err, BurnError::MountFailed(_)));
        let calls = seen.borrow();
        assert!(calls[0].contains("udisksctl mount -b /dev/sr0"), "{calls:?}");
        assert!(
            calls[1].contains("udisksctl unmount -b /dev/sr0"),
            "{calls:?}"
        );
    }

    /// udisksctl that says nothing useful is a `MountFailed` quoting it,
    /// rather than a mount at some guessed path.
    #[test]
    fn a_mount_with_no_mount_point_fails_loudly() {
        let exec = |_: &[String], _: &mut dyn FnMut(&str)| {
            Ok(CmdOutput {
                exit_code: 1,
                lines: lines(&["something went wrong"]),
            })
        };
        let reader = DiscReader::new(BurnDevice::RealDevice("/dev/sr0".to_string()), &exec);
        let err = reader.with_data_tree(|_| Ok(())).unwrap_err();
        assert_eq!(
            err.to_string(),
            "could not mount the disc for verification: udisksctl gave no mount point: \
             something went wrong"
        );
    }

    /// A missing `udisksctl` is reported as a mount failure naming the tool,
    /// not as "xorriso is not available" (`DiscReaderLive.mount`'s mapError).
    #[test]
    fn a_missing_udisksctl_is_a_mount_failure() {
        let exec = |_: &[String], _: &mut dyn FnMut(&str)| {
            Err(BurnError::ToolMissing("cannot launch udisksctl: nope".to_string()))
        };
        let reader = DiscReader::new(BurnDevice::RealDevice("/dev/sr0".to_string()), &exec);
        let err = reader.with_data_tree(|_| Ok(())).unwrap_err();
        assert!(err.to_string().contains("udisksctl not available"), "{err}");
    }
}
```

- [ ] **Step 2: Run and watch it fail**

```bash
cargo test -p diskrement-shell burn::reader
```

- [ ] **Step 3: Write the implementation**

Above the tests in `crates/diskrement-shell/src/burn/reader.rs`:

```rust
//! Reading a burned disc back (`DiscReader.scala`).
//!
//! Real devices are mounted through `udisksctl` — polkit lets a desktop
//! session mount removable media without root — and image files are extracted
//! through xorriso's osirrox. Both expose the same thing to the verifier: a
//! local directory holding the disc's merged `/data` tree.

use std::path::{Path, PathBuf};

use crate::burn::cmd::Exec;
use crate::burn::xorriso::args;
use crate::burn::{BurnDevice, BurnError};

pub struct DiscReader<'a> {
    device: BurnDevice,
    exec: &'a dyn Exec,
}

impl<'a> DiscReader<'a> {
    pub fn new(device: BurnDevice, exec: &'a dyn Exec) -> Self {
        DiscReader { device, exec }
    }

    /// Expose the disc's merged `/data` tree as a local directory for `use_it`.
    ///
    /// The mount is released, and the temporary extraction directory removed,
    /// whether `use_it` succeeded or not.
    pub fn with_data_tree<A>(
        &self,
        use_it: impl FnOnce(&Path) -> Result<A, BurnError>,
    ) -> Result<A, BurnError> {
        match &self.device {
            BurnDevice::RealDevice(path) => {
                let mount_point = self.mount(path)?;
                let out = use_it(&mount_point.join("data"));
                self.unmount(path);
                out
            }
            BurnDevice::ImageFile(_) => {
                // Dropped at the end of this scope, which removes the tree.
                let dir = tempfile::TempDir::new().map_err(BurnError::Io)?;
                let data_dir = dir.path().join("data");
                let result = self.exec.run_quiet(&args::extract(
                    &self.device,
                    "/data",
                    &data_dir,
                    None,
                ))?;
                if result.exit_code != 0 {
                    return Err(BurnError::MountFailed(format!(
                        "osirrox extraction failed: {}",
                        result.tail(10).join(" | ")
                    )));
                }
                use_it(&data_dir)
            }
        }
    }

    /// Extract one file as recorded by a specific session — the tier-2 read of
    /// a version a later session hid.
    pub fn extract_from_session(
        &self,
        session_ordinal: u32,
        disc_path: &str,
        dest: &Path,
    ) -> Result<(), BurnError> {
        let result = self.exec.run_quiet(&args::extract(
            &self.device,
            disc_path,
            dest,
            Some(session_ordinal),
        ))?;
        if result.exit_code != 0 {
            return Err(BurnError::MountFailed(format!(
                "session {session_ordinal} extraction of {disc_path} failed: {}",
                result.tail(10).join(" | ")
            )));
        }
        Ok(())
    }

    fn mount(&self, device_path: &str) -> Result<PathBuf, BurnError> {
        let result = self
            .exec
            .run_quiet(&[
                "udisksctl".to_string(),
                "mount".to_string(),
                "-b".to_string(),
                device_path.to_string(),
            ])
            .map_err(|e| match e {
                BurnError::ToolMissing(detail) => {
                    BurnError::MountFailed(format!("udisksctl not available: {detail}"))
                }
                other => other,
            })?;
        parse_mount_point(&result.lines).ok_or_else(|| {
            BurnError::MountFailed(format!(
                "udisksctl gave no mount point: {}",
                result.tail(10).join(" | ")
            ))
        })
    }

    /// Best-effort: a failed unmount must not fail a verification that has
    /// already read everything it needed.
    fn unmount(&self, device_path: &str) {
        let _ = self.exec.run_quiet(&[
            "udisksctl".to_string(),
            "unmount".to_string(),
            "-b".to_string(),
            device_path.to_string(),
        ]);
    }
}

/// The mount point from `udisksctl mount` output.
///
/// Two shapes, replacing `DiscReaderLive`'s two regexes: the success line
/// `Mounted <dev> at <path>` (with an optional trailing period, which some
/// udisks versions add) and the AlreadyMounted error, which names the path
/// between backtick and apostrophe.
pub fn parse_mount_point(lines: &[String]) -> Option<PathBuf> {
    lines.iter().find_map(|line| {
        if let Some((_, rest)) = line.split_once("already mounted at `") {
            return rest.split_once('\'').map(|(p, _)| PathBuf::from(p));
        }
        let (_, rest) = line.split_once("Mounted ")?;
        let (_, path) = rest.split_once(" at ")?;
        let path = path.trim();
        // Only the *sentence-final* period is dropped, so a directory whose
        // name genuinely ends in one survives everywhere else in the path.
        Some(PathBuf::from(path.strip_suffix('.').unwrap_or(path)))
    })
}
```

Add to `crates/diskrement-shell/src/burn/mod.rs`:

```rust
pub mod reader;
```

**Ordering note:** the AlreadyMounted branch is checked first because that line
also contains the word `Mounted`; the Scala matched the two regexes in the same
order, and swapping them would parse the device name as a path.

- [ ] **Step 4: Run the tests**

```bash
cargo test -p diskrement-shell burn::reader
```

Expected: 7 passed.

- [ ] **Step 5: Commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/diskrement-shell/src/burn/reader.rs crates/diskrement-shell/src/burn/mod.rs
git commit -m "feat(burn): mount a disc or extract an image for read-back"
```

---

### Task 7: The operator port and the shared test kit

**Read first:** `shell/src/main/scala/diskrement/shell/burn/BurnOperator.scala`
and the `ScriptedOperator` at the top of
`shell/src/test/scala/diskrement/shell/burn/XorrisoBurnerSpec.scala`.

**Files:**
- Modify: `crates/diskrement-shell/src/burn/mod.rs` (the `BurnOperator` trait)
- Create: `crates/diskrement-shell/src/burn/testkit.rs`

The operator port is how the burner and the verifier ask a human to swap a disc.
The CLI implements it over the console (Task 11); tests script it. The test kit
is gated so it is available both to in-crate unit tests and to
`crates/diskrement-shell/tests/`, which is what Task 10 needs — the same
mechanism `src/testutil.rs` already uses.

- [ ] **Step 1: Write the failing test**

Create `crates/diskrement-shell/src/burn/testkit.rs` with the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Answers are consumed in order and exhaustion means "no", so a scripted
    /// run can never loop forever on a re-prompt.
    #[test]
    fn the_scripted_operator_answers_in_order_then_refuses() {
        let op = ScriptedOperator::new(&[true, false]);
        assert!(op.confirm_insert("first?"));
        assert!(!op.confirm_insert("second?"));
        assert!(!op.confirm_insert("beyond the script"));
        op.progress("a note");
        assert_eq!(
            op.log(),
            vec![
                "first?".to_string(),
                "second?".to_string(),
                "beyond the script".to_string(),
                "a note".to_string(),
            ]
        );
    }
}
```

- [ ] **Step 2: Run and watch it fail**

```bash
cargo test -p diskrement-shell burn::testkit
```

- [ ] **Step 3: Write the trait and the kit**

In `crates/diskrement-shell/src/burn/mod.rs`, add the trait (below `DiscImage`,
above `BurnResult`) and the module declarations:

```rust
/// Operator interaction during real burns and verifies: insert-disc
/// confirmations and live progress lines. The CLI supplies a console
/// implementation; tests script one.
///
/// `&self` rather than `&mut self`: a scripted implementation mutates through
/// interior mutability, and the burner holds the operator immutably alongside
/// everything else it borrows.
pub trait BurnOperator {
    /// Show `message` and wait. `false` means abort (burn) or skip (verify).
    fn confirm_insert(&self, message: &str) -> bool;
    fn progress(&self, line: &str);
}
```

```rust
#[cfg(any(test, feature = "testutil"))]
pub mod testkit;
```

Create `crates/diskrement-shell/src/burn/testkit.rs`:

```rust
//! Test-only helpers shared by the burn unit tests and by
//! `tests/xorriso_image.rs`.
//!
//! Gated on `cfg(test)` *or* the `testutil` feature so both can reach it: the
//! in-crate unit tests take the first arm, the integration tests the second
//! (`diskrement-shell` is its own dev-dependency with that feature on).
//! Nothing here ships in the binary.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use diskrement_core::{ContentHash, RelPath, ScannedFile, SessionManifest};
use sha2::{Digest, Sha256};

use crate::burn::{BurnOperator, DiscImage, DiscIdentity, DiscMetadata};

/// A `BurnOperator` with canned answers and a recorded transcript
/// (`ScriptedOperator` in `XorrisoBurnerSpec.scala`). Running out of answers
/// means "no", so a re-prompt loop always terminates.
pub struct ScriptedOperator {
    answers: RefCell<std::collections::VecDeque<bool>>,
    log: RefCell<Vec<String>>,
}

impl ScriptedOperator {
    pub fn new(answers: &[bool]) -> Self {
        ScriptedOperator {
            answers: RefCell::new(answers.iter().copied().collect()),
            log: RefCell::new(Vec::new()),
        }
    }

    /// Every prompt and progress line, in order.
    pub fn log(&self) -> Vec<String> {
        self.log.borrow().clone()
    }

    /// Whether any recorded line contains `needle` — the usual assertion.
    pub fn said(&self, needle: &str) -> bool {
        self.log.borrow().iter().any(|l| l.contains(needle))
    }
}

impl BurnOperator for ScriptedOperator {
    fn confirm_insert(&self, message: &str) -> bool {
        self.log.borrow_mut().push(message.to_string());
        self.answers.borrow_mut().pop_front().unwrap_or(false)
    }

    fn progress(&self, line: &str) {
        self.log.borrow_mut().push(line.to_string());
    }
}

/// Is real `xorriso` on this machine? Tests that need it print a reason and
/// return when it is absent (spec §7) rather than failing or, worse, passing
/// silently.
pub fn has_xorriso() -> bool {
    std::process::Command::new("xorriso")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn sha256(body: &str) -> ContentHash {
    let mut h = Sha256::new();
    h.update(body.as_bytes());
    ContentHash(format!("{:x}", h.finalize()))
}

/// The fixed instant every burn unit test uses, matching the Scala specs'
/// `Instant.parse("2026-08-30T10:00:00Z")`.
pub fn ts() -> DateTime<Utc> {
    crate::catalog::time::parse("2026-08-30T10:00:00Z").expect("a valid instant")
}

pub fn identity(session_ordinal: u32) -> DiscIdentity {
    DiscIdentity {
        project_name: "photos".to_string(),
        disc_ordinal: 1,
        session_ordinal,
        created_at: ts(),
    }
}

/// A snapshot directory holding `files`, plus the `DiscImage` that stages it.
///
/// The catalog snapshot is a stub file: the stager copies it verbatim into
/// `/.diskrement/catalog.sqlite` and never opens it, so a real database would
/// only slow the test down.
pub fn image_of(
    base: &Path,
    session_ordinal: u32,
    files: &[(&str, &str)],
    close_disc: bool,
) -> DiscImage {
    let snapshot = base.join(format!("snapshot-{session_ordinal}"));
    let mut scanned = Vec::new();
    for (rel, content) in files {
        let path = snapshot.join(rel);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(&path, content).expect("write");
        scanned.push(ScannedFile {
            rel_path: RelPath((*rel).to_string()),
            content_hash: sha256(content),
            size_bytes: content.len() as u64,
            mtime: ts(),
        });
    }
    // The planner sorts by rel path, and the stager's progress accounting
    // assumes that order; keep it here so a multi-file image is deterministic.
    scanned.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));

    let catalog_snapshot: PathBuf = base.join(format!("catalog-{session_ordinal}.sqlite"));
    std::fs::write(&catalog_snapshot, format!("stub-catalog-{session_ordinal}"))
        .expect("write the stub catalog");

    DiscImage {
        staging_dir: base.join(format!("staging-{session_ordinal}")),
        snapshot_root: snapshot,
        files: scanned,
        metadata: DiscMetadata {
            identity: identity(session_ordinal),
            manifest: SessionManifest {
                project_name: "photos".to_string(),
                disc_ordinal: 1,
                session_ordinal,
                added: Vec::new(),
                modified: Vec::new(),
                deleted: Vec::new(),
            },
            catalog_snapshot,
            pointers: Vec::new(),
        },
        close_disc,
    }
}
```

- [ ] **Step 4: Run the test**

```bash
cargo test -p diskrement-shell burn::testkit
```

Expected: 1 passed. (`RelPath` derives `Ord`,
`crates/diskrement-core/src/model.rs:8`, so `sort_by` on it compiles as
written.)

- [ ] **Step 5: Commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/diskrement-shell/src/burn/testkit.rs crates/diskrement-shell/src/burn/mod.rs
git commit -m "feat(burn): the operator port, and a scripted one for the tests"
```

---

### Task 8: `xorriso::burner` — the real burner

**Read first:** `shell/src/main/scala/diskrement/shell/burn/XorrisoBurner.scala`
and `shell/src/test/scala/diskrement/shell/burn/XorrisoBurnerSpec.scala`.

**Files:**
- Create: `crates/diskrement-shell/src/burn/xorriso/burner.rs`
- Modify: `crates/diskrement-shell/src/burn/xorriso/mod.rs` (`pub mod burner;`)
- Modify: `crates/diskrement-shell/src/burn/mod.rs` (`Burner::Real`)

The sequence is the design and its order is load-bearing:
**preflight → wait for the right disc → stage (with re-hash) → capacity →
burn → post-burn identity check → eject.** Staging after the media check means a
declined disc costs nothing; the capacity check after staging means it is made
against the real image size; and every failure here happens before
`Pipeline::burn` commits, so the catalog is untouched.

- [ ] **Step 1: Write the failing tests**

Create `crates/diskrement-shell/src/burn/xorriso/burner.rs` with the test module
(the `XorrisoBurnerSpec` port, plus two prompt-wording cases the Scala suite
left to the manual checklist):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    use crate::burn::cmd::CmdOutput;
    use crate::burn::testkit::{image_of, ScriptedOperator};
    use crate::progress::NoopSink;

    const IDENTITY_1: &str = r#"{"projectName":"photos","discOrdinal":1,"sessionOrdinal":1,"createdAt":"2026-08-30T10:00:00Z"}"#;
    const IDENTITY_2: &str = r#"{"projectName":"photos","discOrdinal":1,"sessionOrdinal":2,"createdAt":"2026-08-30T10:00:00Z"}"#;
    const IDENTITY_7: &str = r#"{"projectName":"photos","discOrdinal":1,"sessionOrdinal":7,"createdAt":"2026-08-30T10:00:00Z"}"#;

    const BLANK: &[&str] = &["Media status : is blank", "Media space  : 500000s"];
    const APPENDABLE: &[&str] = &[
        "Media status : is written , is appendable",
        "Media summary: 1 session, 57 data blocks, 114k data, 23g free",
        "Media space  : 500000s",
    ];

    /// A scripted `Exec`.
    ///
    /// `toc` and `identities` are queues consumed per matching invocation,
    /// which is how one script expresses "a wrong disc, then the right one"
    /// and "the medium says session 1, the burned disc says session 2".
    struct Script {
        version: String,
        toc: RefCell<VecDeque<Vec<String>>>,
        identities: RefCell<VecDeque<Option<&'static str>>>,
        burn_exit: i32,
        burn_lines: Vec<String>,
        calls: RefCell<Vec<String>>,
    }

    impl Script {
        fn new() -> Self {
            Script {
                version: "xorriso 1.5.8.pl02".to_string(),
                toc: RefCell::new(VecDeque::new()),
                identities: RefCell::new(VecDeque::new()),
                burn_exit: 0,
                burn_lines: vec!["xorriso : UPDATE : 100% done".to_string()],
                calls: RefCell::new(Vec::new()),
            }
        }

        fn toc(mut self, states: &[&[&str]]) -> Self {
            self.toc = RefCell::new(
                states
                    .iter()
                    .map(|s| s.iter().map(|l| (*l).to_string()).collect())
                    .collect(),
            );
            self
        }

        fn identities(mut self, ids: &[Option<&'static str>]) -> Self {
            self.identities = RefCell::new(ids.iter().copied().collect());
            self
        }

        fn version(mut self, v: &str) -> Self {
            self.version = v.to_string();
            self
        }

        fn burn_fails(mut self) -> Self {
            self.burn_exit = 5;
            self.burn_lines = vec!["FAILURE : write error".to_string()];
            self
        }

        fn ran(&self, needle: &str) -> bool {
            self.calls.borrow().iter().any(|c| c.contains(needle))
        }

        fn respond(
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

    #[test]
    fn session_one_on_blank_media_burns_and_post_checks() {
        let dir = tempfile::TempDir::new().unwrap();
        let script = Script::new()
            .toc(&[BLANK])
            .identities(&[Some(IDENTITY_1)]);
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
        let op = ScriptedOperator::new(&[true]);
        let burner = XorrisoBurner::new(
            BurnDevice::RealDevice("/dev/sr0".to_string()),
            &exec,
            &op,
            &NoopSink,
            2,
        );

        let result = burner
            .burn(&image_of(dir.path(), 1, &[("a.txt", "hello")], false))
            .expect("the burn succeeds");
        assert!(result.image_bytes > 0);
        assert!(script.ran("-outdev"), "a first session writes to -outdev");
        assert!(script.ran("-close off"), "an unsealed session leaves it open");
        assert!(script.ran("-eject"), "a real device is ejected afterwards");
    }

    /// The insert prompt is contract: it is what the operator reads, and the
    /// differential harness compares it between the two binaries.
    #[test]
    fn the_insert_prompt_names_the_disc_the_project_and_the_seal() {
        let dir = tempfile::TempDir::new().unwrap();
        let script = Script::new().toc(&[BLANK]).identities(&[Some(IDENTITY_1)]);
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
        let op = ScriptedOperator::new(&[true]);
        XorrisoBurner::new(
            BurnDevice::RealDevice("/dev/sr0".to_string()),
            &exec,
            &op,
            &NoopSink,
            2,
        )
        .burn(&image_of(dir.path(), 1, &[("a.txt", "hello")], true))
        .expect("the burn succeeds");

        assert_eq!(
            op.log()[0],
            "Insert a blank disc for 'photos' disc 1 into /dev/sr0 and confirm. \
             This burn will FINALIZE the disc."
        );
    }

    #[test]
    fn a_top_off_prompt_names_the_last_burned_session() {
        let dir = tempfile::TempDir::new().unwrap();
        let script = Script::new()
            .toc(&[APPENDABLE])
            .identities(&[Some(IDENTITY_1), Some(IDENTITY_2)]);
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
        let op = ScriptedOperator::new(&[true]);
        XorrisoBurner::new(
            BurnDevice::RealDevice("/dev/sr0".to_string()),
            &exec,
            &op,
            &NoopSink,
            2,
        )
        .burn(&image_of(dir.path(), 2, &[("a.txt", "hello")], false))
        .expect("the burn succeeds");

        assert_eq!(
            op.log()[0],
            "Insert disc 1 of 'photos' (last burned session 1) into /dev/sr0 and confirm."
        );
    }

    /// Written media where a blank was asked for: refused, re-prompted, and
    /// accepted once the operator swaps the disc.
    #[test]
    fn session_one_refuses_written_media_and_succeeds_on_the_swap() {
        let dir = tempfile::TempDir::new().unwrap();
        let script = Script::new()
            .toc(&[APPENDABLE, BLANK])
            .identities(&[Some(IDENTITY_1), Some(IDENTITY_1)]);
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
        let op = ScriptedOperator::new(&[true, true]);
        let result = XorrisoBurner::new(
            BurnDevice::RealDevice("/dev/sr0".to_string()),
            &exec,
            &op,
            &NoopSink,
            2,
        )
        .burn(&image_of(dir.path(), 1, &[("a.txt", "hello")], false))
        .expect("the second disc is accepted");
        assert!(result.image_bytes > 0);
        assert!(
            op.said("Not the expected disc: found written media (expected a blank disc)"),
            "{:?}",
            op.log()
        );
    }

    /// A top-off is accepted only when the disc identifies as this project,
    /// this disc, at exactly the previous session ordinal.
    #[test]
    fn a_top_off_requires_the_previous_session_ordinal() {
        let dir = tempfile::TempDir::new().unwrap();
        let script = Script::new()
            .toc(&[APPENDABLE])
            .identities(&[Some(IDENTITY_1), Some(IDENTITY_2)]);
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
        let op = ScriptedOperator::new(&[true]);
        let result = XorrisoBurner::new(
            BurnDevice::RealDevice("/dev/sr0".to_string()),
            &exec,
            &op,
            &NoopSink,
            2,
        )
        .burn(&image_of(dir.path(), 2, &[("a.txt", "hello")], true))
        .expect("the burn succeeds");
        assert!(result.image_bytes > 0);
        assert!(script.ran("-dev"), "an append merges rather than restarts");
        assert!(script.ran("-close on"), "a sealing session closes the disc");
    }

    /// A declined prompt is `Aborted` — exit 0 at the CLI, and nothing burned.
    #[test]
    fn a_declined_prompt_aborts_without_staging() {
        let dir = tempfile::TempDir::new().unwrap();
        let script = Script::new().toc(&[BLANK]).identities(&[]);
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
        let op = ScriptedOperator::new(&[false]);
        let image = image_of(dir.path(), 1, &[("a.txt", "hello")], false);
        let err = XorrisoBurner::new(
            BurnDevice::RealDevice("/dev/sr0".to_string()),
            &exec,
            &op,
            &NoopSink,
            2,
        )
        .burn(&image)
        .unwrap_err();
        assert!(matches!(err, BurnError::Aborted));
        assert!(
            !image.staging_dir.exists(),
            "a declined burn stages nothing"
        );
        assert!(!script.ran("-commit"), "and writes nothing");
    }

    /// 1000 bytes free is less than the fixed 32 MiB overhead, so the burn is
    /// refused after staging and before any write.
    #[test]
    fn insufficient_capacity_refuses_before_writing() {
        let dir = tempfile::TempDir::new().unwrap();
        let script = Script::new()
            .toc(&[&["Media status : is blank", "Media space  : 0s"]])
            .identities(&[]);
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
        let op = ScriptedOperator::new(&[true]);
        let err = XorrisoBurner::new(
            BurnDevice::RealDevice("/dev/sr0".to_string()),
            &exec,
            &op,
            &NoopSink,
            2,
        )
        .burn(&image_of(dir.path(), 1, &[("a.txt", "hello")], false))
        .unwrap_err();
        assert!(matches!(err, BurnError::InsufficientCapacity { .. }), "{err}");
        assert!(!script.ran("-commit"), "nothing was written");
    }

    #[test]
    fn a_failing_xorriso_surfaces_its_exit_code_and_output_tail() {
        let dir = tempfile::TempDir::new().unwrap();
        let script = Script::new()
            .toc(&[BLANK])
            .identities(&[])
            .burn_fails();
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
        let op = ScriptedOperator::new(&[true]);
        let err = XorrisoBurner::new(
            BurnDevice::RealDevice("/dev/sr0".to_string()),
            &exec,
            &op,
            &NoopSink,
            2,
        )
        .burn(&image_of(dir.path(), 1, &[("a.txt", "hello")], false))
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "xorriso failed (exit 5): FAILURE : write error"
        );
    }

    /// The disc must read back as the session we just wrote. A drive that
    /// silently wrote nothing fails here rather than at the next top-off.
    #[test]
    fn a_post_burn_identity_mismatch_fails_the_burn() {
        let dir = tempfile::TempDir::new().unwrap();
        let script = Script::new()
            .toc(&[BLANK])
            .identities(&[Some(IDENTITY_7)]);
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
        let op = ScriptedOperator::new(&[true]);
        let err = XorrisoBurner::new(
            BurnDevice::RealDevice("/dev/sr0".to_string()),
            &exec,
            &op,
            &NoopSink,
            2,
        )
        .burn(&image_of(dir.path(), 1, &[("a.txt", "hello")], false))
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "post-burn sanity check failed: expected identity (photos, disc 1, session 1) \
             readable from the disc, found: Some(DiscIdentity(photos,1,7,2026-08-30T10:00:00Z))"
        );
    }

    /// A disc that reads back as nothing at all renders `None`, matching the
    /// Scala interpolation of an empty `Option`.
    #[test]
    fn an_unreadable_disc_post_burn_renders_none() {
        let dir = tempfile::TempDir::new().unwrap();
        let script = Script::new().toc(&[BLANK]).identities(&[None]);
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
        let op = ScriptedOperator::new(&[true]);
        let err = XorrisoBurner::new(
            BurnDevice::RealDevice("/dev/sr0".to_string()),
            &exec,
            &op,
            &NoopSink,
            2,
        )
        .burn(&image_of(dir.path(), 1, &[("a.txt", "hello")], false))
        .unwrap_err();
        assert!(err.to_string().ends_with("found: None"), "{err}");
    }

    #[test]
    fn xorriso_older_than_1_5_is_refused_before_anything_else() {
        let dir = tempfile::TempDir::new().unwrap();
        let script = Script::new().version("xorriso 1.4.2").toc(&[]).identities(&[]);
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
        let op = ScriptedOperator::new(&[true]);
        let err = XorrisoBurner::new(
            BurnDevice::RealDevice("/dev/sr0".to_string()),
            &exec,
            &op,
            &NoopSink,
            2,
        )
        .burn(&image_of(dir.path(), 1, &[("a.txt", "hello")], false))
        .unwrap_err();
        assert_eq!(err.to_string(), "xorriso 1.4.2 is too old (need 1.5 or newer)");
        assert!(op.log().is_empty(), "the operator was never prompted");
    }

    /// An image file has no tray, so nothing is ejected.
    #[test]
    fn an_image_file_is_never_ejected() {
        let dir = tempfile::TempDir::new().unwrap();
        let script = Script::new().toc(&[BLANK]).identities(&[Some(IDENTITY_1)]);
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
        let op = ScriptedOperator::new(&[true]);
        XorrisoBurner::new(
            BurnDevice::ImageFile(dir.path().join("d.iso")),
            &exec,
            &op,
            &NoopSink,
            2,
        )
        .burn(&image_of(dir.path(), 1, &[("a.txt", "hello")], false))
        .expect("the burn succeeds");
        assert!(!script.ran("-eject"));
    }
}
```

- [ ] **Step 2: Run and watch it fail**

```bash
cargo test -p diskrement-shell burn::xorriso::burner
```

- [ ] **Step 3: Write the implementation**

Above the tests in `crates/diskrement-shell/src/burn/xorriso/burner.rs`:

```rust
//! The real burner (`XorrisoBurner.scala`).
//!
//! Prompt for the disc, inspect it (strict identity check), stage with
//! re-hash, check capacity, burn, sanity-check the result, eject. Any failure
//! leaves the catalog uncommitted, because `Pipeline::burn` commits only after
//! this returns.

use crate::burn::cmd::Exec;
use crate::burn::media::{MediaInspector, MediaStatus};
use crate::burn::xorriso::{args, parse};
use crate::burn::{
    stager, BurnDevice, BurnError, BurnOperator, BurnResult, DiscIdentity, DiscImage,
};
use crate::progress::ProgressSink;

/// Filesystem metadata, session gap and the ISO's own structures. The Scala
/// constant, kept exactly: capacity refusals must trigger in the same places.
const FIXED_OVERHEAD_BYTES: u64 = 32 * 1024 * 1024;

pub struct XorrisoBurner<'a> {
    device: BurnDevice,
    exec: &'a dyn Exec,
    operator: &'a dyn BurnOperator,
    progress: &'a dyn ProgressSink,
    parallelism: usize,
}

impl<'a> XorrisoBurner<'a> {
    pub fn new(
        device: BurnDevice,
        exec: &'a dyn Exec,
        operator: &'a dyn BurnOperator,
        progress: &'a dyn ProgressSink,
        parallelism: usize,
    ) -> Self {
        XorrisoBurner {
            device,
            exec,
            operator,
            progress,
            parallelism,
        }
    }

    pub fn burn(&self, image: &DiscImage) -> Result<BurnResult, BurnError> {
        self.preflight()?;
        let free_bytes = self.await_correct_media(image)?;
        let staged = stager::stage(image, self.progress, self.parallelism)?;
        check_capacity(staged.image_bytes, free_bytes)?;
        self.write(image)?;
        self.post_burn_check(&image.metadata.identity)?;
        self.eject();
        Ok(staged)
    }

    /// xorriso 1.5 is the floor: `-compliance iso_9660_level=3` and the
    /// osirrox flags this code depends on are not all present before it.
    fn preflight(&self) -> Result<(), BurnError> {
        let out = self.exec.run_quiet(&args::version())?;
        match parse::version(&out.lines) {
            None => Err(BurnError::ToolMissing(
                "cannot parse `xorriso --version` output".to_string(),
            )),
            Some((major, minor, raw)) => {
                if major > 1 || (major == 1 && minor >= 5) {
                    Ok(())
                } else {
                    Err(BurnError::ToolTooOld(raw))
                }
            }
        }
    }

    /// Prompt-and-inspect until the right disc is in the drive; return its free
    /// space. A decline is `Aborted`; the wrong disc re-prompts forever, which
    /// is what an operator swapping discs actually wants.
    fn await_correct_media(&self, image: &DiscImage) -> Result<u64, BurnError> {
        let id = &image.metadata.identity;
        let expected = if id.session_ordinal == 1 {
            format!(
                "a blank disc for '{}' disc {}",
                id.project_name, id.disc_ordinal
            )
        } else {
            format!(
                "disc {} of '{}' (last burned session {})",
                id.disc_ordinal,
                id.project_name,
                id.session_ordinal - 1
            )
        };
        let seal_note = if image.close_disc {
            " This burn will FINALIZE the disc."
        } else {
            ""
        };
        let prompt = format!(
            "Insert {expected} into {} and confirm.{seal_note}",
            self.device.xorriso_address()
        );

        loop {
            if !self.operator.confirm_insert(&prompt) {
                return Err(BurnError::Aborted);
            }
            match self.check_media(id)? {
                Ok(free_bytes) => return Ok(free_bytes),
                Err(found) => self.operator.progress(&format!(
                    "Not the expected disc: found {found}. Swap discs and confirm again."
                )),
            }
        }
    }

    /// `Ok(free)` when this is the disc to write; `Err(description)` when it is
    /// not, worded for the operator.
    fn check_media(&self, id: &DiscIdentity) -> Result<Result<u64, String>, BurnError> {
        let status = MediaInspector::new(self.device.clone(), self.exec).inspect()?;
        Ok(match (status, id.session_ordinal) {
            (MediaStatus::NoMedia, _) => Err("no disc in the drive".to_string()),
            (MediaStatus::Blank(free), 1) => Ok(free),
            (MediaStatus::Blank(_), _) => {
                Err("a blank disc (expected written media to append to)".to_string())
            }
            (MediaStatus::Appendable { .. }, 1) => {
                Err("written media (expected a blank disc)".to_string())
            }
            (
                MediaStatus::Appendable {
                    free_bytes,
                    identity: Some(ref found),
                    ..
                },
                n,
            ) if found.project_name == id.project_name
                && found.disc_ordinal == id.disc_ordinal
                && found.session_ordinal + 1 == n =>
            {
                Ok(free_bytes)
            }
            (
                MediaStatus::Appendable {
                    identity: Some(found),
                    ..
                },
                _,
            ) => Err(format!(
                "disc '{}' #{} at session {}",
                found.project_name, found.disc_ordinal, found.session_ordinal
            )),
            (MediaStatus::Appendable { identity: None, .. }, _) => {
                Err("a written non-diskrement disc".to_string())
            }
            (MediaStatus::Closed(_), _) => Err("a finalized (closed) disc".to_string()),
            (MediaStatus::Unsuitable, _) => Err(
                "unsuitable media (e.g. a POW-formatted BD-R) — use a plain unformatted BD-R"
                    .to_string(),
            ),
        })
    }

    fn write(&self, image: &DiscImage) -> Result<(), BurnError> {
        let id = &image.metadata.identity;
        let volid = args::volid(&id.project_name, id.disc_ordinal);
        let command = args::burn(
            &self.device,
            &image.staging_dir,
            id.session_ordinal == 1,
            image.close_disc,
            &volid,
        );
        let result = self.exec.run(&command, &mut |line| {
            // xorriso's percentage lines; everything else is noise the
            // operator does not need mid-burn.
            if line.contains("UPDATE") {
                self.operator.progress(line);
            }
        })?;
        if result.exit_code != 0 {
            return Err(BurnError::BurnFailed {
                exit_code: result.exit_code,
                output_tail: result.tail(10),
            });
        }
        Ok(())
    }

    fn post_burn_check(&self, id: &DiscIdentity) -> Result<(), BurnError> {
        let found = MediaInspector::new(self.device.clone(), self.exec).read_identity()?;
        let matches = found.as_ref().is_some_and(|f| {
            f.project_name == id.project_name
                && f.disc_ordinal == id.disc_ordinal
                && f.session_ordinal == id.session_ordinal
        });
        if matches {
            return Ok(());
        }
        Err(BurnError::PostBurnCheckFailed(format!(
            "expected identity ({}, disc {}, session {}) readable from the disc, found: {}",
            id.project_name,
            id.disc_ordinal,
            id.session_ordinal,
            match &found {
                Some(f) => format!("Some({f})"),
                None => "None".to_string(),
            }
        )))
    }

    /// Best-effort: the burn already succeeded, and an eject hiccup must not
    /// turn a written disc into a failed run.
    fn eject(&self) {
        if let BurnDevice::ImageFile(_) = self.device {
            return;
        }
        let ok = self
            .exec
            .run_quiet(&args::eject(&self.device))
            .map(|r| r.exit_code == 0)
            .unwrap_or(false);
        if !ok {
            self.operator.progress("note: eject failed");
        }
    }
}

/// One percent of slack plus the fixed overhead — the Scala arithmetic,
/// unchanged.
fn check_capacity(staged_bytes: u64, free_bytes: u64) -> Result<(), BurnError> {
    let needed = staged_bytes + staged_bytes / 100 + FIXED_OVERHEAD_BYTES;
    if needed <= free_bytes {
        Ok(())
    } else {
        Err(BurnError::InsufficientCapacity {
            needed_bytes: needed,
            free_bytes,
        })
    }
}
```

**Note on `found.session_ordinal + 1 == n`:** the Scala wrote
`found.sessionOrdinal == n - 1`. Written that way in Rust, `n - 1` underflows
for `n == 0`, and `u32` underflow panics in debug builds. `n` is never 0 in
practice — the `(Blank, 1)` and `(Appendable, 1)` arms already caught session 1
— but the addition form has no such edge at all and means the same thing.

Add to `crates/diskrement-shell/src/burn/xorriso/mod.rs`:

```rust
pub mod burner;
```

And in `crates/diskrement-shell/src/burn/mod.rs`, replace the `Burner` enum and
its `burn`:

```rust
/// The burn backends. `--burner real|simulated` selects between them, and the
/// verifier arm moves with it (spec §5.2).
pub enum Burner<'a> {
    Simulated(simulated::SimulatedBurner<'a>),
    Real(xorriso::burner::XorrisoBurner<'a>),
}

impl Burner<'_> {
    pub fn burn(&self, image: &DiscImage) -> Result<BurnResult, BurnError> {
        match self {
            Burner::Simulated(b) => b.burn(image),
            Burner::Real(b) => b.burn(image),
        }
    }
}
```

- [ ] **Step 4: Run the tests**

```bash
cargo test -p diskrement-shell burn::xorriso::burner
```

Expected: 11 passed. If `a_declined_prompt_aborts_without_staging` fails on the
`staging_dir` assertion, do not weaken it: it is the proof that the media wait
precedes staging, which is the whole reason the sequence is ordered this way.

- [ ] **Step 5: Commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/diskrement-shell/src/burn/xorriso crates/diskrement-shell/src/burn/mod.rs
git commit -m "feat(burn): the xorriso burner, its media wait and its post-burn check"
```

---

### Task 9: The read-back disc verifier

**Read first:** `shell/src/main/scala/diskrement/shell/burn/DiscVerifier.scala`
and `shell/src/test/scala/diskrement/shell/burn/RealDiscVerifierSpec.scala`.

**Files:**
- Modify: `crates/diskrement-shell/src/burn/verifier.rs`
- Modify: `crates/diskrement-shell/src/pipeline/mod.rs` (one signature)

Two tiers. **Tier 1** hashes every file in the disc's merged `/data` view
against `visible`, and counts an expected-but-absent file as a mismatch.
**Tier 2** extracts each version a later session of the same disc hid and hashes
it against `superseded`; anything unreadable is a mismatch, because an
unreadable archived version is exactly what verification exists to find. A
declined disc is a **skip**, and a skipped disc is never marked verified.

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `crates/diskrement-shell/src/burn/verifier.rs`:

```rust
    mod disc {
        use super::*;
        use crate::burn::cmd::CmdOutput;
        use crate::burn::testkit::ScriptedOperator;
        use crate::burn::BurnDevice;
        use std::cell::RefCell;

        const APPENDABLE: &[&str] = &[
            "Media status : is written , is appendable",
            "Media summary: 1 session, 57 data blocks, 114k data, 23g free",
            "Media space  : 500000s",
        ];
        const IDENTITY: &str = r#"{"projectName":"photos","discOrdinal":1,"sessionOrdinal":1,"createdAt":"2026-08-30T10:00:00Z"}"#;
        const OTHER_DISC: &str = r#"{"projectName":"photos","discOrdinal":9,"sessionOrdinal":1,"createdAt":"2026-08-30T10:00:00Z"}"#;

        fn expectation() -> DiscExpectation {
            DiscExpectation {
                project_name: "photos".to_string(),
                disc_ordinal: 1,
                latest_session: BTreeMap::new(),
                visible: BTreeMap::new(),
                superseded: Vec::new(),
            }
        }

        /// A queue of identity payloads, one per `-extract` of project.json.
        struct Toc {
            identities: RefCell<std::collections::VecDeque<&'static str>>,
        }

        impl Toc {
            fn new(ids: &[&'static str]) -> Self {
                Toc {
                    identities: RefCell::new(ids.iter().copied().collect()),
                }
            }

            fn respond(
                &self,
                cmd: &[String],
                _: &mut dyn FnMut(&str),
            ) -> Result<CmdOutput, BurnError> {
                if cmd.iter().any(|a| a == "-toc") {
                    return Ok(CmdOutput {
                        exit_code: 0,
                        lines: APPENDABLE.iter().map(|s| (*s).to_string()).collect(),
                    });
                }
                if cmd.iter().any(|a| a == "-extract") {
                    // `args::extract` ends with `<disc path> <destination>`,
                    // so the two tails say which read this is.
                    let disc_path = &cmd[cmd.len() - 2];
                    let dest = cmd.last().expect("a destination");
                    if disc_path == "/.diskrement/project.json" {
                        let json = self
                            .identities
                            .borrow_mut()
                            .pop_front()
                            .expect("the script ran out of identities");
                        std::fs::write(dest, json).expect("writable destination");
                    } else {
                        // A `/data` extraction produces a directory; an empty
                        // one is the right answer for an empty expectation.
                        std::fs::create_dir_all(dest).expect("writable destination");
                    }
                    return Ok(CmdOutput {
                        exit_code: 0,
                        lines: Vec::new(),
                    });
                }
                Ok(CmdOutput {
                    exit_code: 0,
                    lines: Vec::new(),
                })
            }
        }

        /// Declining is a skip, not an abort: `verify` sweeps many discs and
        /// the operator may not have all of them to hand. A skipped disc is
        /// never marked verified — that is what the flag is for.
        #[test]
        fn a_declined_disc_is_skipped_and_nothing_is_read() {
            let toc = Toc::new(&[]);
            let exec = |c: &[String], l: &mut dyn FnMut(&str)| toc.respond(c, l);
            let op = ScriptedOperator::new(&[false]);
            let verifier = DiscVerifier::new(
                BurnDevice::RealDevice("/dev/sr0".to_string()),
                &exec,
                &op,
            );
            assert_eq!(
                verifier.verify(&expectation()).unwrap(),
                VerifyOutcome {
                    files_checked: 0,
                    mismatches: Vec::new(),
                    skipped: true,
                }
            );
            assert_eq!(
                op.log(),
                vec![
                    "Insert disc 1 of 'photos' into /dev/sr0 to verify it \
                     (or decline to skip this disc)."
                        .to_string()
                ]
            );
        }

        /// The wrong disc re-prompts, and the message carries the `MediaStatus`
        /// rendering the operator needs to see what is actually in the drive
        /// (the plan's D3).
        #[test]
        fn the_wrong_disc_re_prompts_naming_what_was_found() {
            let toc = Toc::new(&[OTHER_DISC]);
            let exec = |c: &[String], l: &mut dyn FnMut(&str)| toc.respond(c, l);
            // Confirm once with the wrong disc, then give up.
            let op = ScriptedOperator::new(&[true, false]);
            let outcome = DiscVerifier::new(
                BurnDevice::RealDevice("/dev/sr0".to_string()),
                &exec,
                &op,
            )
            .verify(&expectation())
            .unwrap();
            assert!(outcome.skipped);
            assert!(
                op.said(
                    "Not the expected disc (found: Appendable(1024000000,1,\
                     Some(DiscIdentity(photos,9,1,2026-08-30T10:00:00Z)))). \
                     Swap discs and confirm again."
                ),
                "{:?}",
                op.log()
            );
        }

        /// Any session ordinal of the right disc is acceptable to `verify` —
        /// unlike a top-off, which demands exactly the previous one.
        #[test]
        fn the_right_disc_at_any_session_is_accepted() {
            let toc = Toc::new(&[IDENTITY]);
            let exec = |c: &[String], l: &mut dyn FnMut(&str)| toc.respond(c, l);
            let op = ScriptedOperator::new(&[true]);
            // No `visible`, no `superseded`: the disc is accepted and both
            // tiers have nothing to check. The extraction of `/data` is what
            // an image-backed run exercises (tests/xorriso_image.rs).
            let outcome = DiscVerifier::new(
                BurnDevice::ImageFile(std::path::PathBuf::from("/tmp/none.iso")),
                &exec,
                &op,
            )
            .verify(&expectation())
            .unwrap();
            assert!(!outcome.skipped);
            assert_eq!(outcome.files_checked, 0);
        }
    }
```

The free-space number in the re-prompt assertion is `500000 * 2048 =
1024000000`; if the media fixture changes, recompute it rather than loosening
the assertion — the exact rendering is the point of the test.

- [ ] **Step 2: Run and watch it fail**

```bash
cargo test -p diskrement-shell burn::verifier
```

- [ ] **Step 3: Write the implementation**

In `crates/diskrement-shell/src/burn/verifier.rs`, add the imports and the
verifier above the existing `Verifier` enum:

```rust
use crate::burn::cmd::Exec;
use crate::burn::media::{MediaInspector, MediaStatus};
use crate::burn::reader::DiscReader;
use crate::burn::{BurnDevice, BurnOperator, DiscIdentity};
```

```rust
/// Read-back verification of physical (or image) media.
///
/// Tier 1 is the merged `/data` view: every file on the disc hashed against
/// `visible`, and every expected file missing from the disc counted as a
/// mismatch. Tier 2 is the versions this disc's later sessions hid, extracted
/// from the session that recorded them. An operator decline skips the disc.
pub struct DiscVerifier<'a> {
    device: BurnDevice,
    exec: &'a dyn Exec,
    operator: &'a dyn BurnOperator,
}

impl<'a> DiscVerifier<'a> {
    pub fn new(device: BurnDevice, exec: &'a dyn Exec, operator: &'a dyn BurnOperator) -> Self {
        DiscVerifier {
            device,
            exec,
            operator,
        }
    }

    pub fn verify(&self, exp: &DiscExpectation) -> Result<VerifyOutcome, BurnError> {
        if !self.await_disc(exp)? {
            return Ok(VerifyOutcome {
                files_checked: 0,
                mismatches: Vec::new(),
                skipped: true,
            });
        }
        let (checked_1, mut mismatches) = self.tier1(exp)?;
        let (checked_2, tier2) = self.tier2(exp)?;
        mismatches.extend(tier2);
        mismatches.sort();
        mismatches.dedup();
        Ok(VerifyOutcome {
            files_checked: checked_1 + checked_2,
            mismatches,
            skipped: false,
        })
    }

    /// The insert loop. Unlike the burner's, *any* session ordinal of the
    /// right disc is acceptable, and a decline is a skip rather than an abort.
    fn await_disc(&self, exp: &DiscExpectation) -> Result<bool, BurnError> {
        let prompt = format!(
            "Insert disc {} of '{}' into {} to verify it (or decline to skip this disc).",
            exp.disc_ordinal,
            exp.project_name,
            self.device.xorriso_address()
        );
        let matches = |id: &DiscIdentity| {
            id.project_name == exp.project_name && id.disc_ordinal == exp.disc_ordinal
        };
        loop {
            if !self.operator.confirm_insert(&prompt) {
                return Ok(false);
            }
            let status = MediaInspector::new(self.device.clone(), self.exec).inspect()?;
            let ok = match &status {
                MediaStatus::Appendable {
                    identity: Some(id), ..
                } => matches(id),
                MediaStatus::Closed(Some(id)) => matches(id),
                _ => false,
            };
            if ok {
                return Ok(true);
            }
            self.operator.progress(&format!(
                "Not the expected disc (found: {status}). Swap discs and confirm again."
            ));
        }
    }

    /// The merged view: what a mount shows.
    fn tier1(&self, exp: &DiscExpectation) -> Result<(usize, Vec<RelPath>), BurnError> {
        let reader = DiscReader::new(self.device.clone(), self.exec);
        reader.with_data_tree(|data_dir| {
            let mut files = Vec::new();
            for entry in walkdir::WalkDir::new(data_dir).sort_by_file_name() {
                let entry = entry.map_err(|e| BurnError::Io(std::io::Error::other(e)))?;
                if entry.file_type().is_file() {
                    files.push(entry.into_path());
                }
            }

            let mut found = BTreeMap::new();
            let mut mismatches = Vec::new();
            for file in &files {
                let rel = relativize(data_dir, file);
                let actual = hasher::hash(file).map_err(BurnError::Io)?;
                if exp.visible.get(&rel) != Some(&actual) {
                    mismatches.push(rel.clone());
                }
                found.insert(rel, actual);
            }
            let missing: Vec<RelPath> = exp
                .visible
                .keys()
                .filter(|rel| !found.contains_key(*rel))
                .cloned()
                .collect();

            let checked = files.len() + missing.len();
            mismatches.extend(missing);
            Ok((checked, mismatches))
        })
    }

    /// The versions later sessions of this same disc hid.
    ///
    /// Each extraction goes to its own fresh path, so nothing depends on
    /// whether osirrox would overwrite an existing file, and an extraction or
    /// hash failure is a mismatch rather than an error: an archived version
    /// that cannot be read is precisely the finding `verify` exists for.
    fn tier2(&self, exp: &DiscExpectation) -> Result<(usize, Vec<RelPath>), BurnError> {
        if exp.superseded.is_empty() {
            return Ok((0, Vec::new()));
        }
        let dir = tempfile::TempDir::new().map_err(BurnError::Io)?;
        let reader = DiscReader::new(self.device.clone(), self.exec);
        let mut mismatches = Vec::new();
        for (i, sv) in exp.superseded.iter().enumerate() {
            let dest = dir.path().join(format!("tier2-{i}.bin"));
            let checked = reader
                .extract_from_session(
                    sv.session_ordinal,
                    &format!("/data/{}", sv.rel_path.as_str()),
                    &dest,
                )
                .and_then(|()| hasher::hash(&dest).map_err(BurnError::Io));
            match checked {
                Ok(actual) if actual == sv.content_hash => {}
                _ => mismatches.push(sv.rel_path.clone()),
            }
        }
        Ok((exp.superseded.len(), mismatches))
    }
}
```

Then give the enum its second arm (and, with it, a lifetime):

```rust
/// The verification backends, chosen by the same decision that chooses the
/// burner (spec §5.2).
pub enum Verifier<'a> {
    Staging(StagingVerifier),
    Disc(DiscVerifier<'a>),
}

impl Verifier<'_> {
    pub fn verify(&self, exp: &DiscExpectation) -> Result<VerifyOutcome, BurnError> {
        match self {
            Verifier::Staging(v) => v.verify(exp),
            Verifier::Disc(v) => v.verify(exp),
        }
    }
}
```

And in `crates/diskrement-shell/src/pipeline/mod.rs`, widen the two signatures
that name it (`verify` and `verify_disc`):

```rust
        verifier: &Verifier<'_>,
```

**Why `dedup` in `verify` and not in the Scala:** tier 1 and tier 2 are disjoint
by construction — tier 1 reports paths visible on the disc, tier 2 reports paths
hidden on it — so the dedup can only fire if a path is somehow both, and
reporting the same file twice in the CLI's mismatch list would be a rendering
bug, not information. Sorting has the same justification P6 recorded for
`StagingVerifier`: `Files.walk`'s order was unspecified, so the Scala's
multi-mismatch report was not reproducible run to run.

- [ ] **Step 4: Run the tests**

```bash
cargo test -p diskrement-shell burn::verifier
```

Expected: the four P6 staging tests plus 3 new ones, all green.

- [ ] **Step 5: Commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/diskrement-shell/src/burn/verifier.rs crates/diskrement-shell/src/pipeline/mod.rs
git commit -m "feat(verify): the two-tier read-back disc verifier"
```

---

### Task 10: The image-backed suite — real `xorriso`, no hardware

**Read first:** `shell/src/test/scala/diskrement/shell/burn/XorrisoImageITSpec.scala`,
the image half of `DiscReaderSpec.scala`, and `RealDiscVerifierSpec.scala`.

**Files:**
- Create: `crates/diskrement-shell/tests/xorriso_image.rs`

This is the phase's exit-bar test: the production code path end to end against
the real tool, through `stdio:` image files. It needs no drive and no media.
Two `stdio:` quirks recorded in Plan 4a apply and must not be "fixed" here:
`-close on` has no observable effect on an image file (it stays appendable), and
an image reports a fake huge free space — closing and capacity are assertable
only on real media, which is what the manual checklist is for.

- [ ] **Step 1: Write the suite**

Create `crates/diskrement-shell/tests/xorriso_image.rs`:

```rust
//! The real xorriso path, end to end, against `stdio:` image files
//! (`XorrisoImageITSpec.scala`, plus the image halves of `DiscReaderSpec` and
//! `RealDiscVerifierSpec`).
//!
//! Every test here spawns the actual tool. When `xorriso` is not installed
//! they skip — *loudly*, naming the reason and the fix (spec §7): a suite that
//! silently checked nothing is exactly the failure mode CLAUDE.md records for
//! the sbt IT suite.

use std::collections::BTreeMap;
use std::path::Path;

use diskrement_core::RelPath;
use diskrement_shell::burn::cmd::{self, CmdOutput, Exec};
use diskrement_shell::burn::media::{MediaInspector, MediaStatus};
use diskrement_shell::burn::reader::DiscReader;
use diskrement_shell::burn::testkit::{has_xorriso, image_of, sha256, ScriptedOperator};
use diskrement_shell::burn::verifier::{DiscExpectation, DiscVerifier, VerifyOutcome};
use diskrement_shell::burn::xorriso::{args, burner::XorrisoBurner};
use diskrement_shell::burn::{BurnDevice, BurnError};
use diskrement_shell::catalog::model::SupersededVersion;
use diskrement_shell::progress::NoopSink;

/// `false` — with a printed reason — when this machine cannot run the suite.
fn ready(test: &str) -> bool {
    if has_xorriso() {
        return true;
    }
    eprintln!(
        "SKIP [{test}]: xorriso is not installed, so the real burn path cannot be \
         exercised here. Install the libisoburn/xorriso package and re-run."
    );
    false
}

/// The production `Exec`.
///
/// A `fn` pointer rather than a closure: the blanket impl in `burn::cmd` is
/// higher-ranked over the two reference arguments, and a `fn` pointer type
/// satisfies it for every lifetime with no inference to go wrong.
type SystemExec = fn(&[String], &mut dyn FnMut(&str)) -> Result<CmdOutput, BurnError>;
const SYSTEM_EXEC: SystemExec = cmd::system;

/// Burn one session of `files` onto `iso`, through the real burner.
fn burn_session(base: &Path, iso: &Path, session: u32, files: &[(&str, &str)]) {
    let exec = SYSTEM_EXEC;
    let operator = ScriptedOperator::new(&[true]);
    let burner = XorrisoBurner::new(
        BurnDevice::ImageFile(iso.to_path_buf()),
        &exec,
        &operator,
        &NoopSink,
        2,
    );
    let result = burner
        .burn(&image_of(base, session, files, false))
        .unwrap_or_else(|e| panic!("session {session} failed: {e}"));
    assert!(result.image_bytes > 0, "session {session} wrote nothing");
}

/// The core claim: two sessions, the second hiding a path the first wrote; the
/// merged view shows the new version and the old session still holds the old
/// one. That is what makes a top-off safe and tier-2 verification possible.
#[test]
fn a_second_session_hides_the_first_version_without_destroying_it() {
    if !ready("a_second_session_hides_the_first_version_without_destroying_it") {
        return;
    }
    let dir = tempfile::TempDir::new().expect("temp dir");
    let iso = dir.path().join("disc.iso");
    let exec = SYSTEM_EXEC;
    let inspector = MediaInspector::new(BurnDevice::ImageFile(iso.clone()), &exec);

    burn_session(
        dir.path(),
        &iso,
        1,
        &[("a.txt", "hello v1"), ("b.txt", "stable")],
    );
    match inspector.inspect().expect("the image inspects") {
        MediaStatus::Appendable {
            sessions,
            identity: Some(id),
            ..
        } => {
            assert_eq!(sessions, 1);
            assert_eq!(id.session_ordinal, 1);
        }
        other => panic!("after session 1: {other}"),
    }

    burn_session(dir.path(), &iso, 2, &[("a.txt", "hello v2")]);
    match inspector.inspect().expect("the image inspects") {
        MediaStatus::Appendable {
            sessions,
            identity: Some(id),
            ..
        } => {
            assert_eq!(sessions, 2);
            assert_eq!(id.session_ordinal, 2);
        }
        other => panic!("after session 2: {other}"),
    }

    let device = BurnDevice::ImageFile(iso.clone());
    let merged = dir.path().join("merged-a.txt");
    let out: CmdOutput = exec
        .run_quiet(&args::extract(&device, "/data/a.txt", &merged, None))
        .expect("the extraction runs");
    assert_eq!(out.exit_code, 0, "{:?}", out.lines);
    assert_eq!(
        std::fs::read_to_string(&merged).expect("read"),
        "hello v2",
        "the merged view shows the newest version"
    );

    let old = dir.path().join("old-a.txt");
    let out = exec
        .run_quiet(&args::extract(&device, "/data/a.txt", &old, Some(1)))
        .expect("the extraction runs");
    assert_eq!(out.exit_code, 0, "{:?}", out.lines);
    assert_eq!(
        std::fs::read_to_string(&old).expect("read"),
        "hello v1",
        "session 1 still holds the version session 2 hid"
    );
}

/// A top-off onto a disc at the wrong session is refused, re-prompted, and —
/// when the operator gives up — aborted. Nothing is written.
#[test]
fn a_top_off_at_the_wrong_session_is_refused_until_the_operator_gives_up() {
    if !ready("a_top_off_at_the_wrong_session_is_refused_until_the_operator_gives_up") {
        return;
    }
    let dir = tempfile::TempDir::new().expect("temp dir");
    let iso = dir.path().join("foreign.iso");
    burn_session(dir.path(), &iso, 1, &[("x.txt", "other")]);

    let exec = SYSTEM_EXEC;
    // The image identifies as photos/disc 1/session 1; burning session 3
    // expects to find session 2. One retry, then decline.
    let operator = ScriptedOperator::new(&[true, false]);
    let err = XorrisoBurner::new(
        BurnDevice::ImageFile(iso.clone()),
        &exec,
        &operator,
        &NoopSink,
        2,
    )
    .burn(&image_of(dir.path(), 3, &[("y.txt", "new")], false))
    .expect_err("the wrong disc is never written");
    assert!(matches!(err, BurnError::Aborted), "{err}");
    assert!(
        operator.said("disc 'photos' #1 at session 1"),
        "the operator is told what was actually in the drive: {:?}",
        operator.log()
    );
}

/// `DiscReader` over an image: the merged tree for tier 1, one session's copy
/// for tier 2.
#[test]
fn the_reader_exposes_the_merged_tree_and_a_single_session() {
    if !ready("the_reader_exposes_the_merged_tree_and_a_single_session") {
        return;
    }
    let dir = tempfile::TempDir::new().expect("temp dir");
    let iso = dir.path().join("disc.iso");
    burn_session(dir.path(), &iso, 1, &[("a.txt", "v1")]);
    burn_session(dir.path(), &iso, 2, &[("a.txt", "v2")]);

    let exec = SYSTEM_EXEC;
    let reader = DiscReader::new(BurnDevice::ImageFile(iso.clone()), &exec);
    let merged = reader
        .with_data_tree(|data| std::fs::read_to_string(data.join("a.txt")).map_err(BurnError::Io))
        .expect("the tree is readable");
    assert_eq!(merged, "v2");

    let old = dir.path().join("old.txt");
    reader
        .extract_from_session(1, "/data/a.txt", &old)
        .expect("session 1 is readable");
    assert_eq!(std::fs::read_to_string(&old).expect("read"), "v1");
}

/// Two-tier verification against a real image: a clean disc passes, a
/// tampered expectation mismatches in both tiers, and a declined disc is
/// skipped without being read.
#[test]
fn two_tier_verification_passes_clean_mismatches_tampered_and_skips_declined() {
    if !ready("two_tier_verification_passes_clean_mismatches_tampered_and_skips_declined") {
        return;
    }
    let dir = tempfile::TempDir::new().expect("temp dir");
    let iso = dir.path().join("disc.iso");
    burn_session(dir.path(), &iso, 1, &[("a.txt", "v1"), ("b.txt", "keep")]);
    burn_session(dir.path(), &iso, 2, &[("a.txt", "v2")]);

    let expectation = DiscExpectation {
        project_name: "photos".to_string(),
        disc_ordinal: 1,
        latest_session: BTreeMap::new(),
        visible: BTreeMap::from([
            (RelPath("a.txt".to_string()), sha256("v2")),
            (RelPath("b.txt".to_string()), sha256("keep")),
        ]),
        superseded: vec![SupersededVersion {
            session_ordinal: 1,
            rel_path: RelPath("a.txt".to_string()),
            content_hash: sha256("v1"),
        }],
    };

    let exec = SYSTEM_EXEC;
    let device = BurnDevice::ImageFile(iso.clone());

    let accept = ScriptedOperator::new(&[true]);
    let clean = DiscVerifier::new(device.clone(), &exec, &accept)
        .verify(&expectation)
        .expect("verification runs");
    assert_eq!(
        clean,
        VerifyOutcome {
            files_checked: 3, // two visible + one superseded
            mismatches: Vec::new(),
            skipped: false,
        }
    );

    let mut tampered = expectation.clone();
    tampered
        .visible
        .insert(RelPath("b.txt".to_string()), sha256("TAMPERED"));
    tampered.superseded = vec![SupersededVersion {
        session_ordinal: 1,
        rel_path: RelPath("a.txt".to_string()),
        content_hash: sha256("WRONG"),
    }];
    let accept = ScriptedOperator::new(&[true]);
    let bad = DiscVerifier::new(device.clone(), &exec, &accept)
        .verify(&tampered)
        .expect("verification runs");
    assert_eq!(
        bad.mismatches,
        vec![
            RelPath("a.txt".to_string()),
            RelPath("b.txt".to_string())
        ],
        "both tiers report, sorted"
    );

    let decline = ScriptedOperator::new(&[false]);
    let skipped = DiscVerifier::new(device, &exec, &decline)
        .verify(&expectation)
        .expect("verification runs");
    assert_eq!(
        skipped,
        VerifyOutcome {
            files_checked: 0,
            mismatches: Vec::new(),
            skipped: true,
        }
    );
}

/// A file the catalog expects and the disc does not have is a mismatch, not a
/// silent pass — the case a hash-only comparison misses by construction.
#[test]
fn a_file_missing_from_the_disc_is_a_mismatch() {
    if !ready("a_file_missing_from_the_disc_is_a_mismatch") {
        return;
    }
    let dir = tempfile::TempDir::new().expect("temp dir");
    let iso = dir.path().join("disc.iso");
    burn_session(dir.path(), &iso, 1, &[("only.txt", "here")]);

    let exec = SYSTEM_EXEC;
    let operator = ScriptedOperator::new(&[true]);
    let outcome = DiscVerifier::new(BurnDevice::ImageFile(iso), &exec, &operator)
        .verify(&DiscExpectation {
            project_name: "photos".to_string(),
            disc_ordinal: 1,
            latest_session: BTreeMap::new(),
            visible: BTreeMap::from([
                (RelPath("only.txt".to_string()), sha256("here")),
                (RelPath("ghost.txt".to_string()), sha256("missing")),
            ]),
            superseded: Vec::new(),
        })
        .expect("verification runs");
    assert_eq!(outcome.mismatches, vec![RelPath("ghost.txt".to_string())]);
    assert_eq!(outcome.files_checked, 2, "one present plus one missing");
}
```

- [ ] **Step 2: Run the suite and read the output**

```bash
cargo test -p diskrement-shell --test xorriso_image -- --nocapture
```

Expected on a machine with xorriso: **5 passed, no `SKIP` lines**. If any `SKIP`
line appears, the run proved nothing — install xorriso before claiming this task
is done. (`DiscExpectation` must derive `Clone` for the `tampered` copy; it
already does — `verifier.rs:37`.)

- [ ] **Step 3: Commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/diskrement-shell/tests/xorriso_image.rs
git commit -m "test(burn): the real xorriso path end to end against image files"
```

---

### Task 11: Wire the real backend into the CLI

**Read first:** `cli/src/main/scala/diskrement/cli/Runner.scala`
(`ConsoleBurnOperator` and the `Backend.Real` arm of `withPipeline`) and
`cli/src/main/scala/diskrement/cli/Render.scala:95-117`.

**Files:**
- Modify: `crates/diskrement-cli/src/prompt.rs` (`ConsoleOperator`)
- Modify: `crates/diskrement-cli/src/render.rs` (`error_line`)
- Modify: `crates/diskrement-cli/src/main.rs` (both real arms; the error print)

Three edits and one deletion. The deletion matters most: the two "not
implemented yet" refusals go away, and with them the last reason a user has to
pass `--burner simulated` on a machine with a drive.

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `crates/diskrement-cli/src/render.rs`:

```rust
    /// `Render.handle`'s one special case: an aborted burn is a successful run
    /// that wrote nothing, so it renders as `Aborted.` and exits 0 — not as
    /// the error's own "aborted by operator" wording.
    #[test]
    fn an_aborted_burn_renders_as_aborted_and_nothing_else_is_special() {
        use diskrement_shell::burn::BurnError;
        use diskrement_shell::error::PipelineError;

        assert_eq!(
            error_line(&PipelineError::Burn(BurnError::Aborted)),
            "Aborted."
        );
        assert_eq!(PipelineError::Burn(BurnError::Aborted).exit_code(), 0);
        assert_eq!(
            error_line(&PipelineError::Burn(BurnError::NoMedia("/dev/sr0".into()))),
            "no disc in /dev/sr0"
        );
        assert_eq!(
            error_line(&PipelineError::ProjectNotFound("photos".into())),
            "project not found: photos"
        );
    }
```

And append to `mod tests` in `crates/diskrement-cli/src/prompt.rs`:

```rust
    /// The console operator is the CLI's `BurnOperator`: a confirmation that
    /// never assumes yes (an insert prompt has no `--yes`), and progress lines
    /// on stdout.
    #[test]
    fn the_console_operator_refuses_on_eof() {
        use diskrement_shell::burn::BurnOperator;
        // Reading real stdin in a test is not viable; what is asserted here is
        // the wiring — `confirm_insert` goes through `confirm`, which refuses
        // on EOF, and `progress` does not panic.
        let op = ConsoleOperator;
        op.progress("a line");
        assert!(!confirm_from("Insert?", false, || None));
        let _: &dyn BurnOperator = &op;
    }
```

- [ ] **Step 2: Run and watch them fail**

```bash
cargo test -p diskrement-cli render:: prompt::
```

- [ ] **Step 3: Add `error_line` and `ConsoleOperator`**

In `crates/diskrement-cli/src/render.rs`, add near the other top-level
functions:

```rust
/// The stderr line for a failed run (`Render.handle`'s message half).
///
/// `PipelineError`'s own `Display` is the message everywhere except an aborted
/// burn: that one is not a fault to report but an action the operator took, so
/// it reads as `Aborted.` and exits 0.
pub fn error_line(err: &PipelineError) -> String {
    match err {
        PipelineError::Burn(diskrement_shell::burn::BurnError::Aborted) => "Aborted.".to_string(),
        other => other.to_string(),
    }
}
```

In `crates/diskrement-cli/src/prompt.rs`:

```rust
use diskrement_shell::burn::BurnOperator;

/// The operator port over the console (`ConsoleBurnOperator` in Runner.scala).
///
/// `confirm_insert` never assumes yes: `burn --yes` answers "burn the staged
/// plan?", not "is the right disc in the drive?", and no flag may skip the
/// second question.
pub struct ConsoleOperator;

impl BurnOperator for ConsoleOperator {
    fn confirm_insert(&self, message: &str) -> bool {
        confirm(message, false)
    }

    fn progress(&self, line: &str) {
        println!("{line}");
    }
}
```

- [ ] **Step 4: Wire both arms in `main.rs`**

**Replace** the two existing `diskrement_shell::burn::…` import lines (do not
add alongside them — a duplicate `Verifier` or `SimulatedBurner` import is
`E0252`, which cost P7 a rework), and add `Path` to the `std::path` import:

```rust
use std::path::{Path, PathBuf};

use diskrement_shell::burn::cmd::{self, CmdOutput, Exec};
use diskrement_shell::burn::verifier::{DiscVerifier, StagingVerifier, Verifier};
use diskrement_shell::burn::xorriso::burner::XorrisoBurner;
use diskrement_shell::burn::{simulated::SimulatedBurner, BurnError, Burner};
```

Replace the error print in `main()`:

```rust
            Err(err) => {
                eprintln!("{}", render::error_line(&err));
                ExitCode::from(err.exit_code() as u8)
            }
```

Add the one shared edge, next to `env_lookup`:

```rust
/// The production `Exec`.
///
/// A `fn` pointer, not a closure: the blanket impl in `burn::cmd` is
/// higher-ranked over both reference arguments, and a `fn` pointer type
/// satisfies it for every lifetime with no inference to go wrong.
/// `&SYSTEM_EXEC` then coerces to `&dyn Exec` wherever a burner or a verifier
/// needs one.
type SystemExec = fn(&[String], &mut dyn FnMut(&str)) -> Result<CmdOutput, BurnError>;
const SYSTEM_EXEC: SystemExec = cmd::system;
```

Replace the whole `Command::Burn` backend `match` (the arm that currently
returns "the real burner is not implemented yet") with:

```rust
            let par = diskrement_shell::fs::scanner::parallelism_from_env();
            for w in &par.warnings {
                eprintln!("{w}");
            }
            let exec = SYSTEM_EXEC;
            let operator = prompt::ConsoleOperator;
            let burner = match backend::resolve(cli.burner, cli.device.as_deref(), &env_lookup) {
                backend::Backend::Simulated => {
                    Burner::Simulated(SimulatedBurner::new(&sink, par.value))
                }
                backend::Backend::Real(device) => Burner::Real(XorrisoBurner::new(
                    device, &exec, &operator, &sink, par.value,
                )),
            };
```

and replace `resolve_verifier` with:

```rust
/// The verification backend, chosen by the same one decision that chooses the
/// burner (spec §5.2): verifying a simulated burn by mounting a physical disc
/// is meaningless, so the two arms move together.
fn resolve_verifier<'a>(
    burner: Option<args::BurnerKind>,
    device: Option<&str>,
    work_dir: &Path,
    exec: &'a dyn Exec,
    operator: &'a dyn diskrement_shell::burn::BurnOperator,
) -> Verifier<'a> {
    match backend::resolve(burner, device, &env_lookup) {
        backend::Backend::Simulated => {
            Verifier::Staging(StagingVerifier::new(work_dir.join("staging")))
        }
        backend::Backend::Real(device) => {
            Verifier::Disc(DiscVerifier::new(device, exec, operator))
        }
    }
}
```

Its caller in the `Command::Verify` arm loses its `?` (resolution can no longer
fail) and gains the two locals, which must be declared **before** the verifier
so they outlive it:

```rust
            let exec = SYSTEM_EXEC;
            let operator = prompt::ConsoleOperator;
            let verifier = resolve_verifier(
                cli.burner,
                cli.device.as_deref(),
                std::path::Path::new(&p.work_dir),
                &exec,
                &operator,
            );
```

`PathBuf` is still used by the dispatch arms, so keep both names in that
`std::path` import.

- [ ] **Step 5: Run the CLI unit tests**

```bash
cargo test -p diskrement-cli --bin diskrement
```

Expected: green. Two failures are expected in `tests/binary.rs` — the two
"not implemented yet" tests — and Task 12 replaces them; if you would rather
not leave the tree red between two commits, do Task 12's Step 1 first and
commit the pair together.

- [ ] **Step 6: Commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/diskrement-cli/src
git commit -m "feat(cli): wire the real burner and the read-back verifier"
```

---

### Task 12: The real backend end to end through the spawned binary

**Files:**
- Modify: `crates/diskrement-cli/tests/binary.rs`

Three tests replace the two refusals. None of them needs a drive: the burn path
stops at the insert prompt, and the verify path stops at the same question one
level up. Both are exactly what an operator sees with no disc in the tray.

- [ ] **Step 1: Delete the two "not implemented" tests**

Remove `asking_for_the_real_burner_is_refused_for_now` (in the P5 module) and
`verify_on_the_real_backend_refuses_until_p8` (in the P6 module). Their subject
no longer exists; keeping them as `#[ignore]` would leave a test that asserts
the opposite of the contract.

- [ ] **Step 2: Write the replacements**

Add a new module at the end of `crates/diskrement-cli/tests/binary.rs`:

```rust
/// P8: the real backend, driven as far as a machine with no drive can go.
mod p8 {
    use super::*;

    /// Is real `xorriso` installed? The burn path preflights it, so a machine
    /// without it can only test the tool-missing branch.
    fn has_xorriso() -> bool {
        std::process::Command::new("xorriso")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// `--burner real` asks for a disc. With no stdin, the answer is "no", and
    /// an aborted burn is a successful run that wrote nothing: exit 0,
    /// `Aborted.` on stderr, and a catalog with no discs in it.
    #[test]
    fn a_real_burn_asks_for_the_disc_and_an_unanswered_prompt_aborts() {
        if !has_xorriso() {
            eprintln!(
                "SKIP [a_real_burn_asks_for_the_disc_and_an_unanswered_prompt_aborts]: \
                 xorriso is not installed; the burn path preflights it. Install the \
                 libisoburn/xorriso package and re-run."
            );
            return;
        }
        let p = project();
        init_scan_plan(&p);

        Command::cargo_bin("diskrement")
            .expect("binary builds")
            .arg("--catalog")
            .arg(&p.catalog)
            .args(["--burner", "real", "--device", "/dev/sr0", "burn", "p", "--yes"])
            .env_remove("DISKREMENT_BURNER")
            .env_remove("DISKREMENT_DEVICE")
            .assert()
            .code(0)
            .stdout(contains(
                "Insert a blank disc for 'p' disc 1 into /dev/sr0 and confirm. [y/N]",
            ))
            .stderr(contains("Aborted."));

        // Commit-after-success: the aborted burn committed nothing.
        base(&p)
            .args(["status", "p"])
            .assert()
            .success()
            .stdout(contains("Last burn: never"));
        // And the staged plan survives, so the operator can retry.
        assert!(p.work.join("staged/plan.json").is_file());
    }

    /// Without xorriso on `PATH` the same run fails at the preflight, with the
    /// message that names the package to install — exit 6, the media/burner
    /// rung.
    #[test]
    fn a_real_burn_without_xorriso_says_what_to_install() {
        let p = project();
        init_scan_plan(&p);

        Command::cargo_bin("diskrement")
            .expect("binary builds")
            .arg("--catalog")
            .arg(&p.catalog)
            .args(["--burner", "real", "burn", "p", "--yes"])
            .env_remove("DISKREMENT_BURNER")
            // An empty PATH reaches only the child: assert_cmd invokes the
            // binary under test by absolute path.
            .env("PATH", "/nonexistent")
            .assert()
            .code(6)
            .stderr(contains("xorriso is not available"))
            .stderr(contains("install the libisoburn/xorriso package"));
    }

    /// `verify` on the real backend asks for each disc and skips the ones the
    /// operator does not produce. A skipped disc is reported as skipped, and
    /// the run still exits 0 — nothing was found wrong, because nothing was
    /// checked.
    #[test]
    fn a_real_verify_asks_for_each_disc_and_skips_the_ones_not_produced() {
        let f = burned_project();

        Command::cargo_bin("diskrement")
            .expect("binary builds")
            .env("DISKREMENT_BURNER", "real")
            .env("DISKREMENT_DEVICE", "/dev/sr0")
            .arg("--catalog")
            .arg(&f.catalog)
            .args(["verify", "p"])
            .assert()
            .code(0)
            .stdout(contains(
                "Insert disc 1 of 'p' into /dev/sr0 to verify it (or decline to skip this disc).",
            ))
            .stdout(contains("disc 1: skipped (not checked)"));
    }
}
```

**Check the helper names before running:** this module reuses `project()`,
`init_scan_plan()`, `base()`, `burned_project()` and `contains` from the file's
existing modules. If the P5/P6 modules keep them private to themselves, promote
the ones you need to the file's top-level `mod` scope rather than duplicating
them — a second `init_scan_plan` is a second fixture that will drift.

- [ ] **Step 3: Run the binary suite**

```bash
cargo test -p diskrement-cli --test binary -- --nocapture
```

Expected: green, with no `SKIP` line on a machine that has xorriso.

- [ ] **Step 4: Commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/diskrement-cli/tests/binary.rs
git commit -m "test(cli): the real backend's prompts, abort and skip through the binary"
```

---

### Task 13: The differential task — teach the harness about the real backend

**Read first:** `crates/diskrement-difftest/README.md` and spec §8.1.

**Files:**
- Modify: `crates/diskrement-difftest/src/scenario.rs`
- Modify: `crates/diskrement-difftest/src/runner.rs`
- Modify: `crates/diskrement-difftest/src/corpus.rs`
- Modify: `crates/diskrement-difftest/README.md`

P8 lands no new *command*, so `RUST_IMPLEMENTED` does not change — say so in the
commit message, because every phase since P3 has changed it and a reviewer will
look. What P8 does land is a second *backend*, and the harness cannot reach it:
`runner` pins `DISKREMENT_BURNER=simulated` for every step of every scenario, by
design, so that a differential run never tries to drive hardware.

The narrow thing that makes the real backend comparable without hardware: with
no stdin, both binaries stop at the operator prompt. That prompt is stdout, and
stdout is compared. So one scenario now pins, across both implementations and
the interleaving, the insert-prompt wording, the verify-prompt wording, the
`skipped (not checked)` render, and — through the state diff — that an aborted
burn commits nothing.

- [ ] **Step 1: Add the per-step backend to `scenario.rs`**

```rust
/// Which burn backend one command step runs under.
///
/// Scenario-level would have been simpler, but the interesting case is
/// precisely a project burned with the simulated backend and then *verified*
/// with the real one — which is what a user does when they flip the
/// environment variable, and what spec §3.2 makes a contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// `DISKREMENT_BURNER=simulated`, `DISKREMENT_DEVICE` unset. The default
    /// for every step, so a scenario can never drive hardware by accident.
    Simulated,
    /// `DISKREMENT_BURNER=real`, `DISKREMENT_DEVICE=<device>`. `{work}` and
    /// `{source}` are substituted in `device` as they are in arguments.
    ///
    /// Only ever pointed at a path that does not exist: the real backend asks
    /// the operator before it opens the device, and a runner-spawned child has
    /// no stdin, so the device is never touched.
    Real { device: &'static str },
}
```

Add the field to `Step::Cmd`:

```rust
    /// Invoke the binary. `--catalog <path>` is prepended by the runner.
    Cmd {
        label: &'static str,
        args: Vec<String>,
        expect: Expect,
        backend: Backend,
    },
```

Set it in the existing constructors and add the new one:

```rust
pub fn cmd(label: &'static str, args: &[&str]) -> Step {
    Step::Cmd {
        label,
        args: args.iter().map(|a| a.to_string()).collect(),
        expect: Expect::Full,
        backend: Backend::Simulated,
    }
}

pub fn cmd_exit_only(label: &'static str, args: &[&str]) -> Step {
    Step::Cmd {
        label,
        args: args.iter().map(|a| a.to_string()).collect(),
        expect: Expect::ExitOnly,
        backend: Backend::Simulated,
    }
}

/// A step run against the real burner/verifier on `device`.
pub fn cmd_real(label: &'static str, device: &'static str, args: &[&str]) -> Step {
    Step::Cmd {
        label,
        args: args.iter().map(|a| a.to_string()).collect(),
        expect: Expect::Full,
        backend: Backend::Real { device },
    }
}
```

And derive the tool requirement from the steps rather than declaring it, so it
cannot be forgotten:

```rust
impl Scenario {
    /// External tools this scenario needs. A real-backend step preflights
    /// `xorriso`, so a machine without it must skip rather than compare two
    /// different "cannot launch" messages — the OS error text is rendered by
    /// the JVM in one binary and by Rust's `io::Error` in the other, and
    /// that difference is uninteresting.
    pub fn required_tools(&self) -> Vec<&'static str> {
        let real = self.steps.iter().any(|s| {
            matches!(
                s,
                Step::Cmd {
                    backend: Backend::Real { .. },
                    ..
                }
            )
        });
        if real {
            vec!["xorriso"]
        } else {
            Vec::new()
        }
    }
}
```

Add a unit test in that file's `mod tests`:

```rust
    #[test]
    fn only_a_real_backend_step_requires_a_tool() {
        assert!(sample().required_tools().is_empty());
        let with_real = Scenario {
            name: "r",
            disc_bytes: None,
            steps: vec![cmd_real("burn", "/nope", &["burn", "p", "--yes"])],
        };
        assert_eq!(with_real.required_tools(), vec!["xorriso"]);
        // A real step is still a `burn` for the RUST_IMPLEMENTED gate.
        assert_eq!(
            with_real.commands().into_iter().collect::<Vec<_>>(),
            vec!["burn"]
        );
    }
```

- [ ] **Step 2: Apply it in `runner.rs`**

Extend the `Step::Cmd` destructuring to bind `backend`, and replace the two
fixed burner lines in the env block:

```rust
                cmd.args(&argv)
                    .current_dir(&scratch.root)
                    .env("HOME", &scratch.home)
                    .env("XDG_DATA_HOME", scratch.home.join(".local/share"))
                    .env("STATE_DIRECTORY", scratch.home.join("state"))
                    .env_remove("DISKREMENT_HASH_PARALLELISM");
                match backend {
                    Backend::Simulated => {
                        cmd.env("DISKREMENT_BURNER", "simulated")
                            .env_remove("DISKREMENT_DEVICE");
                    }
                    Backend::Real { device } => {
                        cmd.env("DISKREMENT_BURNER", "real").env(
                            "DISKREMENT_DEVICE",
                            device.replace("{source}", &source).replace("{work}", &work),
                        );
                    }
                }
```

Add the tool probe and the skip, next to the `rust_gap` check at the top of
`run`:

```rust
/// Is `tool` on this machine's PATH?
fn tool_present(tool: &str) -> bool {
    std::process::Command::new(tool)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
```

```rust
    for tool in scenario.required_tools() {
        if !tool_present(tool) {
            return Err(Skip(format!(
                "scenario '{}' needs {tool}, which is not installed here. \
                 Install it and re-run; until then this scenario is not compared.",
                scenario.name
            )));
        }
    }
```

Import `Backend` alongside the other scenario types at the top of `runner.rs`.

- [ ] **Step 3: Add the scenario to `corpus.rs`**

```rust
/// The real backend, as far as a machine with no drive can be driven.
///
/// A runner-spawned child has no stdin, so every operator prompt answers
/// itself: the burn stops before opening the device and the verify skips the
/// disc. What that pins across both binaries is the *wording* of the two
/// prompts, the `skipped (not checked)` line, the exit codes (an aborted burn
/// is 0, a skipped verify is 0), and — through the end-state diff — that the
/// aborted burn committed nothing while the simulated one that follows
/// committed everything.
///
/// The device is a path inside the scratch tree that is never created. The
/// real backend prompts before it opens anything, so nothing touches it; and
/// keeping it under the scratch root means the normalizer redacts it out of
/// the compared prompt.
pub fn real_backend() -> Scenario {
    Scenario {
        name: "real-backend",
        disc_bytes: None,
        steps: vec![
            write("a.bin", 400),
            cmd(
                "init p",
                &["init", "p", "--source", "{source}", "--work-dir", "{work}"],
            ),
            cmd("scan p", &["scan", "p"]),
            cmd("plan p", &["plan", "p"]),
            cmd_real(
                "burn with the real backend and no disc",
                "{work}/no-such-drive",
                &["burn", "p", "--yes"],
            ),
            // Commit-after-success: the aborted burn left the catalog alone.
            cmd("status after the aborted burn", &["status", "p"]),
            cmd("burn p", &["burn", "p", "--yes"]),
            cmd_real(
                "verify with the real backend",
                "{work}/no-such-drive",
                &["verify", "p"],
            ),
            // The skipped disc was not marked verified, so the simulated
            // verify still has work to do.
            cmd("verify p", &["verify", "p"]),
        ],
    }
}
```

Register it in `all()`, after `harvest_replay()`, and import `cmd_real`.

- [ ] **Step 4: Document it**

In `crates/diskrement-difftest/README.md`, under "Extending it", add:

```markdown
- **A step that needs the real backend?** Use `scenario::cmd_real`, pointing it
  at a path that does not exist. The real burner and verifier both ask the
  operator before opening the device, and a runner-spawned child has no stdin,
  so the prompt is the whole comparison and no hardware is touched. A scenario
  with such a step requires `xorriso` and skips — with the reason printed —
  where it is not installed.
```

- [ ] **Step 5: Run the harness in all three modes**

```bash
sbt cli/stage
cargo build -p diskrement-cli
DISKREMENT_SCALA_BIN=$PWD/cli/target/universal/stage/bin/diskrement \
  cargo test -p diskrement-difftest -- --ignored --nocapture
```

Expected: bootstrap **13 scenarios reproduced identically**; all-rust **13
compared, 0 skipped**; interleaved **13 compared, 0 skipped**. Read the counts,
not the exit code.

If `real-backend` diverges, **do not add a normalization rule.** The likely
causes, in order: the insert prompt's wording (compare against
`XorrisoBurner.scala`'s `awaitCorrectMedia` character by character), the
`Aborted.` render (Task 11), or a stdout/stderr split — Scala prints the prompt
with `Console.print` (stdout, no newline) and the abort message with
`Console.printLineError` (stderr).

- [ ] **Step 6: Commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/diskrement-difftest
git commit -m "feat(difftest): compare the real backend's prompts without hardware

RUST_IMPLEMENTED is unchanged: P8 lands a backend, not a command."
```

---

### Task 14: Land the phase

**Files:**
- Modify: `docs/smoke-test-real-media.md`
- Modify: `CLAUDE.md`
- Modify: `docs/superpowers/plans/2026-09-07-rust-rewrite-p8-real-burner.md`

- [ ] **Step 1: Point the manual checklist at both binaries**

`docs/smoke-test-real-media.md` is the only test of the paths no automated
suite can reach (a real drive, real media, `udisksctl`). Its items are
binary-agnostic; what changes is that there are now two binaries to run them
with. Add, immediately under the title block:

```markdown
**Since the Rust rewrite's P8** this checklist applies to both implementations.
Run it with the Rust binary (`cargo run -p diskrement-cli -- <args>`, or the
built `target/debug/diskrement`); the Scala launcher (`sbt cli/stage`, then
`cli/target/universal/stage/bin/diskrement`) remains available until P10 for
comparing behavior on any item that surprises you. Item 5 (Tier-2 verification)
and item 2 (udisks mount) are the two the automated image-backed suite cannot
reach at all — `tests/xorriso_image.rs` covers osirrox extraction, but never a
mount.
```

- [ ] **Step 2: Record the phase's gotchas in `CLAUDE.md`**

Under "Rust rewrite gotchas (P2+)", append what this phase learned. Write only
what was actually observed while implementing — the list below is what this plan
predicts, and anything that turned out differently belongs here instead:

```markdown
- **`std::process` has no `redirectErrorStream`.** `burn::cmd::system` reads
  stdout on the calling thread and stderr on a second one into a shared buffer.
  Only stdout lines reach the `on_line` callback, which is correct — xorriso's
  `UPDATE` progress is on stdout — but it means a stderr-only tool would report
  no progress.
- **xorriso extracts over an existing file** (validated 2026-09-07, 1.5.8.pl02),
  but every extraction in this tree still targets a fresh path inside a
  `TempDir`, so nothing depends on that.
- **`-toc -tell_media_space` prints `Media status` twice**, once per
  sub-command. The parser takes the first match; do not "fix" it into an
  exactly-once assertion.
- **`stdio:` image files are not real media.** `-close on` has no observable
  effect on them and free space is a fake huge number, so sealing and capacity
  refusals are assertable only on real media (`docs/smoke-test-real-media.md`).
- **The real backend prompts before it opens the device.** That is what lets
  `tests/binary.rs` and the differential harness exercise it with no drive: a
  child with no stdin declines, and the burn aborts (exit 0) before anything is
  touched.
```

- [ ] **Step 3: Run the full verification**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

All clean. Record the counts; they must exceed the 443 passed / 3 ignored
baseline. Then confirm the two skippable suites actually ran:

```bash
cargo test -p diskrement-shell --test xorriso_image -- --nocapture 2>&1 | grep -c SKIP
cargo test -p diskrement-cli --test binary -- --nocapture 2>&1 | grep -c SKIP
```

Expected: `0` from both. A nonzero count means this machine could not run the
phase's exit-bar tests, and the phase is not done.

- [ ] **Step 4: Confirm the corpus is untouched**

```bash
git status --porcelain fixtures/
cd fixtures && sha256sum -c SHA256SUMS --quiet; cd ..
```

Expected: no output from either. (`SHA256SUMS` must be checked from inside
`fixtures/`; its paths are relative to that directory.)

- [ ] **Step 5: Run the differential suite once more, in all three modes**

```bash
sbt cli/stage
cargo build -p diskrement-cli
DISKREMENT_SCALA_BIN=$PWD/cli/target/universal/stage/bin/diskrement \
  cargo test -p diskrement-difftest -- --ignored --nocapture
```

Every mode must report **13 scenarios, 0 skipped**.

- [ ] **Step 6: Confirm the Scala tree still builds**

```bash
sbt clean test
```

P8 touches nothing under `core/`, `shell/`, `cli/`, but the branch must leave
the Scala build green until P10. Expected: 230 passed, 0 failed.

- [ ] **Step 7: Append the execution deviations**

Fill in the `## Execution deviations` and `## Execution verification` sections
at the end of this document, recording every place the implementation diverged
from what is written above and why. Per CLAUDE.md, deviations live in the plan
doc, not in commit messages. Carry forward anything P9 and P10 will need — in
particular: whether the real-backend differential scenario proved stable across
runs, and which real-media checklist items are still unrun.

- [ ] **Step 8: Merge and clean up**

```bash
git add docs/superpowers/plans/2026-09-07-rust-rewrite-p8-real-burner.md docs/smoke-test-real-media.md CLAUDE.md
git commit -m "docs(plan): record the P8 execution deviations"
git switch feat/rust-rewrite
git merge --no-ff feat/rewrite-p8 -m "Merge feat/rewrite-p8: the real xorriso backend — the media inspector, the disc reader, the two-tier read-back verifier and the burner's media wait, proven end to end against real xorriso through stdio: images, with the operator prompts compared against the Scala binary in all three differential modes"
git branch -d feat/rewrite-p8
```

---

## Phase exit bar (spec §9, P8 row)

- [ ] `cargo test --workspace` green, above the 443/3 baseline
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean
- [ ] `cargo fmt --check` clean
- [ ] **The image IT is green where `xorriso` is installed** — five tests in
      `crates/diskrement-shell/tests/xorriso_image.rs`, **0 SKIP lines**, run
      with `--nocapture` and the count read (Task 10)
- [ ] **Burner environment semantics pinned** — `--burner real` and
      `DISKREMENT_BURNER=real` reach the xorriso burner and the disc verifier,
      the flag beats the environment, an unknown environment value falls back,
      and an empty `DISKREMENT_DEVICE` is ignored (P5's `backend::resolve`
      tests, now with both arms live: Tasks 11, 12, 13)
- [ ] A burn onto the wrong disc is refused, re-prompted and never written
      (Tasks 8, 10)
- [ ] A declined insert prompt is `Aborted` → **exit 0**, renders `Aborted.`,
      and commits nothing (Tasks 8, 11, 12, 13)
- [ ] Every other media/burner failure is **exit 6** with its Scala message
      verbatim (Task 2)
- [ ] Two-tier verification: the merged view and the superseded versions, with
      an absent expected file counted as a mismatch and a declined disc skipped
      rather than reported clean (Tasks 9, 10)
- [ ] The differential suite passes in all three modes with **0 skipped**,
      including the new `real-backend` scenario (Task 13)
- [ ] `fixtures/` unchanged and `SHA256SUMS` still verifies
- [ ] `sbt clean test` still green
- [ ] The real-media checklist is updated to cover both binaries; **running it
      on hardware is a P10 bar item**, not P8's (spec §11)

## Spec coverage

| Spec requirement | Task |
|---|---|
| §1 feature parity: both burner backends, the real xorriso path | 8, 11 |
| §1 deferred debt ported as-is (`writeDisc` does not refuse a dead disc; `archivePointers` has no dead flag) | none — deliberately untouched, stated in Global Constraints |
| §3.1 `/.diskrement/project.json` as the disc's identity; timestamps only through `catalog::time` | 5 |
| §3.2 `DISKREMENT_DEVICE` / `DISKREMENT_BURNER`; exit 6 for media/burner; an aborted burn exits 0 | 2, 11, 12 |
| §3.2 output wording is contract outside clap's argument errors (the `Display` shapes, D3) | 5, 8, 9 |
| §5.2 a port becomes a pure function plus a thin edge; the two closed backend enums | 1, 3, 4, 8, 9 |
| §5.2 `CmdRunner` disappears as a seam | partially — see D1, a one-method `Exec` survives with its reason |
| §5.3 the `burn/{cmd,media,reader}.rs` and `burn/xorriso/{args,parse,burner}.rs` layout | 1, 3, 4, 5, 6, 8 |
| §5.4 the three external commands (`xorriso`, `udisksctl`, `mount`) | 1, 6 |
| §5.4 dependency list | D2 — `tempfile` moves to runtime, recorded in the manifest |
| §7 a skip states its reason and the run says how much actually ran | 10, 12, 13 |
| §8.1 the phase's differential task; a normalization rule is a hole in the oracle | 13 |
| §9 P8 scope and exit bar (image IT green; burner env semantics pinned) | 10, 11, 12, 13 |
| §11 the hardware-dependent phase; the real-media run is a P10 bar item | 14 |
| CLAUDE.md: commit after success — every new failure mode leaves the catalog untouched | 8, 12, 13 |
| CLAUDE.md: `Pipeline` returns data and does no I/O | 9 (the only pipeline edit is one lifetime) |

## Execution deviations

*(recorded 2026-09-07, Task 14, on `feat/rewrite-p8` at `9c65899`)*

1. **Task 1 (burn::cmd) — stderr reaches `on_line` after all.** The plan's test
   and its implementation note contradicted each other: the note predicted only
   stdout lines reach the callback, but the test — written to the Scala oracle's
   behavior (`redirectErrorStream(true)` + `onLine`, where merged lines all
   reach the callback) — demanded more. The test won: `burn::cmd::system`
   streams stdout live on the calling thread, buffers stderr on a second
   thread, and replays the buffered stderr lines through `on_line` after both
   streams drain. Buffer order approximates the interleaving, which is enough
   for `tail` and `classify_failure`. The CLAUDE.md gotcha bullet records the
   implemented behavior, not the plan's predicted text. Any consumer of the old
   "only stdout" note must re-read `burn::cmd`.
2. **Task 3 (xorriso::args) — the flagged `PathBuf` import gotcha
   materialized.** Exactly as the plan's own warning anticipated; the import
   moved into `mod tests` per the plan's prescription.
3. **Task 8 (xorriso::burner) — plan slip on the expected count.** Step 4 said
   "expect 11 passed" but the step's mandated test module defines 12 tests;
   12 is correct and all 12 pass.
4. **Task 9 (disc verifier) — one lifetime beyond the claimed edit.**
   `Verifier<'a>` forced a mechanical `Verifier<'_>` fix (E0106) in
   `diskrement-cli/src/main.rs` `resolve_verifier`, beyond the plan's "the only
   pipeline edit is one lifetime" claim. Inert: the function is rewritten by
   Task 11 anyway.
5. **Task 12 (binary tests) — fixture helpers promoted to file-top scope.**
   The brief's no-duplicate-fixtures rule won over inline setup: `cmd`,
   `Project`, `project`, `base`, `init_scan_plan` (from p5) and `Fixture`,
   `burned_project` (from p6) are hoisted to file-top scope in `binary.rs`
   rather than duplicated.
6. **Formatting-only reflows.** Rustfmt reflowed several brief-verbatim code
   blocks (whitespace only); the plan's own `cargo fmt --check` gate requires
   it. Token- and behavior-identical, per P4/P6 precedent.

**Carry-forward for P9 and P10:**

- **The real-media checklist has not been run on hardware.** No item has been
  exercised against the Rust binary; items 6 and 8 carry Scala-era captures
  only (2026-08-30, recorded in the doc itself). The hardware run is a P10 bar
  item (spec §11) — P8's Step 1 only re-points the doc at both binaries.
- **The real-backend differential scenario proved stable across every run
  performed:** 13 scenarios in all three modes with 0 skipped, in Task 13's
  runs and again in Task 14's confirmation run, with no normalization rule
  added or widened.

## Execution verification

All commands run on `feat/rewrite-p8` at `9c65899`, before the merge:

- `cargo test --workspace`: **506 passed, 0 failed, 3 ignored** across 20 test
  targets (baseline 443 passed / 3 ignored → +63, no drops; the three ignored
  are the `#[ignore]`d differential entry points).
- `cargo clippy --workspace --all-targets -- -D warnings`: clean (exit 0).
- `cargo fmt --check`: clean (exit 0). Working-directory note: run it from the
  repo root; from `crates/` the virtual workspace manifest yields rustfmt's
  "Failed to find targets" — a target-discovery quirk, not a formatting
  failure.
- Skippable suites actually ran (spec §7):
  `cargo test -p diskrement-shell --test xorriso_image -- --nocapture` →
  **0 SKIP lines** (5 passed, 0 failed);
  `cargo test -p diskrement-cli --test binary -- --nocapture` →
  **0 SKIP lines** (41 passed, 0 failed).
- `fixtures/` untouched: `git status --porcelain fixtures/` empty;
  `sha256sum -c SHA256SUMS` (run inside `fixtures/`) verified every file
  silently (exit 0).
- Differential suite in all three modes (after a fresh `sbt cli/stage` and
  `cargo build -p diskrement-cli`, `DISKREMENT_SCALA_BIN` = the staged
  launcher): bootstrap (all-Scala) **13 scenarios reproduced identically**;
  all-rust **13 compared, 0 skipped**; interleaved **13 compared, 0 skipped**.
  Exit 0; no normalization rule touched.
- `sbt clean test`: green, exit 0, 0 failed. core (munit, sbt's standard
  summary) **51 passed**; shell (zio) **156 passed**; cli (zio) **74 passed** —
  the two zio summaries sum to the expected **230 passed**, 281 including
  core's separately-reported summary. Attribution correction against P7's
  record: 74 is cli's zio summary, not "core (munit)" — munit's count is the
  `Passed: Total 51` line. Same three totals as P7 (51/74/156), corrected
  mapping; no test-count drop.
