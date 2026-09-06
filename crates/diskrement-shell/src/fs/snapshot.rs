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
    NotCowCapable { work_dir: PathBuf, detail: String },
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
///
/// Each file's **mtime and permissions are copied after the clone** (`cp -a`
/// did this): `ioctl(FICLONE)` stamps the destination with the current time,
/// and the scanner's size+mtime fast path compares exact millis instants —
/// without this, every scan of a fresh snapshot would re-hash and the
/// fingerprint cache would never hit.
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
            let meta = entry.metadata()?;
            std::fs::set_permissions(&target, meta.permissions())?;
            let mtime = meta.modified()?;
            let f = std::fs::File::options().write(true).open(&target)?;
            f.set_modified(mtime)?;
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

    fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
        // The probe hands us a plain file, not a directory (`cp` copies
        // either; so must the stand-in cloner).
        if src.is_file() {
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)?;
            }
            return std::fs::copy(src, dst).map(|_| ());
        }
        std::fs::create_dir_all(dst)?;
        for entry in walkdir::WalkDir::new(src) {
            let entry = entry?;
            let rel = entry.path().strip_prefix(src).unwrap();
            let target = dst.join(rel);
            if entry.file_type().is_dir() {
                std::fs::create_dir_all(target)?;
            } else if entry.file_type().is_file() {
                std::fs::copy(entry.path(), target)?;
            }
        }
        Ok(())
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
        assert!(!clone_unsupported(&std::io::Error::other(
            "failed to clone"
        ))); // errno-less
        assert!(!clone_unsupported(&std::io::Error::from_raw_os_error(13))); // EACCES
    }
}
