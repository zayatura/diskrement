//! Read-back verification (`DiscVerifier.scala`,
//! `StagingDiscVerifier.scala`).
//!
//! `Verifier` is a closed enum for the same reason `Burner` is: there are
//! exactly two substrates, ever, and `--burner real|simulated` selects both
//! arms together — verifying a simulated burn by mounting a physical disc is
//! meaningless (spec §5.2).

use std::collections::BTreeMap;
use std::path::PathBuf;

use diskrement_core::{ContentHash, DiscKey, RelPath};

use crate::burn::cmd::Exec;
use crate::burn::media::{MediaInspector, MediaStatus};
use crate::burn::reader::DiscReader;
use crate::burn::{BurnDevice, BurnError, BurnOperator, DiscIdentity};
use crate::catalog::model::SupersededVersion;
use crate::fs::hasher;
use crate::fs::scanner::relativize;

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
pub struct StagingVerifier {
    staging_root: PathBuf,
}

impl StagingVerifier {
    pub fn new(staging_root: PathBuf) -> Self {
        StagingVerifier { staging_root }
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

        // Sorted, unlike the Scala original: `Files.walk` order is
        // unspecified, so a multi-mismatch report was not reproducible run to
        // run. Sorting can only make the rendered list stable, and every
        // oracle in the corpus has at most one mismatch.
        let mut files = Vec::new();
        for entry in walkdir::WalkDir::new(&data_dir).sort_by_file_name() {
            let entry = entry.map_err(|e| BurnError::Io(std::io::Error::other(e)))?;
            if entry.file_type().is_file() {
                files.push(entry.into_path());
            }
        }

        let mut mismatches = Vec::new();
        for file in &files {
            let rel = relativize(&data_dir, file);
            let actual = hasher::hash(file).map_err(BurnError::Io)?;
            if exp.latest_session.get(&rel) != Some(&actual) {
                mismatches.push(rel);
            }
        }
        mismatches.sort();

        Ok(VerifyOutcome {
            files_checked: files.len(),
            mismatches,
            skipped: false,
        })
    }
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
}

impl<'a> DiscVerifier<'a> {
    pub fn new(device: BurnDevice, exec: &'a dyn Exec, operator: &'a dyn BurnOperator) -> Self {
        DiscVerifier {
            device,
            exec,
            operator,
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
        let reader = DiscReader::new(self.device.clone(), self.exec);
        reader.with_data_tree(|data_dir| {
            let mut files = Vec::new();
            for entry in walkdir::WalkDir::new(data_dir).sort_by_file_name() {
                let entry = entry.map_err(|e| BurnError::Io(std::io::Error::other(e)))?;
                if entry.file_type().is_file() {
                    files.push(entry.into_path());
                }
            }

            let mut found = BTreeMap::new();
            let mut mismatches = Vec::new();
            for file in &files {
                let rel = relativize(data_dir, file);
                let actual = hasher::hash(file).map_err(BurnError::Io)?;
                if exp.visible.get(&rel) != Some(&actual) {
                    mismatches.push(rel.clone());
                }
                found.insert(rel, actual);
            }
            let missing: Vec<RelPath> = exp
                .visible
                .keys()
                .filter(|rel| !found.contains_key(*rel))
                .cloned()
                .collect();

            let checked = files.len() + missing.len();
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
            match checked {
                Ok(actual) if actual == sv.content_hash => {}
                _ => mismatches.push(sv.rel_path.clone()),
            }
        }
        Ok((exp.superseded.len(), mismatches))
    }
}

/// The verification backends, chosen by the same decision that chooses the
/// burner (spec §5.2).
pub enum Verifier<'a> {
    Staging(StagingVerifier),
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

        let v = StagingVerifier::new(staging);
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

        let out = StagingVerifier::new(staging)
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
            StagingVerifier::new(staging)
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
        let out = StagingVerifier::new(staging)
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

    mod disc {
        use super::*;
        use crate::burn::cmd::CmdOutput;
        use crate::burn::testkit::ScriptedOperator;
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

        /// A queue of identity payloads, one per `-extract` of project.json.
        struct Toc {
            identities: RefCell<std::collections::VecDeque<&'static str>>,
        }

        impl Toc {
            fn new(ids: &[&'static str]) -> Self {
                Toc {
                    identities: RefCell::new(ids.iter().copied().collect()),
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
                    } else {
                        // A `/data` extraction produces a directory; an empty
                        // one is the right answer for an empty expectation.
                        std::fs::create_dir_all(dest).expect("writable destination");
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
            let verifier =
                DiscVerifier::new(BurnDevice::RealDevice("/dev/sr0".to_string()), &exec, &op);
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
            let outcome =
                DiscVerifier::new(BurnDevice::RealDevice("/dev/sr0".to_string()), &exec, &op)
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
            // No `visible`, no `superseded`: the disc is accepted and both
            // tiers have nothing to check. The extraction of `/data` is what
            // an image-backed run exercises (tests/xorriso_image.rs).
            let outcome = DiscVerifier::new(
                BurnDevice::ImageFile(std::path::PathBuf::from("/tmp/none.iso")),
                &exec,
                &op,
            )
            .verify(&expectation())
            .unwrap();
            assert!(!outcome.skipped);
            assert_eq!(outcome.files_checked, 0);
        }
    }
}
