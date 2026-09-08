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
use crate::scenario::{Backend, Expect, Mode, Scenario, Step, Which};

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
///
/// `frobnicate` is listed for the error-paths scenario: it is the
/// unknown-subcommand *probe*, which exercises the argument parser (fully
/// implemented, `Expect::ExitOnly`) rather than a command, and would
/// otherwise block that scenario forever.
pub const RUST_IMPLEMENTED: &[&str] = &[
    "burn",
    "disc",
    "frobnicate",
    "history",
    "init",
    "plan",
    "reset",
    "scan",
    "status",
    "verify",
];

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

/// Deterministic mtime base for the files the harness writes:
/// 2020-09-13T12:26:40Z — a whole second, so the millisecond truncation
/// `FileSystem.stat` applies on both sides is an identity over every value
/// derived from it.
const SOURCE_MTIME_BASE_MILLIS: u64 = 1_600_000_000_000;

/// Pin a written file's mtime to `unix_millis`.
///
/// **Why the runner, not the normalizer:** a freshly written file's mtime is
/// whatever the kernel's coarse clock says when the write lands, so whether two
/// back-to-back `write` steps share one (millisecond-truncated) instant is a
/// timing accident. Two runs of the *same* scenario can then disagree on the
/// equality relations the instant-ordinals rule deliberately preserves — the
/// bootstrap caught exactly this on `plan-spill-and-seal`, where `a.bin` and
/// `b.bin` shared one mtime in one run and straddled a tick in the other. The
/// frozen corpus never sees this (harvested once); the live runner therefore
/// makes the environment deterministic instead of erasing the value. Pinned
/// from the write *index*, the same role `(rel_path, version)` plays for
/// bytes: values stay per-file distinct, so a scanner that recorded the wrong
/// file's mtime still diverges, and a `modify` step still gets a later mtime
/// than the write it supersedes, keeping the fingerprint cache's fast path
/// honest.
fn pin_mtime(path: &Path, unix_millis: u64) -> Result<(), Skip> {
    let file = std::fs::File::options()
        .write(true)
        .open(path)
        .map_err(|e| Skip(format!("reopening {}: {e}", path.display())))?;
    let time = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(unix_millis);
    file.set_modified(time)
        .map_err(|e| Skip(format!("pinning mtime on {}: {e}", path.display())))
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
    pub scenario: String,
    pub mode: Mode,
    pub steps: Vec<StepOutcome>,
    pub scratch: Scratch,
}

/// Refuse, with a reason, a run this environment cannot support.
///
/// Separate from `Session` because it is a property of the *scenario* — which
/// commands it needs and which tools — rather than of the scratch tree.
pub fn preflight(bins: &Binaries, scenario: &Scenario, mode: Mode) -> Result<(), Skip> {
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

    for tool in scenario.required_tools() {
        if !tool_present(tool) {
            return Err(Skip(format!(
                "scenario '{}' needs {tool}, which is not installed here. \
                 Install it and re-run; until then this scenario is not compared.",
                scenario.name
            )));
        }
    }
    Ok(())
}

/// One scratch tree, its environment, and the ability to apply steps to it one
/// at a time.
///
/// `run` folds a whole scenario over this; the sweep holds three of them open
/// at once and diffs between steps. The counters live here rather than in the
/// caller because both of them — the write index that pins mtimes and the
/// command index that drives the alternation — must advance exactly once per
/// applicable step, and a caller that forgot one would produce a difference
/// that looks like a porting bug.
pub struct Session<'a> {
    bins: &'a Binaries,
    mode: Mode,
    disc_bytes: Option<u64>,
    scratch: Scratch,
    write_index: u32,
    command_index: usize,
}

impl<'a> Session<'a> {
    /// Create the scratch tree. `Skip` when this machine cannot host one.
    ///
    /// A `mode` that needs the Rust binary requires `preflight` to have passed
    /// first: `invoke` panics rather than skipping when that binary is absent.
    pub fn open(bins: &'a Binaries, mode: Mode, disc_bytes: Option<u64>) -> Result<Self, Skip> {
        Ok(Session {
            bins,
            mode,
            disc_bytes,
            scratch: scratch()?,
            write_index: 0,
            command_index: 0,
        })
    }

