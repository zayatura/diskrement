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
use crate::burn::media::{MediaInspector, MediaStatus};
use crate::burn::reader::DiscReader;
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

        let hashed = hash_tree(&data_dir, self.sink, &verifying_label(exp))?;
        let mut mismatches: Vec<RelPath> = hashed
            .iter()
            .filter(|(rel, actual)| exp.latest_session.get(rel) != Some(actual))
            .map(|(rel, _)| rel.clone())
            .collect();
        mismatches.sort();

        Ok(VerifyOutcome {
            files_checked: hashed.len(),
            mismatches,
            skipped: false,
        })
    }
}

/// The label of a disc's read-back phase, the same for both backends.
fn verifying_label(exp: &DiscExpectation) -> String {
    format!("verifying disc {}", exp.disc_name)
}

/// Hash every file under `data_dir`, as one progress phase.
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
) -> Result<Vec<(RelPath, ContentHash)>, BurnError> {
    let walk_err = |e: walkdir::Error| BurnError::Io(std::io::Error::other(e));
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(data_dir).sort_by_file_name() {
        let entry = entry.map_err(walk_err)?;
        if entry.file_type().is_file() {
            let size = entry.metadata().map_err(walk_err)?.len();
            files.push((entry.into_path(), size));
        }
    }

    let total_bytes = files.iter().map(|(_, size)| size).sum();
    progress::phase(sink, label, total_bytes, files.len(), || {
        files
            .iter()
            .map(|(file, _)| {
                let actual =
                    hasher::hash_with(file, &mut |n| sink.advance(n, 0)).map_err(BurnError::Io)?;
                sink.advance(0, 1);
                Ok((relativize(data_dir, file), actual))
            })
            .collect()
    })
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
}

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
        reader.with_data_tree(|data_dir| {
            let found: BTreeMap<RelPath, ContentHash> =
                hash_tree(data_dir, self.sink, &verifying_label(exp))?
                    .into_iter()
                    .collect();
            let mut mismatches: Vec<RelPath> = found
                .iter()
                .filter(|(rel, actual)| exp.visible.get(*rel) != Some(*actual))
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
