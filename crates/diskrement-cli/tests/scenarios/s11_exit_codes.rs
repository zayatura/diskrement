//! `ScenarioExitCodes`: application errors propagate as exit 2 rather than
//! collapsing to 0. Exit codes are a stable contract (`render.rs`): 2 =
//! state/lookup, 3 = environment/oversize, 4 = verify mismatch, 5 =
//! unexpected, 6 = media/burner.

use crate::support;

#[test]
fn verify_of_an_unknown_project_and_burn_without_a_plan_both_exit_two() {
    let Some(s) = support::scratch_or_skip("exit codes") else {
        return;
    };
    let cli = s.cli();
    support::write(&s.source, "a.txt", 64);

    cli.init(&s, "p");

    assert_eq!(cli.run(&["verify", "does-not-exist"]).code, 2);
    assert_eq!(cli.run(&["burn", "p", "--yes"]).code, 2);
}
