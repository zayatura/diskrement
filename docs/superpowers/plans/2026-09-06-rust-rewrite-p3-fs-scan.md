# Rust Rewrite P3 — fs, scanner, cache, snapshot, init/scan/status Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land P3's second half — the `fs` subsystem (hasher, hash-parallelism,
walk/stat, scanner, fingerprint cache, CoW snapshot), the staged scan, the
pipeline's `init`/`scan`/`status`, and the first real CLI — ending with a
runnable Rust binary that the differential harness exercises in all three modes.

**Architecture:** Spec §5.3/§5.5. Plain structs with inherent methods, no ports
or layers: a port becomes a pure function plus a thin edge. The scanner takes
its hasher as a closure parameter (the one seam the Scala version injected that
Rust replaces with a function argument). `reflink_copy::reflink` clones single
files only on Linux, so the snapshot walks the tree and reflinks file-by-file —
still no external `cp`.

**Tech Stack:** Rust 2021; `sha2`, `walkdir`, `rayon`, `reflink-copy`,
`rusqlite` (bundled), `serde_json`, `chrono`, `clap` 4 derive — all already
workspace dependencies. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-09-05-rust-rewrite-design.md` — §3 (CLI
compatibility), §5.3 (layout), §5.5 (fs/pipeline/errors/cli subsystems), §9
(P3's exit bar). Scala sources are the behavioral oracle: read the file named in
each task before porting it.

---

## Global Constraints

- **Branch:** continue on `feat/rewrite-p3` (the differential-harness plan
  already landed there). Never commit directly to `feat/rust-rewrite`.
- **Do not touch** `core/`, `shell/`, `cli/`, `build.sbt`, `project/`,
  `fixtures/`, `scripts/harvest-fixtures.sh`.
- **After every task:** `cargo test --workspace`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo fmt --check` — all clean, no drop in test count. Baseline at plan
  start: **138 passed** (113 pre-harness + 25 difftest unit tests).
- **Commit scopes:** `feat(fs):`, `test(fs):`, `feat(pipeline):`, `feat(cli):`,
  `feat(difftest):`, `docs(cli):`.
- **Time formatting only through `diskrement_shell::catalog::time`**
  (`format`/`parse`). Never `chrono`'s serde or `Display` for stored values.
- **User-visible strings and exit codes are contract** (§3.2): copy them
  verbatim from the Scala sources named in each task.
- **Appending `init`, `scan`, `status` to `runner::RUST_IMPLEMENTED` happens in
  the same commit that lands them in the binary** (Task 15).
- mtime normalization to **millisecond truncation** happens exactly once, in
  the scanner's stat; every consumer sees truncated values.
- The fingerprint cache is **mutable and disposable** (upserts,
  wipe-and-recreate on corruption) — deliberately unlike the append-only
  catalog.
- **Scan writes no catalog rows.** Snapshot → hash → stage → cache; the diff is
  computed against the catalog view on the fly.

## Scala oracle files (read before porting each area)

| area | Scala source | Scala spec |
|---|---|---|
| hasher | `shell/src/main/scala/diskrement/shell/fs/Hasher.scala` | `HasherSpec.scala` |
| parallelism | `.../fs/HashParallelism.scala` | `HashParallelismSpec.scala` |
| walk/stat + scanner | `.../fs/{FileSystem,FileScanner}.scala` | `FileSystemSpec.scala`, `FileScannerSpec.scala` |
| fingerprint cache | `.../fs/FingerprintCache.scala` | `FingerprintCacheSpec.scala` |
| snapshot | `.../fs/{SourceSnapshot,Reflink}.scala` | `SourceSnapshotSpec.scala`, `CheckCowCapableSpec.scala` |
| staged scan | `.../pipeline/StagedStore.scala` | `StagedStoreSpec.scala` |
| pipeline | `.../pipeline/{Pipeline,PipelineLive,PipelineError}.scala` | `PipelineInitStatusHistorySpec.scala`, `PipelineRescanSpec.scala`, `PipelineScanResetSpec.scala` |
| paths | `cli/src/main/scala/diskrement/cli/PathResolver.scala` | `PathResolverSpec.scala` |
| args | `cli/src/main/scala/diskrement/cli/{Args,Commands}.scala` | `CommandsSpec.scala` |
| render/exit | `cli/src/main/scala/diskrement/cli/Render.scala` | `RenderSpec.scala`, `ExitMappingSpec.scala` |
| progress | `cli/.../ConsoleProgress.scala`, `shell/.../ProgressSink.scala` | `ConsoleProgressSpec.scala`, `ProgressSinkSpec.scala` |

All Scala paths above are relative to `shell/src/main/scala/diskrement/shell/`
unless they start with `cli/`.

## File Structure

| file | responsibility |
|---|---|
| `crates/diskrement-shell/src/lib.rs` (modify) | declare `progress`, `fs`, `pipeline` modules |
| `crates/diskrement-shell/src/progress.rs` (create) | the `ProgressSink` trait, `NoopSink`, the `phase` helper |
| `crates/diskrement-shell/src/fs/mod.rs` (create) | module wiring |
| `crates/diskrement-shell/src/fs/hasher.rs` (create) | SHA-256 streaming hash |
| `crates/diskrement-shell/src/fs/scanner.rs` (create) | parallelism resolution, walk, stat, fast path, scan |
| `crates/diskrement-shell/src/fs/fingerprint_cache.rs` (create) | the sidecar SQLite cache |
| `crates/diskrement-shell/src/fs/snapshot.rs` (create) | CoW probe, reflink tree, create/release |
| `crates/diskrement-shell/src/error.rs` (modify) | add `PipelineError`: contract messages + exit codes |
| `crates/diskrement-shell/src/pipeline/mod.rs` (create) | `Pipeline`, `PipelinePaths` |
| `crates/diskrement-shell/src/pipeline/staged.rs` (create) | `staged/scan.json` read/write |
| `crates/diskrement-shell/src/pipeline/reports.rs` (create) | `ScanReport`, `StatusReport` (data only) |
| `crates/diskrement-cli/src/paths.rs` (create) | catalog/data-dir precedence, sidecar naming |
| `crates/diskrement-cli/src/args.rs` (create) | clap surface |
| `crates/diskrement-cli/src/render.rs` (create) | exact output strings, `gb` |
| `crates/diskrement-cli/src/progress.rs` (create) | stderr progress reporter |
| `crates/diskrement-cli/src/main.rs` (modify) | dispatch, error → stderr + exit code |

---

### Task 1: The hasher

**Files:**
- Create: `crates/diskrement-shell/src/fs/mod.rs`, `crates/diskrement-shell/src/fs/hasher.rs`
- Modify: `crates/diskrement-shell/src/lib.rs` (add `pub mod fs;`)

**Interfaces:**
- Produces: `pub fn hash(path: &Path) -> std::io::Result<ContentHash>` in
  `diskrement_shell::fs::hasher`. Task 5 passes it into `scan`; Task 9 passes
  it into the pipeline.

- [ ] **Step 1: Declare the module**

Append to `crates/diskrement-shell/src/lib.rs`:

```rust
pub mod fs;
```

Create `crates/diskrement-shell/src/fs/mod.rs`:

```rust
//! The filesystem subsystem: hashing, scanning, the fingerprint cache, and the
//! CoW source snapshot (spec §5.3).

pub mod hasher;
```

- [ ] **Step 2: Write the failing tests**

`crates/diskrement-shell/src/fs/hasher.rs`, test module only:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_an_empty_file_to_the_sha256_of_empty() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("empty");
        std::fs::write(&p, b"").unwrap();
        assert_eq!(
            hash(&p).unwrap(),
            ContentHash("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into())
        );
    }

    #[test]
    fn hashes_abc() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("abc");
        std::fs::write(&p, b"abc").unwrap();
        assert_eq!(
            hash(&p).unwrap(),
            ContentHash("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into())
        );
    }

    #[test]
    fn chunked_digest_matches_a_one_shot_digest_for_a_large_input() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("big");
        let data: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&p, &data).unwrap();
        use sha2::Digest;
        let mut one_shot = sha2::Sha256::new();
        one_shot.update(&data);
        let expected = ContentHash(format!("{:x}", one_shot.finalize()));
        assert_eq!(hash(&p).unwrap(), expected);
    }

    #[test]
    fn fails_on_a_nonexistent_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let err = hash(&dir.path().join("nope")).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }
}
```

- [ ] **Step 3: Run and watch it fail**

Run: `cargo test -p diskrement-shell hasher`
Expected: FAIL — `cannot find function 'hash' in this scope`.

- [ ] **Step 4: Implement**

Prepend to `crates/diskrement-shell/src/fs/hasher.rs` (port of `Hasher.scala`:
64 KiB buffer, read loop, lowercase hex):

```rust
//! SHA-256 file hashing (`Hasher.scala`).

use std::io::Read;
use std::path::Path;

use diskrement_core::ContentHash;
use sha2::Digest;

/// Stream `path` in 64 KiB chunks and return the lowercase-hex SHA-256.
pub fn hash(path: &Path) -> std::io::Result<ContentHash> {
    let mut file = std::fs::File::open(path)?;
    let mut digest = sha2::Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        digest.update(&buf[..n]);
    }
    Ok(ContentHash(format!("{:x}", digest.finalize())))
}

#[cfg(test)]
mod tests {
```

- [ ] **Step 5: Run and watch it pass, then commit**

Run: `cargo test -p diskrement-shell hasher` — 4 passed. Then the workspace
trio (test/clippy/fmt) clean.

```bash
git add crates/diskrement-shell/src/lib.rs crates/diskrement-shell/src/fs
git commit -m "feat(fs): the sha-256 hasher"
```

---

### Task 2: Hash-parallelism resolution

**Files:**
- Create: `crates/diskrement-shell/src/fs/scanner.rs`
- Modify: `crates/diskrement-shell/src/fs/mod.rs` (add `pub mod scanner;`)
- Oracle: `HashParallelism.scala`, `HashParallelismSpec.scala`

**Interfaces:**
- Produces: `pub struct Parallelism { pub value: usize, pub warnings: Vec<String> }`,
  `pub const DEFAULT_PARALLELISM: usize = 6`,
  `pub fn resolve_parallelism(get_env: impl Fn(&str) -> Option<String>) -> Parallelism`,
  `pub fn parallelism_from_env() -> Parallelism`.
  Tasks 5 and 9 consume `parallelism_from_env()`; Task 9 prints `.warnings` to
  stderr.

- [ ] **Step 1: Write the failing tests**

`crates/diskrement-shell/src/fs/scanner.rs`, test module only; add
`pub mod scanner;` to `fs/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + '_ {
        move |k: &str| {
            vars.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn defaults_to_6_when_the_env_var_is_unset() {
        assert_eq!(resolve_parallelism(env(&[])).value, 6);
    }

    #[test]
    fn reads_a_positive_value() {
        let r = resolve_parallelism(env(&[("DISKREMENT_HASH_PARALLELISM", "3")]));
        assert_eq!(r.value, 3);
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn ignores_non_numeric_and_non_positive_values_with_a_warning() {
        for raw in ["banana", "0", "-1"] {
            let r = resolve_parallelism(env(&[("DISKREMENT_HASH_PARALLELISM", raw)]));
            assert_eq!(r.value, 6, "raw={raw:?}");
            assert!(!r.warnings.is_empty(), "raw={raw:?} must warn");
        }
        // present-but-empty is *unset*, like whitespace-only: no warning
        let r = resolve_parallelism(env(&[("DISKREMENT_HASH_PARALLELISM", "")]));
        assert_eq!(r.value, 6);
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn trims_whitespace_and_clamps_huge_values() {
        assert_eq!(
            resolve_parallelism(env(&[("DISKREMENT_HASH_PARALLELISM", " 3")])).value,
            3
        );
        let clamped = resolve_parallelism(env(&[("DISKREMENT_HASH_PARALLELISM", "100000")]));
        assert_eq!(clamped.value, 64);
        assert_eq!(
            clamped.warnings[0],
            "clamping DISKREMENT_HASH_PARALLELISM 100000 to 64"
        );
        // whitespace-only is treated as unset, with no warning
        let ws = resolve_parallelism(env(&[("DISKREMENT_HASH_PARALLELISM", "  ")]));
        assert_eq!(ws.value, 6);
        assert!(ws.warnings.is_empty());
        // the unusable warning carries the raw, untrimmed value verbatim
        let bad = resolve_parallelism(env(&[("DISKREMENT_HASH_PARALLELISM", "banana")]));
        assert_eq!(
            bad.warnings[0],
            "ignoring unusable DISKREMENT_HASH_PARALLELISM 'banana' (want a positive int); using 6"
        );
    }
}
```

- [ ] **Step 2: Run and watch it fail**

Run: `cargo test -p diskrement-shell scanner`
Expected: FAIL — `cannot find function 'resolve_parallelism'`.

- [ ] **Step 3: Implement**

Prepend to `scanner.rs`:

```rust
//! The parallel scanner (`FileScanner.scala`, `HashParallelism.scala`,
//! `FileSystem.scala`).

/// `DISKREMENT_HASH_PARALLELISM` resolution: default 6, clamped to 64,
/// empty/whitespace-only treated as unset, unusable values fall back to 6 with
/// a warning carrying the raw value (`HashParallelism.scala`).
pub struct Parallelism {
    pub value: usize,
    pub warnings: Vec<String>,
}

pub const DEFAULT_PARALLELISM: usize = 6;
const MAX_PARALLELISM: usize = 64;

pub fn parallelism_from_env() -> Parallelism {
    resolve_parallelism(|k| std::env::var(k).ok())
}

pub fn resolve_parallelism(get_env: impl Fn(&str) -> Option<String>) -> Parallelism {
    let Some(raw) = get_env("DISKREMENT_HASH_PARALLELISM") else {
        return Parallelism { value: DEFAULT_PARALLELISM, warnings: Vec::new() };
    };
    if raw.trim().is_empty() {
        return Parallelism { value: DEFAULT_PARALLELISM, warnings: Vec::new() };
    }
    match raw.trim().parse::<usize>() {
        Ok(n) if n > 0 && n <= MAX_PARALLELISM => {
            Parallelism { value: n, warnings: Vec::new() }
        }
        // Zero parses fine but is unusable, like the Scala `filter(_ > 0)`.
        Ok(0) | Err(_) => Parallelism {
            value: DEFAULT_PARALLELISM,
            warnings: vec![format!(
                "ignoring unusable DISKREMENT_HASH_PARALLELISM '{raw}' (want a positive int); using {DEFAULT_PARALLELISM}"
            )],
        },
        Ok(n) => Parallelism {
            value: MAX_PARALLELISM,
            warnings: vec![format!("clamping DISKREMENT_HASH_PARALLELISM {n} to {MAX_PARALLELISM}")],
        },
    }
}

#[cfg(test)]
mod tests {
```

- [ ] **Step 4: Pass, trio, commit**

Run: `cargo test -p diskrement-shell scanner` — 4 passed. Trio clean.

```bash
git add crates/diskrement-shell/src/fs
git commit -m "feat(fs): hash-parallelism resolution with contract warnings"
```

---

### Task 3: The progress sink port

**Files:**
- Create: `crates/diskrement-shell/src/progress.rs`
- Modify: `crates/diskrement-shell/src/lib.rs` (add `pub mod progress;`)
- Oracle: `ProgressSink.scala`, `ProgressSinkSpec.scala`

**Interfaces:**
- Produces:
  `pub trait ProgressSink: Send + Sync { fn start(&self, label: &str, total_bytes: u64, total_files: usize); fn advance(&self, delta_bytes: u64, delta_files: usize); fn done(&self); }`,
  `pub struct NoopSink;`, and
  `pub fn phase<T>(sink: &dyn ProgressSink, label: &str, total_bytes: u64, total_files: usize, work: impl FnOnce() -> T) -> T`.
  Task 5 consumes the trait; Task 9 stores `&'a dyn ProgressSink` in the
  pipeline; Task 13 implements the trait for the CLI reporter.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Recording(Mutex<Vec<String>>);
    impl Recording {
        fn events(&self) -> Vec<String> {
            self.0.lock().unwrap().clone()
        }
    }
    impl ProgressSink for Recording {
        fn start(&self, label: &str, total_bytes: u64, total_files: usize) {
            self.0
                .lock()
                .unwrap()
                .push(format!("start:{label}:{total_bytes}:{total_files}"));
        }
        fn advance(&self, delta_bytes: u64, delta_files: usize) {
            self.0
                .lock()
                .unwrap()
                .push(format!("advance:{delta_bytes}:{delta_files}"));
        }
        fn done(&self) {
            self.0.lock().unwrap().push("done".to_string());
        }
    }

    #[test]
    fn phase_reports_start_then_done_on_success() {
        let sink = Recording::default();
        let out = phase(&sink, "hashing", 100, 2, || {
            sink.advance(60, 1);
            "ok"
        });
        assert_eq!(out, "ok");
        assert_eq!(sink.events(), vec!["start:hashing:100:2", "advance:60:1", "done"]);
    }

    #[test]
    fn phase_reports_done_even_when_the_work_fails() {
        let sink = Recording::default();
        let out: Result<(), &str> = phase(&sink, "hashing", 10, 1, || Err("boom"));
        assert!(out.is_err());
        assert_eq!(sink.events(), vec!["start:hashing:10:1", "done"]);
    }
}
```

- [ ] **Step 2: Run and watch it fail**

Run: `cargo test -p diskrement-shell progress`
Expected: FAIL — `cannot find type 'ProgressSink'`.

- [ ] **Step 3: Implement**

```rust
//! The progress-reporting port (`ProgressSink.scala`). The pipeline reports
//! through this trait and stays free of console I/O; the CLI provides the
//! stderr renderer (cli/src/progress.rs) and tests use `NoopSink` or a
//! recording sink.

/// Implementations must be thread-safe and cheap: `advance` is called from
/// parallel hashing workers. `start` opens a phase; one instance may serve
/// several phases in sequence.
pub trait ProgressSink: Send + Sync {
    fn start(&self, label: &str, total_bytes: u64, total_files: usize);
    fn advance(&self, delta_bytes: u64, delta_files: usize);
    fn done(&self);
}

/// The test sink (`ProgressSink.Noop`).
pub struct NoopSink;

impl ProgressSink for NoopSink {
    fn start(&self, _label: &str, _total_bytes: u64, _total_files: usize) {}
    fn advance(&self, _delta_bytes: u64, _delta_files: usize) {}
    fn done(&self) {}
}

/// `start` … work … `done`, with `done` guaranteed on the failure path too
/// (`ProgressSink.phase`'s `ensuring`). Panics are not covered: a panic during
/// a scan is a bug that fails the process either way.
pub fn phase<T>(
    sink: &dyn ProgressSink,
    label: &str,
    total_bytes: u64,
    total_files: usize,
    work: impl FnOnce() -> T,
) -> T {
    sink.start(label, total_bytes, total_files);
    let out = work();
    sink.done();
    out
}
```

- [ ] **Step 4: Pass, trio, commit**

```bash
git add crates/diskrement-shell/src/lib.rs crates/diskrement-shell/src/progress.rs
git commit -m "feat(shell): the progress sink port"
```

---

### Task 4: Walk and stat

**Files:**
- Modify: `crates/diskrement-shell/src/fs/scanner.rs`
- Oracle: `FileSystem.scala`, `FileSystemSpec.scala`

**Interfaces:**
- Produces (crate-visible so this file's tests reach them):
  `pub(crate) fn walk(root: &Path) -> std::io::Result<Vec<PathBuf>>` — regular
  files only, symlinks excluded, dotfiles included; and
  `pub(crate) struct FileStat { pub size_bytes: u64, pub mtime: DateTime<Utc> }`,
  `pub(crate) fn stat(path: &Path) -> std::io::Result<FileStat>` — **mtime
  truncated to milliseconds**, the single normalization point.

- [ ] **Step 1: Write the failing tests** (append to the existing test module)

```rust
    #[test]
    fn walk_returns_regular_files_including_dotfiles_but_not_symlinks_or_dirs() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        std::fs::write(root.join("a.txt"), b"x").unwrap();
        std::fs::write(root.join(".hidden"), b"x").unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/b.txt"), b"x").unwrap();
        #[cfg(unix)]
        let _ = std::os::unix::fs::symlink(root.join("a.txt"), root.join("link.txt"));
        let mut got: Vec<String> = walk(root)
            .unwrap()
            .into_iter()
            .map(|p| p.strip_prefix(root).unwrap().display().to_string())
            .collect();
        got.sort();
        assert_eq!(got, vec![".hidden", "a.txt", "sub/b.txt"]);
    }

    #[test]
    fn stat_truncates_mtime_to_milliseconds() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("f");
        std::fs::write(&p, b"12345").unwrap();
        let st = stat(&p).unwrap();
        assert_eq!(st.size_bytes, 5);
        assert_eq!(
            st.mtime,
            st.mtime - chrono::Duration::nanoseconds(
                (st.mtime.timestamp_subsec_nanos() % 1_000_000) as i64
            )
        );
    }
```

- [ ] **Step 2: Run and watch it fail**

Run: `cargo test -p diskrement-shell scanner`
Expected: FAIL — `cannot find function 'walk'`.

- [ ] **Step 3: Implement** (place above the parallelism code in `scanner.rs`)

```rust
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use diskrement_core::RelPath;

/// Regular files under `root`, symlinks excluded, dotfiles included
/// (`FileSystemLive.walk`). Order is directory order; `scan` sorts at the end.
pub(crate) fn walk(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = entry.map_err(std::io::Error::other)?;
        if entry.file_type().is_file() {
            out.push(entry.into_path());
        }
    }
    Ok(out)
}

