//! Scenario 3: `plan --finalize` seals disc 1 even though it has room, so
//! the next cycle's change must open disc 2.

use crate::{disc, support};

#[test]
fn after_finalize_new_changes_go_to_disc_two() {
    let Some(s) = support::scratch_or_skip("s03 finalized -> new disc") else {
        return;
    };
    let cli = s.cli();
    support::write(&s.source, "a.txt", 128);

    cli.init(&s, "p");

    let pl1 = cli.run(&["plan", "p", "--finalize"]);
    assert_eq!(pl1.code, 0);
    assert!(pl1.out_has("will FINALIZE"));

    let b1 = cli.run(&["burn", "p", "--yes"]);
    assert_eq!(b1.code, 0);
    assert!(b1.out_has("Wrote disc 1"));
    assert!(b1.out_has("FINALIZED"));

    assert_eq!(disc::data_files(&s.disc_dir(1)), support::set(&["a.txt"]));
    assert_eq!(disc::manifest(&s.disc_dir(1)).disc_ordinal, 1);

    // Cycle 2: a new file cannot go on the sealed disc.
    support::write(&s.source, "b.txt", 128);
    let pl2 = cli.run(&["plan", "p"]);
    assert_eq!(pl2.code, 0);
    assert!(pl2.out_has("new disc"));

    let b2 = cli.run(&["burn", "p", "--yes"]);
    assert_eq!(b2.code, 0);
    assert!(b2.out_has("Wrote disc 2"));

    assert_eq!(disc::data_files(&s.disc_dir(2)), support::set(&["b.txt"]));
    let man2 = disc::manifest(&s.disc_dir(2));
    assert_eq!(man2.disc_ordinal, 2);
    assert_eq!(man2.session_ordinal, 1);

    let ha = cli.run(&["history", "p", "a.txt"]);
    assert_eq!(ha.code, 0);
    assert!(ha.out_has("CURRENT"));
}
