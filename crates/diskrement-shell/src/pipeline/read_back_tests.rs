//! `burn --verify` through the pipeline (burn-verify spec §5.1, §7.2): the
//! real burner and the disc verifier on one scripted drive. A first session
//! is "mounted" at the tree the burner staged, which has a disc's layout; an
//! appended disc is mounted at a hand-built merged view.

use std::cell::{Cell, RefCell};

use chrono::TimeZone as _;

use super::*;
use crate::burn::cmd::Exec;
use crate::burn::testkit::{ScriptedExec, ScriptedOperator, APPENDABLE, BLANK};
use crate::burn::verifier::DiscVerifier;
use crate::burn::xorriso::burner::XorrisoBurner;
use crate::burn::{BurnDevice, BurnOperator};
use crate::progress::NoopSink;
use reports::DiscReadBack;

const ID_1: &str =
    r#"{"projectName":"p","discOrdinal":1,"sessionOrdinal":1,"createdAt":"2026-08-30T10:00:00Z"}"#;
const ID_1_S2: &str =
    r#"{"projectName":"p","discOrdinal":1,"sessionOrdinal":2,"createdAt":"2026-08-30T10:00:00Z"}"#;
const ID_1A: &str = r#"{"projectName":"p","discOrdinal":1,"copySet":0,"copies":2,"sessionOrdinal":1,"createdAt":"2026-08-30T10:00:00Z"}"#;
const ID_1B: &str = r#"{"projectName":"p","discOrdinal":1,"copySet":1,"copies":2,"sessionOrdinal":1,"createdAt":"2026-08-30T10:00:00Z"}"#;

struct Env {
    _dir: tempfile::TempDir,
    catalog: Catalog,
    work: PathBuf,
    scan_cache: PathBuf,
}

fn now() -> DateTime<Utc> {
    Utc.timestamp_opt(1_757_100_066, 0).unwrap()
}

fn env(copies: u32) -> Env {
    let dir = tempfile::TempDir::new().unwrap();
    let work = dir.path().join("work");
    let source = dir.path().join("source");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(&source).unwrap();
    let mut catalog = Catalog::open(&dir.path().join("catalog.sqlite")).unwrap();
    catalog
        .create_project(&ProjectInit {
            name: "p".to_string(),
            sources: Sources::Single(source),
            work_dir: work.display().to_string(),
            default_disc_type: DiscType::BdR,
            margin_percent: 2.0,
            copies,
            created_at: now(),
        })
        .unwrap();
    Env {
        scan_cache: dir.path().join("scancache.sqlite"),
        _dir: dir,
        catalog,
        work,
    }
}

fn pipeline(env: &mut Env) -> Pipeline<'_> {
    Pipeline::new(
        &mut env.catalog,
        PipelinePaths {
            work_dir: env.work.clone(),
            scan_cache: env.scan_cache.clone(),
        },
        &NoopSink,
    )
}

/// Put `files` in the snapshot and stage a plan for them against the
/// catalog as it stands, the way `plan` would.
fn stage(env: &mut Env, files: &[(&str, &[u8])]) {
    let snapshot = env.work.join("snapshot");
    std::fs::create_dir_all(&snapshot).unwrap();
    let mut scanned = Vec::new();
    for (rel, body) in files {
        let abs = snapshot.join(rel);
        std::fs::write(&abs, body).unwrap();
        scanned.push(ScannedFile {
            rel_path: RelPath((*rel).to_string()),
            content_hash: hasher::hash(&abs).unwrap(),
            size_bytes: body.len() as u64,
            mtime: now(),
        });
    }
    staged::write_scan(&env.work, &scanned).unwrap();
    pipeline(env).plan_staged("p", None, false).unwrap();
}

/// A disc as its mount shows it: `files` under `data/`, identity `id`.
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