/// Size and mtime, the scanner's whole view of a file. **mtime is truncated to
/// milliseconds here** — the single normalization point (`FileSystem.stat`'s
/// doc names it), so every stored or compared mtime is a millis instant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileStat {
    pub size_bytes: u64,
    pub mtime: DateTime<Utc>,
}

pub(crate) fn stat(path: &Path) -> std::io::Result<FileStat> {
    let meta = std::fs::metadata(path)?; // nofollow: stats the link itself
    let ts: DateTime<Utc> = meta.modified()?.into();
    let mtime = ts - chrono::Duration::nanoseconds((ts.timestamp_subsec_nanos() % 1_000_000) as i64);
    Ok(FileStat { size_bytes: meta.len(), mtime })
}

/// `RelPath` of `path` under `root` (`snapshotRoot.relativize`).
pub(crate) fn relativize(root: &Path, path: &Path) -> RelPath {
    RelPath(path.strip_prefix(root).unwrap_or(path).display().to_string())
}
```

- [ ] **Step 4: Pass, trio, commit**

```bash
git add crates/diskrement-shell/src/fs/scanner.rs
git commit -m "feat(fs): walk and stat with millis mtime normalization"
```

---

### Task 5: The scanner

**Files:**
- Modify: `crates/diskrement-shell/src/fs/scanner.rs`
- Oracle: `FileScanner.scala`, `FileScannerSpec.scala`

**Interfaces:**
- Consumes: `hasher::hash` (Task 1), `ProgressSink`/`phase` (Task 3),
  `walk`/`stat`/`relativize` (Task 4), `FileFingerprint`
  (`crate::catalog::model`).
- Produces:
  `pub struct ScanOutput { pub files: Vec<ScannedFile>, pub hashed: usize }`;
  `#[derive(Debug, thiserror::Error)] #[error("failed scanning {rel_path}")] pub struct ScanError { pub rel_path: RelPath, #[source] pub source: std::io::Error }`;
  `pub fn scan(root: &Path, prior: &BTreeMap<RelPath, FileFingerprint>, verify: bool, parallelism: usize, sink: &dyn ProgressSink, hash: &(dyn Fn(&Path) -> std::io::Result<ContentHash> + Sync)) -> Result<ScanOutput, ScanError>`.
  The hasher is a parameter, not a trait: the CLI passes `hasher::hash`
  (Task 9), tests pass fakes.

- [ ] **Step 1: Write the failing tests** (append to the test module)

```rust
    mod scan_tests {
        use super::*;
        use crate::progress::{NoopSink, ProgressSink};
        use diskrement_core::ScannedFile;
        use std::collections::BTreeMap;
        use std::sync::Mutex;

        const HASH: &(dyn Fn(&Path) -> std::io::Result<ContentHash> + Sync) =
            &crate::fs::hasher::hash;

        fn file(root: &Path, rel: &str, bytes: &[u8]) {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, bytes).unwrap();
        }

        /// First scan a directory for real, then rebuild `prior` from its
        /// output — that yields true sizes and millis mtimes without
        /// duplicating the stat logic. `override_hash` plants a sentinel hash
        /// so a fast-path hit is distinguishable from a fresh digest.
        fn prior_from(
            files: &[ScannedFile],
            override_hash: impl Fn(&str) -> Option<String>,
        ) -> BTreeMap<RelPath, FileFingerprint> {
            files
                .iter()
                .map(|f| {
                    let h = override_hash(f.rel_path.as_str())
                        .unwrap_or_else(|| f.content_hash.0.clone());
                    (
                        f.rel_path.clone(),
                        FileFingerprint {
                            size_bytes: f.size_bytes,
                            mtime: f.mtime,
                            content_hash: ContentHash(h),
                        },
                    )
                })
                .collect()
        }

        #[derive(Default)]
        struct Recording(Mutex<Vec<String>>);
        impl Recording {
            fn events(&self) -> Vec<String> {
                self.0.lock().unwrap().clone()
            }
        }
        impl ProgressSink for Recording {
            fn start(&self, label: &str, total_bytes: u64, total_files: usize) {
                self.0
                    .lock()
                    .unwrap()
                    .push(format!("start:{label}:{total_bytes}:{total_files}"));
            }
            fn advance(&self, delta_bytes: u64, delta_files: usize) {
                self.0
                    .lock()
                    .unwrap()
                    .push(format!("advance:{delta_bytes}:{delta_files}"));
            }
            fn done(&self) {
                self.0.lock().unwrap().push("done".to_string());
            }
        }

        fn scan_ok(
            root: &Path,
            prior: &BTreeMap<RelPath, FileFingerprint>,
            verify: bool,
            sink: &dyn ProgressSink,
        ) -> ScanOutput {
            scan(root, prior, verify, 4, sink, HASH).unwrap()
        }

        #[test]
        fn fast_path_reuses_the_stored_hash_without_hashing() {
            let dir = tempfile::TempDir::new().unwrap();
            let root = dir.path();
            file(root, "a", b"aaa");
            file(root, "b", b"bb");
            let first = scan_ok(root, &BTreeMap::new(), false, &NoopSink);
            let prior = prior_from(&first.files, |rel| Some(format!("old-{rel}")));
            let second = scan_ok(root, &prior, false, &NoopSink);
            assert_eq!(second.hashed, 0);
            let hashes: Vec<&str> =
                second.files.iter().map(|f| f.content_hash.as_str()).collect();
            assert_eq!(hashes, vec!["old-a", "old-b"]);
        }

        #[test]
        fn a_size_or_mtime_change_forces_a_rehash() {
            let dir = tempfile::TempDir::new().unwrap();
            let root = dir.path();
            file(root, "a", b"aaa");
            file(root, "b", b"bb");
            let first = scan_ok(root, &BTreeMap::new(), false, &NoopSink);
            // a: same size, later mtime
            let f = std::fs::File::options().write(true).open(root.join("a")).unwrap();
            f.set_modified(
                std::time::SystemTime::now() + std::time::Duration::from_millis(5_000),
            )
            .unwrap();
            drop(f);
            // b: different size
            file(root, "b", b"bbb");
            let prior = prior_from(&first.files, |_| None);
            let second = scan_ok(root, &prior, false, &NoopSink);
            assert_eq!(second.hashed, 2);
        }

        #[test]
        fn verify_forces_a_rehash_even_when_the_fingerprint_matches() {
            let dir = tempfile::TempDir::new().unwrap();
            let root = dir.path();
            file(root, "a", b"aaa");
            let first = scan_ok(root, &BTreeMap::new(), false, &NoopSink);
            let prior = prior_from(&first.files, |_| None);
            let second = scan_ok(root, &prior, true, &NoopSink);
            assert_eq!(second.hashed, 1);
        }

        #[test]
        fn output_is_sorted_by_relpath_across_the_hit_and_miss_partitions() {
            let dir = tempfile::TempDir::new().unwrap();
            let root = dir.path();
            file(root, "z", b"zzz");
            file(root, "a", b"aaa");
            let first = scan_ok(root, &BTreeMap::new(), false, &NoopSink);
            let a_hash = first.files.iter().find(|f| f.rel_path.as_str() == "a").unwrap();
            let prior =
                prior_from(&first.files, |rel| (rel == "z").then(|| "old-z".to_string()));
            let second = scan_ok(root, &prior, false, &NoopSink);
            assert_eq!(second.hashed, 1);
            let got: Vec<(String, &str)> = second
                .files
                .iter()
                .map(|f| (f.rel_path.as_str().to_string(), f.content_hash.as_str()))
                .collect();
            assert_eq!(
                got,
                vec![("a".to_string(), a_hash.content_hash.as_str()), ("z".to_string(), "old-z")]
            );
        }

        #[test]
        fn many_files_hash_to_a_sorted_result_under_parallelism() {
            let dir = tempfile::TempDir::new().unwrap();
            let root = dir.path();
            for i in 1..=100u32 {
                file(root, &format!("f{i:03}"), format!("{i}").as_bytes());
            }
            let out = scan(root, &BTreeMap::new(), false, 4, &NoopSink, HASH).unwrap();
            assert_eq!(out.hashed, 100);
            let names: Vec<String> =
                out.files.iter().map(|f| f.rel_path.as_str().to_string()).collect();
            let expected: Vec<String> = (1..=100).map(|i| format!("f{i:03}")).collect();
            assert_eq!(names, expected);
        }

        #[test]
        fn a_hash_failure_fails_the_scan_with_filefailed_for_that_file() {
            let dir = tempfile::TempDir::new().unwrap();
            let root = dir.path();
            file(root, "good", b"g");
            file(root, "bad", b"b");
            let fail = |p: &Path| -> std::io::Result<ContentHash> {
                if p.ends_with("bad") {
                    Err(std::io::Error::other("boom"))
                } else {
                    crate::fs::hasher::hash(p)
                }
            };
            let err = scan(root, &BTreeMap::new(), false, 1, &NoopSink, &fail).unwrap_err();
            assert_eq!(err.rel_path, RelPath("bad".into()));
        }

        #[test]
        fn progress_covers_only_the_files_actually_hashed() {
            let dir = tempfile::TempDir::new().unwrap();
            let root = dir.path();
            file(root, "hit", b"0123456789"); // 10 bytes, will be a hit
            let miss_bytes = vec![b'x'; 32];
            file(root, "miss", &miss_bytes);
            let first = scan_ok(root, &BTreeMap::new(), false, &NoopSink);
            let prior = prior_from(
                &first.files,
                |rel| (rel == "hit").then(|| "old-hit".to_string()),
            );
            let sink = Recording::default();
            let out = scan_ok(root, &prior, false, &sink);
            assert_eq!(out.hashed, 1);
            assert_eq!(
                sink.events(),
                vec!["start:hashing:32:1", "advance:32:1", "done"]
            );
        }

        #[test]
        fn a_scan_with_no_misses_emits_no_progress_events_at_all() {
            let dir = tempfile::TempDir::new().unwrap();
            let root = dir.path();
            file(root, "a", b"aaa");
            let first = scan_ok(root, &BTreeMap::new(), false, &NoopSink);
            let prior = prior_from(&first.files, |_| None);
            let sink = Recording::default();
            let out = scan_ok(root, &prior, false, &sink);
            assert_eq!(out.hashed, 0);
            assert_eq!(out.files.len(), 1);
            assert!(sink.events().is_empty());
        }
    }
```

- [ ] **Step 2: Run and watch it fail**

Run: `cargo test -p diskrement-shell scanner`
Expected: FAIL — `cannot find function 'scan'`.

- [ ] **Step 3: Implement** (in `scanner.rs`, after the walk/stat code)

