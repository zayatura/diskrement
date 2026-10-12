//! Scenario 2: disc 1 is left OPEN after cycle 1, so cycle 2's add, modify
//! and delete are written to the same disc as session 2. Staging then holds
//! only session 2's files; the cumulative view comes from status and history.

use crate::{disc, support};

#[test]
fn the_second_session_lands_on_disc_one_as_session_two() {
    let Some(s) = support::scratch_or_skip("s02 append to open disc") else {
        return;
    };
    let cli = s.cli();
    support::write(&s.source, "a.txt", 128);
    support::write(&s.source, "b.txt", 128);

    cli.init(&s, "p");

    // Cycle 1: both files to disc 1, left open.
    cli.run(&["plan", "p"]);
    let b1 = cli.run(&["burn", "p", "--yes"]);
    assert_eq!(b1.code, 0);
    assert!(b1.out_has("Wrote disc 1"));
    assert!(b1.out_has("left OPEN"));

    let man1 = disc::manifest(&s.disc_dir(1));
    assert_eq!(man1.disc_ordinal, 1);
    assert_eq!(man1.session_ordinal, 1);
    assert_eq!(
        disc::data_files(&s.disc_dir(1)),
        support::set(&["a.txt", "b.txt"])
    );

    // Cycle 2: modify a, add c, delete b.
    support::modify(&s.source, "a.txt", 128);
    support::write(&s.source, "c.txt", 64);
    support::delete(&s.source, "b.txt");

    let pl2 = cli.run(&["plan", "p"]);
    assert_eq!(pl2.code, 0);
    assert!(pl2.out_has("Scanned: 1 added, 1 modified, 1 deleted, 0 unchanged."));
    assert!(pl2.out_has("appending to open disc 1"));

    let b2 = cli.run(&["burn", "p", "--yes"]);
    assert_eq!(b2.code, 0);
    assert!(b2.out_has("Wrote disc 1"));
    assert!(b2.out_has("left OPEN"));

    // Staging now holds session 2 only.
    let man2 = disc::manifest(&s.disc_dir(1));
    assert_eq!(man2.disc_ordinal, 1);
    assert_eq!(man2.session_ordinal, 2);
    assert_eq!(
        disc::data_files(&s.disc_dir(1)),
        support::set(&["a.txt", "c.txt"])
    );
    assert_eq!(disc::manifest_added(&man2), support::set(&["c.txt"]));
    assert_eq!(disc::manifest_modified(&man2), support::set(&["a.txt"]));
    assert_eq!(disc::manifest_deleted(&man2), support::set(&["b.txt"]));

    let ha = cli.run(&["history", "p", "a.txt"]);
    assert_eq!(ha.code, 0);
    assert!(ha.out_has("SUPERSEDED"));
    assert!(ha.out_has("CURRENT"));

    let hb = cli.run(&["history", "p", "b.txt"]);
    assert_eq!(hb.code, 0);
    assert!(hb.out_has("DELETED"));

    let status = cli.run(&["status", "p"]);
    assert_eq!(status.code, 0);
    assert!(status.out_has("Current files: 2"));
}