/// `burn --verify` on the real backend at /dev/sr0.
fn burn_verifying(
    env: &mut Env,
    exec: &dyn Exec,
    op: &dyn BurnOperator,
) -> Result<BurnReport, PipelineError> {
    let device = BurnDevice::RealDevice("/dev/sr0".to_string());
    let burner = Burner::Real(XorrisoBurner::new(device.clone(), exec, op, &NoopSink, 2));
    let verifier = Verifier::Disc(DiscVerifier::new(device, exec, op, &NoopSink));
    let step = VerifyAfterBurn {
        verifier: &verifier,
        operator: op,
    };
    pipeline(env).burn_with("p", now(), &burner, Some(&step))
}

/// `burn_verifying` whose read-back may fall back to xorriso, into the
/// project's work dir, as `burn --verify` sets it up (spec §8).
fn burn_verifying_with_fallback(
    env: &mut Env,
    exec: &dyn Exec,
    op: &dyn BurnOperator,
) -> Result<BurnReport, PipelineError> {
    let device = BurnDevice::RealDevice("/dev/sr0".to_string());
    let burner = Burner::Real(XorrisoBurner::new(device.clone(), exec, op, &NoopSink, 2));
    let verifier = Verifier::Disc(
        DiscVerifier::new(device, exec, op, &NoopSink).reading_back_through_xorriso_in(&env.work),
    );
    let step = VerifyAfterBurn {
        verifier: &verifier,
        operator: op,
    };
    pipeline(env).burn_with("p", now(), &burner, Some(&step))
}

/// `(disc ordinal, copy set, session ordinal)` of every session in `state`.
fn sessions(env: &Env, state: SessionState) -> Vec<(u32, u32, u32)> {
    let mut out: Vec<(u32, u32, u32)> = env
        .catalog
        .sessions_by_state(ProjectId(1), state)
        .unwrap()
        .iter()
        .map(|s| (s.disc.ordinal, s.disc.copy_set, s.ordinal))
        .collect();
    out.sort_unstable();
    out
}

/// Records every command in order, then answers from `script`.
fn recording<'a>(script: &'a ScriptedExec, calls: &'a RefCell<Vec<String>>) -> impl Exec + 'a {
    move |c: &[String], l: &mut dyn FnMut(&str)| {
        calls.borrow_mut().push(c.join(" "));
        script.respond(c, l)
    }
}

/// Session 1 of disc 1, burned and read back clean.
fn burn_first_session(env: &mut Env) {
    stage(env, &[("a.bin", b"aaaa")]);
    let tree = env.work.join("staging/disc-1");
    let script = ScriptedExec::new()
        .toc(&[BLANK, APPENDABLE])
        .identities(&[Some(ID_1), Some(ID_1)])
        .mounts(&[Some(&tree)]);
    let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
    burn_verifying(env, &exec, &ScriptedOperator::new(&[true])).unwrap();
}

#[test]
fn a_disc_that_reads_back_clean_is_committed_verified_and_ejected_after() {
    let mut env = env(1);
    stage(&mut env, &[("a.bin", b"aaaa")]);
    let tree = env.work.join("staging/disc-1");
    let script = ScriptedExec::new()
        .toc(&[BLANK, APPENDABLE])
        .identities(&[Some(ID_1), Some(ID_1)])
        .mounts(&[Some(&tree)]);
    let calls = RefCell::new(Vec::new());
    let exec = recording(&script, &calls);
    let op = ScriptedOperator::new(&[true]);

    let report = burn_verifying(&mut env, &exec, &op).unwrap();

    assert_eq!(
        report.discs[0].read_back,
        Some(DiscReadBack::Verified {
            files: 1,
            through_xorriso: false
        })
    );
    assert_eq!(sessions(&env, SessionState::Verified), vec![(1, 0, 1)]);
    assert!(sessions(&env, SessionState::Burned).is_empty());
    let calls = calls.borrow();
    let mount = calls
        .iter()
        .position(|c| c.starts_with("udisksctl mount"))
        .expect("a read-back");
    let ejects: Vec<usize> = calls
        .iter()
        .enumerate()
        .filter(|(_, c)| c.contains("-eject"))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(ejects.len(), 1, "{calls:?}");
    assert!(
        ejects[0] > mount,
        "ejected only after the read-back: {calls:?}"
    );
    assert!(!op.said("Reinsert"), "{:?}", op.log());
}