```rust
use std::collections::BTreeMap;

use diskrement_core::{ContentHash, ScannedFile};

use crate::catalog::model::FileFingerprint;
use crate::progress::{phase, ProgressSink};

/// What one scan produced (`FileScanner.ScanOutput`): every file in the
/// snapshot, fully hashed (reused or fresh), sorted by rel path; `hashed` is
/// the count hashed this run.
pub struct ScanOutput {
    pub files: Vec<ScannedFile>,
    pub hashed: usize,
}

/// A per-file scan failure (`ScanError.FileFailed`): message
/// `failed scanning <relPath>`. A walk failure carries the *root* as the rel
/// path (`FileScanner.scala`'s behavior).
#[derive(Debug, thiserror::Error)]
#[error("failed scanning {rel_path}")]
pub struct ScanError {
    pub rel_path: RelPath,
    #[source]
    pub source: std::io::Error,
}

fn failed(rel_path: RelPath, source: std::io::Error) -> ScanError {
    ScanError { rel_path, source }
}

/// Scan the snapshot at `root` (`FileScannerLive.scan`).
///
/// Fast path: a `prior` fingerprint whose size **and** millis-truncated mtime
/// both equal the current stat is trusted when `!verify`, reusing its stored
/// hash without reading the file. Misses are hashed on a rayon pool of
/// `parallelism` threads (floored at 1); the first error fails the scan.
/// Output is merged and sorted by rel path. Progress covers **only the miss
/// set**; a scan with no misses emits no progress events at all.
pub fn scan(
    root: &Path,
    prior: &BTreeMap<RelPath, FileFingerprint>,
    verify: bool,
    parallelism: usize,
    sink: &dyn ProgressSink,
    hash: &(dyn Fn(&Path) -> std::io::Result<ContentHash> + Sync),
) -> Result<ScanOutput, ScanError> {
    use rayon::prelude::*;

    let paths = walk(root).map_err(|e| failed(RelPath(root.display().to_string()), e))?;

    struct Candidate {
        rel: RelPath,
        abs: PathBuf,
        st: FileStat,
    }
    let candidates: Vec<Candidate> = paths
        .into_iter()
        .map(|abs| {
            let rel = relativize(root, &abs);
            let st = stat(&abs).map_err(|e| failed(rel.clone(), e))?;
            Ok(Candidate { rel, abs, st })
        })
        .collect::<Result<Vec<_>, ScanError>>()?;

    let mut reused: Vec<ScannedFile> = Vec::new();
    let mut misses: Vec<Candidate> = Vec::new();
    for c in candidates {
        let hit = !verify
            && prior
                .get(&c.rel)
                .is_some_and(|fp| fp.size_bytes == c.st.size_bytes && fp.mtime == c.st.mtime);
        if hit {
            let fp = &prior[&c.rel];
            reused.push(ScannedFile {
                rel_path: c.rel,
                content_hash: fp.content_hash.clone(),
                size_bytes: c.st.size_bytes,
                mtime: c.st.mtime,
            });
        } else {
            misses.push(c);
        }
    }

    let mut hashed_files: Vec<ScannedFile> = Vec::new();
    if !misses.is_empty() {
        let total_bytes: u64 = misses.iter().map(|c| c.st.size_bytes).sum();
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(parallelism.max(1))
            .build()
            .map_err(|e| failed(RelPath(root.display().to_string()), std::io::Error::other(e)))?;
        // `phase` guarantees `done` even when a hash fails mid-pool.
        let hashed: Vec<Result<ScannedFile, ScanError>> = phase(
            sink,
            "hashing",
            total_bytes,
            misses.len(),
            || {
                pool.install(|| {
                    misses
                        .into_par_iter()
                        .map(|c| {
                            let h = hash(&c.abs).map_err(|e| failed(c.rel.clone(), e))?;
                            sink.advance(c.st.size_bytes, 1);
                            Ok(ScannedFile {
                                rel_path: c.rel,
                                content_hash: h,
                                size_bytes: c.st.size_bytes,
                                mtime: c.st.mtime,
                            })
                        })
                        .collect()
                })
            },
        );
        for r in hashed {
            hashed_files.push(r?);
        }
    }

    let mut files = reused;
    files.extend(hashed_files);
    files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(ScanOutput { files, hashed: hashed_files.len() })
}
```

Notes:
- `sink` is `&dyn ProgressSink` (`Send + Sync`) so `advance` can be called from
  pool workers; the trait's contract already requires cheap thread-safe
  implementations.
- rayon's `collect::<Vec<Result<_, _>>>` is used, then the first `Err` is
  propagated — the Scala contract is "first error fails the phase" without
  promising *which* error when several occur, so index-order propagation is a
  faithful port.

- [ ] **Step 4: Pass, trio, commit**

Run: `cargo test -p diskrement-shell scanner` — 12 passed (2 walk/stat + 2
parallelism + 8 scan), plus `parallelism`/`progress` suites still green. Trio
clean.

```bash
git add crates/diskrement-shell/src/fs/scanner.rs
git commit -m "feat(fs): the parallel scanner with size+mtime fast path"
```

---

### Task 6: The fingerprint cache

**Files:**
- Create: `crates/diskrement-shell/src/fs/fingerprint_cache.rs`
- Modify: `crates/diskrement-shell/src/fs/mod.rs` (add `pub mod fingerprint_cache;`)
- Oracle: `FingerprintCache.scala`, `FingerprintCacheSpec.scala`

**Interfaces:**
- Produces:
  `#[derive(Debug, thiserror::Error)] #[error("fingerprint cache I/O error")] pub struct CacheError(#[source] pub rusqlite::Error)` — soft, never fatal;
  `pub struct FingerprintCache { path: PathBuf }` with
  `pub fn new(path: PathBuf) -> Self`,
  `pub fn read_all(&self) -> Result<BTreeMap<RelPath, FileFingerprint>, CacheError>`,
  `pub fn put_all(&self, entries: &BTreeMap<RelPath, FileFingerprint>) -> Result<(), CacheError>`.
  Task 9 constructs one at `PipelinePaths::scan_cache` and treats every error
  as a warning (read → empty map, write → ignore).

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use diskrement_core::ContentHash;

    fn fp(size: u64, mtime: &str, h: &str) -> FileFingerprint {
        FileFingerprint {
            size_bytes: size,
            mtime: crate::catalog::time::parse(mtime).unwrap(),
            content_hash: ContentHash(h.into()),
        }
    }

    fn map(entries: &[(&str, FileFingerprint)]) -> BTreeMap<RelPath, FileFingerprint> {
        entries
            .iter()
            .map(|(r, f)| (RelPath(r.to_string()), f.clone()))
            .collect()
    }

    #[test]
    fn read_all_on_a_missing_file_returns_empty() {
        let dir = tempfile::TempDir::new().unwrap();
        let cache = FingerprintCache::new(dir.path().join("proj.scancache.sqlite"));
        assert!(cache.read_all().unwrap().is_empty());
    }

    #[test]
    fn put_all_then_read_all_round_trips_and_put_all_upserts() {
        let dir = tempfile::TempDir::new().unwrap();
        let cache = FingerprintCache::new(dir.path().join("proj.scancache.sqlite"));
        let first = map(&[
            ("a", fp(1, "2026-06-01T00:00:00Z", "h1")),
            ("b", fp(2, "2026-06-01T00:00:01Z", "h2")),
        ]);
        cache.put_all(&first).unwrap();
        let second = map(&[("a", fp(9, "2026-06-01T00:00:02Z", "h9"))]);
        cache.put_all(&second).unwrap();
        let got = cache.read_all().unwrap();
        assert_eq!(got.get(&RelPath("a".into())).unwrap().content_hash.as_str(), "h9");
        assert_eq!(got.get(&RelPath("b".into())).unwrap().content_hash.as_str(), "h2");
    }

    #[test]
    fn the_schema_version_stamp_is_1() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("proj.scancache.sqlite");
        let cache = FingerprintCache::new(path.clone());
        cache
            .put_all(&map(&[("a", fp(1, "2026-06-01T00:00:00Z", "h1"))]))
            .unwrap();
        let conn = rusqlite::Connection::open(&path).unwrap();
        let v: i64 = conn.query_row("pragma user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 1);
    }

    #[test]
    fn a_corrupt_cache_is_recreated_empty_and_stays_usable() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("proj.scancache.sqlite");
        std::fs::write(&path, b"not a database at all").unwrap();
        let cache = FingerprintCache::new(path);
        assert!(cache.read_all().unwrap().is_empty()); // recreate-on-read
        cache
            .put_all(&map(&[("a", fp(1, "2026-06-01T00:00:00Z", "h1"))]))
            .unwrap();
        assert_eq!(cache.read_all().unwrap().len(), 1);
    }

    #[test]
    fn a_future_schema_version_is_recreated_empty() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("proj.scancache.sqlite");
        let cache = FingerprintCache::new(path.clone());
        cache
            .put_all(&map(&[("a", fp(1, "2026-06-01T00:00:00Z", "h1"))]))
            .unwrap();
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("pragma user_version = 99").unwrap();
        drop(conn);
        assert!(cache.read_all().unwrap().is_empty());
    }

    #[test]
    fn a_row_with_an_undecodable_mtime_is_skipped_not_fatal() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("proj.scancache.sqlite");
        let cache = FingerprintCache::new(path.clone());
        cache
            .put_all(&map(&[("a", fp(1, "2026-06-01T00:00:00Z", "h1"))]))
            .unwrap();
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute(
            "insert into fingerprint values ('bad', 2, 'not-a-timestamp', 'h2', '2026-06-01T00:00:00Z')",
            [],
        )
        .unwrap();
        drop(conn);
        let got = cache.read_all().unwrap();
        assert_eq!(got.len(), 1);
        assert!(got.contains_key(&RelPath("a".into())));
    }

    #[test]
    fn a_locked_file_fails_softly_and_never_destroys_a_healthy_cache() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("proj.scancache.sqlite");
        let cache = FingerprintCache::new(path.clone());
        let entries = map(&[("a", fp(1, "2026-06-01T00:00:00Z", "h1"))]);
        cache.put_all(&entries).unwrap();
        // Hold the write lock from a second connection; busy_timeout=5000
        // makes this test take ~5s, exactly as the Scala spec's does.
        let guard = rusqlite::Connection::open(&path).unwrap();
        guard.execute_batch("begin immediate").unwrap();
        let err = cache.put_all(&entries).unwrap_err();
        assert_ne!(format!("{:?}", err.0), "");
        drop(guard);
        assert_eq!(cache.read_all().unwrap().len(), 1); // not wiped
    }
}
```

- [ ] **Step 2: Run and watch it fail**

Run: `cargo test -p diskrement-shell fingerprint_cache`
Expected: FAIL — `cannot find struct 'FingerprintCache'`.

- [ ] **Step 3: Implement**

```rust
//! The fingerprint sidecar (`FingerprintCache.scala`): a disposable,
//! upsert-able SQLite cache of size+mtime→hash observations, deliberately
//! outside the append-only catalog. A corrupt or version-incompatible file is
//! deleted and recreated empty; every other failure is a soft error the
//! caller treats as a warning.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::Utc;
use diskrement_core::{ContentHash, RelPath};
use rusqlite::{Connection, ErrorCode, OpenFlags};

use crate::catalog::model::FileFingerprint;
use crate::catalog::time;

const SCHEMA_VERSION: i64 = 1;

#[derive(Debug, thiserror::Error)]
#[error("fingerprint cache I/O error")]
pub struct CacheError(#[source] pub rusqlite::Error);

pub struct FingerprintCache {
    path: PathBuf,
}

enum Op {
    Read,
    Write,
}

impl Op {
    fn name(&self) -> &'static str {
        match self {
            Op::Read => "read",
            Op::Write => "write",
        }
    }
}

impl FingerprintCache {
    pub fn new(path: PathBuf) -> Self {
        FingerprintCache { path }
    }

    pub fn read_all(&self) -> Result<BTreeMap<RelPath, FileFingerprint>, CacheError> {
        self.guarded(Op::Read, |conn| {
            let mut stmt = conn.prepare(
                "select rel_path, size_bytes, mtime, content_hash from fingerprint",
            )?;
            let mut rows = stmt.query([])?;
            let mut out = BTreeMap::new();
            let mut skipped = 0usize;
            while let Some(row) = rows.next()? {
                let mtime: String = row.get(2)?;
                let Ok(mtime) = time::parse(&mtime) else {
                    skipped += 1;
                    continue;
                };
                out.insert(
                    RelPath(row.get::<_, String>(0)?),
                    FileFingerprint {
                        size_bytes: row.get::<_, i64>(1)?.max(0) as u64,
                        mtime,
                        content_hash: ContentHash(row.get(3)?),
                    },
                );
            }
            if skipped > 0 {
                eprintln!(
                    "fingerprint cache at {}: skipped {skipped} unreadable row(s)",
                    self.path.display()
                );
            }
            Ok(out)
        })
    }

    pub fn put_all(&self, entries: &BTreeMap<RelPath, FileFingerprint>) -> Result<(), CacheError> {
        let updated_at = time::format(&Utc::now()); // one stamp for the whole batch
        self.guarded(Op::Write, |conn| {
            let tx = conn.transaction()?;
            {
                let mut stmt = tx.prepare(
                    "insert into fingerprint (rel_path, size_bytes, mtime, content_hash, updated_at)
                     values (?1, ?2, ?3, ?4, ?5)
                     on conflict(rel_path) do update set
                       size_bytes = excluded.size_bytes, mtime = excluded.mtime,
                       content_hash = excluded.content_hash, updated_at = excluded.updated_at",
                )?;
                for (rel, fp) in entries {
                    stmt.execute(rusqlite::params![
                        rel.as_str(),
                        fp.size_bytes as i64,
                        time::format(&fp.mtime),
                        fp.content_hash.as_str(),
                        updated_at,
                    ])?;
                }
            }
            tx.commit()?;
            Ok(())
        })
    }

    /// One connection per call: the wipe-and-retry path must be able to delete
    /// the file out from under nothing (`FingerprintCacheLive.openConn`).
    fn guarded<T>(
        &self,
        op: Op,
        f: impl Fn(&Connection) -> Result<T, rusqlite::Error>,
    ) -> Result<T, CacheError> {
        match self.attempt(&f) {
            Ok(t) => Ok(t),
            Err(e) if is_corrupt(&e) => {
                eprintln!(
                    "fingerprint cache at {} is corrupt ({e}); recreating empty",
                    self.path.display()
                );
                self.wipe();
                self.attempt(&f).map_err(CacheError)
            }
            Err(e) => {
                eprintln!(
                    "fingerprint cache {} failed at {}: {e}",
                    op.name(),
                    self.path.display()
                );
                Err(CacheError(e))
            }
        }
    }

    fn attempt<T>(
        &self,
        f: &impl Fn(&Connection) -> Result<T, rusqlite::Error>,
    ) -> Result<T, rusqlite::Error> {
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let uri = format!("file:{}", self.path.display());
        let conn = Connection::open_with_flags(
            &uri,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_URI,
        )?;
        conn.busy_timeout(std::time::Duration::from_millis(5000))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        let version: i64 =
            conn.query_row("pragma user_version", [], |r| r.get(0))?; // must be read after open
        match version {
            0 => {
                conn.execute_batch(
                    "create table if not exists fingerprint (
                       rel_path     text primary key,
                       size_bytes   integer not null,
                       mtime        text not null,
                       content_hash text not null,
                       updated_at   text not null)",
                )?;
                conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            }
            v if v == SCHEMA_VERSION => {}
            other => {
                return Err(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(ErrorCode::DatabaseCorrupt),
                    Some(format!("cache schema v{other}, expected v{SCHEMA_VERSION}")),
                ));
            }
        }
        f(&conn)
    }

    /// Delete the file and its SQLite sidecars; errors ignored (`wipe`).
    fn wipe(&self) {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.path.display()));
        }
    }
}

