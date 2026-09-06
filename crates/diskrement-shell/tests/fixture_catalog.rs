//! Every read method, against catalogs the Scala build actually wrote.
//!
//! The scenario behind the corpus (see `fixtures/README.md`): two 800-byte
//! files spill across discs 1-2, a modify+delete cycle lands a@2 on disc 3,
//! disc 3 is marked dead, a recovery burn puts a@2 on disc 4, and
//! `plan --finalize` seals disc 4 with a file-less session.

mod support;

use std::collections::BTreeMap;

use diskrement_core::{ContentHash, DiscState, DiscStatus, ManifestChangeKind, RelPath};
use diskrement_shell::catalog::model::{
    ArchivePointer, FileHistoryEntry, SessionState, VersionStatus,
};
use diskrement_shell::catalog::{time, Catalog};
use support::{fixture_copy, fixture_dbs};

const A1: &str = "8e19511b7991430a1950f27907234183151bb0dbf1fbf28cd5b765088feb9f70";
const A2: &str = "e148cf0e8c66a7c18ea6be52a5215dd2a770d74b6c014d51d1e1e0463e882e77";
const B1: &str = "3413ff9e166e40ad8798e36609f28433496ee3a0f8984f73d33b0a80ffef2a89";

fn open(rel: &str) -> (tempfile::TempDir, Catalog) {
    let (dir, path) = fixture_copy(rel);
    let catalog = Catalog::open(&path).expect("a Scala-written catalog opens");
    (dir, catalog)
}

fn rel(path: &str) -> RelPath {
    RelPath(path.to_string())
}

fn hash(hex: &str) -> ContentHash {
    ContentHash(hex.to_string())
}

#[test]
fn every_fixture_catalog_opens_and_reads_without_error() {
    let catalogs = fixture_dbs("catalog");
    assert_eq!(
        catalogs.len(),
        11,
        "the corpus should hold 11 catalog snapshots"
    );
    for name in &catalogs {
        let (_dir, cat) = open(name);
        let project = cat
            .find_project("p")
            .unwrap_or_else(|e| panic!("{name}: {e}"))
            .unwrap_or_else(|| panic!("{name}: the harvest project is missing"));
        cat.read_view(project.id)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        cat.read_disc_states(project.id)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        cat.fingerprints(project.id)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        cat.archive_pointers(project.id)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
    }
}

#[test]
fn the_initial_catalog_holds_a_project_and_no_discs() {
    let (_dir, cat) = open("catalog/01-after-init.sqlite");
    let project = cat
        .find_project("p")
        .expect("query")
        .expect("project exists");
    assert_eq!(project.default_disc_type.as_str(), "BD_R");
    assert_eq!(project.copies, 1);
    assert_eq!(project.margin_percent, 2.0);
    // A nine-digit fractional timestamp, written by the Scala build and read
    // back through `catalog::time` unchanged.
    assert_eq!(
        time::format(&project.created_at),
        "2026-09-05T20:01:04.016371777Z"
    );

    assert!(cat.read_view(project.id).expect("view").current.is_empty());
    assert!(cat.read_disc_states(project.id).expect("states").is_empty());
    assert!(cat
        .sessions_by_state(project.id, SessionState::Burned)
        .expect("sessions")
        .is_empty());
}

#[test]
fn the_catalog_after_the_first_burn_reports_one_full_disc() {
    let (_dir, cat) = open("catalog/03-after-burn1.sqlite");
    let project = cat
        .find_project("p")
        .expect("query")
        .expect("project exists");

    let view = cat.read_view(project.id).expect("view");
    assert_eq!(view.current.len(), 1);
    assert_eq!(view.current[&rel("a.bin")].content_hash, hash(A1));
    assert_eq!(view.current[&rel("a.bin")].size_bytes, 800);

    assert_eq!(
        cat.read_disc_states(project.id).expect("states"),
        vec![DiscState {
            ordinal: 1,
            status: DiscStatus::Finalized,
            usable_bytes: 1000,
            used_bytes: 800,
        }]
    );

    let burned = cat
        .sessions_by_state(project.id, SessionState::Burned)
        .expect("sessions");
    assert_eq!(burned.len(), 1);
    assert_eq!((burned[0].disc_ordinal, burned[0].ordinal), (1, 1));
    assert_eq!(
        burned[0].burned_at.map(|at| time::format(&at)).as_deref(),
        Some("2026-09-05T20:01:06.623959846Z")
    );

    let fingerprints = cat.fingerprints(project.id).expect("fingerprints");
    assert_eq!(fingerprints.len(), 1);
    assert_eq!(fingerprints[&rel("a.bin")].content_hash, hash(A1));
    assert_eq!(
        time::format(&fingerprints[&rel("a.bin")].mtime),
        "2026-09-05T20:01:03.260Z"
    );
}

