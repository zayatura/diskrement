//! Scenario 1: init → scan → plan → burn → verify, with the staged disc read
//! back and compared against the source byte for byte.

use crate::{disc, support};

#[test]
fn init_scan_plan_burn_verify_and_the_disc_contents_equal_the_source() {
    let Some(s) = support::scratch_or_skip("s01 happy path") else {
        return;
    };
    let cli = s.cli();
    support::write(&s.source, "docs/a.txt", 256);
    support::write(&s.source, "img/b.bin", 512);

    cli.init(&s, "p");

    let scan = cli.run(&["scan", "p"]);
    assert_eq!(scan.code, 0);
    assert!(scan.out_has("Scanned: 2 added, 0 modified, 0 deleted, 0 unchanged."));

    let plan = cli.run(&["plan", "p"]);
    assert_eq!(plan.code, 0);
    assert!(plan.out_has("Plan: disc 1"));
    assert!(plan.out_has("new disc"));

    let burn = cli.run(&["burn", "p", "--yes"]);
    assert_eq!(burn.code, 0);
    assert!(burn.out_has("Wrote disc 1"));

    // The oracle: read the freshly staged disc 1 before verify touches it.
    let d1 = s.disc_dir(1);
    let data = disc::data_files(&d1);
    let man = disc::manifest(&d1);
    let proj = disc::project_json(&d1);
    let pts = disc::pointers(&d1);

    assert_eq!(data, support::set(&["docs/a.txt", "img/b.bin"]));
    assert_eq!(disc::manifest_files(&man), data);
    assert_eq!(man.disc_ordinal, 1);
    assert_eq!(man.session_ordinal, 1);
    assert_eq!(
        disc::manifest_added(&man),
        support::set(&["docs/a.txt", "img/b.bin"])
    );
    assert!(man.deleted.is_empty());
    assert_eq!(proj.project_name, "p");
    // pointers.json holds only superseded and deleted versions: empty here.
    assert!(pts.pointers.is_empty());
    assert!(disc::content_matches(&d1, &s.source, "docs/a.txt"));
    assert!(disc::content_matches(&d1, &s.source, "img/b.bin"));
    assert!(disc::catalog_table_count(&d1) > 0);

    let verify = cli.run(&["verify", "p"]);
    assert_eq!(verify.code, 0);
    assert!(verify.out_has("files OK"));

    let status = cli.run(&["status", "p"]);
    assert_eq!(status.code, 0);
    assert!(status.out_has("Current files: 2"));
}
