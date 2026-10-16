//! The real xorriso path, end to end, against `stdio:` image files
//! (`XorrisoImageITSpec.scala`, plus the image halves of `DiscReaderSpec` and
//! `RealDiscVerifierSpec`).
//!
//! Every test here spawns the actual tool. When `xorriso` is not installed
//! they skip — *loudly*, naming the reason and the fix (spec §7): a suite that
//! silently checked nothing is exactly the failure mode CLAUDE.md records for
//! the sbt IT suite.

use std::collections::BTreeMap;
use std::path::Path;

use diskrement_core::{DiscKey, RelPath};
use diskrement_shell::burn::cmd::{self, CmdOutput, Exec};
use diskrement_shell::burn::media::{MediaInspector, MediaStatus};
use diskrement_shell::burn::reader::DiscReader;
use diskrement_shell::burn::testkit::{has_xorriso, image_of, sha256, ScriptedOperator};
use diskrement_shell::burn::verifier::{DiscExpectation, DiscVerifier, VerifyOutcome};
use diskrement_shell::burn::xorriso::{args, burner::XorrisoBurner};
use diskrement_shell::burn::{BurnDevice, BurnError};
use diskrement_shell::catalog::model::SupersededVersion;
use diskrement_shell::progress::NoopSink;

/// `false` — with a printed reason — when this machine cannot run the suite.
fn ready(test: &str) -> bool {
    if has_xorriso() {
        return true;
    }
    eprintln!(
        "SKIP [{test}]: xorriso is not installed, so the real burn path cannot be \
         exercised here. Install the libisoburn/xorriso package and re-run."
    );
    false
}

/// The production `Exec`.
///
/// A `fn` pointer rather than a closure: the blanket impl in `burn::cmd` is
/// higher-ranked over the two reference arguments, and a `fn` pointer type
/// satisfies it for every lifetime with no inference to go wrong.
type SystemExec = fn(&[String], &mut dyn FnMut(&str)) -> Result<CmdOutput, BurnError>;
const SYSTEM_EXEC: SystemExec = cmd::system;

/// Burn one session of `files` onto `iso`, through the real burner.
fn burn_session(base: &Path, iso: &Path, session: u32, files: &[(&str, &str)]) {
    let exec = SYSTEM_EXEC;
    let operator = ScriptedOperator::new(&[true]);
    let burner = XorrisoBurner::new(
        BurnDevice::ImageFile(iso.to_path_buf()),
        &exec,
        &operator,
        &NoopSink,
        2,
    );
    let result = burner
        .burn(&image_of(base, session, files, false))
        .unwrap_or_else(|e| panic!("session {session} failed: {e}"));
    assert!(result.image_bytes > 0, "session {session} wrote nothing");
}

/// The core claim: two sessions, the second hiding a path the first wrote; the
/// merged view shows the new version and the old session still holds the old
/// one. That is what makes a top-off safe and tier-2 verification possible.
#[test]
fn a_second_session_hides_the_first_version_without_destroying_it() {
    if !ready("a_second_session_hides_the_first_version_without_destroying_it") {
        return;
    }
    let dir = tempfile::TempDir::new().expect("temp dir");
    let iso = dir.path().join("disc.iso");
    let exec = SYSTEM_EXEC;
    let inspector = MediaInspector::new(BurnDevice::ImageFile(iso.clone()), &exec);

    burn_session(
        dir.path(),
        &iso,
        1,
        &[("a.txt", "hello v1"), ("b.txt", "stable")],
    );
    match inspector.inspect().expect("the image inspects") {
        MediaStatus::Appendable {
            sessions,
            identity: Some(id),
            ..
        } => {
            assert_eq!(sessions, 1);
            assert_eq!(id.session_ordinal, 1);
        }
        other => panic!("after session 1: {other}"),
    }

    burn_session(dir.path(), &iso, 2, &[("a.txt", "hello v2")]);
    match inspector.inspect().expect("the image inspects") {
        MediaStatus::Appendable {
            sessions,
            identity: Some(id),
            ..
        } => {
            assert_eq!(sessions, 2);
            assert_eq!(id.session_ordinal, 2);
        }
        other => panic!("after session 2: {other}"),
    }

    let device = BurnDevice::ImageFile(iso.clone());
    let merged = dir.path().join("merged-a.txt");
    let out: CmdOutput = exec
        .run_quiet(&args::extract(&device, "/data/a.txt", &merged, None))
        .expect("the extraction runs");
    assert_eq!(out.exit_code, 0, "{:?}", out.lines);
    assert_eq!(
        std::fs::read_to_string(&merged).expect("read"),
        "hello v2",
        "the merged view shows the newest version"
    );

    let old = dir.path().join("old-a.txt");
    let out = exec
        .run_quiet(&args::extract(&device, "/data/a.txt", &old, Some(1)))
        .expect("the extraction runs");
    assert_eq!(out.exit_code, 0, "{:?}", out.lines);
    assert_eq!(
        std::fs::read_to_string(&old).expect("read"),
        "hello v1",
        "session 1 still holds the version session 2 hid"
    );
}

