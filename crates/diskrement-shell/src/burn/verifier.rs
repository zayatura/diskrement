//! Read-back verification (`DiscVerifier.scala`,
//! `StagingDiscVerifier.scala`).
//!
//! `Verifier` is a closed enum for the same reason `Burner` is: there are
//! exactly two substrates, ever, and `--burner real|simulated` selects both
//! arms together — verifying a simulated burn by mounting a physical disc is
//! meaningless (spec §5.2).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use diskrement_core::{ContentHash, DiscKey, RelPath};

use crate::burn::cmd::Exec;
use crate::burn::json::ProjectJson;
use crate::burn::media::{MediaInspector, MediaStatus};
use crate::burn::reader::DiscReader;
use crate::burn::xorriso::parse;
use crate::burn::{BurnDevice, BurnError, BurnOperator, DiscIdentity};
use crate::catalog::model::SupersededVersion;
use crate::fs::hasher;
use crate::fs::scanner::relativize;
use crate::progress::{self, ProgressSink};

/// What the catalog says one disc should hold.
///
/// `latest_session` is what the simulated backend's staging dir contains;
/// `visible` and `superseded` describe the physical disc — the merged mount
/// view, plus the versions later sessions of the same disc hid. Each verifier
/// consumes the fields that match its substrate, so the staging verifier reads
/// only `latest_session` and the disc verifier reads only the other two.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscExpectation {
    pub project_name: String,
    pub disc: DiscKey,
    /// `DiscKey::name` for this project: the staging directory and the prompt.
    pub disc_name: String,
    pub latest_session: BTreeMap<RelPath, ContentHash>,
    pub visible: BTreeMap<RelPath, ContentHash>,
    /// The bytes of `visible`: what reading the disc back into a directory
    /// needs room for (burn-verify spec §8.2).
    pub visible_bytes: u64,
    pub superseded: Vec<SupersededVersion>,
}

/// The result of checking one disc.
///
/// `skipped` means nothing was checked — no staged data, or an operator
/// who declined the disc. A skipped disc is never marked verified, which is the
/// whole reason the flag exists rather than "zero files, zero mismatches".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyOutcome {
    pub files_checked: usize,
    pub mismatches: Vec<RelPath>,
    pub skipped: bool,
}

/// What `burn --verify` learns from reading back the disc it just wrote,
/// before it is recorded (burn-verify spec §4.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadBack {
    /// Every file is there and hashes as recorded. `through_xorriso` when
    /// the kernel could not mount the disc and xorriso read it instead
    /// (burn-verify spec §8).
    Passed { files: usize, through_xorriso: bool },
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
                through_xorriso: false,
            }
        } else {
            ReadBack::Failed {
                files: o.files_checked,
                mismatches: o.mismatches,
            }
        }
    }
}

/// Tier 1's and tier 2's findings as one read-back. Tier 1's mismatches are
/// evidence against the disc whatever tier 2 does: a tier 2 that failed to
/// set up says nothing about the disc, and passing its error on would let a
/// known-bad disc be treated as unverifiable and then recorded. Only with no
/// evidence is it an error, which the pipeline treats as unverifiable.
fn combine_tiers(
    tier1: (usize, Vec<RelPath>),
    tier2: Result<(usize, Vec<RelPath>), BurnError>,
) -> Result<ReadBack, BurnError> {
    let (checked_1, mut mismatches) = tier1;
    let (checked_2, hidden) = match tier2 {
        Ok(found) => found,
        Err(_) if !mismatches.is_empty() => (0, Vec::new()),
        Err(e) => return Err(e),
    };
    mismatches.extend(hidden);
    mismatches.sort();
    mismatches.dedup();
    Ok(ReadBack::from_outcome(VerifyOutcome {
        files_checked: checked_1 + checked_2,
        mismatches,
        skipped: false,
    }))
}

/// Free bytes for an unprivileged writer on the filesystem holding `dir`.
fn free_bytes(dir: &Path) -> std::io::Result<u64> {
    let st = rustix::fs::statvfs(dir)?;
    Ok(st.f_bavail.saturating_mul(st.f_frsize))
}

/// `24.4 GB`, the way the progress bars count.
fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

/// Remove a read-back dir, absent or not. xorriso restores the disc's
/// permissions, so a directory may come back without write permission, which
/// would stop the removal; those are made writable and the removal retried.
fn remove_readback(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if std::fs::remove_dir_all(dir).is_ok() || !dir.exists() {
        return;
    }
    for entry in walkdir::WalkDir::new(dir).into_iter().flatten() {
        if entry.file_type().is_dir() {
            let _ = std::fs::set_permissions(entry.path(), std::fs::Permissions::from_mode(0o755));
        }
    }
    if let Err(e) = std::fs::remove_dir_all(dir) {
        eprintln!(
            "could not remove the read-back dir {} ({e}); it is safe to delete by hand",
            dir.display()
        );
    }
}

