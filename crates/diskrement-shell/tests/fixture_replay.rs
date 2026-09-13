//! Writes the scenario's first burn cycle with the Rust store and diffs the
//! result against the catalog the Scala build wrote at the same point.
//!
//! This is the one cross-implementation check on the *write* path that does not
//! go through the pipeline, and it covers the
//! failure the spec calls highest-consequence (§11): a Rust binary writing a
//! catalog the Scala version would not recognise.

mod support;

use std::str::FromStr;

use diskrement_core::{ContentHash, DiscStatus, DiscType, RelPath};
use diskrement_shell::catalog::model::{
    CatalogUpdate, DiscWrite, FileWrite, ProjectInit, SessionWrite,
};
use diskrement_shell::catalog::{time, Catalog};
use support::{assert_dumps_match, dump_catalog, open_fixture};

#[test]
fn replaying_the_first_burn_reproduces_the_scala_catalog_row_for_row() {
    let fixture = open_fixture("catalog/03-after-burn1.sqlite");

    // Every input is read out of the fixture, so this compares the two writers
    // rather than comparing the fixture to values typed into this file.
    let (name, source_root, work_dir, disc_type, margin_percent, copies, created_at): (
        String,
        String,
        String,
        String,
        f64,
        i64,
        String,
    ) = fixture
        .query_row(
            "select name, source_root, work_dir, default_disc_type,
                    margin_percent, copies, created_at
             from project where id = 1",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .expect("the fixture has a project row");

    let (disc_ordinal, disc_type_row, usable_bytes, status, label): (
        i64,
        String,
        i64,
        String,
        String,
    ) = fixture
        .query_row(
            "select ordinal, disc_type, usable_bytes, status, label from disc where id = 1",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .expect("the fixture has a disc row");

    let (session_ordinal, burned_at, manifest_hash): (i64, String, String) = fixture
        .query_row(
            "select ordinal, burned_at, manifest_hash from session where id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("the fixture has a session row");

    let (rel_path, content_hash, size_bytes, mtime): (String, String, i64, String) = fixture
        .query_row(
            "select rel_path, content_hash, size_bytes, mtime from file_version where id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("the fixture has a file version");

    let (disc_path, copy_index): (String, i64) = fixture
        .query_row(
            "select disc_path, copy_index from placement where id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("the fixture has a placement");

    let dir = tempfile::TempDir::new().expect("a writable temp directory");
    let mut catalog = Catalog::open(&dir.path().join("catalog.sqlite")).expect("catalog opens");

    let project = catalog
        .create_project(&ProjectInit {
            name,
            source_root,
            work_dir,
            default_disc_type: DiscType::from_str(&disc_type).expect("known disc type"),
            margin_percent,
            copies: copies as u32,
            created_at: time::parse(&created_at).expect("the fixture timestamp parses"),
        })
        .expect("project is created");

    catalog
        .apply_update(&CatalogUpdate {
            project_id: project,
            finalize_disc_ordinals: Vec::new(),
            discs: vec![DiscWrite {
                ordinal: disc_ordinal as u32,
                is_new: true,
                disc_type: DiscType::from_str(&disc_type_row).expect("known disc type"),
                usable_bytes: usable_bytes as u64,
                status: DiscStatus::from_str(&status).expect("known status"),
                label,
                session: SessionWrite {
                    ordinal: session_ordinal as u32,
                    burned_at: time::parse(&burned_at).expect("the fixture timestamp parses"),
                    manifest_hash,
                    added: vec![FileWrite {
                        rel_path: RelPath(rel_path),
                        content_hash: ContentHash(content_hash),
                        size_bytes: size_bytes as u64,
                        mtime: time::parse(&mtime).expect("the fixture timestamp parses"),
                        disc_path,
                        copy_index: copy_index as u32,
                    }],
                    modified: Vec::new(),
                    deleted: Vec::new(),
                },
            }],
        })
        .expect("the burn is recorded");

    assert_dumps_match(
        &dump_catalog(&fixture),
        &dump_catalog(catalog.raw_connection_for_tests()),
    );
}
