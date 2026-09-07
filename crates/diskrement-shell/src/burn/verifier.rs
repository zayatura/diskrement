//! Read-back verification (`DiscVerifier.scala`,
//! `StagingDiscVerifier.scala`).
//!
//! `Verifier` is a closed enum for the same reason `Burner` is: there are
//! exactly two substrates, ever, and `--burner real|simulated` selects both
//! arms together — verifying a simulated burn by mounting a physical disc is
//! meaningless (spec §5.2). P6 lands `Staging`; P8 adds `Disc`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use diskrement_core::{ContentHash, RelPath};

use crate::burn::BurnError;
use crate::catalog::model::SupersededVersion;
use crate::fs::hasher;
use crate::fs::scanner::relativize;

/// What the catalog says one disc should hold.
///
/// `latest_session` is what the simulated backend's staging dir contains;
/// `visible` and `superseded` describe the physical disc — the merged mount
/// view, plus the versions later sessions of the same disc hid. Each verifier
/// consumes the fields that match its substrate, so P6's staging verifier reads
/// only `latest_session` and P8's disc verifier reads only the other two.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscExpectation {
    pub project_name: String,
    pub disc_ordinal: u32,
    pub latest_session: BTreeMap<RelPath, ContentHash>,
    pub visible: BTreeMap<RelPath, ContentHash>,
    pub superseded: Vec<SupersededVersion>,
}

/// The result of checking one disc.
///
/// `skipped` means nothing was checked — no staged data, or (P8) an operator
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
            .join(format!("disc-{}", exp.disc_ordinal))
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

/// The verification backends. One variant now; P8 adds `Disc`.
pub enum Verifier {
    Staging(StagingVerifier),
}

impl Verifier {
    pub fn verify(&self, exp: &DiscExpectation) -> Result<VerifyOutcome, BurnError> {
        match self {
            Verifier::Staging(v) => v.verify(exp),
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
            disc_ordinal: ordinal,
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
            disc_ordinal: 9,
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
}