/// The only triggers for wipe-and-recreate: our own version-mismatch signal,
/// or SQLite saying the file is not a usable database. Everything else —
/// locked, busy, permissions — is a soft failure that must NOT delete the
/// file (pinned by the lock test above).
fn is_corrupt(e: &rusqlite::Error) -> bool {
    match e {
        rusqlite::Error::SqliteFailure(ffi, _) => matches!(
            ffi.code,
            ErrorCode::NotADatabase | ErrorCode::DatabaseCorrupt
        ),
        _ => false,
    }
}
```

**Writing notes:** `time::parse` failures convert into a row skip, not an
error. The size column is `i64` in SQLite; clamp negatives to 0 defensively.

- [ ] **Step 4: Pass, trio, commit**

Run: `cargo test -p diskrement-shell` — all green (the lock test adds ~5 s).
Trio clean.

```bash
git add crates/diskrement-shell/src/fs
git commit -m "feat(fs): the disposable fingerprint sidecar with wipe-and-retry"
```

---

### Task 7: The CoW snapshot

**Files:**
- Create: `crates/diskrement-shell/src/fs/snapshot.rs`
- Modify: `crates/diskrement-shell/src/fs/mod.rs` (add `pub mod snapshot;`)
- Oracle: `SourceSnapshot.scala`, `Reflink.scala`, `SourceSnapshotSpec.scala`, `CheckCowCapableSpec.scala`

**Interfaces:**
- Produces:
  `pub struct SnapshotHandle { pub root: PathBuf }`;
  `#[derive(Debug, thiserror::Error)] pub enum SnapshotError` with variants
  `NotCowCapable { work_dir: PathBuf, detail: String }` (message
  `"work dir is not on a CoW-capable filesystem (need btrfs or XFS-with-reflink): {work_dir} [{detail}]"`),
  `SourceMissing(PathBuf)` (`"source root does not exist: {0}"`),
  `Io(#[source] std::io::Error)` (`"snapshot I/O error"`);
  `pub fn clone_unsupported(e: &std::io::Error) -> bool`;
  `pub fn create(source_root: &Path, work_dir: &Path) -> Result<SnapshotHandle, SnapshotError>`;
  `pub fn release(handle: &SnapshotHandle)`;
  `pub fn check_cow_capable(work_dir: &Path) -> Result<(), SnapshotError>`.
  Task 9 calls `create` (errors → `PipelineError::Failed`) and
  `check_cow_capable` (errors → `PipelineError::NotCowCapable`).

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// A plain recursive copy stands in for a successful reflink: the
    /// classification logic only ever sees the cloner's result.
    fn ok_cloner(src: &Path, dst: &Path) -> std::io::Result<()> {
        copy_tree(src, dst)
    }

    fn unsupported_cloner(_src: &Path, _dst: &Path) -> std::io::Result<()> {
        Err(std::io::Error::from_raw_os_error(95)) // EOPNOTSUPP
    }

    fn other_cloner(_src: &Path, _dst: &Path) -> std::io::Result<()> {
        Err(std::io::Error::other("disk on fire"))
    }

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::TempDir::new().unwrap();
        let src = dir.path().join("src");
        let work = dir.path().join("work");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("f.txt"), b"hello").unwrap();
        std::fs::write(src.join("sub/g.txt"), b"world").unwrap();
        std::fs::create_dir_all(&work).unwrap();
        (dir, src, work)
    }

    #[test]
    fn create_reflinks_the_source_into_workdir_snapshot() {
        let (_d, src, work) = fixture();
        let h = create_with(&src, &work, ok_cloner).unwrap();
        assert_eq!(h.root, work.join("snapshot"));
        assert_eq!(std::fs::read(h.root.join("f.txt")).unwrap(), b"hello");
        assert_eq!(std::fs::read(h.root.join("sub/g.txt")).unwrap(), b"world");
    }

    #[test]
    fn create_clears_a_stale_snapshot_dir_first() {
        let (_d, src, work) = fixture();
        std::fs::create_dir_all(work.join("snapshot")).unwrap();
        std::fs::write(work.join("snapshot/old.txt"), b"stale").unwrap();
        let h = create_with(&src, &work, ok_cloner).unwrap();
        assert!(!h.root.join("old.txt").exists());
        assert!(h.root.join("f.txt").exists());
    }

    #[test]
    fn a_missing_source_fails_with_sourcemissing() {
        let (_d, _src, work) = fixture();
        let err = create_with(&work.join("nope"), &work, ok_cloner).unwrap_err();
        assert!(matches!(err, SnapshotError::SourceMissing(_)));
    }

    #[test]
    fn a_notcowcapable_clone_error_propagates_as_is() {
        let (_d, src, work) = fixture();
        let err = create_with(&src, &work, unsupported_cloner).unwrap_err();
        assert!(matches!(err, SnapshotError::NotCowCapable { .. }));
    }

    #[test]
    fn an_ioerror_from_the_cloner_propagates_as_is() {
        let (_d, src, work) = fixture();
        let err = create_with(&src, &work, other_cloner).unwrap_err();
        assert!(matches!(err, SnapshotError::Io(_)));
    }

    #[test]
    fn release_removes_the_snapshot_dir() {
        let (_d, src, work) = fixture();
        let h = create_with(&src, &work, ok_cloner).unwrap();
        release(&h);
        assert!(!h.root.exists());
    }

    #[test]
    fn check_cow_capable_succeeds_when_the_cloner_can_reflink() {
        let (_d, _src, work) = fixture();
        check_cow_capable_with(&work, ok_cloner).unwrap();
    }

    #[test]
    fn check_cow_capable_fails_notcowcapable_when_it_cannot() {
        let (_d, _src, work) = fixture();
        let err = check_cow_capable_with(&work, unsupported_cloner).unwrap_err();
        assert!(matches!(err, SnapshotError::NotCowCapable { .. }));
    }

    #[test]
    fn the_real_reflink_works_on_a_cow_filesystem_or_skips_loudly() {
        let dir = tempfile::TempDir::new().unwrap();
        let work = dir.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let probe_src = work.join("cow-probe.src");
        std::fs::write(&probe_src, b"0").unwrap();
        if reflink_copy::reflink(&probe_src, work.join("cow-probe.clone")).is_err() {
            eprintln!(
                "SKIP: {} is not on a CoW filesystem; the real-reflink test needs \
                 TMPDIR on btrfs/XFS/bcachefs to execute.",
                dir.path().display()
            );
            return;
        }
        let _ = std::fs::remove_file(work.join("cow-probe.clone"));
        let src = work.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("f.txt"), b"hello").unwrap();
        let h = create(&src, &work).unwrap();
        assert_eq!(std::fs::read(h.root.join("f.txt")).unwrap(), b"hello");
    }

    #[test]
    fn clone_unsupported_matches_the_capability_errnos_only() {
        assert!(clone_unsupported(&std::io::Error::from_raw_os_error(18))); // EXDEV
        assert!(clone_unsupported(&std::io::Error::from_raw_os_error(95))); // EOPNOTSUPP
        assert!(!clone_unsupported(&std::io::Error::other("failed to clone"))); // errno-less
        assert!(!clone_unsupported(&std::io::Error::from_raw_os_error(13))); // EACCES
    }
}
```

**Writing notes:** `copy_tree` is a small test helper (recursive
`create_dir_all` + `std::fs::copy`) defined once in the test module.

- [ ] **Step 2: Run and watch it fail**

Run: `cargo test -p diskrement-shell snapshot`
Expected: FAIL — `cannot find function 'create_with'`.

- [ ] **Step 3: Implement**

```rust
//! The CoW source snapshot (`SourceSnapshot.scala`). `cp --reflink=always -a`
//! is replaced by a directory walk that reflinks file-by-file with
//! `reflink_copy` — the crate's `reflink` handles regular files only on
//! Linux, so the walk is the port of `-a`. The clone step is a parameter so
//! the hard-fail/probe classification is testable without a filesystem that
//! refuses reflinks.

use std::path::{Path, PathBuf};

/// `Reflink.scala`'s "this filesystem cannot reflink here", translated from
/// English-cp-stderr sniffing (which forced `LC_ALL=C`) to the errnos
/// `ioctl(FICLONE)` returns for it: `EXDEV` (18, cross-device) and
/// `EOPNOTSUPP` (95, also reported as `ENOTSUP`). Any other error is a real
/// failure, not a capability signal.
pub fn clone_unsupported(e: &std::io::Error) -> bool {
    matches!(e.raw_os_error(), Some(18 | 95))
}

#[derive(Debug)]
pub struct SnapshotHandle {
    pub root: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    #[error(
        "work dir is not on a CoW-capable filesystem (need btrfs or XFS-with-reflink): {work_dir} [{detail}]"
    )]
    NotCowCapable {
        work_dir: PathBuf,
        detail: String,
    },
    #[error("source root does not exist: {0}")]
    SourceMissing(PathBuf),
    #[error("snapshot I/O error")]
    Io(#[source] std::io::Error),
}

fn classify(work_dir: &Path, e: std::io::Error) -> SnapshotError {
    if clone_unsupported(&e) {
        SnapshotError::NotCowCapable {
            work_dir: work_dir.to_path_buf(),
            detail: e.to_string(),
        }
    } else {
        SnapshotError::Io(e)
    }
}