/// This project's disc `exp.disc`, at exactly session `session`.
fn is_session(exp: &DiscExpectation, session: u32, id: &DiscIdentity) -> bool {
    id.project_name == exp.project_name && id.disc() == exp.disc && id.session_ordinal == session
}

/// Verifies a disc against the work-dir staging tree (the simulated backend).
///
/// The staging dir holds only the disc's **latest** session, so
/// `latest_session` is the reference; an absent `data/` directory is a skip.
pub struct StagingVerifier<'a> {
    staging_root: PathBuf,
    sink: &'a dyn ProgressSink,
}

impl<'a> StagingVerifier<'a> {
    pub fn new(staging_root: PathBuf, sink: &'a dyn ProgressSink) -> Self {
        StagingVerifier { staging_root, sink }
    }

    pub fn verify(&self, exp: &DiscExpectation) -> Result<VerifyOutcome, BurnError> {
        let data_dir = self
            .staging_root
            .join(format!("disc-{}", exp.disc_name))
            .join("data");
        if !data_dir.exists() {
            return Ok(VerifyOutcome {
                files_checked: 0,
                mismatches: Vec::new(),
                skipped: true,
            });
        }

        let hashed = hash_tree(&data_dir, self.sink, &verifying_label(exp));
        let mut mismatches: Vec<RelPath> = hashed
            .iter()
            .filter(|(rel, actual)| mismatched(&exp.latest_session, rel, actual))
            .map(|(rel, _)| rel.clone())
            .collect();
        mismatches.sort();

        Ok(VerifyOutcome {
            files_checked: hashed.len(),
            mismatches,
            skipped: false,
        })
    }

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
}

/// The label of a disc's read-back phase, the same for both backends.
fn verifying_label(exp: &DiscExpectation) -> String {
    format!("verifying disc {}", exp.disc_name)
}

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
    actual.as_ref().is_none_or(|a| expected.get(rel) != Some(a))
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
    sink: &'a dyn ProgressSink,
    /// Where `read_back` may extract the disc through xorriso when the kernel
    /// cannot mount it; `None` keeps it mount-only (plain `verify`).
    readback_dir: Option<PathBuf>,
}

/// Room kept free beyond the disc's own bytes when reading it back through
/// xorriso: the work dir's filesystem must not end up full.
const READBACK_MARGIN_BYTES: u64 = 1 << 30;

impl<'a> DiscVerifier<'a> {
    pub fn new(
        device: BurnDevice,
        exec: &'a dyn Exec,
        operator: &'a dyn BurnOperator,
        sink: &'a dyn ProgressSink,
    ) -> Self {
        DiscVerifier {
            device,
            exec,
            operator,
            sink,
            readback_dir: None,
        }
    }

    /// Let `read_back` fall back to extracting the disc through xorriso into
    /// `<work_dir>/readback` when the kernel cannot mount it, which is the
    /// rule straight after a burn on drives whose tray software cannot close
    /// (burn-verify spec §8). Only `burn --verify` sets this.
    pub fn reading_back_through_xorriso_in(mut self, work_dir: &Path) -> Self {
        self.readback_dir = Some(work_dir.join("readback"));
        self
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
        let not_mountable = match tier1 {
            Ok(Ok(found)) => return combine_tiers(found, self.tier2(exp)),
            Ok(Err(stale)) => stale,
            Err(BurnError::MountFailed(why)) => format!(
                "could not mount it ({}); a freshly burned disc usually needs a reinsert \
                 before the system can mount it",
                why.trim_end_matches('.')
            ),
            Err(e) => return Err(e),
        };
        match self.through_xorriso(exp, &not_mountable) {
            Ok(found) => Ok(match combine_tiers(found, self.tier2(exp))? {
                ReadBack::Passed { files, .. } => ReadBack::Passed {
                    files,
                    through_xorriso: true,
                },
                other => other,
            }),
            Err(why) => Ok(ReadBack::Unverifiable(why)),
        }
    }

