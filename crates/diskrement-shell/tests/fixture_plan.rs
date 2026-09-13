//! Re-plan the harvested cycles from the corpus and diff the result against
//! the plan the Scala build staged at the same moment (spec §4).
//!
//! Every input comes out of `fixtures/`: the catalog state, the staged scan,
//! the project's disc type and margin, and the harvest's capacity override.
//! Nothing here is a value typed into this file and asserted against itself.

mod support;

use std::path::PathBuf;

use diskrement_core::Capacity;
use diskrement_shell::catalog::Catalog;
use diskrement_shell::pipeline::planning::{effective_capacity, plan_from};
use diskrement_shell::pipeline::reports::PlanReport;
use diskrement_shell::pipeline::staged;
use support::{fixture_copy, fixtures_dir};

/// `DISKREMENT_DISC_BYTES` for the harvest run (`fixtures/README.md`).
const HARVEST_DISC_BYTES: u64 = 1_000;

/// Re-plan one harvested moment. Returns (what Rust plans, what Scala staged).
fn replan(catalog_fixture: &str, work_fixture: &str) -> (PlanReport, PlanReport) {
    let (_tmp, catalog_path) = fixture_copy(catalog_fixture);
    let catalog = Catalog::open(&catalog_path).expect("the fixture catalog is already current");
    let project = catalog
        .find_project("p")
        .expect("readable")
        .expect("the harvest project is named p");
    let work: PathBuf = fixtures_dir().join(work_fixture);

    let scanned = staged::read_scan(&work)
        .expect("the fixture staged scan decodes")
        .expect("the fixture staged a scan");
    let view = catalog.read_view(project.id).expect("readable");
    let states = catalog.read_disc_states(project.id).expect("readable");
    let cap = effective_capacity(
        Capacity::from_disc_type(project.default_disc_type, project.margin_percent),
        Some(HARVEST_DISC_BYTES),
    );
    let ours = plan_from(
        &view,
        &scanned,
        &states,
        project.default_disc_type,
        cap,
        false,
    );
    let theirs = staged::read_plan(&work)
        .expect("the fixture staged plan decodes")
        .expect("the fixture staged a plan");
    // `_tmp` is a real binding, so the temp catalog outlives every read
    // above; drop order takes `catalog` down before it.
    (ours, theirs)
}

/// Cycle 1: two 800-byte files against a 1000-byte disc. The plan spills, so
/// this cycle's disc is sealed and one disc's worth remains.
#[test]
fn replanning_cycle_one_reproduces_the_scala_staged_plan() {
    let (ours, theirs) = replan("catalog/02-after-scan1.sqlite", "work/01-after-plan1");
    assert_eq!(ours, theirs);
    // Spelled out, so a failure says which property drifted rather than
    // printing two structs.
    assert_eq!(ours.disc.ordinal, 1);
    assert!(ours.disc.is_new);
    assert!(ours.disc.will_finalize, "the plan spilled");
    assert_eq!(ours.remaining.more_discs, 1);
    assert_eq!(ours.remaining.more_bytes, 800);
    assert!(ours.tombstones.is_empty());
}

/// Cycle 2: `a.bin` is superseded and `b.bin` deleted. Disc 2 is OPEN with
/// 200 bytes free, which the 800-byte new version cannot top off — so the
/// cycle plans a fresh disc 3 rather than burning an empty finalizing
/// placeholder, and the deletion rides along as a tombstone.
#[test]
fn replanning_cycle_two_reproduces_the_scala_staged_plan() {
    let (ours, theirs) = replan("catalog/06-after-scan3.sqlite", "work/02-after-plan3");
    assert_eq!(ours, theirs);
    assert_eq!(ours.disc.ordinal, 3, "the empty-open-disc skip");
    assert!(ours.disc.is_new);
    assert!(!ours.disc.will_finalize);
    assert_eq!(
        ours.tombstones
            .iter()
            .map(|p| p.as_str())
            .collect::<Vec<_>>(),
        vec!["b.bin"]
    );
    assert_eq!(ours.remaining.more_discs, 0);
}

/// The exit-bar claim in the spec's own words: the staged JSON is semantically
/// identical to the fixture's. Field order is not an oracle (spec §4), so the
/// comparison is between parsed values.
#[test]
fn the_staged_plan_json_is_semantically_identical_to_the_fixtures() {
    for (catalog_fixture, work_fixture) in [
        ("catalog/02-after-scan1.sqlite", "work/01-after-plan1"),
        ("catalog/06-after-scan3.sqlite", "work/02-after-plan3"),
    ] {
        let (ours, _) = replan(catalog_fixture, work_fixture);
        let out = tempfile::TempDir::new().expect("a writable temp directory");
        staged::write_plan(out.path(), &ours).expect("the plan stages");

        let mine: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(out.path().join("staged/plan.json")).unwrap(),
        )
        .unwrap();
        let theirs: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(fixtures_dir().join(work_fixture).join("staged/plan.json"))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(mine, theirs, "{work_fixture}");
    }
}