/// Recursively clone `src` to `dst` by reflinking each regular file. Not
/// atomic; the caller has already removed any stale `dst`. Symlinks are
/// excluded, matching the scanner's walk.
fn reflink_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in walkdir::WalkDir::new(src).follow_links(false) {
        let entry = entry.map_err(std::io::Error::other)?;
        let rel = entry.path().strip_prefix(src).unwrap_or(entry.path());
        let target = dst.join(rel);
        let ft = entry.file_type();
        if ft.is_dir() {
            std::fs::create_dir_all(&target)?;
        } else if ft.is_file() {
            reflink_copy::reflink(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// `create` with the cloner injected; the real entry point passes
/// `reflink_tree`. Order follows `SourceSnapshotLive.create`: existence
/// check, stale-snapshot removal, work-dir creation, then the clone. A
/// clone-unsupported failure is **hard** — there is no plain-copy fallback
/// for the source freeze.
pub fn create_with(
    source_root: &Path,
    work_dir: &Path,
    clone: impl Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<SnapshotHandle, SnapshotError> {
    if !source_root.exists() {
        return Err(SnapshotError::SourceMissing(source_root.to_path_buf()));
    }
    let dst = work_dir.join("snapshot");
    if dst.exists() {
        std::fs::remove_dir_all(&dst).map_err(SnapshotError::Io)?;
    }
    std::fs::create_dir_all(work_dir).map_err(SnapshotError::Io)?;
    // NotCowCapable's work_dir is the work dir itself (cp's dst.getParent).
    clone(source_root, &dst).map_err(|e| classify(work_dir, e))?;
    Ok(SnapshotHandle { root: dst })
}

pub fn create(source_root: &Path, work_dir: &Path) -> Result<SnapshotHandle, SnapshotError> {
    create_with(source_root, work_dir, reflink_tree)
}

/// Best-effort recursive delete (`release`): the snapshot is disposable by
/// construction, so errors are ignored.
pub fn release(handle: &SnapshotHandle) {
    let _ = std::fs::remove_dir_all(&handle.root);
}

/// `checkCowCapable` with the cloner injected: a filesystem is CoW-capable
/// iff a real reflink clone of a 1-byte file inside `work_dir` succeeds — no
/// filesystem-name sniffing.
pub fn check_cow_capable_with(
    work_dir: &Path,
    clone: impl Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<(), SnapshotError> {
    std::fs::create_dir_all(work_dir).map_err(SnapshotError::Io)?;
    let src = work_dir.join("cow-probe.src");
    let dst = work_dir.join("cow-probe.src.clone");
    std::fs::write(&src, b"0").map_err(SnapshotError::Io)?;
    let result = clone(&src, &dst).map_err(|e| classify(work_dir, e));
    let _ = std::fs::remove_file(&src);
    let _ = std::fs::remove_file(&dst);
    result
}

pub fn check_cow_capable(work_dir: &Path) -> Result<(), SnapshotError> {
    check_cow_capable_with(work_dir, |s, d| reflink_copy::reflink(s, d))
}
```

- [ ] **Step 4: Pass, trio, commit**

Run: `cargo test -p diskrement-shell snapshot` — 11 passed (the real-reflink
test executes on this machine's btrfs `/tmp`). Trio clean.

```bash
git add crates/diskrement-shell/src/fs
git commit -m "feat(fs): the CoW source snapshot via per-file reflinks"
```

---

### Task 8: Staged scan + PipelineError

**Files:**
- Create: `crates/diskrement-shell/src/pipeline/mod.rs`, `staged.rs`, `reports.rs`
- Modify: `crates/diskrement-shell/src/error.rs`, `crates/diskrement-shell/src/lib.rs` (add `pub mod pipeline;`)
- Oracle: `StagedStore.scala`, `PipelineError.scala`, `Render.scala` (error text)

**Interfaces:**
- Produces:
  `pub fn write_scan(work_dir: &Path, files: &[ScannedFile]) -> std::io::Result<()>`;
  `pub fn read_scan(work_dir: &Path) -> Result<Option<Vec<ScannedFile>>, serde_json::Error>`
  (both in `pipeline::staged`);
  `pub struct ScanReport { pub added: usize, pub modified: usize, pub deleted: usize, pub unchanged: usize, pub hashed: Option<usize>, pub scanned_files: usize }`;
  `pub struct StatusReport { pub discs: Vec<DiscState>, pub open_disc_ordinal: Option<u32>, pub total_current_files: usize, pub last_burn_at: Option<DateTime<Utc>> }`
  (in `pipeline::reports`);
  `pub enum PipelineError` with `ProjectNotFound(String)`,
  `NotSameFilesystem { source: PathBuf, work: PathBuf }`,
  `NotCowCapable { work_dir: PathBuf, detail: String }`, `Failed(String)`,
  Display strings per the contract, and `pub fn exit_code(&self) -> i32`
  (2, 3, 3, 5).

- [ ] **Step 1: Write the failing tests**

`crates/diskrement-shell/src/pipeline/staged.rs` tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use diskrement_core::ContentHash;

    fn sf(rel: &str) -> ScannedFile {
        ScannedFile {
            rel_path: RelPath(rel.into()),
            content_hash: ContentHash("deadbeef".into()),
            size_bytes: 10,
            mtime: crate::catalog::time::parse("2026-06-01T00:00:00Z").unwrap(),
        }
    }

    #[test]
    fn round_trips_a_staged_scan() {
        let dir = tempfile::TempDir::new().unwrap();
        let files = vec![sf("a.txt"), sf("sub/b.txt")];
        write_scan(dir.path(), &files).unwrap();
        assert_eq!(read_scan(dir.path()).unwrap().unwrap(), files);
    }

    #[test]
    fn read_returns_none_when_nothing_is_staged() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(read_scan(dir.path()).unwrap().is_none());
    }

    #[test]
    fn a_corrupt_staged_scan_is_a_decode_error() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("staged")).unwrap();
        std::fs::write(dir.path().join("staged/scan.json"), "{not json").unwrap();
        assert!(read_scan(dir.path()).unwrap_err().is_syntax());
    }

    #[test]
    fn the_json_field_spellings_match_the_scala_codec() {
        let dir = tempfile::TempDir::new().unwrap();
        write_scan(dir.path(), &[sf("a.txt")]).unwrap();
        let raw = std::fs::read_to_string(dir.path().join("staged/scan.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let e = &v[0];
        assert_eq!(e["relPath"], "a.txt");
        assert_eq!(e["contentHash"], "deadbeef");
        assert_eq!(e["sizeBytes"], 10);
        assert_eq!(e["mtime"], "2026-06-01T00:00:00Z");
    }
}
```

`crates/diskrement-shell/src/error.rs` tests (append a test module):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_are_the_contract_for_the_p3_variants() {
        assert_eq!(PipelineError::ProjectNotFound("photos".into()).exit_code(), 2);
        assert_eq!(
            PipelineError::NotSameFilesystem { source: "/s".into(), work: "/w".into() }.exit_code(),
            3
        );
        assert_eq!(
            PipelineError::NotCowCapable { work_dir: "/w".into(), detail: "d".into() }.exit_code(),
            3
        );
        assert_eq!(PipelineError::Failed("x".into()).exit_code(), 5);
    }

    #[test]
    fn messages_match_the_scala_render_verbatim() {
        assert_eq!(
            PipelineError::ProjectNotFound("photos".into()).to_string(),
            "project not found: photos"
        );
        assert_eq!(
            PipelineError::NotSameFilesystem {
                source: "/srv/src".into(),
                work: "/srv/work".into()
            }
            .to_string(),
            "work dir /srv/work is not on the same filesystem as source /srv/src"
        );
        assert_eq!(
            PipelineError::NotCowCapable { work_dir: "/w".into(), detail: "clone said no".into() }
                .to_string(),
            "work dir is not CoW-capable: /w [clone said no]"
        );
        assert_eq!(
            PipelineError::Failed("project already exists: p".into()).to_string(),
            "internal error: project already exists: p (this is a bug)"
        );
    }
}
```

- [ ] **Step 2: Run and watch it fail**

Run: `cargo test -p diskrement-shell pipeline error`
Expected: FAIL — modules/types missing.

- [ ] **Step 3: Implement**

`crates/diskrement-shell/src/pipeline/reports.rs`:

```rust
//! Pipeline reports: plain data the CLI renders (`Pipeline.scala`'s result
//! types). The pipeline returns these and performs no path resolution.

use chrono::{DateTime, Utc};

use diskrement_core::DiscState;

/// `ScanResult` — `hashed` is `None` when a staged scan was reused.
pub struct ScanReport {
    pub added: usize,
    pub modified: usize,
    pub deleted: usize,
    pub unchanged: usize,
    pub hashed: Option<usize>,
    pub scanned_files: usize,
}

/// `StatusReport`.
pub struct StatusReport {
    pub discs: Vec<DiscState>,
    pub open_disc_ordinal: Option<u32>,
    pub total_current_files: usize,
    pub last_burn_at: Option<DateTime<Utc>>,
}
```

`crates/diskrement-shell/src/pipeline/staged.rs`:

```rust
//! The staged scan (`StagedStore.scala`, scan half): `<work>/staged/scan.json`
//! is the hand-off from `scan` to `plan` and the recovery unit's first
//! artifact. Field spellings are the Scala zio-json codec's, byte-compatible.

use std::io::Write;
use std::path::{Path, PathBuf};

use diskrement_core::{RelPath, ScannedFile};
use serde::{Deserialize, Serialize};

use crate::catalog::time;

#[derive(Serialize, Deserialize)]
struct ScanJsonEntry {
    #[serde(rename = "relPath")]
    rel_path: String,
    #[serde(rename = "contentHash")]
    content_hash: String,
    #[serde(rename = "sizeBytes")]
    size_bytes: u64,
    mtime: String,
}

/// `<workDir>/staged/scan.json`.
pub fn scan_path(work_dir: &Path) -> PathBuf {
    work_dir.join("staged").join("scan.json")
}

/// Compact JSON, like zio-json's `.toJson`.
pub fn write_scan(work_dir: &Path, files: &[ScannedFile]) -> std::io::Result<()> {
    let entries: Vec<ScanJsonEntry> = files
        .iter()
        .map(|f| ScanJsonEntry {
            rel_path: f.rel_path.as_str().to_string(),
            content_hash: f.content_hash.as_str().to_string(),
            size_bytes: f.size_bytes,
            mtime: time::format(&f.mtime),
        })
        .collect();
    std::fs::create_dir_all(scan_path(work_dir).parent().expect("staged has a parent"))?;
    let mut out = std::fs::File::create(scan_path(work_dir))?;
    out.write_all(serde_json::to_string(&entries).expect("serializable").as_bytes())?;
    Ok(())
}

/// `Ok(None)` when nothing is staged; a decode failure is the caller's
/// defect (`corrupt scan.json: …` in the Scala `RuntimeException`).
pub fn read_scan(work_dir: &Path) -> Result<Option<Vec<ScannedFile>>, serde_json::Error> {
    let raw = match std::fs::read_to_string(scan_path(work_dir)) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(serde_json::Error::io(e)),
    };
    let entries: Vec<ScanJsonEntry> = serde_json::from_str(&raw)?;
    let mut files = Vec::with_capacity(entries.len());
    for e in entries {
        let mtime = time::parse(&e.mtime)
            .map_err(|err| serde_json::Error::custom(err.to_string()))?;
        files.push(ScannedFile {
            rel_path: RelPath(e.rel_path),
            content_hash: diskrement_core::ContentHash(e.content_hash),
            size_bytes: e.size_bytes,
            mtime,
        });
    }
    Ok(Some(files))
}
```

`crates/diskrement-shell/src/pipeline/mod.rs`:

```rust
//! The pipeline (`Pipeline.scala`): returns reports, performs no path
//! resolution. P3 lands `init`, `scan` and `status`; later phases append the
//! rest of the nine methods.

pub mod reports;
pub mod staged;
```

Append to `crates/diskrement-shell/src/error.rs`:

```rust
/// `PipelineError.scala` — the variants P3 can produce. Later phases append
/// the plan/burn/verify variants; the exit ladder is the stable contract
/// (spec §3.2) and is pinned by the CLI's ExitMapping port.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("project not found: {0}")]
    ProjectNotFound(String),
    #[error("work dir {work} is not on the same filesystem as source {source}")]
    NotSameFilesystem {
        source: std::path::PathBuf,
        work: std::path::PathBuf,
    },
    #[error("work dir is not CoW-capable: {work_dir} [{detail}]")]
    NotCowCapable {
        work_dir: std::path::PathBuf,
        detail: String,
    },
    /// Unexpected-but-handled failure: rendered as
    /// `internal error: <detail> (this is a bug)`, exit 5.
    #[error("internal error: {0} (this is a bug)")]
    Failed(String),
}

impl PipelineError {
    pub fn exit_code(&self) -> i32 {
        match self {
            PipelineError::ProjectNotFound(_) => 2,
            PipelineError::NotSameFilesystem { .. } | PipelineError::NotCowCapable { .. } => 3,
            PipelineError::Failed(_) => 5,
        }
    }
}
```

- [ ] **Step 4: Pass, trio, commit**

```bash
git add crates/diskrement-shell/src/error.rs crates/diskrement-shell/src/lib.rs crates/diskrement-shell/src/pipeline
git commit -m "feat(pipeline): staged scan json and the P3 pipeline errors"
```

---

### Task 9: Pipeline init, scan, status

**Files:**
- Modify: `crates/diskrement-shell/src/pipeline/mod.rs`
- Oracle: `PipelineLive.scala` (init/scan/status), `PipelineInitStatusHistorySpec.scala`, `PipelineRescanSpec.scala`, `PipelineScanResetSpec.scala` (case 1)

**Interfaces:**
- Consumes: everything above; `Catalog::{create_project, find_project, fingerprints, read_view, read_disc_states, sessions_by_state}`;
  `diskrement_core::diff::diff`.
- Produces:
  `pub struct PipelinePaths { pub work_dir: PathBuf, pub scan_cache: PathBuf }`;
  `pub struct Pipeline<'a>` with
  `pub fn new(catalog: &'a mut Catalog, paths: PipelinePaths, progress: &'a dyn ProgressSink) -> Self`,
  `pub fn init(&mut self, pi: &ProjectInit) -> Result<ProjectId, PipelineError>`,
  `pub fn scan(&mut self, name: &str, verify: bool, force: bool) -> Result<ScanReport, PipelineError>`,
  `pub fn status(&self, name: &str) -> Result<StatusReport, PipelineError>`.
  Task 14 constructs this from the CLI.

**Deviation from the Scala spec, deliberate:** `PipelineInitStatusHistorySpec`
case 2 ("init fails NotCowCapable when the work dir is not CoW-capable") used a
fake snapshot port; Rust has no port to fake, and the classification is already
pinned by Task 7's `check_cow_capable_fails_notcowcapable_when_it_cannot`. The
pipeline-level wiring is one `map_err` arm. Case is covered, not skipped.

- [ ] **Step 1: Write the failing tests** (in `pipeline/mod.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::progress::NoopSink;
    use diskrement_core::DiscType;
    use std::path::PathBuf;

    /// A leak-free setup: catalog and pipeline share one heap allocation kept
    /// alive by the returned guard struct.
    struct Env {
        _dirs: Vec<tempfile::TempDir>,
        src: PathBuf,
        work: PathBuf,
        pipeline: Pipeline<'static>,
    }

    fn setup() -> Env {
        let cat_dir = tempfile::TempDir::new().unwrap();
        let src = tempfile::TempDir::new().unwrap();
        let work = tempfile::TempDir::new().unwrap();
        let catalog = Box::leak(Box::new(
            Catalog::open(&cat_dir.path().join("catalog.sqlite")).unwrap(),
        ));
        let pipeline = Pipeline::new(
            catalog,
            PipelinePaths {
                work_dir: work.path().to_path_buf(),
                scan_cache: work.path().join("cat.p.scancache.sqlite"),
            },
            &NoopSink,
        );
        Env {
            _dirs: vec![cat_dir, src, work],
            src: src.path().to_path_buf(),
            work: work.path().to_path_buf(),
            pipeline,
        }
    }

    fn project_init(env: &Env) -> ProjectInit {
        ProjectInit {
            name: "p".into(),
            source_root: env.src.display().to_string(),
            work_dir: env.work.display().to_string(),
            default_disc_type: DiscType::BdR,
            margin_percent: 2.0,
            copies: 1,
            created_at: crate::catalog::testutil::ts(),
        }
    }

    fn write_src(env: &Env, rel: &str, bytes: &[u8]) {
        let p = env.src.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    #[test]
    fn init_creates_a_project_and_status_reports_zero_discs() {
        let mut e = setup();
        let id = e.pipeline.init(&project_init(&e)).unwrap();
        assert_eq!(id.0, 1);
        let r = e.pipeline.status("p").unwrap();
        assert!(r.discs.is_empty());
        assert_eq!(r.total_current_files, 0);
        assert_eq!(r.open_disc_ordinal, None);
        assert_eq!(r.last_burn_at, None);
    }

    #[test]
    fn init_twice_fails_with_the_duplicate_project_detail() {
        let mut e = setup();
        e.pipeline.init(&project_init(&e)).unwrap();
        let err = e.pipeline.init(&project_init(&e)).unwrap_err();
        match err {
            PipelineError::Failed(detail) => {
                assert!(detail.contains("project already exists: p"), "{detail}")
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn status_on_an_unknown_project_fails_projectnotfound() {
        let e = setup();
        let err = e.pipeline.status("nosuch").unwrap_err();
        assert!(matches!(err, PipelineError::ProjectNotFound(_)));
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn scan_on_a_fresh_project_reports_all_added_and_stages_them() {
        let mut e = setup();
        write_src(&e, "a.txt", b"aaaaa");
        write_src(&e, "sub/b.txt", b"b");
        e.pipeline.init(&project_init(&e)).unwrap();
        let r = e.pipeline.scan("p", false, false).unwrap();
        assert_eq!((r.added, r.modified, r.deleted, r.unchanged), (2, 0, 0, 0));
        assert_eq!(r.hashed, Some(2));
        assert_eq!(r.scanned_files, 2);
        assert_eq!(
            staged::read_scan(&e.work).unwrap().unwrap().len(),
            2
        );
    }

    #[test]
    fn a_second_scan_without_burn_reuses_the_staged_scan() {
        // PipelineScanResetSpec case 1's staged side + the reuse rule: the
        // second scan reports hashed = None and still says added (nothing
        // burned yet).
        let mut e = setup();
        write_src(&e, "a.txt", b"a");
        write_src(&e, "b.txt", b"bb");
        e.pipeline.init(&project_init(&e)).unwrap();
        assert_eq!(e.pipeline.scan("p", false, false).unwrap().hashed, Some(2));
        let second = e.pipeline.scan("p", false, false).unwrap();
        assert_eq!(second.hashed, None);
        assert_eq!(second.added, 2);
    }

    #[test]
    fn a_forced_rescan_hashes_nothing_once_the_cache_is_primed() {
        let mut e = setup();
        for (n, bytes) in [("a.txt", &b"a"[..]), ("b.txt", b"bb"), ("c.txt", b"ccc")] {
            write_src(&e, n, bytes);
        }
        e.pipeline.init(&project_init(&e)).unwrap();
        assert_eq!(e.pipeline.scan("p", false, false).unwrap().hashed, Some(3));
        let second = e.pipeline.scan("p", false, true).unwrap(); // force: fresh scan
        assert_eq!(second.hashed, Some(0));
        assert_eq!(second.added, 3); // nothing burned yet
        assert_eq!(second.unchanged, 0);
    }

    #[test]
    fn a_forced_rescan_rehashes_only_a_modified_file() {
        let mut e = setup();
        write_src(&e, "a.txt", b"a");
        write_src(&e, "b.txt", b"bb");
        e.pipeline.init(&project_init(&e)).unwrap();
        e.pipeline.scan("p", false, false).unwrap();
        write_src(&e, "b.txt", b"bbb");
        assert_eq!(e.pipeline.scan("p", false, true).unwrap().hashed, Some(1));
    }

    #[test]
    fn verify_forces_a_full_rehash_even_with_a_primed_cache() {
        let mut e = setup();
        write_src(&e, "a.txt", b"a");
        write_src(&e, "b.txt", b"bb");
        e.pipeline.init(&project_init(&e)).unwrap();
        e.pipeline.scan("p", false, false).unwrap();
        assert_eq!(e.pipeline.scan("p", true, true).unwrap().hashed, Some(2));
    }

    #[test]
    fn verify_with_a_staged_scan_and_no_force_still_rehashes() {
        let mut e = setup();
        write_src(&e, "a.txt", b"a");
        write_src(&e, "b.txt", b"bb");
        e.pipeline.init(&project_init(&e)).unwrap();
        e.pipeline.scan("p", false, false).unwrap();
        assert_eq!(e.pipeline.scan("p", true, false).unwrap().hashed, Some(2));
    }

    /// Replace the pipeline with one whose cache path is a directory: every
    /// cache op then fails non-corruptly (open error), exercising the
    /// never-fatal rule end to end. The cache TempDir is leaked so it
    /// outlives the leaked pipeline, same as the catalog.
    fn with_poisoned_cache(e: &Env) -> Pipeline<'static> {
        let catalog = Box::leak(Box::new(
            Catalog::open(&e._dirs[0].path().join("catalog.sqlite")).unwrap(),
        ));
        let cache_dir = Box::leak(Box::new(tempfile::TempDir::new().unwrap()));
        std::fs::create_dir_all(cache_dir.path().join("cache")).unwrap();
        Pipeline::new(
            catalog,
            PipelinePaths {
                work_dir: e.work.clone(),
                scan_cache: cache_dir.path().join("cache"), // a directory
            },
            &NoopSink,
        )
    }

    #[test]
    fn a_failing_cache_read_still_succeeds_and_hashes_everything() {
        let e = setup();
        let mut pl = with_poisoned_cache(&e);
        write_src(&e, "a.txt", b"a");
        write_src(&e, "b.txt", b"bb");
        write_src(&e, "c.txt", b"ccc");
        pl.init(&project_init(&e)).unwrap();
        let r = pl.scan("p", false, false).unwrap();
        assert_eq!(r.hashed, Some(3));
        assert_eq!(r.added, 3);
    }

    #[test]
    fn a_failing_cache_write_still_succeeds_and_nothing_was_cached() {
        let e = setup();
        let mut pl = with_poisoned_cache(&e);
        write_src(&e, "a.txt", b"a");
        write_src(&e, "b.txt", b"bb");
        write_src(&e, "c.txt", b"ccc");
        pl.init(&project_init(&e)).unwrap();
        assert_eq!(pl.scan("p", false, false).unwrap().hashed, Some(3));
        // nothing cached, so the forced scan re-hashes everything
        assert_eq!(pl.scan("p", false, true).unwrap().hashed, Some(3));
    }
}
```

**Prerequisite, one line:** in `crates/diskrement-shell/src/catalog/mod.rs`,
change `mod testutil;` to `pub(crate) mod testutil;` — its items are already
`pub(crate)`, but the module itself is private to `catalog`, so
`pipeline`'s tests could not path into `crate::catalog::testutil`.

- [ ] **Step 2: Run and watch it fail**

Run: `cargo test -p diskrement-shell pipeline`
Expected: FAIL — `Pipeline`, `PipelinePaths` not found.

- [ ] **Step 3: Implement**

Replace the contents of `crates/diskrement-shell/src/pipeline/mod.rs`:

```rust
//! The pipeline (`Pipeline.scala`): returns reports, performs no path
//! resolution, and — for scan — **writes no catalog rows**. P3 lands `init`,
//! `scan` and `status`; later phases append the rest of the nine methods.

pub mod reports;
pub mod staged;

use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use diskrement_core::{diff, DiscStatus, RelPath, ScannedFile};

use crate::catalog::model::{FileFingerprint, Project, ProjectId, ProjectInit, SessionState};
use crate::catalog::Catalog;
use crate::error::PipelineError;
use crate::fs::fingerprint_cache::FingerprintCache;
use crate::fs::hasher;
use crate::fs::scanner::{self, parallelism_from_env, ScanOutput};
use crate::fs::snapshot;
use crate::progress::ProgressSink;

use reports::{ScanReport, StatusReport};

/// The paths a pipeline needs, resolved by the CLI (`ProjectPaths` +
/// `CachePath` in the Scala wiring). `work_dir` for `scan`/`status` comes from
/// the project row, not the CLI flag.
pub struct PipelinePaths {
    pub work_dir: PathBuf,
    pub scan_cache: PathBuf,
}

pub struct Pipeline<'a> {
    catalog: &'a mut Catalog,
    paths: PipelinePaths,
    progress: &'a dyn ProgressSink,
}

impl<'a> Pipeline<'a> {
    pub fn new(
        catalog: &'a mut Catalog,
        paths: PipelinePaths,
        progress: &'a dyn ProgressSink,
    ) -> Self {
        Pipeline { catalog, paths, progress }
    }

    fn failed(e: impl std::fmt::Display) -> PipelineError {
        PipelineError::Failed(e.to_string())
    }

    fn project(&self, name: &str) -> Result<Project, PipelineError> {
        self.catalog
            .find_project(name)
            .map_err(Self::failed)?
            .ok_or_else(|| PipelineError::ProjectNotFound(name.to_string()))
    }

    /// `init`: same-filesystem check, CoW probe, then the one project row. No
    /// snapshot, no scan, no other rows (`PipelineLive.init`).
    pub fn init(&mut self, pi: &ProjectInit) -> Result<ProjectId, PipelineError> {
        let src = PathBuf::from(&pi.source_root);
        let work = PathBuf::from(&pi.work_dir);
        // `getFileStore` equality via st_dev; a missing source surfaces here
        // as Failed (exit 5), exactly like the Scala getFileStore throw.
        let (sdev, wdev) = same_filesystem(&src, &work).map_err(Self::failed)?;
        if sdev != wdev {
            return Err(PipelineError::NotSameFilesystem { source: src, work });
        }
        snapshot::check_cow_capable(&work).map_err(|e| PipelineError::NotCowCapable {
            work_dir: work,
            detail: e.to_string(),
        })?;
        self.catalog.create_project(pi).map_err(Self::failed)
    }

    /// `scan`: reuse a staged scan unless `--force`/`--verify`; otherwise
    /// snapshot → hash → stage → cache, and diff against the catalog view
    /// (`PipelineLive.scan`). `hashed` is `None` on the reuse path.
    pub fn scan(
        &mut self,
        name: &str,
        verify: bool,
        force: bool,
    ) -> Result<ScanReport, PipelineError> {
        let p = self.project(name)?;
        if !force && !verify {
            if let Some(files) = staged::read_scan(&self.paths.work_dir)
                .map_err(|e| Self::failed(format!("corrupt scan.json: {e}")))?
            {
                return self.count(&p, &files, None);
            }
        }
        let (files, hashed) = self.fresh_scan(&p, verify)?;
        self.count(&p, &files, Some(hashed))
    }

    fn fresh_scan(
        &mut self,
        p: &Project,
        verify: bool,
    ) -> Result<(Vec<ScannedFile>, usize), PipelineError> {
        let src = PathBuf::from(&p.source_root);
        // Snapshot failures (missing source, non-CoW, I/O) are Failed here —
        // exit 5 — because only `init` produces the typed 3s (`PipelineLive`
        // maps SnapshotError to Failed in freshScan).
        let handle = snapshot::create(&src, &self.paths.work_dir).map_err(Self::failed)?;
        let prior: BTreeMap<RelPath, FileFingerprint> =
            self.catalog.fingerprints(p.id).map_err(Self::failed)?;
        // The cache never fails a scan; a read failure falls back to empty.
        let cached = FingerprintCache::new(self.paths.scan_cache.clone())
            .read_all()
            .unwrap_or_default();
        let mut merged = prior;
        // The cache wins on conflict: it is fresher — it re-fingerprinted
        // touched-but-identical files so later scans fast-path them.
        for (k, v) in cached {
            merged.insert(k, v);
        }
        let par = parallelism_from_env();
        for w in &par.warnings {
            eprintln!("{w}");
        }
        let out: ScanOutput = scanner::scan(
            &handle.root,
            &merged,
            verify,
            par.value,
            self.progress,
            &hasher::hash,
        )
        .map_err(Self::failed)?;
        staged::write_scan(&self.paths.work_dir, &out.files).map_err(Self::failed)?;
        // Cache write is best-effort; failure already warned with the path.
        let mut entries = BTreeMap::new();
        for f in &out.files {
            entries.insert(
                f.rel_path.clone(),
                FileFingerprint {
                    size_bytes: f.size_bytes,
                    mtime: f.mtime,
                    content_hash: f.content_hash.clone(),
                },
            );
        }
        let _ = FingerprintCache::new(self.paths.scan_cache.clone()).put_all(&entries);
        // The snapshot is NOT released: burn re-derives `<work>/snapshot`;
        // only `reset` (P6) removes it.
        let ScanOutput { files, hashed } = out;
        Ok((files, hashed))
    }

    fn count(
        &self,
        p: &Project,
        files: &[ScannedFile],
        hashed: Option<usize>,
    ) -> Result<ScanReport, PipelineError> {
        let view = self.catalog.read_view(p.id).map_err(Self::failed)?;
        let cs = diff::diff(&view, files);
        Ok(ScanReport {
            added: cs.added.len(),
            modified: cs.modified.len(),
            deleted: cs.deleted.len(),
            unchanged: cs.unchanged.len(),
            hashed,
            scanned_files: files.len(),
        })
    }

    /// `status`: catalog reads only, never mutates (`PipelineLive.status`).
    pub fn status(&self, name: &str) -> Result<StatusReport, PipelineError> {
        let p = self.project(name)?;
        let discs = self.catalog.read_disc_states(p.id).map_err(Self::failed)?;
        let view = self.catalog.read_view(p.id).map_err(Self::failed)?;
        let burned = self
            .catalog
            .sessions_by_state(p.id, SessionState::Burned)
            .map_err(Self::failed)?;
        let verified = self
            .catalog
            .sessions_by_state(p.id, SessionState::Verified)
            .map_err(Self::failed)?;
        let last_burn_at = burned
            .iter()
            .chain(verified.iter())
            .filter_map(|s| s.burned_at)
            .max();
        Ok(StatusReport {
            open_disc_ordinal: discs
                .iter()
                .find(|d| d.status == DiscStatus::Open)
                .map(|d| d.ordinal),
            total_current_files: view.current.len(),
            discs,
            last_burn_at,
        })
    }
}

/// `getFileStore` equality via `st_dev` (`PipelineLive.sameFilesystem`): both
/// paths must exist and carry the same device id.
fn same_filesystem(source: &Path, work: &Path) -> std::io::Result<(u64, u64)> {
    Ok((std::fs::metadata(source)?.dev(), std::fs::metadata(work)?.dev()))
}
```

**Writing notes:** confirm `SessionState`, `Project`, `FileFingerprint` paths
against `catalog/model.rs`; if `DiscStatus` comparison needs `PartialEq`, it
derives it. If `catalog::testutil` visibility blocks a unit-test import, copy
the `ts()` helper into the test module instead of widening visibility.

- [ ] **Step 4: Pass, trio, commit**

Run: `cargo test -p diskrement-shell pipeline` — 11 passed. Trio clean.

```bash
git add crates/diskrement-shell/src/pipeline/mod.rs
git commit -m "feat(pipeline): init, scan and status"
```

---

### Task 10: CLI path resolution

**Files:**
- Create: `crates/diskrement-cli/src/paths.rs`
- Modify: `crates/diskrement-cli/src/main.rs` (declare `mod paths;` at top)
- Oracle: `PathResolver.scala`, `PathResolverSpec.scala`

**Interfaces:**
- Produces:
  `pub struct ResolvedPaths { pub catalog: PathBuf, pub data_dir: PathBuf }`;
  `pub fn resolve(catalog: Option<&Path>, data_dir: Option<&Path>, project: &str, get_env: &dyn Fn(&str) -> Option<String>, home: &Path) -> ResolvedPaths`;
  `pub fn scan_cache_path(catalog: &Path, project: &str) -> PathBuf`.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn envmap(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + '_ {
        let map: HashMap<String, String> =
            vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |k: &str| map.get(k).cloned()
    }

    #[test]
    fn defaults_to_home_and_names_the_catalog_after_the_project() {
        let r = resolve(None, None, "photos", &envmap(&[]), Path::new("/home/u"));
        assert_eq!(
            r.catalog,
            PathBuf::from("/home/u/.local/share/diskrement/photos.sqlite")
        );
        assert_eq!(r.data_dir, PathBuf::from("/home/u/.local/share/diskrement"));
    }

    #[test]
    fn xdg_data_home_wins_over_the_default() {
        let r = resolve(
            None,
            None,
            "photos",
            &envmap(&[("XDG_DATA_HOME", "/xdg")]),
            Path::new("/home/u"),
        );
        assert_eq!(r.catalog, PathBuf::from("/xdg/diskrement/photos.sqlite"));
    }

    #[test]
    fn state_directory_beats_xdg_and_takes_the_first_colon_entry() {
        let r = resolve(
            None,
            None,
            "photos",
            &envmap(&[("XDG_DATA_HOME", "/xdg"), ("STATE_DIRECTORY", "/state/a:/state/b")]),
            Path::new("/home/u"),
        );
        assert_eq!(r.data_dir, PathBuf::from("/state/a"));
    }

    #[test]
    fn data_dir_flag_beats_the_environment() {
        let r = resolve(
            None,
            Some(Path::new("/data")),
            "photos",
            &envmap(&[("STATE_DIRECTORY", "/state")]),
            Path::new("/home/u"),
        );
        assert_eq!(r.data_dir, PathBuf::from("/data"));
        assert_eq!(r.catalog, PathBuf::from("/data/photos.sqlite"));
    }

    #[test]
    fn empty_env_values_are_treated_as_unset() {
        let r = resolve(
            None,
            None,
            "photos",
            &envmap(&[("STATE_DIRECTORY", ""), ("XDG_DATA_HOME", "")]),
            Path::new("/home/u"),
        );
        assert_eq!(r.data_dir, PathBuf::from("/home/u/.local/share/diskrement"));
    }

    #[test]
    fn catalog_flag_is_used_directly_and_its_parent_becomes_the_data_dir() {
        let r = resolve(
            Some(Path::new("/data/cat/proj.sqlite")),
            None,
            "p",
            &envmap(&[]),
            Path::new("/home/u"),
        );
        assert_eq!(r.catalog, PathBuf::from("/data/cat/proj.sqlite"));
        assert_eq!(r.data_dir, PathBuf::from("/data/cat"));
    }

    #[test]
    fn a_relative_catalog_flag_is_normalized_against_the_cwd() {
        let cwd = std::env::current_dir().unwrap();
        let r = resolve(
            Some(Path::new("rel/cat.sqlite")),
            None,
            "p",
            &envmap(&[]),
            Path::new("/home/u"),
        );
        assert_eq!(r.catalog, cwd.join("rel/cat.sqlite"));
        assert_eq!(r.data_dir, cwd.join("rel"));
    }

    #[test]
    fn the_scancache_sibling_follows_stem_dot_project() {
        assert_eq!(
            scan_cache_path(Path::new("/data/cat/proj.sqlite"), "p"),
            PathBuf::from("/data/cat/proj.p.scancache.sqlite")
        );
        assert_eq!(
            scan_cache_path(Path::new("/home/u/.local/share/diskrement/p.sqlite"), "p"),
            PathBuf::from("/home/u/.local/share/diskrement/p.p.scancache.sqlite")
        );
        // two projects sharing a catalog file get distinct caches
        assert_ne!(
            scan_cache_path(Path::new("/c/proj.sqlite"), "a"),
            scan_cache_path(Path::new("/c/proj.sqlite"), "b")
        );
    }
}
```

- [ ] **Step 2: Run and watch it fail**, then **Step 3: implement**

```rust
//! Catalog/data-dir resolution (`PathResolver.scala`): `--catalog` >
//! `--data-dir` > `$STATE_DIRECTORY` (first `:`-entry, set-but-empty = unset)
//! > `$XDG_DATA_HOME/diskrement` (empty = unset) >
//! `$HOME/.local/share/diskrement`. The default catalog file name embeds the
//! project name; the fingerprint sidecar is
//! `<catalog-stem>.<project>.scancache.sqlite` beside it.

use std::path::{Path, PathBuf};

pub struct ResolvedPaths {
    pub catalog: PathBuf,
    pub data_dir: PathBuf,
}

/// `toAbsolutePath`: relatives resolve against the cwd, nothing more.
fn absolute(p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(p)
    }
}

pub fn resolve(
    catalog: Option<&Path>,
    data_dir: Option<&Path>,
    project: &str,
    get_env: &dyn Fn(&str) -> Option<String>,
    home: &Path,
) -> ResolvedPaths {
    if let Some(c) = catalog {
        let catalog = absolute(c);
        let data_dir = catalog
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        return ResolvedPaths { catalog, data_dir };
    }
    let data_dir = data_dir
        .map(absolute)
        .or_else(|| {
            get_env("STATE_DIRECTORY")
                .filter(|s| !s.is_empty())
                .and_then(|s| s.split(':').next().map(str::to_string))
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
        })
        .or_else(|| {
            get_env("XDG_DATA_HOME")
                .filter(|s| !s.is_empty())
                .map(|s| PathBuf::from(s).join("diskrement"))
        })
        .unwrap_or_else(|| home.join(".local/share/diskrement"));
    ResolvedPaths {
        catalog: data_dir.join(format!("{project}.sqlite")),
        data_dir,
    }
}

pub fn scan_cache_path(catalog: &Path, project: &str) -> PathBuf {
    let name = catalog
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = name.strip_suffix(".sqlite").unwrap_or(&name);
    catalog.with_file_name(format!("{stem}.{project}.scancache.sqlite"))
}
```

- [ ] **Step 4: Pass, trio, commit**

```bash
git add crates/diskrement-cli/src/paths.rs crates/diskrement-cli/src/main.rs
git commit -m "feat(cli): catalog path precedence and the scancache sibling rule"
```

---

### Task 11: The argument surface

**Files:**
- Create: `crates/diskrement-cli/src/args.rs`
- Modify: `crates/diskrement-cli/src/main.rs` (declare `mod args;`)
- Oracle: `Args.scala`, `Commands.scala`, `CommandsSpec.scala`

**Interfaces:**
- Produces: `pub struct Cli` (clap `Parser`) with global `--catalog`,
  `--data-dir`, `--device`, `--burner`, and
  `#[command(subcommand)] pub command: Command`;
  `pub enum Command { Init(InitArgs), Scan(ScanArgs), Status(StatusArgs) }` with
  `InitArgs { name: String, source: PathBuf, work_dir: PathBuf, disc_type: DiscType, margin: f64, copies: u32 }`,
  `ScanArgs { name: String, verify: bool, force: bool }`,
  `StatusArgs { name: String }`.
  Only the P3 subcommands exist now; later phases append variants. Task 14
  consumes this from `main`.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use diskrement_core::DiscType;

    #[test]
    fn init_with_defaults() {
        let cli = Cli::try_parse_from([
            "diskrement", "init", "photos", "--source", "/srv/p", "--work-dir", "/srv/w",
        ])
        .unwrap();
        let Command::Init(a) = cli.command else { panic!("not init") };
        assert_eq!(a.name, "photos");
        assert_eq!(a.source, PathBuf::from("/srv/p"));
        assert_eq!(a.work_dir, PathBuf::from("/srv/w"));
        assert_eq!(a.disc_type, DiscType::BdR);
        assert_eq!(a.margin, 2.0);
        assert_eq!(a.copies, 1);
    }

    #[test]
    fn init_with_explicit_disc_type_margin_copies() {
        let cli = Cli::try_parse_from([
            "diskrement", "init", "photos", "--source", "/s", "--work-dir", "/w",
            "--disc-type", "BD_R_DL", "--margin", "5.0", "--copies", "2",
        ])
        .unwrap();
        let Command::Init(a) = cli.command else { panic!("not init") };
        assert_eq!(a.disc_type, DiscType::BdRDl);
        assert_eq!(a.margin, 5.0);
        assert_eq!(a.copies, 2);
    }

    #[test]
    fn scan_flags_default_off_and_parse_on() {
        let off = Cli::try_parse_from(["diskrement", "scan", "photos"]).unwrap();
        let Command::Scan(a) = off.command else { panic!("not scan") };
        assert!(!a.verify && !a.force);
        let on =
            Cli::try_parse_from(["diskrement", "scan", "photos", "--verify", "--force"]).unwrap();
        let Command::Scan(a) = on.command else { panic!("not scan") };
        assert!(a.verify && a.force);
    }

    #[test]
    fn status_takes_only_a_name() {
        let cli = Cli::try_parse_from(["diskrement", "status", "photos"]).unwrap();
        assert!(matches!(cli.command, Command::Status(_)));
    }

    #[test]
    fn the_global_catalog_flag_is_captured_before_or_after_the_subcommand() {
        let before =
            Cli::try_parse_from(["diskrement", "--catalog", "/c/x.sqlite", "status", "p"]).unwrap();
        assert_eq!(before.catalog, Some(PathBuf::from("/c/x.sqlite")));
        let after =
            Cli::try_parse_from(["diskrement", "status", "p", "--catalog", "/c/x.sqlite"]).unwrap();
        assert_eq!(after.catalog, Some(PathBuf::from("/c/x.sqlite")));
    }

    #[test]
    fn an_unknown_disc_type_is_rejected() {
        assert!(Cli::try_parse_from([
            "diskrement", "init", "p", "--source", "/s", "--work-dir", "/w",
            "--disc-type", "ZIP",
        ])
        .is_err());
    }

    #[test]
    fn an_unknown_subcommand_is_rejected() {
        assert!(Cli::try_parse_from(["diskrement", "frobnicate", "p"]).is_err());
    }
}
```

- [ ] **Step 2: Run and watch it fail**

Run: `cargo test -p diskrement-cli args`
Expected: FAIL — `Cli` not found.

- [ ] **Step 3: Implement**

```rust
//! The clap surface (`Commands.scala`). Wording of clap's usage and argument
//! errors may drift from zio-cli — the one sanctioned divergence (spec §3.2);
//! the difftest harness declares such steps `Expect::ExitOnly`.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use diskrement_core::DiscType;

