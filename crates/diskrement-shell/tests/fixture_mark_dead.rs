//! `disc mark-dead` against the harvested corpus (spec §4).
//!
//! `07-after-burn3.sqlite` is the catalog the moment before the Scala build
//! marked disc 3 dead; `08-after-mark-dead.sqlite` is the moment after. The
//! Rust `mark_dead`, applied to a copy of the first at the same instant, must
//! reproduce the second row for row — which is the §11 failure this whole
//! corpus exists to catch, on the write path mark-dead adds.

mod support;

use diskrement_core::DiscStatus;
use diskrement_shell::catalog::model::ProjectId;
use diskrement_shell::catalog::{time, Catalog};
use diskrement_shell::pipeline::{Pipeline, PipelinePaths};
use diskrement_shell::progress::NoopSink;

use support::{assert_dumps_match, dump_catalog, fixture_copy, open_fixture};

/// The instant the Scala build recorded, read back out of the corpus rather
/// than typed here: nine fractional digits, which is exactly the variable
/// precision §3.1 says `Instant.toString` produces and `chrono`'s
/// `SecondsFormat::AutoSi` must reproduce.
fn harvested_event_time() -> String {
    open_fixture("catalog/08-after-mark-dead.sqlite")
        .query_row(
            "select at from disc_event where kind = 'MARKED_DEAD'",
            [],
            |row| row.get::<_, String>(0),
        )
        .expect("the corpus recorded exactly one MARKED_DEAD event")
}

#[test]
fn the_harvested_event_time_has_nanosecond_precision() {
    // If this ever stops holding, the round-trip assertion below stops
    // testing the hard case and the failure would be silent.
    assert_eq!(harvested_event_time(), "2026-09-05T20:01:13.784981123Z");
}

#[test]
fn marking_disc_3_dead_reproduces_the_scala_catalog_row_for_row() {
    let at = time::parse(&harvested_event_time()).expect("the fixture timestamp parses");

    let (_dir, path) = fixture_copy("catalog/07-after-burn3.sqlite");
    let mut catalog = Catalog::open(&path).expect("the copied catalog opens");
    let work = _dir.path().join("work");

    let report = {
        let mut pipeline = Pipeline::new(
            &mut catalog,
            PipelinePaths {
                work_dir: work.clone(),
                scan_cache: work.join("unused.scancache.sqlite"),
            },
            &NoopSink,
        );
        pipeline.mark_dead("p", 3, at).expect("mark-dead succeeds")
    };

    // Step 12 of the render oracle: nothing lost, one file to re-burn.
    assert_eq!(report.disc_ordinal, 3);
    assert!(report.lost.is_empty(), "{:?}", report.lost);
    assert_eq!(
        report
            .reburn
            .iter()
            .map(|r| (r.rel_path.as_str(), r.content_hash.as_str()))
            .collect::<Vec<_>>(),
        vec![(
            "a.bin",
            "e148cf0e8c66a7c18ea6be52a5215dd2a770d74b6c014d51d1e1e0463e882e77"
        )]
    );

    let expected = dump_catalog(&open_fixture("catalog/08-after-mark-dead.sqlite"));
    let actual = dump_catalog(catalog.raw_connection_for_tests());
    assert_dumps_match(&expected, &actual);
}

/// The unchanged-disc-row invariant, checked against the corpus: the Scala
/// build left disc 3's stored status at OPEN, and so must this one.
#[test]
fn the_dead_discs_row_still_reads_open_in_both_implementations() {
    let scala: String = open_fixture("catalog/08-after-mark-dead.sqlite")
        .query_row(
            "select status from disc where project_id = 1 and ordinal = 3",
            [],
            |row| row.get(0),
        )
        .expect("the corpus has disc 3");
    assert_eq!(scala, "OPEN");

    let at = time::parse(&harvested_event_time()).expect("the fixture timestamp parses");
    let (_dir, path) = fixture_copy("catalog/07-after-burn3.sqlite");
    let mut catalog = Catalog::open(&path).expect("the copied catalog opens");
    let work = _dir.path().join("work");
    {
        let mut pipeline = Pipeline::new(
            &mut catalog,
            PipelinePaths {
                work_dir: work.clone(),
                scan_cache: work.join("unused.scancache.sqlite"),
            },
            &NoopSink,
        );
        pipeline.mark_dead("p", 3, at).expect("mark-dead succeeds");
    }

    let rust: String = catalog
        .raw_connection_for_tests()
        .query_row(
            "select status from disc where project_id = 1 and ordinal = 3",
            [],
            |row| row.get(0),
        )
        .expect("the copy has disc 3");
    assert_eq!(rust, "OPEN", "dead-ness is derived, never stored");

    // …and the derived read must nonetheless say DEAD, or the "unchanged row"
    // half would be satisfied by simply doing nothing.
    let states = catalog.read_disc_states(ProjectId(1)).expect("states read");
    assert_eq!(
        states.iter().find(|s| s.ordinal == 3).map(|s| s.status),
        Some(DiscStatus::Dead)
    );
}