#[test]
fn a_marked_dead_disc_reads_dead_without_its_row_having_changed() {
    let (_dir, cat) = open("catalog/08-after-mark-dead.sqlite");
    let project = cat
        .find_project("p")
        .expect("query")
        .expect("project exists");

    assert_eq!(
        cat.read_disc_states(project.id).expect("states"),
        vec![
            DiscState {
                ordinal: 1,
                status: DiscStatus::Finalized,
                usable_bytes: 1000,
                used_bytes: 800,
            },
            DiscState {
                ordinal: 2,
                status: DiscStatus::Open,
                usable_bytes: 1000,
                used_bytes: 800,
            },
            DiscState {
                ordinal: 3,
                status: DiscStatus::Dead,
                usable_bytes: 1000,
                used_bytes: 800,
            },
        ]
    );

    // The stored row still says OPEN — DEAD came out of `disc_event`.
    let stored: String = cat
        .raw_connection_for_tests()
        .query_row("select status from disc where ordinal = 3", [], |r| {
            r.get(0)
        })
        .expect("the disc row exists");
    assert_eq!(stored, "OPEN");

    // a.bin's current version is on the dead disc and b.bin is deleted, so
    // nothing is live: this is exactly the state a recovery burn starts from.
    assert!(cat.read_view(project.id).expect("view").current.is_empty());
}

#[test]
fn fingerprints_are_deliberately_not_filtered_by_dead_disc() {
    // Ported as-is with the rest of the deferred debt (spec §1): a dead disc
    // does not change the source file, so the scanner may still reuse its hash.
    // The asymmetry with `read_view` above is intended, and this pins it.
    let (_dir, cat) = open("catalog/08-after-mark-dead.sqlite");
    let project = cat
        .find_project("p")
        .expect("query")
        .expect("project exists");

    assert!(cat.read_view(project.id).expect("view").current.is_empty());

    let fingerprints = cat.fingerprints(project.id).expect("fingerprints");
    assert_eq!(fingerprints.len(), 1);
    assert_eq!(fingerprints[&rel("a.bin")].content_hash, hash(A2));
}

#[test]
fn file_history_reports_lost_per_physical_copy() {
    // The invariant from CLAUDE.md, in its natural example: after the recovery
    // burn, a@2 exists twice — the dead disc-3 copy reads LOST and the disc-4
    // copy reads CURRENT, though both carry the same content hash.
    let (_dir, cat) = open("catalog/11-after-finalize.sqlite");
    let project = cat
        .find_project("p")
        .expect("query")
        .expect("project exists");

    assert_eq!(
        cat.file_history(project.id, &rel("a.bin"))
            .expect("history"),
        vec![
            FileHistoryEntry {
                content_hash: hash(A1),
                size_bytes: 800,
                disc_ordinal: 1,
                session_ordinal: 1,
                status: VersionStatus::Superseded,
            },
            FileHistoryEntry {
                content_hash: hash(A2),
                size_bytes: 800,
                disc_ordinal: 3,
                session_ordinal: 1,
                status: VersionStatus::Lost,
            },
            FileHistoryEntry {
                content_hash: hash(A2),
                size_bytes: 800,
                disc_ordinal: 4,
                session_ordinal: 1,
                status: VersionStatus::Current,
            },
        ]
    );

    assert_eq!(
        cat.file_history(project.id, &rel("b.bin"))
            .expect("history"),
        vec![FileHistoryEntry {
            content_hash: hash(B1),
            size_bytes: 800,
            disc_ordinal: 2,
            session_ordinal: 1,
            status: VersionStatus::Deleted,
        }]
    );
}

#[test]
fn the_final_catalog_reports_its_pointers_and_per_disc_contents() {
    let (_dir, cat) = open("catalog/11-after-finalize.sqlite");
    let project = cat
        .find_project("p")
        .expect("query")
        .expect("project exists");

    assert_eq!(
        cat.archive_pointers(project.id).expect("pointers"),
        vec![
            ArchivePointer {
                rel_path: rel("a.bin"),
                content_hash: hash(A1),
                size_bytes: 800,
                disc_ordinal: 1,
                session_ordinal: 1,
                disc_path: "/data/a.bin".to_string(),
                kind: ManifestChangeKind::Added,
            },
            ArchivePointer {
                rel_path: rel("a.bin"),
                content_hash: hash(A2),
                size_bytes: 800,
                disc_ordinal: 3,
                session_ordinal: 1,
                disc_path: "/data/a.bin".to_string(),
                kind: ManifestChangeKind::Modified,
            },
            ArchivePointer {
                rel_path: rel("b.bin"),
                content_hash: hash(B1),
                size_bytes: 800,
                disc_ordinal: 2,
                session_ordinal: 1,
                disc_path: "/data/b.bin".to_string(),
                kind: ManifestChangeKind::Deleted,
            },
        ]
    );

    // Disc 4's last session is the `plan --finalize` seal, which placed no
    // files — so the per-session view is empty while the whole-disc view is not.
    assert!(cat
        .placements_for_disc(project.id, 4)
        .expect("placements")
        .is_empty());
    assert_eq!(
        cat.disc_recorded_contents(project.id, 4)
            .expect("contents")
            .visible,
        BTreeMap::from([(rel("a.bin"), hash(A2))])
    );

    let disc3 = cat.disc_recorded_contents(project.id, 3).expect("contents");
    assert_eq!(disc3.visible, BTreeMap::from([(rel("a.bin"), hash(A2))]));
    assert!(disc3.superseded.is_empty());

    let verified = cat
        .sessions_by_state(project.id, SessionState::Verified)
        .expect("sessions");
    assert_eq!(verified.len(), 1);
    assert_eq!((verified[0].disc_ordinal, verified[0].ordinal), (1, 1));
}
