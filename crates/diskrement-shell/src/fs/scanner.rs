//! The parallel scanner (`FileScanner.scala`, `HashParallelism.scala`,
//! `FileSystem.scala`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use diskrement_core::{ContentHash, RelPath, ScannedFile};

use crate::catalog::model::FileFingerprint;
use crate::progress::{phase, ProgressSink};

/// Regular files under `root`, symlinks excluded, dotfiles included
/// (`FileSystemLive.walk`). Order is directory order; `scan` sorts at the end.
#[allow(dead_code)]
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
    let mtime =
        ts - chrono::Duration::nanoseconds((ts.timestamp_subsec_nanos() % 1_000_000) as i64);
    Ok(FileStat {
        size_bytes: meta.len(),
        mtime,
    })
}

/// `RelPath` of `path` under `root` (`snapshotRoot.relativize`).
pub(crate) fn relativize(root: &Path, path: &Path) -> RelPath {
    RelPath(
        path.strip_prefix(root)
            .unwrap_or(path)
            .display()
            .to_string(),
    )
}

/// What one scan produced (`FileScanner.ScanOutput`): every file in the
/// snapshot, fully hashed (reused or fresh), sorted by rel path; `hashed` is
/// the count hashed this run.
#[derive(Debug)]
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
            .map_err(|e| {
                failed(
                    RelPath(root.display().to_string()),
                    std::io::Error::other(e),
                )
            })?;
        // `phase` guarantees `done` even when a hash fails mid-pool. The
        // `misses.len()` argument is evaluated before the closure moves
        // `misses`.
        let hashed: Vec<Result<ScannedFile, ScanError>> =
            phase(sink, "hashing", total_bytes, misses.len(), || {
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
            });
        for r in hashed {
            hashed_files.push(r?);
        }
    }

    let hashed_count = hashed_files.len();
    let mut files = reused;
    files.extend(hashed_files);
    files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(ScanOutput {
        files,
        hashed: hashed_count,
    })
}

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
        return Parallelism {
            value: DEFAULT_PARALLELISM,
            warnings: Vec::new(),
        };
    };
    if raw.trim().is_empty() {
        return Parallelism {
            value: DEFAULT_PARALLELISM,
            warnings: Vec::new(),
        };
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
            warnings: vec![format!(
                "clamping DISKREMENT_HASH_PARALLELISM {n} to {MAX_PARALLELISM}"
            )],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
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
            st.mtime
                - chrono::Duration::nanoseconds(
                    (st.mtime.timestamp_subsec_nanos() % 1_000_000) as i64
                )
        );
    }

    mod scan_tests {
        use super::*;
        use crate::progress::{NoopSink, ProgressSink};
        use diskrement_core::{ContentHash, ScannedFile};
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
            let hashes: Vec<&str> = second
                .files
                .iter()
                .map(|f| f.content_hash.as_str())
                .collect();
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
            let f = std::fs::File::options()
                .write(true)
                .open(root.join("a"))
                .unwrap();
            f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_millis(5_000))
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
            // Make `a` a miss the way the Scala FakeFs test does: its current
            // stat no longer matches the prior fingerprint (the fast path
            // never compares hashes).
            let f = std::fs::File::options()
                .write(true)
                .open(root.join("a"))
                .unwrap();
            f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_millis(5_000))
                .unwrap();
            drop(f);
            let a_hash = crate::fs::hasher::hash(&root.join("a")).unwrap();
            let prior = prior_from(&first.files, |rel| {
                (rel == "z").then(|| "old-z".to_string())
            });
            let second = scan_ok(root, &prior, false, &NoopSink);
            assert_eq!(second.hashed, 1);
            let got: Vec<(String, String)> = second
                .files
                .iter()
                .map(|f| (f.rel_path.as_str().to_string(), f.content_hash.0.clone()))
                .collect();
            assert_eq!(
                got,
                vec![
                    ("a".to_string(), a_hash.0),
                    ("z".to_string(), "old-z".to_string())
                ]
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
            let names: Vec<String> = out
                .files
                .iter()
                .map(|f| f.rel_path.as_str().to_string())
                .collect();
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
            // Stale the "miss" file's prior so it really is a miss.
            let f = std::fs::File::options()
                .write(true)
                .open(root.join("miss"))
                .unwrap();
            f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_millis(5_000))
                .unwrap();
            drop(f);
            let prior = prior_from(&first.files, |rel| {
                (rel == "hit").then(|| "old-hit".to_string())
            });
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
}