    /// Tier 1 through xorriso, for a disc the kernel cannot mount: extract
    /// `/data` into the read-back dir, hash it, remove it. `Err` is why that
    /// was not possible, worded after `not_mountable`; it never says anything
    /// about the disc, so the caller treats it as unverifiable.
    fn through_xorriso(
        &self,
        exp: &DiscExpectation,
        not_mountable: &str,
    ) -> Result<(usize, Vec<RelPath>), String> {
        let dir = match (&self.readback_dir, &self.device) {
            (Some(dir), BurnDevice::RealDevice(_)) => dir,
            _ => return Err(not_mountable.to_string()),
        };
        let room = dir.parent().unwrap_or(dir);
        remove_readback(dir);
        let need = exp.visible_bytes.saturating_add(READBACK_MARGIN_BYTES);
        match free_bytes(room) {
            Ok(free) if free >= need => {}
            Ok(free) => {
                return Err(format!(
                    "{not_mountable}; reading it through xorriso instead needs {} free in {}, \
                     and only {} is",
                    gb(need),
                    room.display(),
                    gb(free)
                ))
            }
            Err(e) => {
                return Err(format!(
                    "{not_mountable}; the free space in {} could not be read ({e})",
                    room.display()
                ))
            }
        }

        self.operator.progress(&format!(
            "Disc {} is not mountable yet: {not_mountable}. Reading it through xorriso \
             instead, into {} (needs {}).",
            exp.disc_name,
            dir.display(),
            gb(exp.visible_bytes)
        ));
        let reader = DiscReader::new(self.device.clone(), self.exec);
        let total = exp.visible_bytes;
        self.sink.start_bytes(
            &format!("reading disc {} through xorriso", exp.disc_name),
            total,
        );
        let mut reported = 0u64;
        let extracted = reader.extract_data_tree(dir, &mut |line| {
            if let Some(bytes) = parse::restored_bytes(line) {
                let bytes = bytes.min(total);
                if bytes > reported {
                    self.sink.advance(bytes - reported, 0);
                    reported = bytes;
                }
            }
        });
        if extracted.is_ok() && total > reported {
            self.sink.advance(total - reported, 0);
        }
        self.sink.done();

        let found = match extracted {
            Ok(()) => Ok(compare_visible(dir, exp, self.sink)),
            Err(e) => Err(format!(
                "{not_mountable}; reading it through xorriso failed too: {e}"
            )),
        };
        remove_readback(dir);
        found
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
                "the mounted disc still shows session {} of disc {} \
                 (the system has not noticed the new session)",
                id.session_ordinal,
                id.disc_name()
            )),
            None => Some("the mounted disc has no readable /.diskrement/project.json".to_string()),
        }
    }

    /// The insert loop. Unlike the burner's, *any* session ordinal of the
    /// right disc is acceptable, and a decline is a skip rather than an abort.
    fn await_disc(&self, exp: &DiscExpectation) -> Result<bool, BurnError> {
        let prompt = format!(
            "Insert disc {} of '{}' into {} to verify it (or decline to skip this disc).",
            exp.disc_name,
            exp.project_name,
            self.device.xorriso_address()
        );
        let matches =
            |id: &DiscIdentity| id.project_name == exp.project_name && id.disc() == exp.disc;
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
        // A just-inserted disc can take seconds to spin up and mount, and the
        // walk that sizes the phase reads every directory off the disc.
        self.operator
            .progress(&format!("Mounting disc {}…", exp.disc_name));
        let reader = DiscReader::new(self.device.clone(), self.exec);
        reader.with_data_tree(|data_dir| Ok(compare_visible(data_dir, exp, self.sink)))
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
        // xorriso extracts each file silently, so this phase advances one
        // whole file at a time, by the size the catalog recorded.
        let label = format!("extracting earlier versions on disc {}", exp.disc_name);
        let total_bytes = exp.superseded.iter().map(|sv| sv.size_bytes).sum();
        let mismatches =
            progress::phase(self.sink, &label, total_bytes, exp.superseded.len(), || {
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
                    self.sink.advance(sv.size_bytes, 1);
                    match checked {
                        Ok(actual) if actual == sv.content_hash => {}
                        _ => mismatches.push(sv.rel_path.clone()),
                    }
                }
                mismatches
            });
        Ok((exp.superseded.len(), mismatches))
    }
}

