//! The parallel scanner (`FileScanner.scala`, `HashParallelism.scala`,
//! `FileSystem.scala`).

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use diskrement_core::RelPath;

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
///
/// The `allow` below is temporary: `scan` (next commit) consumes all three.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) struct FileStat {
    pub size_bytes: u64,
    pub mtime: DateTime<Utc>,
}

#[allow(dead_code)]
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
#[allow(dead_code)]
pub(crate) fn relativize(root: &Path, path: &Path) -> RelPath {
    RelPath(
        path.strip_prefix(root)
            .unwrap_or(path)
            .display()
            .to_string(),
    )
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
}