#[derive(Parser, Debug)]
#[command(name = "diskrement", version, about = "Archival backup to optical discs")]
pub struct Cli {
    #[arg(long, global = true)]
    pub catalog: Option<PathBuf>,
    #[arg(long, global = true)]
    pub data_dir: Option<PathBuf>,
    #[arg(long, global = true)]
    pub device: Option<String>,
    #[arg(long, global = true)]
    pub burner: Option<String>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Create a project and run filesystem checks.
    Init {
        name: String,
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        work_dir: PathBuf,
        #[arg(long, default_value = "BD_R")]
        disc_type: DiscType,
        #[arg(long, default_value_t = 2.0)]
        margin: f64,
        #[arg(long, default_value_t = 1)]
        copies: u32,
    },
    /// Snapshot the source and stage a diff.
    Scan {
        name: String,
        #[arg(long, default_value_t = false)]
        verify: bool,
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Show discs, totals, and last burn.
    Status { name: String },
}
```

**Writing notes:** `DiscType` already implements `FromStr` with
`Err = String` and `Display`, which is exactly what clap needs for
`default_value = "BD_R"`. `--burner` is declared now (the difftest runner sets
it) but its value is unused until P8's backend resolution. **Exit codes:**
`main` must use `Cli::try_parse` (not `parse()`) and map argument errors to
exit **1** — clap's default is 2, which the contract reserves for
state/lookup; zio-cli used 1 (Task 14 wires this).

- [ ] **Step 4: Pass, trio, commit**

```bash
git add crates/diskrement-cli/src/args.rs crates/diskrement-cli/src/main.rs
git commit -m "feat(cli): the init/scan/status argument surface"
```

---

### Task 12: Render + exit mapping

**Files:**
- Create: `crates/diskrement-cli/src/render.rs`
- Modify: `crates/diskrement-cli/src/main.rs` (declare `mod render;`)
- Oracle: `Render.scala` (`init`, `scan`, `status`, `handle`), `RenderSpec.scala`, `ExitMappingSpec.scala`

**Interfaces:**
- Consumes: `ScanReport`, `StatusReport` (`pipeline::reports`), `Project`
  (`catalog::model`), `ProjectId`.
- Produces:
  `pub fn init_done(name: &str, id: ProjectId) -> String`;
  `pub fn scan_report(r: &ScanReport) -> String`;
  `pub fn status_report(p: &Project, r: &StatusReport) -> String`;
  `pub fn gb(bytes: u64) -> String`.
  Error text is `PipelineError`'s `Display` (contract, Task 8); Task 14 prints
  it to stderr and uses `exit_code()`.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use diskrement_core::{DiscState, DiscStatus, DiscType};

    fn ts(y: i32, mo: u32, d: u32) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc.with_ymd_and_hms(y, mo, d, 14, 3, 0).unwrap()
    }

    fn project() -> Project {
        Project {
            id: ProjectId(1),
            name: "photos".into(),
            source_root: "/srv/photos".into(),
            work_dir: "/srv/work".into(),
            default_disc_type: DiscType::BdR,
            margin_percent: 2.0,
            copies: 1,
            created_at: ts(2026, 1, 1),
        }
    }

    fn report() -> StatusReport {
        StatusReport {
            discs: vec![
                DiscState {
                    ordinal: 1,
                    status: DiscStatus::Finalized,
                    usable_bytes: 25_000_000_000,
                    used_bytes: 24_800_000_000,
                },
                DiscState {
                    ordinal: 2,
                    status: DiscStatus::Open,
                    usable_bytes: 25_000_000_000,
                    used_bytes: 3_200_000_000,
                },
            ],
            open_disc_ordinal: Some(2),
            total_current_files: 556,
            last_burn_at: Some(ts(2026, 6, 21)),
        }
    }

    #[test]
    fn the_init_confirmation_line_is_exact() {
        assert_eq!(init_done("photos", ProjectId(7)), "Created project photos (id 7).");
    }

    #[test]
    fn a_fresh_scan_reports_hashed_and_reused() {
        let r = ScanReport {
            added: 2,
            modified: 1,
            deleted: 0,
            unchanged: 3,
            hashed: Some(3),
            scanned_files: 6,
        };
        assert_eq!(
            scan_report(&r),
            "Scanned: 2 added, 1 modified, 0 deleted, 3 unchanged.\n\
             Hashed 3 of 6 files (3 fingerprints reused)."
        );
    }

    #[test]
    fn a_reused_staged_scan_omits_the_hashed_line() {
        let r = ScanReport {
            added: 2,
            modified: 1,
            deleted: 0,
            unchanged: 3,
            hashed: None,
            scanned_files: 6,
        };
        assert_eq!(scan_report(&r), "Scanned: 2 added, 1 modified, 0 deleted, 3 unchanged.");
    }

    #[test]
    fn the_status_table_has_the_exact_layout() {
        let out = status_report(&project(), &report());
        assert!(
            out.contains("Project photos  (source /srv/photos, BD_R, margin 2.0%)"),
            "{out}"
        );
        assert!(out.contains("Discs:"), "{out}");
        assert!(out.contains("  #1   FINALIZED  24.8 GB / 25.0 GB"), "{out}");
        assert!(out.contains("  #2   OPEN       3.2 GB / 25.0 GB  <- open"), "{out}");
        assert!(
            out.contains("Current files: 556      Last burn: 2026-06-21T14:03:00Z"),
            "{out}"
        );
    }

    #[test]
    fn never_is_printed_when_nothing_was_burned() {
        let mut r = report();
        r.last_burn_at = None;
        r.discs.clear();
        let out = status_report(&project(), &r);
        assert!(out.contains("Current files: 556      Last burn: never"), "{out}");
    }

    #[test]
    fn gb_formats_one_decimal() {
        assert_eq!(gb(24_800_000_000), "24.8 GB");
        assert_eq!(gb(25_000_000_000), "25.0 GB");
        assert_eq!(gb(0), "0.0 GB");
    }
}
```

- [ ] **Step 2: Run and watch it fail**

Run: `cargo test -p diskrement-cli render`
Expected: FAIL — functions missing.

- [ ] **Step 3: Implement**

```rust
//! Output rendering (`Render.scala`). Success text goes to stdout; error text
//! is `PipelineError`'s `Display` and goes to stderr with its exit code.

use diskrement_core::ProjectId;
use diskrement_shell::catalog::model::Project;
use diskrement_shell::catalog::time;
use diskrement_shell::pipeline::reports::{ScanReport, StatusReport};

pub fn init_done(name: &str, id: ProjectId) -> String {
    format!("Created project {name} (id {}).", id.0)
}

/// `Scanned: …`, plus the `Hashed N of M files` line for a fresh scan only.
pub fn scan_report(r: &ScanReport) -> String {
    let summary = format!(
        "Scanned: {} added, {} modified, {} deleted, {} unchanged.",
        r.added, r.modified, r.deleted, r.unchanged
    );
    match r.hashed {
        Some(h) => format!(
            "{summary}\nHashed {h} of {} files ({} fingerprints reused).",
            r.scanned_files,
            r.scanned_files - h
        ),
        None => summary,
    }
}

/// `f"${bytes / 1.0e9}%.1f GB"` (`Render.gb`).
pub fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1.0e9)
}

pub fn status_report(p: &Project, r: &StatusReport) -> String {
    // Two spaces after the project name; margin is %.1f plus a literal %.
    let mut out = format!(
        "Project {}  (source {}, {}, margin {:.1}%)\n",
        p.name, p.source_root, p.default_disc_type, p.margin_percent
    );
    out.push_str("Discs:\n");
    for d in &r.discs {
        let open = if r.open_disc_ordinal == Some(d.ordinal) { "  <- open" } else { "" };
        out.push_str(&format!(
            "  #{:<3} {:<10} {} / {}{}\n",
            d.ordinal,
            d.status.as_str(),
            gb(d.used_bytes),
            gb(d.usable_bytes),
            open
        ));
    }
    let last = r
        .last_burn_at
        .as_ref()
        .map(time::format)
        .unwrap_or_else(|| "never".to_string());
    // Six spaces between the count and "Last burn:"; no trailing newline —
    // `main` prints with println!.
    out.push_str(&format!(
        "Current files: {}      Last burn: {}",
        r.total_current_files, last
    ));
    out
}
```

**Writing note:** `DiscStatus::as_str()`/`DiscType::as_str()` return the
frozen spellings (`OPEN`/`FINALIZED`/`DEAD`, `BD_R`/…) — confirmed in
`crates/diskrement-core`. Do not rely on `Display` for these in stored-value
contexts; in render output either is acceptable if the frozen spelling is what
appears (verify with the test above).

- [ ] **Step 4: Pass, trio, commit**

```bash
git add crates/diskrement-cli/src/render.rs crates/diskrement-cli/src/main.rs
git commit -m "feat(cli): init/scan/status rendering"
```

---

### Task 13: The progress reporter

**Files:**
- Create: `crates/diskrement-cli/src/progress.rs`
- Modify: `crates/diskrement-cli/src/main.rs` (declare `mod progress;`)
- Oracle: `ConsoleProgress.scala`, `ConsoleProgressSpec.scala`

**Interfaces:**
- Produces:
  `pub fn stderr_is_tty() -> bool`;
  `pub fn format(label: &str, done_bytes: u64, total_bytes: u64, done_files: usize, total_files: usize, elapsed: std::time::Duration) -> String`;
  `pub struct ConsoleProgress { … }` implementing
  `diskrement_shell::progress::ProgressSink` (consumed by Task 14).

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_line_format_is_the_pinned_shape() {
        // Inputs chosen so every component is exact: rate = 12.3e9 /
        // 14_642_857_142 ns = exactly 840.0 MB/s; ETA = 401.3e9 / 840e6 =
        // 477.7 s → `7m57s`.
        assert_eq!(
            format(
                "hashing",
                12_300_000_000,
                413_600_000_000,
                3_021,
                75_618,
                Duration::from_nanos(14_642_857_142)
            ),
            "hashing 12.3/413.6 GB · 3021/75618 files · 840 MB/s · ETA 7m57s"
        );
    }

