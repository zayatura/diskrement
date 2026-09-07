//! `session.manifest_hash` is the sha256 of the *compact* manifest JSON, so the
//! Rust encoder's field order and whitespace are catalog content (spec §3.1).
//!
//! Both halves come out of the corpus: the manifest from the disc tree the
//! Scala build staged, the hash from the catalog it committed. Nothing here is
//! a value typed into this file and asserted against itself.

mod support;

use diskrement_shell::burn::json::{manifest_hash, ManifestJson};
use support::{fixture_text, open_fixture};

/// The catalog holding every session the harvest produced.
const FINAL_CATALOG: &str = "catalog/11-after-finalize.sqlite";

fn hash_from_catalog(disc_ordinal: u32, session_ordinal: u32) -> String {
    let conn = open_fixture(FINAL_CATALOG);
    conn.query_row(
        "select s.manifest_hash
           from session s join disc d on s.disc_id = d.id
          where d.ordinal = ?1 and s.ordinal = ?2",
        (disc_ordinal, session_ordinal),
        |row| row.get(0),
    )
    .unwrap_or_else(|e| panic!("disc {disc_ordinal} session {session_ordinal}: {e}"))
}

fn check(disc: u32) {
    let raw = fixture_text(&format!("discs/disc-{disc}/.diskrement/manifest.json"));
    let dto: ManifestJson =
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("disc {disc} manifest.json: {e}"));
    let manifest = dto
        .to_manifest()
        .unwrap_or_else(|e| panic!("disc {disc} manifest.json: {e}"));
    assert_eq!(manifest.disc_ordinal, disc, "fixture names its own disc");

    let expected = hash_from_catalog(disc, manifest.session_ordinal);
    assert_eq!(
        manifest_hash(&manifest),
        expected,
        "disc {disc}: the Rust manifest hash differs from the one the Scala \
         build stored. The compact JSON encoding has drifted — check field \
         order and whitespace, not the hash function."
    );
}

/// Disc 1: the initial-add shape.
#[test]
fn the_initial_add_manifest_hashes_to_the_stored_value() {
    check(1);
}

/// Disc 2: the spill remainder, another add session.
#[test]
fn the_second_add_manifest_hashes_to_the_stored_value() {
    check(2);
}

/// Disc 3: the modify+delete shape — the only one with a non-empty `modified`
/// and a non-empty `deleted`.
#[test]
fn the_modify_and_delete_manifest_hashes_to_the_stored_value() {
    check(3);
}

/// Disc 4: the file-less seal session (`plan --finalize`), all three lists
/// empty and `sessionOrdinal` 2.
#[test]
fn the_seal_manifest_hashes_to_the_stored_value() {
    check(4);
}
