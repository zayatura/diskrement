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
/// to `size` — the recipe of `scripts/harvest-fixtures.sh`, which lived in
/// this repository until P10 and now exists only on the `scala` branch, and
/// which the P0 corpus was harvested with. `Tree.scala` used a Java `Random`
/// seeded from `String.hashCode`, which Rust cannot reproduce and nothing
/// needs it to: the property that matters is determinism plus
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
