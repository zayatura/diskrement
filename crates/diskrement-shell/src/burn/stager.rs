//! Stages one disc tree and re-hashes it (`Stager.scala`).
//!
//! `<staging>/data` is the payload and `<staging>/.diskrement` the metadata;
//! for the real burner (P8) this staged tree is exactly what xorriso masters,
//! so the simulated and real backends share it.
//!
//! The copy is `Stager.scala`'s `cp --reflink=always --parents -p` batches,
//! translated the way `fs::snapshot` translated the source snapshot's: an
//! in-process clone per file with the mtime and permissions restored
//! afterwards, and the "this filesystem cannot clone" test moved from
//! English `cp` stderr to `ioctl(FICLONE)`'s errnos. The batching went with
//! the subprocess; there is no process to amortise any more.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::burn::{snapshot_writer, BurnError, BurnResult, DiscImage};
use crate::fs::hasher;
use crate::fs::snapshot::{clone_unsupported, copy_file_times_and_mode};
use crate::progress::{phase, ProgressSink};

pub fn stage(
    image: &DiscImage,
    progress: &dyn ProgressSink,
    parallelism: usize,
) -> Result<BurnResult, BurnError> {
    stage_with(image, progress, parallelism, |src, dst| {
        reflink_copy::reflink(src, dst)
    })
}

