//! `pointers.json` as the Scala build wrote it, byte for byte.
//!
//! Pointers are read from the *live* catalog before the burn's update commits,
//! so they lag one cycle: discs 1-3 all carry `[]`, and only disc 4 — staged by
//! the final `plan --finalize` burn, whose starting catalog was
//! `10-after-burn4` — has entries. That pair is the only non-empty rendering
//! the corpus contains, which is why it gets a test of its own rather than
//! riding along with the burn replay.

mod support;

use diskrement_shell::burn::json::{to_pretty, PointersJson};
use diskrement_shell::catalog::Catalog;
use support::{fixture_copy, fixture_text};

/// The live catalog immediately before the seal burn that staged `disc-4`.
const CATALOG_BEFORE_SEAL_BURN: &str = "catalog/10-after-burn4.sqlite";

#[test]
fn rendering_the_pointers_reproduces_the_file_on_disc_four() {
    let (_tmp, path) = fixture_copy(CATALOG_BEFORE_SEAL_BURN);
    let catalog = Catalog::open(&path).expect("the fixture catalog is already current");
    let project = catalog
        .find_project("p")
        .expect("readable")
        .expect("the harvest project is named p");

    let pointers = catalog.archive_pointers(project.id).expect("readable");

    assert_eq!(
        to_pretty(&PointersJson::from(&pointers)),
        fixture_text("discs/disc-4/.diskrement/pointers.json"),
        "the rendered pointers differ from the file the Scala build wrote"
    );
}

/// The lag is deliberate and load-bearing: if `archive_pointers` were read
/// after the update committed, disc 4 would list a@3 as superseded by itself.
/// Discs 1-3 pin the empty shape.
#[test]
fn the_first_three_discs_carry_an_empty_pointer_file() {
    for disc in 1..=3 {
        assert_eq!(
            fixture_text(&format!("discs/disc-{disc}/.diskrement/pointers.json")),
            "{\n  \"pointers\" : []\n}",
            "disc {disc}"
        );
    }
}