#[test]
fn a_failed_first_session_is_not_recorded_and_a_retry_on_a_new_blank_passes() {
    let mut env = env(1);
    stage(&mut env, &[("a.bin", b"aaaa")]);
    let tree = env.work.join("staging/disc-1");
    let script = ScriptedExec::new()
        .toc(&[BLANK, APPENDABLE, BLANK, APPENDABLE])
        .identities(&[Some(ID_1), Some(ID_1), Some(ID_1), Some(ID_1)])
        .mounts(&[Some(&tree), Some(&tree)]);
    let burns = Cell::new(0);
    // The first disc comes out bad; the retry, restaged from the snapshot, is good.
    let exec = |c: &[String], l: &mut dyn FnMut(&str)| {
        if c.iter().any(|a| a == "-commit") {
            burns.set(burns.get() + 1);
            if burns.get() == 1 {
                std::fs::write(tree.join("data/a.bin"), b"rot!").unwrap();
            }
        }
        script.respond(c, l)
    };
    // The first blank, then the new blank for the retry.
    let op = ScriptedOperator::new(&[true, true]);

    let report = burn_verifying(&mut env, &exec, &op).unwrap();

    assert_eq!(burns.get(), 2);
    assert!(
        op.said(
            "Disc 1 FAILED verification: 1 of 1 files bad (a.bin). It is not recorded; discard it."
        ),
        "{:?}",
        op.log()
    );
    assert!(op.said("To retry disc 1, insert a new blank disc when asked; decline to stop."));
    assert_eq!(
        report.discs[0].read_back,
        Some(DiscReadBack::Verified {
            files: 1,
            through_xorriso: false
        })
    );
    assert_eq!(sessions(&env, SessionState::Verified), vec![(1, 0, 1)]);
    assert!(
        sessions(&env, SessionState::Burned).is_empty(),
        "exactly one session, the retry's"
    );
}

#[test]
fn declining_the_retry_stops_with_verify_failed_and_records_nothing() {
    let mut env = env(1);
    stage(&mut env, &[("a.bin", b"aaaa")]);
    let tree = env.work.join("staging/disc-1");
    let script = ScriptedExec::new()
        .toc(&[BLANK, APPENDABLE])
        .identities(&[Some(ID_1), Some(ID_1)])
        .mounts(&[Some(&tree)]);
    let exec = |c: &[String], l: &mut dyn FnMut(&str)| {
        if c.iter().any(|a| a == "-commit") {
            std::fs::write(tree.join("data/a.bin"), b"rot!").unwrap();
        }
        script.respond(c, l)
    };
    let op = ScriptedOperator::new(&[true, false]);

    let err = burn_verifying(&mut env, &exec, &op).unwrap_err();

    assert!(
        matches!(
            &err,
            PipelineError::VerifyFailed { disc, session: 1, bad: 1, checked: 1, appended: false }
                if disc == "1"
        ),
        "{err:?}"
    );
    assert_eq!(err.exit_code(), 4);
    assert!(sessions(&env, SessionState::Burned).is_empty());
    assert!(sessions(&env, SessionState::Verified).is_empty());
    assert!(script.ran("-eject"), "the failed disc is ejected");
    assert!(
        env.work.join("staged/plan.json").is_file(),
        "the plan survives for the next attempt"
    );
}