    #[test]
    fn zero_totals_render_the_parked_string() {
        assert_eq!(
            format("hashing", 0, 0, 0, 0, Duration::from_secs(0)),
            "hashing 0.0/0.0 GB · 0/0 files · 0 MB/s · ETA --"
        );
    }

    #[test]
    fn the_eta_uses_minutes_and_padded_seconds() {
        // rate = 999_000 B/s -> "1 MB/s"; remaining = 999_000 B -> ETA 1.0s
        // -> "0m01s".
        assert_eq!(
            format("hashing", 999_000, 1_998_000, 1, 1, Duration::from_secs(1)),
            "hashing 0.0/0.0 GB · 1/1 files · 1 MB/s · ETA 0m01s"
        );
    }
}
```

**Derivation, so nobody re-derives it wrong:** rate = done/elapsed; when
elapsed is zero or done is zero, rate is 0 and the ETA is `ETA --`. ETA
seconds = remaining/rate, rendered `{}m{:02}s` from integer division. The
format — `label X.X/Y.Y GB · N/M files · R MB/s · ETA …` — is the contract
(pinned by `ConsoleProgressSpec`); the digits above are computed from these
exact inputs, not copied from the Scala spec's example.

- [ ] **Step 2: Run and watch it fail**

Run: `cargo test -p diskrement-cli progress`
Expected: FAIL — `format` not found.

- [ ] **Step 3: Implement**

```rust
//! The stderr progress reporter (`ConsoleProgress.scala`). Hand-rolled rather
//! than `indicatif`: CLI compatibility makes matching the existing behavior
//! worth more than the dependency saves (spec §5.5).
//!
//! Cadence: 1 s on a TTY, 10 s otherwise. Emission happens inside `advance`
//! (no background thread); `done` emits a final snapshot only if something
//! was already emitted. TTY frames are `"\r" + line + ESC[K` (erase to end of
//! line so a shrinking line leaves no residue); non-TTY frames are plain
//! newline-terminated lines so logs stay parseable.

use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use diskrement_shell::progress::ProgressSink;

pub fn stderr_is_tty() -> bool {
    // TERM=dumb explicitly disables (ConsoleProgress.stderrIsTty).
    if std::env::var("TERM").as_deref() == Ok("dumb") {
        return false;
    }
    std::io::stderr().is_terminal()
}

/// The pinned line shape (`ConsoleProgress.format`):
/// `f"$label ${done/1e9}%.1f/${total/1e9}%.1f GB · $files · ${rate}%.0f MB/s · $eta"`.
pub fn format(
    label: &str,
    done_bytes: u64,
    total_bytes: u64,
    done_files: usize,
    total_files: usize,
    elapsed: Duration,
) -> String {
    let rate = if elapsed.as_secs_f64() > 0.0 {
        done_bytes as f64 / elapsed.as_secs_f64()
    } else {
        0.0
    };
    let eta_seconds = if rate > 0.0 {
        ((total_bytes.saturating_sub(done_bytes)) as f64 / rate) as i64
    } else {
        -1
    };
    let eta = if eta_seconds < 0 {
        "ETA --".to_string()
    } else {
        format!("ETA {}m{:02}s", eta_seconds / 60, eta_seconds % 60)
    };
    format!(
        "{label} {:.1}/{:.1} GB · {}/{} files · {:.0} MB/s · {}",
        done_bytes as f64 / 1.0e9,
        total_bytes as f64 / 1.0e9,
        done_files,
        total_files,
        rate / 1.0e6,
        eta
    )
}

struct State {
    started: Instant,
    last_emit: Option<Instant>,
    done_bytes: u64,
    done_files: usize,
}

pub struct ConsoleProgress {
    label: Mutex<String>,
    total_bytes: AtomicU64,
    total_files: AtomicU64,
    done_bytes: AtomicU64,
    done_files: AtomicU64,
    started: Mutex<Option<Instant>>,
    last_emit: Mutex<Option<Instant>>,
    emitted: AtomicBool,
    tty: bool,
}

impl ConsoleProgress {
    pub fn new() -> Self {
        ConsoleProgress {
            label: Mutex::new(String::new()),
            total_bytes: AtomicU64::new(0),
            total_files: AtomicU64::new(0),
            done_bytes: AtomicU64::new(0),
            done_files: AtomicU64::new(0),
            started: Mutex::new(None),
            last_emit: Mutex::new(None),
            emitted: AtomicBool::new(false),
            tty: stderr_is_tty(),
        }
    }

    fn interval(&self) -> Duration {
        if self.tty {
            Duration::from_secs(1)
        } else {
            Duration::from_secs(10)
        }
    }

    fn snapshot(&self, elapsed: Duration) -> String {
        format(
            &self.label.lock().unwrap(),
            self.done_bytes.load(Ordering::Relaxed),
            self.total_bytes.load(Ordering::Relaxed),
            self.done_files.load(Ordering::Relaxed),
            self.total_files.load(Ordering::Relaxed) as usize,
            elapsed,
        )
    }

    fn emit(&self) {
        let Some(started) = *self.started.lock().unwrap() else { return };
        let line = self.snapshot(started.elapsed());
        let mut err = std::io::stderr().lock();
        if self.tty {
            let _ = write!(err, "\r{line}\x1b[K");
            let _ = err.flush();
        } else {
            let _ = writeln!(err, "{line}");
        }
        *self.last_emit.lock().unwrap() = Some(Instant::now());
        self.emitted.store(true, Ordering::Relaxed);
    }
}

impl Default for ConsoleProgress {
    fn default() -> Self {
        Self::new()
    }
}

impl ProgressSink for ConsoleProgress {
    fn start(&self, label: &str, total_bytes: u64, total_files: usize) {
        *self.label.lock().unwrap() = label.to_string();
        self.total_bytes.store(total_bytes, Ordering::Relaxed);
        self.total_files.store(total_files as u64, Ordering::Relaxed);
        self.done_bytes.store(0, Ordering::Relaxed);
        self.done_files.store(0, Ordering::Relaxed);
        *self.started.lock().unwrap() = Some(Instant::now());
        *self.last_emit.lock().unwrap() = None;
        self.emitted.store(false, Ordering::Relaxed);
    }

    fn advance(&self, delta_bytes: u64, delta_files: usize) {
        self.done_bytes.fetch_add(delta_bytes, Ordering::Relaxed);
        self.done_files.fetch_add(delta_files as u64, Ordering::Relaxed);
        let due = match *self.last_emit.lock().unwrap() {
            Some(last) => last.elapsed() >= self.interval(),
            None => match *self.started.lock().unwrap() {
                Some(started) => started.elapsed() >= self.interval(),
                None => false,
            },
        };
        if due {
            self.emit();
        }
    }

    fn done(&self) {
        // A final snapshot only if something was already emitted (done is
        // silent when the phase never crossed the first interval).
        if self.emitted.load(Ordering::Relaxed) {
            self.emit();
            if self.tty {
                let mut err = std::io::stderr().lock();
                let _ = writeln!(err);
            }
        }
    }
}
```

- [ ] **Step 4: Pass, trio, commit**

```bash
git add crates/diskrement-cli/src/progress.rs crates/diskrement-cli/src/main.rs
git commit -m "feat(cli): the stderr progress reporter"
```

---

### Task 14: Wire the binary

**Files:**
- Modify: `crates/diskrement-cli/src/main.rs` (replace the stub)
- Modify: `crates/diskrement-cli/tests/binary.rs` (extend)
- Oracle: `Main.scala`, `Runner.scala`, `Render.handle` (error → stderr + exit code)

**Interfaces:**
- Consumes: everything Tasks 1–13 produced.
- Produces: the `diskrement` binary with `init`/`scan`/`status`, `--version`,
  the path precedence, and the exit ladder for P3 errors.

- [ ] **Step 1: Write the failing integration tests** (extend
  `crates/diskrement-cli/tests/binary.rs`; keep the existing `--version` test)

```rust
mod p3 {
    use assert_cmd::Command;
    use predicates::str::contains;

