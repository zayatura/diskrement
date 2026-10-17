//! Scenario 17: `run` at two copies burns a three-pair backlog in one
//! command, each disc read back verified, and stops at the open last pair.

use crate::support;

#[test]
fn run_burns_the_backlog_and_stops_at_the_open_last_pair() {
    let Some(s) = support::scratch_or_skip("s17 run") else {
        return;
    };
    // One 800-byte file per 1000-byte disc: three discs per set.
    let cli = s.cli_with_disc_bytes(1000);
    for f in ["a.bin", "b.bin", "c.bin"] {
        support::write(&s.source, f, 800);
    }
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

    let run = cli.run(&["run", "p", "--yes"]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert!(run.out_has("Cycle 1: 1A ("), "{}", run.stdout);
    assert!(run.out_has("Cycle 3: 3A ("), "{}", run.stdout);
    assert!(run.out_has("stays open"), "{}", run.stdout);
    assert!(!run.out_has("Cycle 4"), "{}", run.stdout);
    assert_eq!(
        run.stdout.matches("Read back: verified (1 files).").count(),
        6,
        "{}",
        run.stdout
    );
    assert!(
        run.out_has("verified: 1A, 1B, 2A, 2B, 3A, 3B."),
        "{}",
        run.stdout
    );

    let status = cli.run(&["status", "p"]);
    assert!(status.out_has("#3A  OPEN"), "{}", status.stdout);
    assert!(status.out_has("#3B  OPEN"), "{}", status.stdout);
    assert!(status.out_has("#2B  FINALIZED"), "{}", status.stdout);

    let again = cli.run(&["run", "p", "--yes"]);
    assert_eq!(again.code, 0, "{}{}", again.stdout, again.stderr);
    assert!(again.out_has("Nothing to burn"), "{}", again.stdout);

    let verify = cli.run(&["verify", "p"]);
    assert_eq!(verify.code, 0, "{}{}", verify.stdout, verify.stderr);
}
