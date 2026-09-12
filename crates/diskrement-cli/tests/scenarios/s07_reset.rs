//! Scenario 7: `reset` discards the staged scan, so the next `plan` fails
//! with exit 2. This is the scenario that proves the CLI propagates
//! application exit codes rather than collapsing them to 0.

use crate::support;

#[test]
fn reset_clears_the_staged_scan_and_a_following_plan_errors() {
    let Some(s) = support::scratch_or_skip("s07 reset") else {
        return;
    };
    let cli = s.cli();
    support::write(&s.source, "a.txt", 64);

    cli.init(&s, "p");

    assert_eq!(cli.run(&["scan", "p"]).code, 0);

    let reset = cli.run(&["reset", "p"]);
    assert_eq!(reset.code, 0);
    assert!(reset.out_has("Reset p."));

    let plan = cli.run(&["plan", "p"]);
    assert_eq!(plan.code, 2);
    assert!(!plan.stderr.is_empty(), "exit 2 must explain itself");
    // CLAUDE.md: StalePlan/NoStagedScan advice is always "re-run plan",
    // never "run reset" — reset destroys staging the verifier needs.
    assert!(
        !plan.stderr.contains("reset"),
        "the advice must not suggest reset: {}",
        plan.stderr
    );
}