#[test]
fn a_failed_append_stops_without_a_retry_and_says_to_mark_the_disc_dead() {
    let mut env = env(1);
    burn_first_session(&mut env);
    stage(&mut env, &[("a.bin", b"aaaa"), ("b.bin", b"bbbb")]);
    let disc = mounted(&[("a.bin", b"aaaa"), ("b.bin", b"rot!")], ID_1_S2);
    let script = ScriptedExec::new()
        .toc(&[APPENDABLE, APPENDABLE])
        .identities(&[Some(ID_1), Some(ID_1_S2), Some(ID_1_S2)])
        .mounts(&[Some(disc.path())]);
    let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
    let op = ScriptedOperator::new(&[true]);

    let err = burn_verifying(&mut env, &exec, &op).unwrap_err();

    assert!(
        matches!(
            err,
            PipelineError::VerifyFailed {
                session: 2,
                bad: 1,
                checked: 2,
                appended: true,
                ..
            }
        ),
        "{err:?}"
    );
    assert!(err.to_string().contains("run disc mark-dead 1"), "{err}");
    assert!(!op.said("To retry"), "{:?}", op.log());
    assert_eq!(
        sessions(&env, SessionState::Verified),
        vec![(1, 0, 1)],
        "session 2 was not recorded"
    );
    assert!(sessions(&env, SessionState::Burned).is_empty());
}

#[test]
fn a_mount_still_showing_the_last_session_asks_for_the_disc_again() {
    let mut env = env(1);
    burn_first_session(&mut env);
    stage(&mut env, &[("a.bin", b"aaaa"), ("b.bin", b"bbbb")]);
    let stale = mounted(&[("a.bin", b"aaaa")], ID_1);
    let fresh = mounted(&[("a.bin", b"aaaa"), ("b.bin", b"bbbb")], ID_1_S2);
    let script = ScriptedExec::new()
        .toc(&[APPENDABLE, APPENDABLE, APPENDABLE])
        .identities(&[Some(ID_1), Some(ID_1_S2), Some(ID_1_S2), Some(ID_1_S2)])
        .mounts(&[Some(stale.path()), Some(fresh.path())]);
    let calls = RefCell::new(Vec::new());
    let exec = recording(&script, &calls);
    // Insert for the burn, then reinsert after the stale mount.
    let op = ScriptedOperator::new(&[true, true]);

    let report = burn_verifying(&mut env, &exec, &op).unwrap();

    assert!(
        op.said("Couldn't verify disc 1: the mounted disc still shows session 1"),
        "{:?}",
        op.log()
    );
    assert_eq!(
        report.discs[0].read_back,
        Some(DiscReadBack::Verified {
            files: 2,
            through_xorriso: false
        })
    );
    assert_eq!(
        sessions(&env, SessionState::Verified),
        vec![(1, 0, 1), (1, 0, 2)]
    );
    let ejects = calls
        .borrow()
        .iter()
        .filter(|c| c.contains("-eject"))
        .count();
    assert_eq!(ejects, 2, "one before the reinsert, one when done");
}

/// A read-back that fails for a reason outside the disc (here xorriso's
/// output cannot be parsed) asks for the disc again, like an unverifiable
/// one, and never discards it (spec §4.2).
#[test]
fn a_read_back_error_asks_for_the_disc_again() {
    let mut env = env(1);
    stage(&mut env, &[("a.bin", b"aaaa")]);
    let tree = env.work.join("staging/disc-1");
    let garbage: &[&str] = &["garbage"];
    let script = ScriptedExec::new()
        .toc(&[BLANK, garbage, APPENDABLE])
        .identities(&[Some(ID_1), Some(ID_1)])
        .mounts(&[Some(&tree)]);
    let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
    let op = ScriptedOperator::new(&[true, true]);

    let report = burn_verifying(&mut env, &exec, &op).unwrap();

    assert!(
        op.said("Couldn't verify disc 1: xorriso failed"),
        "{:?}",
        op.log()
    );
    assert_eq!(
        report.discs[0].read_back,
        Some(DiscReadBack::Verified {
            files: 1,
            through_xorriso: false
        })
    );
    assert_eq!(sessions(&env, SessionState::Verified), vec![(1, 0, 1)]);
}