/// The verification backends, chosen by the same decision that chooses the
/// burner (spec §5.2).
pub enum Verifier<'a> {
    Staging(StagingVerifier<'a>),
    Disc(DiscVerifier<'a>),
}

impl Verifier<'_> {
    pub fn verify(&self, exp: &DiscExpectation) -> Result<VerifyOutcome, BurnError> {
        match self {
            Verifier::Staging(v) => v.verify(exp),
            Verifier::Disc(v) => v.verify(exp),
        }
    }

    /// `burn --verify`'s check of the disc just written, before it is
    /// recorded. `session` is the session that burn wrote.
    pub fn read_back(&self, exp: &DiscExpectation, session: u32) -> Result<ReadBack, BurnError> {
        match self {
            Verifier::Staging(v) => v.read_back(exp),
            Verifier::Disc(v) => v.read_back(exp, session),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::burn::testkit::RecordingSink;
    use crate::progress::NoopSink;
    use std::collections::BTreeMap;

    fn sha(body: &[u8]) -> ContentHash {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(body);
        ContentHash(format!("{:x}", h.finalize()))
    }

    fn expectation(pairs: &[(&str, &[u8])], ordinal: u32) -> DiscExpectation {
        DiscExpectation {
            project_name: "p".to_string(),
            disc: DiscKey::new(ordinal, 0),
            disc_name: ordinal.to_string(),
            latest_session: pairs
                .iter()
                .map(|(rel, body)| (RelPath((*rel).to_string()), sha(body)))
                .collect(),
            visible: BTreeMap::new(),
            visible_bytes: 0,
            superseded: Vec::new(),
        }
    }

    /// The `StagingDiscVerifierSpec` port: a clean tree verifies, a corrupted
    /// file mismatches, and a disc with no staging dir is skipped rather than
    /// reported clean.
    #[test]
    fn a_clean_tree_verifies_a_corrupt_file_mismatches_and_a_missing_dir_skips() {
        let dir = tempfile::TempDir::new().unwrap();
        let staging = dir.path().join("staging");
        let data = staging.join("disc-1/data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("a.txt"), b"hello").unwrap();
        std::fs::write(data.join("bad.txt"), b"corrupted").unwrap();

        let v = StagingVerifier::new(staging, &NoopSink);
        let exp = expectation(&[("a.txt", b"hello"), ("bad.txt", b"original")], 1);

        assert_eq!(
            v.verify(&exp).unwrap(),
            VerifyOutcome {
                files_checked: 2,
                mismatches: vec![RelPath("bad.txt".to_string())],
                skipped: false,
            }
        );

        let absent = DiscExpectation {
            disc: DiscKey::new(9, 0),
            disc_name: "9".to_string(),
            ..expectation(&[], 9)
        };
        assert_eq!(
            v.verify(&absent).unwrap(),
            VerifyOutcome {
                files_checked: 0,
                mismatches: Vec::new(),
                skipped: true,
            }
        );
    }

    /// A file present on the disc but absent from the expectation is a
    /// mismatch: `latestSession.get(rel).contains(h)` is false for a missing
    /// key, not merely for a wrong hash.
    #[test]
    fn a_staged_file_the_catalog_never_recorded_is_a_mismatch() {
        let dir = tempfile::TempDir::new().unwrap();
        let staging = dir.path().join("staging");
        let data = staging.join("disc-2/data");
        std::fs::create_dir_all(data.join("nested")).unwrap();
        std::fs::write(data.join("nested/extra.bin"), b"surprise").unwrap();

        let out = StagingVerifier::new(staging, &NoopSink)
            .verify(&expectation(&[], 2))
            .unwrap();
        assert_eq!(out.files_checked, 1);
        assert_eq!(
            out.mismatches,
            vec![RelPath("nested/extra.bin".to_string())]
        );
        assert!(!out.skipped);
    }

    /// The staging tree holds only the disc's LATEST session, so an empty
    /// `data/` — what a `plan --finalize` seal session stages — is a clean
    /// zero-file check, not a skip. `fixtures/discs/disc-4/data/` is exactly
    /// this shape, and run-manifest step 21 prints `disc 4: 0 files OK`.
    #[test]
    fn an_empty_data_dir_is_a_clean_zero_file_check_not_a_skip() {
        let dir = tempfile::TempDir::new().unwrap();
        let staging = dir.path().join("staging");
        std::fs::create_dir_all(staging.join("disc-4/data")).unwrap();

        assert_eq!(
            StagingVerifier::new(staging, &NoopSink)
                .verify(&expectation(&[], 4))
                .unwrap(),
            VerifyOutcome {
                files_checked: 0,
                mismatches: Vec::new(),
                skipped: false,
            }
        );
    }

    /// Two mismatches come back in sorted order. The Scala verifier inherited
    /// `Files.walk`'s unspecified order; sorting is what makes the rendered
    /// mismatch list reproducible.
    #[test]
    fn mismatches_are_reported_in_sorted_order() {
        let dir = tempfile::TempDir::new().unwrap();
        let staging = dir.path().join("staging");
        let data = staging.join("disc-1/data");
        std::fs::create_dir_all(&data).unwrap();
        for name in ["z.bin", "a.bin", "m.bin"] {
            std::fs::write(data.join(name), b"wrong").unwrap();
        }
        let out = StagingVerifier::new(staging, &NoopSink)
            .verify(&expectation(&[], 1))
            .unwrap();
        assert_eq!(
            out.mismatches,
            vec![
                RelPath("a.bin".to_string()),
                RelPath("m.bin".to_string()),
                RelPath("z.bin".to_string()),
            ]
        );
    }

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
            eprintln!(
                "SKIP [an_unreadable_file_is_a_mismatch_and_the_rest_is_still_checked]: running as root"
            );
            return;
        }
        let out = StagingVerifier::new(staging, &NoopSink)
            .verify(&expectation(
                &[("a.txt", b"hello"), ("locked.txt", b"secret")],
                1,
            ))
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
            eprintln!(
                "SKIP [an_unreadable_directory_is_a_mismatch_named_by_its_path]: running as root"
            );
            return;
        }
        let out = StagingVerifier::new(staging, &NoopSink)
            .verify(&expectation(&[("sub/b.txt", b"x")], 1));
        unlock(&sub);
        assert_eq!(out.unwrap().mismatches, vec![RelPath("sub".to_string())]);
    }

    fn rels(names: &[&str]) -> Vec<RelPath> {
        names.iter().map(|n| RelPath((*n).to_string())).collect()
    }

    /// Tier 1's evidence stands whatever tier 2 does: a tier 2 that could not
    /// even set up must not turn a known-bad disc into an unverifiable one,
    /// which the operator may then record.
    #[test]
    fn a_tier_two_failure_never_hides_tier_one_mismatches() {
        let setup = || Err(BurnError::Io(std::io::Error::other("no temp dir")));
        assert_eq!(
            combine_tiers((3, rels(&["b.txt"])), setup()).unwrap(),
            ReadBack::Failed {
                files: 3,
                mismatches: rels(&["b.txt"])
            }
        );
        assert!(combine_tiers((3, Vec::new()), setup()).is_err());
        assert_eq!(
            combine_tiers((3, rels(&["b.txt"])), Ok((2, rels(&["a.txt", "b.txt"])))).unwrap(),
            ReadBack::Failed {
                files: 5,
                mismatches: rels(&["a.txt", "b.txt"])
            }
        );
        assert_eq!(
            combine_tiers((3, Vec::new()), Ok((2, Vec::new()))).unwrap(),
            ReadBack::Passed {
                files: 5,
                through_xorriso: false
            }
        );
    }

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
            v.read_back(&expectation(&[("a.txt", b"hello")], 1))
                .unwrap(),
            ReadBack::Passed {
                files: 1,
                through_xorriso: false
            }
        );
        assert_eq!(
            v.read_back(&expectation(&[("a.txt", b"other")], 1))
                .unwrap(),
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

    /// One phase per disc, sized to what is on it before the first read, so
    /// the bar has a total and an ETA from the start; it advances to exactly
    /// that total and closes.
    #[test]
    fn staging_verify_reports_one_phase_sized_to_the_tree() {
        let dir = tempfile::TempDir::new().unwrap();
        let staging = dir.path().join("staging");
        let data = staging.join("disc-1B/data");
        std::fs::create_dir_all(data.join("nested")).unwrap();
        std::fs::write(data.join("a.txt"), b"hello").unwrap();
        std::fs::write(data.join("nested/b.bin"), vec![7u8; 200_000]).unwrap();

        let sink = RecordingSink::default();
        let exp = DiscExpectation {
            disc: DiscKey::new(1, 1),
            disc_name: "1B".to_string(),
            ..expectation(&[], 1)
        };
        StagingVerifier::new(staging, &sink).verify(&exp).unwrap();

        let events = sink.events();
        assert_eq!(events.first().unwrap(), "start verifying disc 1B 200005 2");
        assert_eq!(events.last().unwrap(), "done");
        assert_eq!(sink.advanced_bytes(), 200_005);
        assert_eq!(sink.advanced_files(), 2);
        assert!(
            events.iter().filter(|e| e.starts_with("advance ")).count() > 2,
            "a 200 kB file advances during its read, not only at its end: {events:?}"
        );
    }

    /// A skipped disc has nothing to report.
    #[test]
    fn a_staging_skip_opens_no_phase() {
        let dir = tempfile::TempDir::new().unwrap();
        let sink = RecordingSink::default();
        let out = StagingVerifier::new(dir.path().join("staging"), &sink)
            .verify(&expectation(&[], 1))
            .unwrap();
        assert!(out.skipped);
        assert!(sink.events().is_empty(), "{:?}", sink.events());
    }

    mod disc {
        use super::*;
        use crate::burn::cmd::CmdOutput;
        use crate::burn::testkit::{RecordingSink, ScriptedOperator};
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
                disc: DiscKey::new(1, 0),
                disc_name: "1".to_string(),
                latest_session: BTreeMap::new(),
                visible: BTreeMap::new(),
                visible_bytes: 0,
                superseded: Vec::new(),
            }
        }

        /// A queue of identity payloads, one per `-extract` of project.json,
        /// and the disc's `/data` tree as (path, body) pairs. A single-file
        /// extraction of `/data/<path>` writes that path's body.
        struct Toc {
            identities: RefCell<std::collections::VecDeque<&'static str>>,
            data: Vec<(&'static str, &'static [u8])>,
        }

        impl Toc {
            fn new(ids: &[&'static str]) -> Self {
                Toc {
                    identities: RefCell::new(ids.iter().copied().collect()),
                    data: Vec::new(),
                }
            }

            fn with_data(ids: &[&'static str], data: &[(&'static str, &'static [u8])]) -> Self {
                Toc {
                    data: data.to_vec(),
                    ..Toc::new(ids)
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
                    } else if disc_path == "/data" {
                        std::fs::create_dir_all(dest).expect("writable destination");
                        for (rel, body) in &self.data {
                            std::fs::write(std::path::Path::new(dest).join(rel), body)
                                .expect("writable destination");
                        }
                    } else {
                        let rel = disc_path.strip_prefix("/data/").expect("a /data path");
                        let (_, body) = self
                            .data
                            .iter()
                            .find(|(r, _)| *r == rel)
                            .expect("the script has this file");
                        std::fs::write(dest, body).expect("writable destination");
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
            assert_eq!(
                out,
                ReadBack::Passed {
                    files: 1,
                    through_xorriso: false
                }
            );
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
                    mismatches: vec![RelPath("a.txt".to_string()), RelPath("b.txt".to_string())],
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
            assert!(
                !script.ran("-extract /data"),
                "without a work dir (plain verify) there is no xorriso fallback"
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
                ScriptedExec::new().toc(&[APPENDABLE]).identities(&[None]),
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
            assert!(
                script.ran("udisksctl unmount"),
                "the stale mount is released"
            );
        }

        /// `read_back` with the xorriso fallback reading into `work`, with
        /// the operator's transcript and the progress events.
        fn read_back_via(
            script: &ScriptedExec,
            exp: &DiscExpectation,
            session: u32,
            work: &std::path::Path,
        ) -> (ReadBack, Vec<String>, Vec<String>) {
            let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
            let op = ScriptedOperator::new(&[]);
            let sink = RecordingSink::default();
            let out = DiscVerifier::new(
                BurnDevice::RealDevice("/dev/sr0".to_string()),
                &exec,
                &op,
                &sink,
            )
            .reading_back_through_xorriso_in(work)
            .read_back(exp, session)
            .unwrap();
            (out, op.log(), sink.events())
        }

        fn sized(pairs: &[(&str, &[u8])]) -> DiscExpectation {
            DiscExpectation {
                visible_bytes: pairs.iter().map(|(_, b)| b.len() as u64).sum(),
                ..visible(pairs)
            }
        }

        /// The kernel cannot mount a disc it has not noticed yet, so the
        /// read-back extracts it through xorriso into the work dir, hashes
        /// that, says so, and leaves nothing behind (burn-verify spec §8).
        #[test]
        fn a_disc_the_kernel_cannot_mount_is_read_through_xorriso() {
            let work = tempfile::TempDir::new().unwrap();
            let files: &[(&str, &[u8])] = &[("a.txt", b"hello"), ("sub/b.txt", b"keep")];
            let script = ScriptedExec::new()
                .toc(&[APPENDABLE])
                .identities(&[Some(IDENTITY)])
                .mounts(&[None])
                .extractions(&[Some(files)]);
            let (out, log, events) = read_back_via(&script, &sized(files), 1, work.path());

            assert_eq!(
                out,
                ReadBack::Passed {
                    files: 2,
                    through_xorriso: true
                }
            );
            assert!(
                log.iter()
                    .any(|l| l.contains("Reading it through xorriso instead")
                        && l.contains("needs 0.0 GB")),
                "{log:?}"
            );
            assert_eq!(events[0], "start_bytes reading disc 1 through xorriso 9");
            assert!(events.contains(&"advance 9 0".to_string()), "{events:?}");
            assert!(
                events
                    .iter()
                    .any(|e| e.starts_with("start verifying disc 1 9 2")),
                "the extracted tree is hashed as its own phase: {events:?}"
            );
            assert!(
                !work.path().join("readback").exists(),
                "nothing left behind"
            );
        }

        /// A mount still showing the previous session falls back the same way.
        #[test]
        fn a_stale_mount_is_read_through_xorriso() {
            let work = tempfile::TempDir::new().unwrap();
            let stale = mounted(&[("a.txt", b"hello")], IDENTITY);
            let files: &[(&str, &[u8])] = &[("a.txt", b"hello"), ("b.txt", b"new")];
            let script = ScriptedExec::new()
                .toc(&[APPENDABLE])
                .identities(&[Some(IDENTITY_2)])
                .mounts(&[Some(stale.path())])
                .extractions(&[Some(files)]);
            let (out, _, _) = read_back_via(&script, &sized(files), 2, work.path());
            assert_eq!(
                out,
                ReadBack::Passed {
                    files: 2,
                    through_xorriso: true
                }
            );
        }

        /// What xorriso extracts is evidence like a mount: a bad file fails.
        #[test]
        fn a_bad_file_read_through_xorriso_fails_the_disc() {
            let work = tempfile::TempDir::new().unwrap();
            let script = ScriptedExec::new()
                .toc(&[APPENDABLE])
                .identities(&[Some(IDENTITY)])
                .mounts(&[None])
                .extractions(&[Some(&[("a.txt", b"HELLO")])]);
            let (out, _, _) =
                read_back_via(&script, &sized(&[("a.txt", b"hello")]), 1, work.path());
            assert_eq!(
                out,
                ReadBack::Failed {
                    files: 1,
                    mismatches: vec![RelPath("a.txt".to_string())],
                }
            );
            assert!(!work.path().join("readback").exists());
        }

        /// A failed extraction says nothing about the disc: unverifiable, so
        /// the operator is asked to reinsert it, never told to discard it.
        #[test]
        fn a_failed_extraction_is_unverifiable() {
            let work = tempfile::TempDir::new().unwrap();
            let script = ScriptedExec::new()
                .toc(&[APPENDABLE])
                .identities(&[Some(IDENTITY)])
                .mounts(&[None])
                .extractions(&[None]);
            let (out, _, _) =
                read_back_via(&script, &sized(&[("a.txt", b"hello")]), 1, work.path());
            assert!(
                matches!(&out, ReadBack::Unverifiable(why)
                    if why.contains("could not mount it")
                        && why.contains("reading it through xorriso failed too")),
                "{out:?}"
            );
            assert!(!work.path().join("readback").exists());
        }

        /// Too little room in the work dir: unverifiable, naming both numbers,
        /// and nothing is extracted.
        #[test]
        fn too_little_space_for_the_extraction_is_unverifiable() {
            let work = tempfile::TempDir::new().unwrap();
            let script = ScriptedExec::new()
                .toc(&[APPENDABLE])
                .identities(&[Some(IDENTITY)])
                .mounts(&[None]);
            let huge = DiscExpectation {
                visible_bytes: u64::MAX / 4,
                ..visible(&[("a.txt", b"hello")])
            };
            let (out, _, _) = read_back_via(&script, &huge, 1, work.path());
            assert!(
                matches!(&out, ReadBack::Unverifiable(why) if why.contains("needs") && why.contains("free")),
                "{out:?}"
            );
            assert!(!script.ran("-extract /data"));
        }

        /// A leftover from a crashed read-back is cleared before extracting.
        #[test]
        fn a_leftover_readback_dir_is_cleared_first() {
            let work = tempfile::TempDir::new().unwrap();
            std::fs::create_dir_all(work.path().join("readback/old")).unwrap();
            std::fs::write(work.path().join("readback/old/stale.txt"), b"x").unwrap();
            let files: &[(&str, &[u8])] = &[("a.txt", b"hello")];
            let script = ScriptedExec::new()
                .toc(&[APPENDABLE])
                .identities(&[Some(IDENTITY)])
                .mounts(&[None])
                .extractions(&[Some(files)]);
            let (out, _, _) = read_back_via(&script, &sized(files), 1, work.path());
            assert_eq!(
                out,
                ReadBack::Passed {
                    files: 1,
                    through_xorriso: true
                },
                "the stale file would have been an unexpected file"
            );
        }

        /// A file the mounted disc cannot read fails the read-back.
        #[test]
        fn an_unreadable_file_on_the_disc_fails_the_read_back() {
            let disc = mounted(&[("a.txt", b"hello"), ("b.txt", b"keep")], IDENTITY);
            if !lock(&disc.path().join("data/b.txt")) {
                eprintln!(
                    "SKIP [an_unreadable_file_on_the_disc_fails_the_read_back]: running as root"
                );
                return;
            }
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
                    mismatches: vec![RelPath("b.txt".to_string())],
                }
            );
        }

        /// An appended disc's read-back also extracts the versions its new
        /// session hid (tier 2), and a bad one fails the disc. Read from an
        /// image file, whose `/data` and single files `Toc` extracts.
        #[test]
        fn a_read_back_checks_the_versions_a_new_session_hid() {
            let hidden = |old: &'static [u8]| DiscExpectation {
                superseded: vec![SupersededVersion {
                    session_ordinal: 1,
                    rel_path: RelPath("a.txt".to_string()),
                    content_hash: sha(old),
                    size_bytes: old.len() as u64,
                }],
                ..visible(&[("a.txt", b"hello")])
            };
            let run = |exp: &DiscExpectation| {
                let toc = Toc::with_data(&[IDENTITY_2], &[("a.txt", b"hello")]);
                let exec = |c: &[String], l: &mut dyn FnMut(&str)| toc.respond(c, l);
                let op = ScriptedOperator::new(&[]);
                DiscVerifier::new(
                    BurnDevice::ImageFile(std::path::PathBuf::from("/tmp/none.iso")),
                    &exec,
                    &op,
                    &NoopSink,
                )
                .read_back(exp, 2)
                .unwrap()
            };
            // Toc serves the session-1 extraction of a.txt with today's body.
            assert_eq!(
                run(&hidden(b"hello")),
                ReadBack::Passed {
                    files: 2,
                    through_xorriso: false
                }
            );
            assert_eq!(
                run(&hidden(b"older")),
                ReadBack::Failed {
                    files: 2,
                    mismatches: vec![RelPath("a.txt".to_string())],
                }
            );
        }

        /// Declining is a skip, not an abort: `verify` sweeps many discs and
        /// the operator may not have all of them to hand. A skipped disc is
        /// never marked verified — that is what the flag is for.
        #[test]
        fn a_declined_disc_is_skipped_and_nothing_is_read() {
            let toc = Toc::new(&[]);
            let exec = |c: &[String], l: &mut dyn FnMut(&str)| toc.respond(c, l);
            let op = ScriptedOperator::new(&[false]);
            let sink = RecordingSink::default();
            let verifier = DiscVerifier::new(
                BurnDevice::RealDevice("/dev/sr0".to_string()),
                &exec,
                &op,
                &sink,
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
                vec!["Insert disc 1 of 'photos' into /dev/sr0 to verify it \
                     (or decline to skip this disc)."
                    .to_string()]
            );
            assert!(sink.events().is_empty(), "{:?}", sink.events());
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
                &NoopSink,
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
            let sink = RecordingSink::default();
            // No `visible`, no `superseded`: the disc is accepted and both
            // tiers have nothing to check. The extraction of `/data` is what
            // an image-backed run exercises (tests/xorriso_image.rs).
            let outcome = DiscVerifier::new(
                BurnDevice::ImageFile(std::path::PathBuf::from("/tmp/none.iso")),
                &exec,
                &op,
                &sink,
            )
            .verify(&expectation())
            .unwrap();
            assert!(!outcome.skipped);
            assert_eq!(outcome.files_checked, 0);
            // An empty disc still opens (and closes) its tier-1 phase; with
            // nothing superseded there is no tier-2 phase.
            assert_eq!(sink.events(), vec!["start verifying disc 1 0 0", "done"]);
        }

        /// Tier 1 is sized to the files on the disc and advances by bytes
        /// within each read; tier 2 is its own phase, sized from the catalog,
        /// one file per extraction. The operator hears about the mount first:
        /// a just-inserted disc can take seconds to spin up.
        #[test]
        fn both_tiers_report_their_own_phase_after_a_mounting_line() {
            let toc = Toc::with_data(&[IDENTITY], &[("a.txt", b"hello"), ("b.txt", b"keep")]);
            let exec = |c: &[String], l: &mut dyn FnMut(&str)| toc.respond(c, l);
            let op = ScriptedOperator::new(&[true]);
            let sink = RecordingSink::default();
            let exp = DiscExpectation {
                visible: BTreeMap::from([
                    (RelPath("a.txt".to_string()), sha(b"hello")),
                    (RelPath("b.txt".to_string()), sha(b"keep")),
                ]),
                superseded: vec![SupersededVersion {
                    session_ordinal: 1,
                    rel_path: RelPath("a.txt".to_string()),
                    content_hash: sha(b"hello"),
                    size_bytes: 5,
                }],
                ..expectation()
            };
            let outcome = DiscVerifier::new(
                BurnDevice::ImageFile(std::path::PathBuf::from("/tmp/none.iso")),
                &exec,
                &op,
                &sink,
            )
            .verify(&exp)
            .unwrap();
            assert_eq!(outcome.mismatches, Vec::<RelPath>::new());
            assert_eq!(outcome.files_checked, 3);

            assert!(op.said("Mounting disc 1…"), "{:?}", op.log());
            let events = sink.events();
            let tier2 = events
                .iter()
                .position(|e| e.starts_with("start extracting"))
                .expect("a tier-2 phase");
            assert_eq!(events[0], "start verifying disc 1 9 2");
            assert_eq!(events[tier2 - 1], "done");
            assert_eq!(
                &events[tier2..],
                [
                    "start extracting earlier versions on disc 1 5 1",
                    "advance 5 1",
                    "done"
                ]
            );
            assert_eq!(sink.advanced_bytes(), 9 + 5);
            assert_eq!(sink.advanced_files(), 2 + 1);
        }
    }
}
