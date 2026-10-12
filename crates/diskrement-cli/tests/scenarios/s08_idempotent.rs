//! Scenario 8: after a full burn, re-planning an untouched source reports
//! everything unchanged, there is nothing left to plan, and burning that plan
//! writes nothing. A file added after that nothing-to-burn cycle is then seen
//! (#4: the old staged scan was reused and hid it).

use crate::{disc, support};

#[test]
fn replanning_an_unchanged_source_plans_nothing_and_a_new_file_is_then_seen() {
    let Some(s) = support::scratch_or_skip("s08 idempotent re-scan") else {
        return;
    };
    let cli = s.cli();
    support::write(&s.source, "a.txt", 64);
    support::write(&s.source, "b.txt", 64);

    cli.init(&s, "p");
    cli.run(&["plan", "p"]);
    cli.run(&["burn", "p", "--yes"]);

    // Every plan scans afresh.
    let pl2 = cli.run(&["plan", "p"]);
    assert_eq!(pl2.code, 0);
    assert!(pl2.out_has("Scanned: 0 added, 0 modified, 0 deleted, 2 unchanged."));
    assert!(pl2.out_has("Nothing to burn: no changes to plan."));

    // Burning that plan writes nothing: disc 1 gets no second session.
    let b2 = cli.run(&["burn", "p", "--yes"]);
    assert_eq!(b2.code, 0, "{}{}", b2.stdout, b2.stderr);
    assert!(
        b2.out_has("Nothing burned: no disc in this plan."),
        "{}",
        b2.stdout
    );
    assert!(!b2.out_has("Wrote disc"), "{}", b2.stdout);
    assert_eq!(disc::manifest(&s.disc_dir(1)).session_ordinal, 1);
    assert!(!s.disc_dir(2).exists());

    // #4: a plan with nothing to burn left its scan staged, and the next
    // cycle reused it and never saw new files. Now the next plan does.
    support::write(&s.source, "c.txt", 64);
    let pl3 = cli.run(&["plan", "p"]);
    assert_eq!(pl3.code, 0, "{}{}", pl3.stdout, pl3.stderr);
    assert!(pl3.out_has("Scanned: 1 added, 0 modified, 0 deleted, 2 unchanged."));
    assert!(!pl3.out_has("Nothing to burn"), "{}", pl3.stdout);
}