/// A top-off onto a disc at the wrong session is refused, re-prompted, and —
/// when the operator gives up — aborted. Nothing is written.
#[test]
fn a_top_off_at_the_wrong_session_is_refused_until_the_operator_gives_up() {
    if !ready("a_top_off_at_the_wrong_session_is_refused_until_the_operator_gives_up") {
        return;
    }
    let dir = tempfile::TempDir::new().expect("temp dir");
    let iso = dir.path().join("foreign.iso");
    burn_session(dir.path(), &iso, 1, &[("x.txt", "other")]);

    let exec = SYSTEM_EXEC;
    // The image identifies as photos/disc 1/session 1; burning session 3
    // expects to find session 2. One retry, then decline.
    let operator = ScriptedOperator::new(&[true, false]);
    let err = XorrisoBurner::new(
        BurnDevice::ImageFile(iso.clone()),
        &exec,
        &operator,
        &NoopSink,
        2,
    )
    .burn(&image_of(dir.path(), 3, &[("y.txt", "new")], false))
    .expect_err("the wrong disc is never written");
    assert!(matches!(err, BurnError::Aborted), "{err}");
    assert!(
        operator.said("disc 'photos' #1 at session 1"),
        "the operator is told what was actually in the drive: {:?}",
        operator.log()
    );
}

/// `DiscReader` over an image: the merged tree for tier 1, one session's copy
/// for tier 2.
#[test]
fn the_reader_exposes_the_merged_tree_and_a_single_session() {
    if !ready("the_reader_exposes_the_merged_tree_and_a_single_session") {
        return;
    }
    let dir = tempfile::TempDir::new().expect("temp dir");
    let iso = dir.path().join("disc.iso");
    burn_session(dir.path(), &iso, 1, &[("a.txt", "v1")]);
    burn_session(dir.path(), &iso, 2, &[("a.txt", "v2")]);

    let exec = SYSTEM_EXEC;
    let reader = DiscReader::new(BurnDevice::ImageFile(iso.clone()), &exec);
    let merged = reader
        .with_data_tree(|data| std::fs::read_to_string(data.join("a.txt")).map_err(BurnError::Io))
        .expect("the tree is readable");
    assert_eq!(merged, "v2");

    let old = dir.path().join("old.txt");
    reader
        .extract_from_session(1, "/data/a.txt", &old)
        .expect("session 1 is readable");
    assert_eq!(std::fs::read_to_string(&old).expect("read"), "v1");
}

