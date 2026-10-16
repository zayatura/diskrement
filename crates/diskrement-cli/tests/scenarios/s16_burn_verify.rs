//! Scenario 16: `burn --verify` at two copies reads each twin back before
//! recording it. Both discs report verified, and a later sweep agrees.

use crate::support;

#[test]
fn burn_verify_reads_back_both_twins_before_recording_them() {
    let Some(s) = support::scratch_or_skip("s16 burn --verify") else {
        return;
    };
    let cli = s.cli();
    support::write(&s.source, "a.txt", 128);
    support::write(&s.source, "b.txt", 128);
    let init = cli.run(&[
        "init",
        "p",
        "--source",
        &s.source.display().to_string(),
        "--work-dir",
        &s.work.display().to_string(),
        "--copies",
        "2",
    ]);
    assert_eq!(init.code, 0, "init: {}{}", init.stdout, init.stderr);
    let plan = cli.run(&["plan", "p"]);
    assert_eq!(plan.code, 0, "plan: {}{}", plan.stdout, plan.stderr);

    let burn = cli.run(&["burn", "p", "--yes", "--verify"]);
    assert_eq!(burn.code, 0, "{}{}", burn.stdout, burn.stderr);
    assert!(burn.out_has("Wrote disc 1A"), "{}", burn.stdout);
    assert!(burn.out_has("Wrote disc 1B"), "{}", burn.stdout);
    assert_eq!(
        burn.stdout
            .matches("Read back: verified (2 files).")
            .count(),
        2,
        "{}",
        burn.stdout
    );

    let verify = cli.run(&["verify", "p"]);
    assert_eq!(verify.code, 0, "{}{}", verify.stdout, verify.stderr);
}