/// `stage` with the clone step injected, so the fallback and the hard-failure
/// classification are testable on a filesystem that happily reflinks.
pub fn stage_with(
    image: &DiscImage,
    progress: &dyn ProgressSink,
    parallelism: usize,
    clone: impl Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<BurnResult, BurnError> {
    let data_dir = image.staging_dir.join("data");
    clear_and_create(&data_dir)?;
    check_all_exist(image)?;
    create_dirs(&data_dir, image)?;
    copy_all(image, &data_dir, clone)?;

    let total_bytes: u64 = image.files.iter().map(|f| f.size_bytes).sum();
    let verified: Result<(), BurnError> =
        phase(progress, "staging", total_bytes, image.files.len(), || {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(parallelism.max(1))
                .build()
                .map_err(|e| BurnError::Io(std::io::Error::other(e)))?;
            pool.install(|| {
                image
                    .files
                    .par_iter()
                    .map(|f| {
                        let staged = data_dir.join(&f.rel_path.0);
                        let actual = hasher::hash(&staged).map_err(BurnError::Io)?;
                        if actual != f.content_hash {
                            return Err(BurnError::HashMismatch {
                                rel_path: f.rel_path.clone(),
                                expected: f.content_hash.clone(),
                                actual,
                            });
                        }
                        progress.advance(f.size_bytes, 1);
                        Ok(())
                    })
                    .collect::<Result<(), BurnError>>()
            })
        });
    verified?;

    snapshot_writer::write(&image.staging_dir.join(".diskrement"), &image.metadata)?;
    let image_bytes = total_staged_bytes(&image.staging_dir)?;
    Ok(BurnResult {
        staging_dir: image.staging_dir.clone(),
        image_bytes,
    })
}

/// Remove any previous staging for this disc and create an empty `data/`.
/// Re-staging must not leak a file the new session no longer plans.
fn clear_and_create(data_dir: &Path) -> Result<(), BurnError> {
    let staging_dir = data_dir
        .parent()
        .expect("data_dir was built by joining onto the staging dir");
    if staging_dir.exists() {
        std::fs::remove_dir_all(staging_dir).map_err(BurnError::Io)?;
    }
    std::fs::create_dir_all(data_dir).map_err(BurnError::Io)
}

fn check_all_exist(image: &DiscImage) -> Result<(), BurnError> {
    for f in &image.files {
        if !image.snapshot_root.join(&f.rel_path.0).exists() {
            return Err(BurnError::SourceFileMissing(f.rel_path.clone()));
        }
    }
    Ok(())
}

/// Pre-create every destination directory.
///
/// Not a `cp --parents` workaround: creating them here is what keeps staged
/// directory permissions and mtimes independent of the *source* directories'.
/// A read-only source directory would otherwise break a later write into it,
/// and directory mtimes would depend on copy order. Only FILE metadata is
/// preserved from the source.
fn create_dirs(data_dir: &Path, image: &DiscImage) -> Result<(), BurnError> {
    let mut parents: BTreeSet<PathBuf> = BTreeSet::new();
    for f in &image.files {
        if let Some(parent) = data_dir.join(&f.rel_path.0).parent() {
            parents.insert(parent.to_path_buf());
        }
    }
    for parent in parents {
        std::fs::create_dir_all(&parent).map_err(BurnError::Io)?;
    }
    Ok(())
}

/// Clone every planned file, falling back to a plain copy for the rest of the
/// burn once the filesystem says it cannot clone. Sequential: the copy is
/// I/O-bound and the Scala version was too.
fn copy_all(
    image: &DiscImage,
    data_dir: &Path,
    clone: impl Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<(), BurnError> {
    let mut fallback = false;
    for f in &image.files {
        let src = image.snapshot_root.join(&f.rel_path.0);
        let dst = data_dir.join(&f.rel_path.0);
        if !fallback {
            match clone(&src, &dst) {
                Ok(()) => {
                    copy_file_metadata(&src, &dst)?;
                    continue;
                }
                Err(e) if clone_unsupported(&e) => {
                    eprintln!(
                        "reflink clone unsupported here; falling back to plain copies for \
                         this burn (staging will read and write every byte)"
                    );
                    fallback = true;
                }
                Err(e) => return Err(BurnError::Io(e)),
            }
        }
        // `fs::copy` carries permissions; the mtime still needs restoring.
        std::fs::copy(&src, &dst).map_err(BurnError::Io)?;
        copy_file_metadata(&src, &dst)?;
    }
    Ok(())
}

/// `ioctl(FICLONE)` stamps the clone with the current time and `fs::copy` does
/// the same, but the staged tree must carry the source's file mtimes — that is
/// what `cp -p` gave the Scala stager (CLAUDE.md).
fn copy_file_metadata(src: &Path, dst: &Path) -> Result<(), BurnError> {
    copy_file_times_and_mode(src, dst).map_err(BurnError::Io)
}

/// Every regular file under the staged tree, metadata included.
fn total_staged_bytes(staging_dir: &Path) -> Result<u64, BurnError> {
    let mut total = 0u64;
    for entry in walkdir::WalkDir::new(staging_dir).follow_links(false) {
        let entry = entry.map_err(|e| BurnError::Io(std::io::Error::other(e)))?;
        if entry.file_type().is_file() {
            total += entry
                .metadata()
                .map_err(|e| BurnError::Io(std::io::Error::other(e)))?
                .len();
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone as _, Utc};
    use diskrement_core::{ContentHash, RelPath, ScannedFile, SessionManifest};
    use std::path::{Path, PathBuf};

    use crate::burn::{DiscIdentity, DiscMetadata};
    use crate::fs::hasher;
    use crate::progress::NoopSink;

    struct Fixture {
        _dir: tempfile::TempDir,
        snapshot: PathBuf,
        staging: PathBuf,
        catalog_snapshot: PathBuf,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::TempDir::new().unwrap();
        let snapshot = dir.path().join("snapshot");
        let staging = dir.path().join("staging/disc-1");
        let catalog_snapshot = dir.path().join("burn-catalog.sqlite");
        std::fs::create_dir_all(&snapshot).unwrap();
        std::fs::write(&catalog_snapshot, b"catalog bytes").unwrap();
        Fixture {
            _dir: dir,
            snapshot,
            staging,
            catalog_snapshot,
        }
    }

    /// Write `rel` under the snapshot root and return the `ScannedFile` the
    /// planner would have produced for it.
    fn source(fx: &Fixture, rel: &str, body: &[u8]) -> ScannedFile {
        let abs = fx.snapshot.join(rel);
        std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
        std::fs::write(&abs, body).unwrap();
        ScannedFile {
            rel_path: RelPath(rel.to_string()),
            content_hash: hasher::hash(&abs).unwrap(),
            size_bytes: body.len() as u64,
            mtime: Utc.timestamp_opt(1_757_100_000, 0).unwrap(),
        }
    }

    fn image(fx: &Fixture, files: Vec<ScannedFile>) -> DiscImage {
        DiscImage {
            staging_dir: fx.staging.clone(),
            snapshot_root: fx.snapshot.clone(),
            files,
            metadata: DiscMetadata {
                identity: DiscIdentity {
                    project_name: "p".to_string(),
                    disc_ordinal: 1,
                    session_ordinal: 1,
                    created_at: Utc.timestamp_opt(1_757_100_066, 0).unwrap(),
                },
                manifest: SessionManifest {
                    project_name: "p".to_string(),
                    disc_ordinal: 1,
                    session_ordinal: 1,
                    added: Vec::new(),
                    modified: Vec::new(),
                    deleted: Vec::new(),
                },
                catalog_snapshot: fx.catalog_snapshot.clone(),
                pointers: Vec::new(),
            },
            close_disc: false,
        }
    }

    #[test]
    fn stages_every_file_under_data_preserving_structure_and_bytes() {
        let fx = fixture();
        let files = vec![
            source(&fx, "a.bin", b"aaaa"),
            source(&fx, "nested/deep/b.bin", b"bbbbbb"),
        ];
        let out = stage(&image(&fx, files), &NoopSink, 2).unwrap();

        assert_eq!(
            std::fs::read(fx.staging.join("data/a.bin")).unwrap(),
            b"aaaa"
        );
        assert_eq!(
            std::fs::read(fx.staging.join("data/nested/deep/b.bin")).unwrap(),
            b"bbbbbb"
        );
        assert_eq!(out.staging_dir, fx.staging);
    }

    #[test]
    fn writes_the_metadata_directory_too() {
        let fx = fixture();
        let files = vec![source(&fx, "a.bin", b"aaaa")];
        stage(&image(&fx, files), &NoopSink, 1).unwrap();

        for name in [
            "project.json",
            "manifest.json",
            "pointers.json",
            "catalog.sqlite",
        ] {
            assert!(
                fx.staging.join(".diskrement").join(name).is_file(),
                "missing {name}"
            );
        }
    }

    /// `imageBytes` counts every regular file under the staging dir, metadata
    /// included — not just the payload.
    #[test]
    fn image_bytes_sums_the_whole_staged_tree() {
        let fx = fixture();
        let files = vec![source(&fx, "a.bin", b"aaaa")];
        let out = stage(&image(&fx, files), &NoopSink, 1).unwrap();

        let mut expected = 0u64;
        for entry in walkdir::WalkDir::new(&fx.staging) {
            let entry = entry.unwrap();
            if entry.file_type().is_file() {
                expected += entry.metadata().unwrap().len();
            }
        }
        assert_eq!(out.image_bytes, expected);
        assert!(out.image_bytes > 4, "metadata is counted too");
    }

    /// File mtimes come from the source. `ioctl(FICLONE)` stamps the clone with
    /// the current time, so a port that forgets to restore them passes every
    /// byte-comparison test and still breaks nothing here — except this.
    #[test]
    fn staged_files_keep_the_source_mtime() {
        let fx = fixture();
        let files = vec![source(&fx, "a.bin", b"aaaa")];
        let src = fx.snapshot.join("a.bin");
        let stamp =
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
        std::fs::File::options()
            .write(true)
            .open(&src)
            .unwrap()
            .set_modified(stamp)
            .unwrap();

        stage(&image(&fx, files), &NoopSink, 1).unwrap();

        let staged = std::fs::metadata(fx.staging.join("data/a.bin")).unwrap();
        assert_eq!(staged.modified().unwrap(), stamp);
    }

    /// Staging is idempotent: the previous tree is removed, so a file that is
    /// no longer planned does not survive into the new session.
    #[test]
    fn restaging_clears_the_previous_tree() {
        let fx = fixture();
        let a = source(&fx, "a.bin", b"aaaa");
        let b = source(&fx, "b.bin", b"bbbb");
        stage(&image(&fx, vec![a.clone(), b]), &NoopSink, 1).unwrap();
        assert!(fx.staging.join("data/b.bin").exists());

        stage(&image(&fx, vec![a]), &NoopSink, 1).unwrap();
        assert!(
            !fx.staging.join("data/b.bin").exists(),
            "stale file survived"
        );
        assert!(fx.staging.join("data/a.bin").exists());
    }

    #[test]
    fn a_file_missing_from_the_snapshot_fails_before_anything_is_copied() {
        let fx = fixture();
        let mut files = vec![source(&fx, "a.bin", b"aaaa")];
        files.push(ScannedFile {
            rel_path: RelPath("gone.bin".to_string()),
            content_hash: ContentHash("00".repeat(32)),
            size_bytes: 4,
            mtime: Utc.timestamp_opt(1_757_100_000, 0).unwrap(),
        });

        let err = stage(&image(&fx, files), &NoopSink, 1).unwrap_err();
        match err {
            BurnError::SourceFileMissing(p) => assert_eq!(p.0, "gone.bin"),
            other => panic!("expected SourceFileMissing, got {other:?}"),
        }
        assert!(
            !fx.staging.join("data/a.bin").exists(),
            "the existence check runs before the copy"
        );
    }

    /// The integrity gate: a staged file whose content does not match the hash
    /// the scan recorded stops the burn.
    #[test]
    fn a_hash_mismatch_stops_the_burn() {
        let fx = fixture();
        let mut f = source(&fx, "a.bin", b"aaaa");
        let real = f.content_hash.clone();
        f.content_hash = ContentHash("11".repeat(32));

        let err = stage(&image(&fx, vec![f]), &NoopSink, 1).unwrap_err();
        match err {
            BurnError::HashMismatch {
                rel_path,
                expected,
                actual,
            } => {
                assert_eq!(rel_path.0, "a.bin");
                assert_eq!(expected.0, "11".repeat(32));
                assert_eq!(actual, real);
            }
            other => panic!("expected HashMismatch, got {other:?}"),
        }
    }

    /// A disc with no files is legal: `plan --finalize` seals a disc with a
    /// file-less session, and the corpus's `disc-4/data/` is empty.
    #[test]
    fn a_seal_session_stages_an_empty_data_directory() {
        let fx = fixture();
        let out = stage(&image(&fx, Vec::new()), &NoopSink, 1).unwrap();

        assert!(fx.staging.join("data").is_dir());
        assert_eq!(
            std::fs::read_dir(fx.staging.join("data")).unwrap().count(),
            0
        );
        assert!(fx.staging.join(".diskrement/manifest.json").is_file());
        assert!(out.image_bytes > 0, "metadata still counts");
    }

    /// Progress is reported once per file, with the phase label the Scala
    /// stager used.
    #[test]
    fn reports_a_staging_phase_over_every_file() {
        use std::sync::Mutex;

        #[derive(Default)]
        struct Rec(Mutex<Vec<String>>);
        impl crate::progress::ProgressSink for Rec {
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

        let fx = fixture();
        let files = vec![
            source(&fx, "a.bin", b"aaaa"),
            source(&fx, "b.bin", b"bbbbbb"),
        ];
        let rec = Rec::default();
        stage(&image(&fx, files), &rec, 1).unwrap();

        let events = rec.0.lock().unwrap().clone();
        assert_eq!(events[0], "start:staging:10:2");
        assert_eq!(
            events.iter().filter(|e| e.starts_with("advance:")).count(),
            2
        );
        assert_eq!(events.last().unwrap(), "done");
    }

    /// `done` still fires when a hash fails mid-phase.
    #[test]
    fn the_staging_phase_closes_even_when_a_hash_mismatches() {
        use std::sync::Mutex;

        #[derive(Default)]
        struct Rec(Mutex<Vec<String>>);
        impl crate::progress::ProgressSink for Rec {
            fn start(&self, _l: &str, _b: u64, _f: usize) {
                self.0.lock().unwrap().push("start".to_string());
            }
            fn advance(&self, _b: u64, _f: usize) {}
            fn done(&self) {
                self.0.lock().unwrap().push("done".to_string());
            }
        }

        let fx = fixture();
        let mut f = source(&fx, "a.bin", b"aaaa");
        f.content_hash = ContentHash("11".repeat(32));
        let rec = Rec::default();
        let _ = stage(&image(&fx, vec![f]), &rec, 1);

        assert_eq!(rec.0.lock().unwrap().clone(), vec!["start", "done"]);
    }

    /// The plain-copy fallback produces the same tree as the reflink path, so
    /// the stager works on tmpfs and any other filesystem that refuses clones.
    #[test]
    fn the_plain_copy_fallback_produces_the_same_tree() {
        let fx = fixture();
        let files = vec![source(&fx, "a.bin", b"aaaa"), source(&fx, "n/b.bin", b"bb")];
        let img = image(&fx, files);

        stage_with(&img, &NoopSink, 1, |_src: &Path, _dst: &Path| {
            Err(std::io::Error::from_raw_os_error(95)) // EOPNOTSUPP
        })
        .unwrap();

        assert_eq!(
            std::fs::read(fx.staging.join("data/a.bin")).unwrap(),
            b"aaaa"
        );
        assert_eq!(
            std::fs::read(fx.staging.join("data/n/b.bin")).unwrap(),
            b"bb"
        );
    }

    /// A read-only source file (every git object is 0444) stages on both copy
    /// paths, keeping its mode and mtime. Restoring the mtime must not need
    /// write access to a destination that already carries the source's mode.
    #[test]
    fn a_read_only_source_file_stages_on_the_clone_and_the_fallback_path() {
        use std::os::unix::fs::PermissionsExt as _;
        let fallback = |_s: &Path, _d: &Path| Err(std::io::Error::from_raw_os_error(95));
        let clone = |s: &Path, d: &Path| std::fs::copy(s, d).map(|_| ());
        for (label, cloner) in [
            (
                "clone",
                &clone as &dyn Fn(&Path, &Path) -> std::io::Result<()>,
            ),
            ("fallback", &fallback),
        ] {
            let fx = fixture();
            let files = vec![source(&fx, "ro.bin", b"read only")];
            let src = fx.snapshot.join("ro.bin");
            let stamp =
                std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
            std::fs::File::options()
                .write(true)
                .open(&src)
                .unwrap()
                .set_modified(stamp)
                .unwrap();
            std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o444)).unwrap();

            stage_with(&image(&fx, files), &NoopSink, 1, cloner)
                .unwrap_or_else(|e| panic!("{label}: {e:?}"));

            let staged = std::fs::metadata(fx.staging.join("data/ro.bin")).unwrap();
            assert_eq!(staged.permissions().mode() & 0o777, 0o444, "{label}");
            assert_eq!(staged.modified().unwrap(), stamp, "{label}");
        }
    }

    /// A clone failure that is *not* a capability signal is a real error and
    /// must not be swallowed by the fallback.
    #[test]
    fn a_non_capability_clone_failure_is_an_error() {
        let fx = fixture();
        let files = vec![source(&fx, "a.bin", b"aaaa")];
        let err = stage_with(&image(&fx, files), &NoopSink, 1, |_s: &Path, _d: &Path| {
            Err(std::io::Error::from_raw_os_error(5)) // EIO
        })
        .unwrap_err();
        assert!(matches!(err, BurnError::Io(_)), "{err:?}");
    }
}