    /// Borrow the scratch tree mid-run, for a snapshot taken between commands.
    /// Use `into_scratch` at the end, when the tree must outlive the session.
    pub fn scratch(&self) -> &Scratch {
        &self.scratch
    }

    /// The mode every command step is dispatched under, so a caller reporting a
    /// difference can label it without keeping a second copy.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// How many command steps have been applied. `Mode::which` takes this.
    pub fn command_index(&self) -> usize {
        self.command_index
    }

    /// Give up the scratch tree, keeping it alive in the caller.
    pub fn into_scratch(self) -> Scratch {
        self.scratch
    }

    /// Apply one step. `Ok(None)` for a tree mutation, which runs no binary.
    ///
    /// Advances the command index for command steps only, so a file write
    /// between two commands never shifts the alternation. The binary is chosen
    /// from this session's mode and the count of commands *before* this one.
    pub fn apply(&mut self, step: &Step) -> Result<Option<StepOutcome>, Skip> {
        match step {
            Step::Write {
                rel_path,
                version,
                size,
            } => {
                let path = self.scratch.source.join(rel_path);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| Skip(format!("creating {}: {e}", parent.display())))?;
                }
                std::fs::write(&path, content(rel_path, *version, *size))
                    .map_err(|e| Skip(format!("writing {}: {e}", path.display())))?;
                pin_mtime(
                    &path,
                    SOURCE_MTIME_BASE_MILLIS + u64::from(self.write_index),
                )?;
                self.write_index += 1;
                Ok(None)
            }
            Step::Delete { rel_path } => {
                let path = self.scratch.source.join(rel_path);
                std::fs::remove_file(&path)
                    .map_err(|e| Skip(format!("deleting {}: {e}", path.display())))?;
                Ok(None)
            }
            Step::Cmd {
                label,
                args,
                expect,
                backend,
            } => {
                // Read before the increment: the binary for a command is
                // chosen by the count of commands *before* it, so deriving
                // `which` afterwards would still alternate but start the
                // interleave with Scala — and stop exercising the Rust→Scala
                // direction, the one the frozen corpus cannot reach.
                let which = self.mode.which(self.command_index);
                self.command_index += 1;
                Ok(Some(self.invoke(label, args, *expect, backend, which)?))
            }
        }
    }

    /// Spawn one binary: the placeholder substitution, the environment
    /// isolation, the backend selection and `DISKREMENT_DISC_BYTES`.
    fn invoke(
        &self,
        label: &str,
        args: &[String],
        expect: Expect,
        backend: &Backend,
        which: Which,
    ) -> Result<StepOutcome, Skip> {
        let bin = match which {
            Which::Scala => &self.bins.scala,
            Which::Rust => self
                .bins
                .rust
                .as_ref()
                .expect("preflight refuses a Rust mode without the Rust binary"),
        };
        // Scenarios cannot know the scratch path, so they write the
        // placeholders `{source}` and `{work}` and the runner fills
        // them in. Substituting here keeps a scenario a pure,
        // machine-independent value that two runs can share.
        let source = self.scratch.source.display().to_string();
        let work = self.scratch.work.display().to_string();
        let mut argv: Vec<String> = vec![
            "--catalog".to_string(),
            self.scratch.catalog.display().to_string(),
        ];
        argv.extend(
            args.iter()
                .map(|a| a.replace("{source}", &source).replace("{work}", &work)),
        );

        let mut cmd = Command::new(bin);
        cmd.args(&argv)
            .current_dir(&self.scratch.root)
            .env("HOME", &self.scratch.home)
            .env("XDG_DATA_HOME", self.scratch.home.join(".local/share"))
            .env("STATE_DIRECTORY", self.scratch.home.join("state"))
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
        match self.disc_bytes {
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
        Ok(StepOutcome {
            label: label.to_string(),
            which,
            argv,
            // A signal-killed child has no code; -1 is not a valid exit
            // code, so it can never collide with a real one.
            exit: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            expect,
        })
    }
}