    fn cmd() -> Command {
        Command::cargo_bin("diskrement").expect("binary builds")
    }

    /// The happy path, end to end, in a temp dir: init → scan → status.
    #[test]
    fn init_scan_status_round_trip() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src/sub")).unwrap();
        std::fs::write(root.join("src/a.txt"), b"hello").unwrap();
        std::fs::write(root.join("src/sub/b.txt"), b"world").unwrap();
        let catalog = root.join("catalog/cat.sqlite");
        std::fs::create_dir_all(catalog.parent().unwrap()).unwrap();
        let work = root.join("work");
        let base = |catalog: &std::path::Path| {
            let mut c = cmd();
            c.arg("--catalog").arg(catalog);
            c
        };

        base(&catalog)
            .args(["init", "p", "--source"])
            .arg(root.join("src"))
            .arg("--work-dir")
            .arg(&work)
            .assert()
            .success()
            .stdout(contains("Created project p (id 1)."));

        base(&catalog)
            .args(["scan", "p"])
            .env("DISKREMENT_HASH_PARALLELISM", "2")
            .assert()
            .success()
            .stdout(contains("Scanned: 2 added, 0 modified, 0 deleted, 0 unchanged."))
            .stdout(contains("Hashed 2 of 2 files (0 fingerprints reused)."));

        base(&catalog).args(["status", "p"]).assert().success().stdout(
            contains("Project p  (source")
                .and(contains("Discs:"))
                .and(contains("Current files: 0      Last burn: never")),
        );

        // The staged scan and the sidecar exist where the contract says.
        assert!(work.join("staged/scan.json").is_file());
        assert!(root.join("catalog/cat.p.scancache.sqlite").is_file());

        // A second scan reuses the staged scan: no Hashed line.
        base(&catalog)
            .args(["scan", "p"])
            .assert()
            .success()
            .stdout(contains("Scanned: 2 added, 0 modified, 0 deleted, 0 unchanged."))
            .stdout(contains("Hashed").not());
    }

    #[test]
    fn an_unknown_project_exits_2_with_the_contract_message() {
        let dir = tempfile::TempDir::new().unwrap();
        let catalog = dir.path().join("cat.sqlite");
        // The catalog does not exist yet; the binary must create it (empty)
        // and report the lookup failure, like the Scala Runner does.
        cmd()
            .arg("--catalog")
            .arg(&catalog)
            .args(["status", "nosuch"])
            .assert()
            .failure()
            .code(2)
            .stderr(contains("project not found: nosuch"));
        assert!(catalog.is_file(), "the binary creates an empty catalog");
    }

    #[test]
    fn scan_on_an_unknown_project_also_exits_2() {
        let dir = tempfile::TempDir::new().unwrap();
        cmd()
            .arg("--catalog")
            .arg(dir.path().join("cat.sqlite"))
            .args(["scan", "nosuch"])
            .assert()
            .failure()
            .code(2)
            .stderr(contains("project not found: nosuch"));
    }

    #[test]
    fn an_argument_parse_error_exits_nonzero_with_clap_wording() {
        cmd()
            .args(["scan"])
            .assert()
            .failure()
            .code(1) // the contract's argument-error exit; clap wording may drift (spec §3.2)
            .stderr(contains("error:"));
    }
}
```

- [ ] **Step 2: Run and watch it fail**

Run: `cargo test -p diskrement-cli --test binary`
Expected: FAIL — the binary does not parse subcommands yet.

- [ ] **Step 3: Implement `main.rs`**

Replace `crates/diskrement-cli/src/main.rs`:

```rust
//! The diskrement binary: resolve paths, dispatch, render, map errors to the
//! exit ladder. Path resolution and rendering live here; the pipeline returns
//! data and performs no path resolution (CLAUDE.md invariant).

mod args;
mod paths;
mod progress;
mod render;

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use args::Command;
use diskrement_core::ProjectInit;
use diskrement_shell::catalog::time;
use diskrement_shell::catalog::{model::Project, Catalog};
use diskrement_shell::error::PipelineError;
use diskrement_shell::pipeline::{staged, Pipeline, PipelinePaths};
use diskrement_shell::progress::{NoopSink, ProgressSink};

fn main() -> ExitCode {
    // try_parse, never parse(): clap exits 2 on usage errors by default, but
    // the contract's 2 means state/lookup — zio-cli used 1 for argument
    // errors (Main.scala's comment), so the port maps them to 1 itself. Only
    // the *wording* of clap's message may drift (spec §3.2).
    match args::Cli::try_parse() {
        Ok(cli) => match dispatch(cli) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("{err}");
                ExitCode::from(err.exit_code() as u8)
            }
        },
        Err(e) => {
            let _ = e.print(); // help/version → stdout, errors → stderr
            ExitCode::from(if e.use_stderr() { 1 } else { 0 })
        }
    }
}

fn env_lookup(k: &str) -> Option<String> {
    std::env::var(k).ok()
}

fn home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}

fn open_catalog(path: &std::path::Path) -> Result<Catalog, PipelineError> {
    Catalog::open(path).map_err(|e| PipelineError::Failed(e.to_string()))
}

/// Print one blob of success output.
fn say(text: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{text}");
}

fn dispatch(cli: args::Cli) -> Result<(), PipelineError> {
    // The project name the default catalog/data-dir resolution needs.
    let project_name = match &cli.command {
        Command::Init(a) => &a.name,
        Command::Scan(a) => &a.name,
        Command::Status(a) => &a.name,
    };
    let resolved = paths::resolve(
        cli.catalog.as_deref(),
        cli.data_dir.as_deref(),
        project_name,
        &env_lookup,
        &home(),
    );
    std::fs::create_dir_all(&resolved.data_dir)
        .map_err(|e| PipelineError::Failed(e.to_string()))?;

    match cli.command {
        Command::Init(a) => {
            // The work dir exists before anything else (Runner.dispatch).
            std::fs::create_dir_all(&a.work_dir)
                .map_err(|e| PipelineError::Failed(e.to_string()))?;
            let mut catalog = open_catalog(&resolved.catalog)?;
            let scan_cache = paths::scan_cache_path(&resolved.catalog, &a.name);
            let sink = progress::ConsoleProgress::new();
            let mut pipeline = Pipeline::new(
                &mut catalog,
                PipelinePaths { work_dir: a.work_dir.clone(), scan_cache },
                &sink,
            );
            let pi = ProjectInit {
                name: a.name.clone(),
                source_root: a.source.display().to_string(),
                work_dir: a.work_dir.display().to_string(),
                default_disc_type: a.disc_type,
                margin_percent: a.margin,
                copies: a.copies,
                created_at: chrono::Utc::now(),
            };
            let id = pipeline.init(&pi)?;
            say(&render::init_done(&a.name, id));
            Ok(())
        }
        Command::Scan(a) => {
            let mut catalog = open_catalog(&resolved.catalog)?;
            let p = find_or_fail(&catalog, &a.name)?;
            let scan_cache = paths::scan_cache_path(&resolved.catalog, &a.name);
            let sink = progress::ConsoleProgress::new();
            let mut pipeline = Pipeline::new(
                &mut catalog,
                PipelinePaths {
                    work_dir: PathBuf::from(&p.work_dir),
                    scan_cache,
                },
                &sink,
            );
            let report = pipeline.scan(&a.name, a.verify, a.force)?;
            say(&render::scan_report(&report));
            Ok(())
        }
        Command::Status(a) => {
            let mut catalog = open_catalog(&resolved.catalog)?;
            let p = find_or_fail(&catalog, &a.name)?;
            let pipeline = Pipeline::new(
                &mut catalog,
                PipelinePaths {
                    work_dir: PathBuf::from(&p.work_dir),
                    scan_cache: paths::scan_cache_path(&resolved.catalog, &a.name),
                },
                &NoopSink,
            );
            let report = pipeline.status(&a.name)?;
            say(&render::status_report(&p, &report));
            Ok(())
        }
    }
}

/// `Runner.loadProject`: resolve the project row first so the pipeline's work
/// dir comes from the catalog, and produce the contract exit-2 on a miss.
fn find_or_fail(catalog: &Catalog, name: &str) -> Result<Project, PipelineError> {
    catalog
        .find_project(name)
        .map_err(|e| PipelineError::Failed(e.to_string()))?
        .ok_or_else(|| PipelineError::ProjectNotFound(name.to_string()))
}
```

**Writing notes:**
- Add `chrono.workspace = true` to `crates/diskrement-cli/Cargo.toml`'s
  `[dependencies]` (chrono is already a workspace dependency) — `Utc::now()`
  is the init timestamp, matching the Scala `Instant.now()`.
- Trim the `use` list to what remains after writing (`staged` and `time` are
  probably unused in main); warnings are failures.
- `pipeline::ConsoleProgress` for `init` is dead reporting (init has no
  phases) but harmless; a `NoopSink` would do equally well — pick one and be
  consistent.

- [ ] **Step 4: Run and watch it pass**

Run: `cargo test -p diskrement-cli --test binary`
Expected: 5 passed (version + 4 new). Then the workspace trio.

- [ ] **Step 5: Commit**

```bash
git add crates/diskrement-cli/src crates/diskrement-cli/tests crates/diskrement-cli/Cargo.toml
git commit -m "feat(cli): wire init, scan and status into the diskrement binary"
```

---

### Task 15: Open the differential gate

**Files:**
- Modify: `crates/diskrement-difftest/src/runner.rs` (one line)
- Oracle: this plan's exit bar; the harness README

**Interfaces:**
- Consumes: the binary from Task 14, the harness from the scaffold plan.

- [ ] **Step 1: Append the three commands to `RUST_IMPLEMENTED`**

```rust
pub const RUST_IMPLEMENTED: &[&str] = &["init", "scan", "status"];
```

- [ ] **Step 2: Run the full differential suite, all three modes**

```bash
export DISKREMENT_SCALA_BIN="$PWD/cli/target/universal/stage/bin/diskrement"
cargo build -p diskrement-cli
cargo test -p diskrement-difftest -- --ignored --nocapture
```

Expected:
- `the_harness_is_deterministic_against_itself`: 3 scenarios reproduced
  identically.
- `rust_matches_scala`: `init-scan-status` and `error-paths` **compared and
  green**; `harvest-replay` skipped (needs plan/burn/…) with the reason
  printed. Summary line: `all-rust: 2 scenario(s) compared, 1 skipped`.
- `interleaved_matches_scala`: same counts — this is the mode where Scala
  reads Rust-written catalogs, staged scans and sidecars mid-project.

If a comparison fails, the diff is a **porting bug**: fix the Rust side to
match the Scala oracle. Never widen a normalization rule.

- [ ] **Step 3: Run the phase's full verification bar**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
git status --porcelain -- core shell cli build.sbt project   # must print nothing
```

Expected: all clean; test count ≥ 138 + every test this plan added.

- [ ] **Step 4: Commit**

```bash
git add crates/diskrement-difftest/src/runner.rs
git commit -m "feat(difftest): teach the harness that init, scan and status exist"
```

---

## Phase exit bar (spec §9, P3 row)

- [ ] First runnable binary: `cargo build -p diskrement-cli` produces
  `target/debug/diskrement`; `init`/`scan`/`status` work end to end
  (Task 14's integration tests).
- [ ] Scancache semantics ported and green (Task 6: 8 `FingerprintCacheSpec`
  cases including wipe-and-retry, upsert, soft-lock failure).
- [ ] `HashParallelismSpec` port green (Task 2: default 6, clamp 64, trim,
  unusable-value warning).
- [ ] Scanner, hasher, snapshot, staged-scan ports green (Tasks 1, 4, 5, 7, 8).
- [ ] The harness runs its scenarios in all three modes with
  `init`/`scan`/`status` implemented: all-rust and interleaved compare 2 of 3
  scenarios green, skip 1 loudly (Task 15).
- [ ] Routine suite, clippy `-D warnings`, fmt clean; Scala tree untouched.
- [ ] Execution deviations appended to this section as they occur.

## Execution deviations

(appended during execution)

- **Task 2 (`resolve_parallelism`):** the plan's match arm
  `Ok(n) if (1..=MAX_PARALLELISM).contains(&n)` sends `Ok(0)` to the *clamp*
  arm (0 parses, 0 > 64 is false). Scala's `toIntOption.filter(_ > 0)` treats
  0 as unusable. Fixed: `Ok(0) | Err(_)` → default + warning. The test helper
  `env(&[(&str, &str)])` also needs an explicit lifetime
  (`fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> … + 'a`); `'_` is ambiguous
  across the three input lifetimes.
- **Task 2 (test correction):** present-but-empty `""` is *unset* (no
  warning), per the Scala empty/whitespace rule — the plan's must-warn loop
  wrongly included it.
- **Tasks 4/5:** `pub(crate)` items with no non-test consumer fail
  `-D warnings`; Task 4 carried temporary `#[allow(dead_code)]` annotations,
  removed when Task 5's `scan` consumed them.
- **Task 5 (two tests corrected):** `output_is_sorted_by_relpath…` and
  `progress_covers_only…` built `prior` from the first scan's real sizes and
  mtimes, so the intended "miss" file *hit* the fast path (which never
  compares hashes) and `hashed` came back 0. The Scala FakeFs tests made the
  miss by giving the prior a stale stat; the Rust tests now bump the miss
  file's mtime after the first scan. Also: `ScanOutput` derives `Debug` (the
  failure test uses `unwrap_err`), and `scan` computes the hashed count
  before `files.extend(hashed_files)` moves the vec (E0382).
- **Task 6:** shell needs the chrono `clock` feature for `Utc::now()` in
  `put_all` (`chrono = { workspace = true, features = ["clock"] }` in shell's
  manifest) — the workspace default excludes it to keep *core* pure; shell is
  the effects crate. The plan's `ffi::Error::new(ErrorCode::…)` also does not
  compile: the constructor takes a `c_int`, so it is
  `ffi::Error::new(rusqlite::ffi::SQLITE_CORRUPT)`. `put_all` uses
  `Connection::unchecked_transaction()` because the cache hands out
  `&Connection` (same rollback-on-drop guard).
- **Task 7 (test helper):** the `ok_cloner` stand-in `copy_tree` must handle a
  plain-file source — the CoW probe passes a file, and `dst.join("")`
  collapsed to the work dir (EISDIR). It now copies a file directly.
- **Task 8:** thiserror reserves a variant field named `source` for the error
  chain, so `NotSameFilesystem { source, … }` does not compile
  (`PathBuf: Error` unsatisfied). The field is renamed `src` — which matches
  the Scala message's `$src` anyway; the rendered string is unchanged.
  `serde_json::Error::custom` is a trait method
  (`<serde_json::Error as serde::de::Error>::custom`), not an inherent. The
  corrupt-scan test asserts `!err.is_io()` rather than `is_syntax()`: the
  category depends on where the parse dies (`{not json` is a data error —
  map vs sequence).
- **Task 9 (the phase's real find):** `ioctl(FICLONE)` stamps the cloned file
  with the *current* mtime — it does not preserve the source's, while the
  Scala `cp -a` did. Verified empirically on btrfs. Consequence: a fresh
  snapshot per scan made every cached fingerprint miss (mtime mismatch) and
  the forced-rescan re-hashed everything, breaking the pinned `Hashed 0 of N`
  reuse output. `reflink_tree` now copies each file's mtime and permissions
  after cloning. Also: the pipeline `Env` test helper must read `TempDir`
  paths *before* moving the dirs into `_dirs` (E0382), and the reports derive
  `Debug` for `unwrap`/panic printing.

## Spec coverage

| spec §9 P3 requirement | task |
|---|---|
| fs: scanner, hasher, fingerprint cache, snapshot | 1, 2, 4, 5, 6, 7 |
| `init` / `scan` / `status` | 8, 9, 11, 12, 14 |
| first runnable binary | 14 |
| scancache semantics port green | 6 |
| `HashParallelismSpec` port green | 2 |
| harness runs its first scenario in all three modes | 15 |
| differential task: extend the corpus for the phase's commands | corpus already has `init-scan-status`; `RUST_IMPLEMENTED` appended in 15 |
| CLI compatibility: flags, env, path precedence, exit codes, render | 10, 11, 12, 14 (wording drift sanctioned only in clap usage/argument errors) |

Deferred by design to later phases (each already named in the spec's phasing
table): `plan`/`--finalize`/oversize/`StalePlan` (P4), `burn` and the embedded
catalog snapshot (P5), `verify`/`history`/`reset` and the full
`ExitMappingSpec` port with insta snapshots (P6), `disc mark-dead` and derived
`DEAD` (P7).




