//! Replay a harvested burn through the real pipeline and diff everything it
//! produced against what the Scala build produced at the same moment
//! (spec §4, §9's P5 row).
//!
//! Every input comes out of `fixtures/`: the pre-burn catalog, the whole work
//! dir (snapshot + staged scan + staged plan), and the `createdAt` the Scala
//! build stamped on the disc. Because the pipeline takes `now` as a parameter,
//! the comparison needs no normalization at all — committed timestamps are
//! compared as values, not as shapes.

mod support;

use std::path::{Path, PathBuf};

use diskrement_shell::burn::json::ProjectJson;
use diskrement_shell::burn::{simulated::SimulatedBurner, Burner};
use diskrement_shell::catalog::{time, Catalog};
use diskrement_shell::pipeline::{Pipeline, PipelinePaths};
use diskrement_shell::progress::NoopSink;
use support::{
    assert_dumps_match, dump_catalog, fixture_text, fixture_work_copy, fixtures_dir, open_fixture,
};

struct Replay {
    _tmp: tempfile::TempDir,
    work: PathBuf,
    catalog_path: PathBuf,
    staged_disc: PathBuf,
}

/// Run one harvested burn cycle against copies of the corpus.
fn replay(work_fixture: &str, pre_catalog: &str, disc_ordinal: u32) -> Replay {
    let (tmp, work) = fixture_work_copy(work_fixture);
    let catalog_path = tmp.path().join("catalog.sqlite");
    std::fs::copy(fixtures_dir().join(pre_catalog), &catalog_path)
        .unwrap_or_else(|e| panic!("copying {pre_catalog}: {e}"));

    // The instant the Scala build used for this burn, read back out of the
    // disc it wrote.
    let raw = fixture_text(&format!(
        "discs/disc-{disc_ordinal}/.diskrement/project.json"
    ));
    let project_json: ProjectJson = serde_json::from_str(&raw).expect("project.json parses");
    let now = time::parse(&project_json.created_at).expect("a frozen-format timestamp");

    let mut catalog = Catalog::open(&catalog_path).expect("the fixture catalog is already current");
    let sink = NoopSink;
    let burner = Burner::Simulated(SimulatedBurner::new(&sink, 2));
    {
        let mut pipeline = Pipeline::new(
            &mut catalog,
            PipelinePaths {
                work_dir: work.clone(),
                scan_cache: tmp.path().join("scancache.sqlite"),
            },
            &sink,
        );
        let report = pipeline
            .burn("p", now, &burner)
            .expect("the harvested cycle burns");
        assert_eq!(report.disc.ordinal, disc_ordinal);
    }
    drop(catalog);

    let staged_disc = work.join("staging").join(format!("disc-{disc_ordinal}"));
    Replay {
        _tmp: tmp,
        work,
        catalog_path,
        staged_disc,
    }
}

/// Every file under `dir`, relative and sorted.
fn rel_files(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(dir).follow_links(false) {
        let entry = entry.expect("readable entry");
        if entry.file_type().is_file() {
            out.push(
                entry
                    .path()
                    .strip_prefix(dir)
                    .expect("walked from dir")
                    .display()
                    .to_string(),
            );
        }
    }
    out.sort();
    out
}

/// Compare a staged disc tree against the corpus's, following
/// `fixtures/README.md`'s rules: payload and JSON byte for byte, the embedded
/// catalog logically.
fn assert_disc_matches(staged: &Path, disc_ordinal: u32) {
    let expected_root = fixtures_dir().join(format!("discs/disc-{disc_ordinal}"));

    // Payload: same set of files, same bytes.
    assert_eq!(
        rel_files(&staged.join("data")),
        rel_files(&expected_root.join("data")),
        "disc {disc_ordinal}: the staged payload has different files"
    );
    for rel in rel_files(&expected_root.join("data")) {
        assert_eq!(
            std::fs::read(staged.join("data").join(&rel)).unwrap(),
            std::fs::read(expected_root.join("data").join(&rel)).unwrap(),
            "disc {disc_ordinal}: data/{rel} differs"
        );
    }

    // Metadata JSON: byte-stable, `createdAt` included, because `now` was
    // pinned to the value the fixture carries.
    for name in ["project.json", "manifest.json", "pointers.json"] {
        assert_eq!(
            std::fs::read_to_string(staged.join(".diskrement").join(name)).unwrap(),
            std::fs::read_to_string(expected_root.join(".diskrement").join(name)).unwrap(),
            "disc {disc_ordinal}: .diskrement/{name} differs"
        );
    }

    // The embedded catalog: logically, never as bytes — SQLite file layout is
    // not stable across library builds, and xerial's SQLite is not
    // libsqlite3-sys's.
    let ours = rusqlite::Connection::open_with_flags(
        staged.join(".diskrement/catalog.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("the embedded catalog opens");
    let theirs = open_fixture(&format!(
        "discs/disc-{disc_ordinal}/.diskrement/catalog.sqlite"
    ));
    assert_dumps_match(&dump_catalog(&theirs), &dump_catalog(&ours));
}

/// Cycle 1: two 800-byte files spill across a 1000-byte disc, so disc 1 holds
/// `a.bin` alone and is sealed. The initial-add manifest shape.
#[test]
fn burning_cycle_one_reproduces_the_scala_disc_and_catalog() {
    let r = replay("work/01-after-plan1", "catalog/02-after-scan1.sqlite", 1);
    assert_disc_matches(&r.staged_disc, 1);

    let ours = rusqlite::Connection::open_with_flags(
        &r.catalog_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("the committed catalog opens");
    let theirs = open_fixture("catalog/03-after-burn1.sqlite");
    assert_dumps_match(&dump_catalog(&theirs), &dump_catalog(&ours));
}

/// Cycle 3: `a.bin` superseded and `b.bin` deleted — the only harvested cycle
/// with a non-empty `modified` list and a tombstone.
#[test]
fn burning_cycle_three_reproduces_the_scala_disc_and_catalog() {
    let r = replay("work/02-after-plan3", "catalog/06-after-scan3.sqlite", 3);
    assert_disc_matches(&r.staged_disc, 3);

    let ours = rusqlite::Connection::open_with_flags(
        &r.catalog_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("the committed catalog opens");
    let theirs = open_fixture("catalog/07-after-burn3.sqlite");
    assert_dumps_match(&dump_catalog(&theirs), &dump_catalog(&ours));
}

/// The embedded snapshot and the committed catalog are two applications of one
/// update and must agree — that equality is the whole reason the double-apply
/// exists instead of a file swap.
#[test]
fn the_embedded_snapshot_equals_the_committed_catalog() {
    let r = replay("work/01-after-plan1", "catalog/02-after-scan1.sqlite", 1);

    let committed = rusqlite::Connection::open_with_flags(
        &r.catalog_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let embedded = rusqlite::Connection::open_with_flags(
        r.staged_disc.join(".diskrement/catalog.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_dumps_match(&dump_catalog(&committed), &dump_catalog(&embedded));
}

/// Burning leaves the work dir in the state the next cycle expects: staging
/// kept (the staged-disc verifier reads it), staged JSON and snapshot gone.
#[test]
fn the_replayed_burn_leaves_the_work_dir_as_the_scala_build_did() {
    let r = replay("work/01-after-plan1", "catalog/02-after-scan1.sqlite", 1);
    assert!(r.staged_disc.is_dir(), "staging survives the burn");
    assert!(!r.work.join("staged/scan.json").exists());
    assert!(!r.work.join("staged/plan.json").exists());
    assert!(!r.work.join("snapshot").exists());
    assert!(!r.work.join("burn-catalog.sqlite").exists());
}