/// Run one scenario end to end.
///
/// Returns `Skip` — never a panic — when the environment cannot support the
/// run, so the caller can report it as a skip with its reason.
pub fn run(bins: &Binaries, scenario: &Scenario, mode: Mode) -> Result<RunOutcome, Skip> {
    preflight(bins, scenario, mode)?;
    let mut session = Session::open(bins, mode, scenario.disc_bytes)?;
    let mut steps = Vec::new();
    for step in &scenario.steps {
        if let Some(outcome) = session.apply(step)? {
            steps.push(outcome);
        }
    }
    Ok(RunOutcome {
        scenario: scenario.name.clone(),
        mode,
        steps,
        scratch: session.into_scratch(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::write;

    #[test]
    fn a_session_counts_only_command_steps() {
        // The sweep asks a session which command index comes next, so that it
        // can choose the binary the way `run` does. Tree mutations must not
        // advance it, or inserting a file write would shift the alternation.
        let bins = Binaries {
            scala: PathBuf::from("/nonexistent/scala"),
            rust: None,
        };
        let mut session = match Session::open(&bins, Mode::Scala, None) {
            Ok(s) => s,
            // No CoW temp dir here; say so rather than passing silently.
            Err(Skip(reason)) => return eprintln!("SKIP: {reason}"),
        };
        assert_eq!(session.command_index(), 0);
        assert!(
            session
                .apply(&write("a.bin", 8))
                .expect("a tree mutation needs no binary")
                .is_none(),
            "a tree mutation produces no step outcome"
        );
        assert_eq!(session.command_index(), 0);
        assert!(session.scratch().source.join("a.bin").is_file());
    }

    #[test]
    fn the_interleaved_mode_runs_the_first_command_through_rust() {
        // Spec §3.1's direction: step 0 must be Rust so that step 1 has Scala
        // reading what Rust wrote — the direction the frozen corpus cannot
        // test. `/bin/true` stands in for both binaries; only `which` is
        // asserted, so nothing here depends on either implementation.
        let bins = Binaries {
            scala: PathBuf::from("/bin/true"),
            rust: Some(PathBuf::from("/bin/true")),
        };
        let scenario = Scenario {
            name: "ordering".into(),
            disc_bytes: None,
            steps: vec![
                crate::scenario::cmd("status p", &["status", "p"]),
                crate::scenario::cmd("status p", &["status", "p"]),
            ],
        };
        match run(&bins, &scenario, Mode::Interleaved) {
            Ok(out) => {
                assert_eq!(out.steps[0].which, Which::Rust);
                assert_eq!(out.steps[1].which, Which::Scala);
            }
            Err(Skip(reason)) => eprintln!("SKIP: {reason}"),
        }
    }

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

    #[test]
    fn pinned_mtimes_are_deterministic_and_per_file_distinct() {
        let dir = TempDir::new().expect("temp dir");
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        std::fs::write(&a, b"x").expect("write a");
        std::fs::write(&b, b"y").expect("write b");
        pin_mtime(&a, SOURCE_MTIME_BASE_MILLIS).expect("pin a");
        pin_mtime(&b, SOURCE_MTIME_BASE_MILLIS + 1).expect("pin b");
        // Re-pinning the same value must not drift: two runs pinning the same
        // scenario land byte-identical instants.
        pin_mtime(&a, SOURCE_MTIME_BASE_MILLIS).expect("re-pin a");
        let mtime_millis = |p: &Path| {
            std::fs::metadata(p)
                .expect("metadata")
                .modified()
                .expect("mtime")
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .expect("after the epoch")
                .as_millis() as u64
        };
        assert_eq!(mtime_millis(&a), SOURCE_MTIME_BASE_MILLIS);
        assert_eq!(mtime_millis(&b), SOURCE_MTIME_BASE_MILLIS + 1);
    }
}
