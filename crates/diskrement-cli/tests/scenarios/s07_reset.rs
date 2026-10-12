//! Scenario 7: `reset` discards the staged plan, so the next `burn` fails with
//! exit 2. This is the scenario that proves the CLI propagates application
//! exit codes rather than collapsing them to 0.

use crate::support;

#[test]
fn reset_clears_the_staged_plan_and_a_following_burn_errors() {
    let Some(s) = support::scratch_or_skip("s07 reset") else {
        return;
    };
    let cli = s.cli();
    support::write(&s.source, "a.txt", 64);

    cli.init(&s, "p");

    assert_eq!(cli.run(&["plan", "p"]).code, 0);

    let reset = cli.run(&["reset", "p"]);
    assert_eq!(reset.code, 0);
    assert!(reset.out_has("Reset p."));

    let burn = cli.run(&["burn", "p", "--yes"]);
    assert_eq!(burn.code, 2);
    assert!(
        burn.err_has("no staged plan for p (run plan)"),
        "{}",
        burn.stderr
    );
    // CLAUDE.md: staged-state advice is always "run plan", never "reset":
    // reset destroys staging the verifier needs.
    assert!(
        !burn.stderr.contains("reset"),
        "the advice must not suggest reset: {}",
        burn.stderr
    );
}
