//! Every read method, against catalogs the Scala build actually wrote.
//!
//! The scenario behind the corpus (see `fixtures/README.md`): two 800-byte
//! files spill across discs 1-2, a modify+delete cycle lands a@2 on disc 3,
//! disc 3 is marked dead, a recovery burn puts a@2 on disc 4, and
//! `plan --finalize` seals disc 4 with a file-less session.

mod support;

use std::collections::BTreeMap;

use diskrement_core::{
    ContentHash, DiscKey, DiscState, DiscStatus, ManifestChangeKind, RelPath, Sources,
};
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

/// A catalog written before roots existed reads back as a single source:
/// V4 adds an empty `project_root` and changes no project row.
#[test]
fn a_v2_catalog_reads_back_as_a_single_source() {
    let (_dir, cat) = open("catalog/01-after-init.sqlite");
    let project = cat
        .find_project("p")
        .expect("query")
        .expect("project exists");
    assert_eq!(
        project.sources,
        Sources::Single("/tmp/diskrement-harvest.RENbbg/source".into())
    );
    let roots: i64 = cat
        .raw_connection_for_tests()
        .query_row("select count(*) from project_root", [], |r| r.get(0))
        .unwrap();
    assert_eq!(roots, 0);
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
            copy_set: 0,
            status: DiscStatus::Finalized,
            usable_bytes: 1000,
            used_bytes: 800,
        }]
    );

    let burned = cat
        .sessions_by_state(project.id, SessionState::Burned)
        .expect("sessions");
    assert_eq!(burned.len(), 1);
    assert_eq!((burned[0].disc.ordinal, burned[0].ordinal), (1, 1));
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
                copy_set: 0,
                status: DiscStatus::Finalized,
                usable_bytes: 1000,
                used_bytes: 800,
            },
            DiscState {
                ordinal: 2,
                copy_set: 0,
                status: DiscStatus::Open,
                usable_bytes: 1000,
                used_bytes: 800,
            },
            DiscState {
                ordinal: 3,
                copy_set: 0,
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
                version: 1,
                content_hash: hash(A1),
                size_bytes: 800,
                disc: DiscKey::new(1, 0),
                session_ordinal: 1,
                status: VersionStatus::Superseded,
                on_dead_disc: false,
            },
            FileHistoryEntry {
                version: 2,
                content_hash: hash(A2),
                size_bytes: 800,
                disc: DiscKey::new(3, 0),
                session_ordinal: 1,
                status: VersionStatus::Lost,
                on_dead_disc: true,
            },
            FileHistoryEntry {
                version: 3,
                content_hash: hash(A2),
                size_bytes: 800,
                disc: DiscKey::new(4, 0),
                session_ordinal: 1,
                status: VersionStatus::Current,
                on_dead_disc: false,
            },
        ]
    );

    assert_eq!(
        cat.file_history(project.id, &rel("b.bin"))
            .expect("history"),
        vec![FileHistoryEntry {
            version: 1,
            content_hash: hash(B1),
            size_bytes: 800,
            disc: DiscKey::new(2, 0),
            session_ordinal: 1,
            status: VersionStatus::Deleted,
            on_dead_disc: false,
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
                copy_set: 0,
                session_ordinal: 1,
                disc_path: "/data/a.bin".to_string(),
                kind: ManifestChangeKind::Added,
            },
            ArchivePointer {
                rel_path: rel("a.bin"),
                content_hash: hash(A2),
                size_bytes: 800,
                disc_ordinal: 3,
                copy_set: 0,
                session_ordinal: 1,
                disc_path: "/data/a.bin".to_string(),
                kind: ManifestChangeKind::Modified,
            },
            ArchivePointer {
                rel_path: rel("b.bin"),
                content_hash: hash(B1),
                size_bytes: 800,
                disc_ordinal: 2,
                copy_set: 0,
                session_ordinal: 1,
                disc_path: "/data/b.bin".to_string(),
                kind: ManifestChangeKind::Deleted,
            },
        ]
    );

    // Disc 4's last session is the `plan --finalize` seal, which placed no
    // files — so the per-session view is empty while the whole-disc view is not.
    assert!(cat
        .placements_for_disc(project.id, DiscKey::new(4, 0))
        .expect("placements")
        .is_empty());
    assert_eq!(
        cat.disc_recorded_contents(project.id, DiscKey::new(4, 0))
            .expect("contents")
            .visible,
        BTreeMap::from([(rel("a.bin"), hash(A2))])
    );

    let disc3 = cat
        .disc_recorded_contents(project.id, DiscKey::new(3, 0))
        .expect("contents");
    assert_eq!(disc3.visible, BTreeMap::from([(rel("a.bin"), hash(A2))]));
    assert!(disc3.superseded.is_empty());

    let verified = cat
        .sessions_by_state(project.id, SessionState::Verified)
        .expect("sessions");
    assert_eq!(verified.len(), 1);
    assert_eq!((verified[0].disc.ordinal, verified[0].ordinal), (1, 1));
}

/// A V2 catalog the Scala build wrote migrates in place (V3, then V4): every
/// disc keeps its id, ordinal and status, lands in set A, and nothing dangles.
#[test]
fn a_v2_catalog_migrates_to_current_keeping_every_disc_id() {
    let before: Vec<(i64, i64, String)> = {
        let conn = support::open_fixture("catalog/11-after-finalize.sqlite");
        let mut stmt = conn
            .prepare("select id, ordinal, status from disc order by id")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert!(!before.is_empty(), "the fixture has discs");

    let (_dir, cat) = open("catalog/11-after-finalize.sqlite");
    let conn = cat.raw_connection_for_tests();
    let after: Vec<(i64, i64, String, i64)> = {
        let mut stmt = conn
            .prepare("select id, ordinal, status, copy_set from disc order by id")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(
        after
            .iter()
            .map(|(i, o, s, _)| (*i, *o, s.clone()))
            .collect::<Vec<_>>(),
        before
    );
    assert!(after.iter().all(|(_, _, _, set)| *set == 0));

    let version: i64 = conn
        .query_row("select version from schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 4, "V3, then V4 on top");
    let dangling: i64 = conn
        .query_row("select count(*) from pragma_foreign_key_check", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(dangling, 0);

    // The rebuilt table's AUTOINCREMENT sequence is at least the highest old
    // id, so a disc inserted after the migration can never reuse an id that
    // a session or disc_event row might once have referenced.
    let max_old = before.iter().map(|(i, _, _)| *i).max().unwrap();
    let seq: i64 = conn
        .query_row(
            "select seq from sqlite_sequence where name = 'disc'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        seq >= max_old,
        "disc sequence {seq} below old max id {max_old}"
    );
    conn.execute(
        "insert into disc (project_id, ordinal, disc_type, usable_bytes, status, label, copy_set)
         values (1, 99, 'BD_R', 1, 'OPEN', 'x', 1)",
        [],
    )
    .unwrap();
    assert!(conn.last_insert_rowid() > max_old);
}
