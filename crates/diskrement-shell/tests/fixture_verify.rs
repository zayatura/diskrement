//! `verify` against the harvested corpus (spec §4).
//!
//! The Rust sweep reads a Scala-written catalog and Scala-staged disc trees and
//! must agree about both halves: which files each disc holds, and which
//! sessions the check is allowed to advance to VERIFIED.

mod support;

use diskrement_shell::burn::verifier::{StagingVerifier, Verifier};
use diskrement_shell::catalog::model::{ProjectId, SessionState};
use diskrement_shell::catalog::Catalog;
use diskrement_shell::error::PipelineError;
use diskrement_shell::pipeline::reports::VerifyReport;
use diskrement_shell::pipeline::{Pipeline, PipelinePaths};
use diskrement_shell::progress::NoopSink;

use support::{fixture_copy, fixture_work_copy};

struct Env {
    _catalog_dir: tempfile::TempDir,
    _work_dir: tempfile::TempDir,
    catalog: Catalog,
    work: std::path::PathBuf,
}

/// Copy one catalog snapshot and one work dir out of the corpus and open the
/// copy. Never `Catalog::open` a fixture in place: it runs the migration
/// runner and writes.
fn env(catalog_rel: &str, work_rel: &str) -> Env {
    let (catalog_dir, catalog_path) = fixture_copy(catalog_rel);
    let (work_dir, work) = fixture_work_copy(work_rel);
    let catalog = Catalog::open(&catalog_path).expect("the copied catalog opens");
    Env {
        _catalog_dir: catalog_dir,
        _work_dir: work_dir,
        catalog,
        work,
    }
}

fn sweep(env: &mut Env, disc: Option<i64>) -> Result<Vec<VerifyReport>, PipelineError> {
    let verifier = Verifier::Staging(StagingVerifier::new(env.work.join("staging")));
    let mut pipeline = Pipeline::new(
        &mut env.catalog,
        PipelinePaths {
            work_dir: env.work.clone(),
            scan_cache: env.work.join("unused.scancache.sqlite"),
        },
        &NoopSink,
    );
    pipeline.verify("p", disc, &verifier)
}

fn discs_in(env: &Env, state: SessionState) -> Vec<u32> {
    env.catalog
        .sessions_by_state(ProjectId(1), state)
        .expect("sessions")
        .iter()
        .map(|s| s.disc.ordinal)
        .collect()
}

/// The Scala build staged these trees and recorded these placements; the Rust
/// verifier must find them clean.
#[test]
fn a_sweep_over_the_corpus_staging_trees_is_clean_on_every_disc() {
    let mut env = env("catalog/05-after-burn2.sqlite", "work/02-after-plan3");
    let reports = sweep(&mut env, None).expect("verify succeeds");

    assert_eq!(
        reports
            .iter()
            .map(|r| (r.disc_ordinal, r.files_checked, r.skipped))
            .collect::<Vec<_>>(),
        vec![(1, 1, false), (2, 1, false)]
    );
    assert!(
        reports.iter().all(|r| r.mismatches.is_empty()),
        "{reports:?}"
    );
}

/// Disc 1's session was already VERIFIED when the corpus was harvested, so the
/// sweep must advance only disc 2 and leave disc 1 alone — the repeat-verify
/// no-op, pinned against real Scala-written state.
#[test]
fn the_sweep_advances_only_the_still_burned_session() {
    let mut env = env("catalog/05-after-burn2.sqlite", "work/02-after-plan3");
    assert_eq!(discs_in(&env, SessionState::Verified), vec![1]);
    assert_eq!(discs_in(&env, SessionState::Burned), vec![2]);

    sweep(&mut env, None).expect("verify succeeds");

    assert_eq!(discs_in(&env, SessionState::Verified), vec![1, 2]);
    assert!(discs_in(&env, SessionState::Burned).is_empty());
}

/// A corrupted byte in a staged file is caught, and the disc is not advanced.
#[test]
fn a_corrupted_corpus_payload_is_caught_and_blocks_the_transition() {
    let mut env = env("catalog/05-after-burn2.sqlite", "work/02-after-plan3");
    std::fs::write(env.work.join("staging/disc-2/data/b.bin"), b"CORRUPT")
        .expect("the work dir copy is writable");

    let reports = sweep(&mut env, None).expect("verify succeeds");
    let disc2 = reports
        .iter()
        .find(|r| r.disc_ordinal == 2)
        .expect("disc 2 is in the sweep");
    assert_eq!(
        disc2.mismatches,
        vec![diskrement_core::RelPath("b.bin".to_string())]
    );
    assert_eq!(
        discs_in(&env, SessionState::Burned),
        vec![2],
        "not advanced"
    );
}

/// `DEAD` is derived from `disc_event`, and the corpus's disc 3 proves it: the
/// `disc.status` column still reads OPEN there. A sweep skips it silently and
/// must leave that column exactly as it found it.
#[test]
fn a_sweep_skips_a_dead_disc_without_mentioning_it() {
    let mut env = env("catalog/08-after-mark-dead.sqlite", "work/02-after-plan3");
    let reports = sweep(&mut env, None).expect("verify succeeds");
    assert_eq!(
        reports.iter().map(|r| r.disc_ordinal).collect::<Vec<_>>(),
        vec![1, 2],
        "disc 3 is dead and must not appear"
    );

    // The stored column, not the derived state: `read_disc_states` answers DEAD
    // from `disc_event` no matter what the row says, so it could not catch an
    // illegal update-in-place here.
    let conn = env.catalog.raw_connection_for_tests();
    let status: String = conn
        .query_row("select status from disc where ordinal = 3", [], |r| {
            r.get(0)
        })
        .expect("the dead disc's row is still there");
    assert_eq!(status, "OPEN", "verify must touch no disc row");
}

/// Naming a dead disc is an assertion of intent about it, and gets an answer.
#[test]
fn naming_the_dead_disc_is_an_error_rather_than_a_silent_success() {
    let mut env = env("catalog/08-after-mark-dead.sqlite", "work/02-after-plan3");
    let err = sweep(&mut env, Some(3)).expect_err("a dead disc is refused");
    assert!(matches!(err, PipelineError::DiscIsDead(3)), "{err}");
    assert_eq!(err.to_string(), "disc 3 is marked dead");
    assert_eq!(err.exit_code(), 2);
}

/// Naming an ordinal the corpus never had is a lookup failure, not a silent exit 0.
#[test]
fn naming_an_ordinal_the_corpus_never_had_is_unknown_disc() {
    let mut env = env("catalog/05-after-burn2.sqlite", "work/02-after-plan3");
    let err = sweep(&mut env, Some(99)).expect_err("an unknown disc is refused");
    assert!(matches!(err, PipelineError::UnknownDisc(99)), "{err}");
    assert_eq!(err.exit_code(), 2);
}