/// Two-tier verification against a real image: a clean disc passes, a
/// tampered expectation mismatches in both tiers, and a declined disc is
/// skipped without being read.
#[test]
fn two_tier_verification_passes_clean_mismatches_tampered_and_skips_declined() {
    if !ready("two_tier_verification_passes_clean_mismatches_tampered_and_skips_declined") {
        return;
    }
    let dir = tempfile::TempDir::new().expect("temp dir");
    let iso = dir.path().join("disc.iso");
    burn_session(dir.path(), &iso, 1, &[("a.txt", "v1"), ("b.txt", "keep")]);
    burn_session(dir.path(), &iso, 2, &[("a.txt", "v2")]);

    let expectation = DiscExpectation {
        project_name: "photos".to_string(),
        disc: DiscKey::new(1, 0),
        disc_name: "1".to_string(),
        latest_session: BTreeMap::new(),
        visible: BTreeMap::from([
            (RelPath("a.txt".to_string()), sha256("v2")),
            (RelPath("b.txt".to_string()), sha256("keep")),
        ]),
        visible_bytes: 6,
        superseded: vec![SupersededVersion {
            session_ordinal: 1,
            rel_path: RelPath("a.txt".to_string()),
            content_hash: sha256("v1"),
            size_bytes: 2,
        }],
    };

    let exec = SYSTEM_EXEC;
    let device = BurnDevice::ImageFile(iso.clone());

    let accept = ScriptedOperator::new(&[true]);
    let clean = DiscVerifier::new(device.clone(), &exec, &accept, &NoopSink)
        .verify(&expectation)
        .expect("verification runs");
    assert_eq!(
        clean,
        VerifyOutcome {
            files_checked: 3, // two visible + one superseded
            mismatches: Vec::new(),
            skipped: false,
        }
    );

    let mut tampered = expectation.clone();
    tampered
        .visible
        .insert(RelPath("b.txt".to_string()), sha256("TAMPERED"));
    tampered.superseded = vec![SupersededVersion {
        session_ordinal: 1,
        rel_path: RelPath("a.txt".to_string()),
        content_hash: sha256("WRONG"),
        size_bytes: 2,
    }];
    let accept = ScriptedOperator::new(&[true]);
    let bad = DiscVerifier::new(device.clone(), &exec, &accept, &NoopSink)
        .verify(&tampered)
        .expect("verification runs");
    assert_eq!(
        bad.mismatches,
        vec![RelPath("a.txt".to_string()), RelPath("b.txt".to_string())],
        "both tiers report, sorted"
    );

    let decline = ScriptedOperator::new(&[false]);
    let skipped = DiscVerifier::new(device, &exec, &decline, &NoopSink)
        .verify(&expectation)
        .expect("verification runs");
    assert_eq!(
        skipped,
        VerifyOutcome {
            files_checked: 0,
            mismatches: Vec::new(),
            skipped: true,
        }
    );
}

/// A file the catalog expects and the disc does not have is a mismatch, not a
/// silent pass — the case a hash-only comparison misses by construction.
#[test]
fn a_file_missing_from_the_disc_is_a_mismatch() {
    if !ready("a_file_missing_from_the_disc_is_a_mismatch") {
        return;
    }
    let dir = tempfile::TempDir::new().expect("temp dir");
    let iso = dir.path().join("disc.iso");
    burn_session(dir.path(), &iso, 1, &[("only.txt", "here")]);

    let exec = SYSTEM_EXEC;
    let operator = ScriptedOperator::new(&[true]);
    let outcome = DiscVerifier::new(BurnDevice::ImageFile(iso), &exec, &operator, &NoopSink)
        .verify(&DiscExpectation {
            project_name: "photos".to_string(),
            disc: DiscKey::new(1, 0),
            disc_name: "1".to_string(),
            latest_session: BTreeMap::new(),
            visible: BTreeMap::from([
                (RelPath("only.txt".to_string()), sha256("here")),
                (RelPath("ghost.txt".to_string()), sha256("missing")),
            ]),
            visible_bytes: 11,
            superseded: Vec::new(),
        })
        .expect("verification runs");
    assert_eq!(outcome.mismatches, vec![RelPath("ghost.txt".to_string())]);
    assert_eq!(outcome.files_checked, 2, "one present plus one missing");
}
