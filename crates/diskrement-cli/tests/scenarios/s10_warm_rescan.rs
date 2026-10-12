//! Scenario 10: the fingerprint cache, end to end.
//!
//! Every `plan` scans, and the scan merges two sources: fingerprints read back
//! from the catalog (populated once a session is burned) and the sidecar cache
//! (written after every scan, burn or no burn). A rescan taken after a burn is
//! explained by either, so it alone proves nothing about the sidecar. Three
//! rescans are pinned instead: one before any burn, one after, and one after a
//! file's size changes.

use crate::support;

#[test]
fn plans_reuse_fingerprints_before_and_after_a_burn_and_a_touched_file_rehashes() {
    let Some(s) = support::scratch_or_skip("s10 warm-cache rescan") else {
        return;
    };
    let cli = s.cli();
    support::write(&s.source, "a.txt", 100);
    support::write(&s.source, "b.txt", 200);
    support::write(&s.source, "c.txt", 300);

    cli.init(&s, "p");

    let plan1 = cli.run(&["plan", "p"]);
    assert_eq!(plan1.code, 0);
    assert!(plan1.out_has("Hashed 3 of 3 files (0 fingerprints reused)."));

    // The discriminating step: nothing is burned yet, so only the sidecar
    // can explain the reuse.
    let plan1b = cli.run(&["plan", "p"]);
    assert_eq!(plan1b.code, 0);
    assert!(plan1b.out_has("Hashed 0 of 3 files (3 fingerprints reused)."));

    cli.run(&["burn", "p", "--yes"]);

    let plan2 = cli.run(&["plan", "p"]);
    assert_eq!(plan2.code, 0);
    assert!(plan2.out_has("Scanned: 0 added, 0 modified, 0 deleted, 3 unchanged."));
    assert!(plan2.out_has("Hashed 0 of 3 files (3 fingerprints reused)."));

    // A size change is a fingerprint miss whichever source answered.
    support::modify(&s.source, "b.txt", 999);
    let plan3 = cli.run(&["plan", "p"]);
    assert_eq!(plan3.code, 0);
    assert!(plan3.out_has("Scanned: 0 added, 1 modified, 0 deleted, 2 unchanged."));
    assert!(plan3.out_has("Hashed 1 of 3 files (2 fingerprints reused)."));

    // Pins the sidecar naming convention the CLI resolves. Mere existence
    // proves nothing about persistence — the file is created on demand by
    // any cache call, a plain read included — so this is a path assertion
    // and nothing more.
    assert!(
        s.catalog
            .parent()
            .unwrap()
            .join("cat.p.scancache.sqlite")
            .exists(),
        "the sidecar is not where the CLI resolves it"
    );
}
