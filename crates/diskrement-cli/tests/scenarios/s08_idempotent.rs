//! Scenario 8: after a full burn, re-scanning an untouched source reports
//! everything unchanged and there is nothing left to plan.

use crate::support;

#[test]
fn rescanning_an_unchanged_source_reports_all_unchanged_and_plans_nothing() {
    let Some(s) = support::scratch_or_skip("s08 idempotent re-scan") else {
        return;
    };
    let cli = s.cli();
    support::write(&s.source, "a.txt", 64);
    support::write(&s.source, "b.txt", 64);

    cli.init(&s, "p");
    cli.run(&["scan", "p"]);
    cli.run(&["plan", "p"]);
    cli.run(&["burn", "p", "--yes"]);

    // burn cleared the staged state, so this really re-scans.
    let sc2 = cli.run(&["scan", "p"]);
    assert_eq!(sc2.code, 0);
    assert!(sc2.out_has("Scanned: 0 added, 0 modified, 0 deleted, 2 unchanged."));

    let pl2 = cli.run(&["plan", "p"]);
    assert_eq!(pl2.code, 0);
    assert!(pl2.out_has("Nothing to burn: no changes to plan."));
}
