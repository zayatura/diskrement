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
pub const RUST_IMPLEMENTED: &[&str] = &["frobnicate", "init", "plan", "scan", "status"];

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
    let mut write_index: u32 = 0;

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
                pin_mtime(&path, SOURCE_MTIME_BASE_MILLIS + u64::from(write_index))?;
                write_index += 1;
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