/// The disc the kernel cannot mount yet is read through xorriso: no
/// reinsert prompt, recorded VERIFIED, ejected once, nothing left in the work
/// dir (spec §8).
#[test]
fn a_disc_the_kernel_cannot_mount_reads_back_through_xorriso_without_a_reinsert() {
    let mut env = env(1);
    stage(&mut env, &[("a.bin", b"aaaa")]);
    let files: &[(&str, &[u8])] = &[("a.bin", b"aaaa")];
    let script = ScriptedExec::new()
        .toc(&[BLANK, APPENDABLE])
        .identities(&[Some(ID_1), Some(ID_1)])
        .mounts(&[None])
        .extractions(&[Some(files)]);
    let calls = RefCell::new(Vec::new());
    let exec = recording(&script, &calls);
    let op = ScriptedOperator::new(&[true]);

    let report = burn_verifying_with_fallback(&mut env, &exec, &op).unwrap();

    assert_eq!(
        report.discs[0].read_back,
        Some(DiscReadBack::Verified {
            files: 1,
            through_xorriso: true
        })
    );
    assert!(!op.said("Reinsert"), "{:?}", op.log());
    assert!(
        op.said("Reading it through xorriso instead"),
        "{:?}",
        op.log()
    );
    assert_eq!(sessions(&env, SessionState::Verified), vec![(1, 0, 1)]);
    let ejects = calls
        .borrow()
        .iter()
        .filter(|c| c.contains("-eject"))
        .count();
    assert_eq!(ejects, 1);
    assert!(!env.work.join("readback").exists());
}

#[test]
fn declining_to_reinsert_records_the_disc_unverified() {
    let mut env = env(1);
    stage(&mut env, &[("a.bin", b"aaaa")]);
    let script = ScriptedExec::new()
        .toc(&[BLANK, APPENDABLE])
        .identities(&[Some(ID_1), Some(ID_1)])
        .mounts(&[None]);
    let calls = RefCell::new(Vec::new());
    let exec = recording(&script, &calls);
    let op = ScriptedOperator::new(&[true, false]);

    let report = burn_verifying(&mut env, &exec, &op).unwrap();

    assert!(
        op.said("Couldn't verify disc 1: could not mount it"),
        "{:?}",
        op.log()
    );
    assert_eq!(report.discs[0].read_back, Some(DiscReadBack::Unverified));
    assert_eq!(sessions(&env, SessionState::Burned), vec![(1, 0, 1)]);
    assert!(sessions(&env, SessionState::Verified).is_empty());
    let ejects = calls
        .borrow()
        .iter()
        .filter(|c| c.contains("-eject"))
        .count();
    assert_eq!(ejects, 1, "ejected for the reinsert, and not again");
}

#[test]
fn a_twin_that_fails_after_its_sibling_committed_is_a_partial_burn() {
    let mut env = env(2);
    stage(&mut env, &[("a.bin", b"aaaa")]);
    let tree_a = env.work.join("staging/disc-1A");
    let tree_b = env.work.join("staging/disc-1B");
    let script = ScriptedExec::new()
        .toc(&[BLANK, APPENDABLE, BLANK, APPENDABLE])
        .identities(&[Some(ID_1A), Some(ID_1A), Some(ID_1B), Some(ID_1B)])
        .mounts(&[Some(&tree_a), Some(&tree_b)]);
    let burns = Cell::new(0);
    let exec = |c: &[String], l: &mut dyn FnMut(&str)| {
        if c.iter().any(|a| a == "-commit") {
            burns.set(burns.get() + 1);
            if burns.get() == 2 {
                std::fs::write(tree_b.join("data/a.bin"), b"rot!").unwrap();
            }
        }
        script.respond(c, l)
    };
    // 1A's blank, 1B's blank, then decline 1B's retry.
    let op = ScriptedOperator::new(&[true, true, false]);

    let err = burn_verifying(&mut env, &exec, &op).unwrap_err();

    assert_eq!(err.exit_code(), 4);
    let text = err.to_string();
    assert!(
        text.ends_with("1A committed; 1B failed verification and was not recorded (re-run burn)"),
        "{text}"
    );
    assert!(
        !text.contains("reset") && !text.contains("not written"),
        "{text}"
    );
    assert_eq!(sessions(&env, SessionState::Verified), vec![(1, 0, 1)]);
    assert!(sessions(&env, SessionState::Burned).is_empty());
}
