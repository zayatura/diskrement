//! Scenario 5: three 400-byte files under a 1000-byte cap. Two fit disc 1,
//! the third spills to disc 2 in the next cycle. The assertion is that the
//! discs *partition* the file set — never which two files the planner chose,
//! which is its business and not a contract.

use crate::{disc, support};

#[test]
fn three_files_over_two_discs_form_a_disjoint_union() {
    let Some(s) = support::scratch_or_skip("s05 spill across cycles") else {
        return;
    };
    let cli = s.cli_with_disc_bytes(1000);
    support::write(&s.source, "f1.bin", 400);
    support::write(&s.source, "f2.bin", 400);
    support::write(&s.source, "f3.bin", 400);

    cli.init(&s, "p");

    // Cycle 1: disc 1 fills with two of the three.
    let pl1 = cli.run(&["plan", "p"]);
    assert_eq!(pl1.code, 0);
    assert!(pl1.out_has("Scanned: 3 added"));
    assert!(pl1.out_has("more discs) remain"));
    cli.run(&["burn", "p", "--yes"]);
    let d1 = disc::data_files(&s.disc_dir(1));
    assert_eq!(d1.len(), 2);

    // Cycle 2: the remaining file goes to disc 2 and completes the set.
    let pl2 = cli.run(&["plan", "p"]);
    assert_eq!(pl2.code, 0);
    assert!(pl2.out_has("This disc completes the set."));
    cli.run(&["burn", "p", "--yes"]);
    let d2 = disc::data_files(&s.disc_dir(2));
    assert_eq!(d2.len(), 1);

    assert!(
        d1.intersection(&d2).next().is_none(),
        "the two discs overlap: {d1:?} / {d2:?}"
    );
    let union: std::collections::BTreeSet<String> = d1.union(&d2).cloned().collect();
    assert_eq!(union, support::set(&["f1.bin", "f2.bin", "f3.bin"]));

    let status = cli.run(&["status", "p"]);
    assert_eq!(status.code, 0);
    assert!(status.out_has("Current files: 3"));

    assert_eq!(cli.run(&["verify", "p"]).code, 0);
}
