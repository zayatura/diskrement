//! The spawned-binary harness. This is the P0 analogue of `BuildSmokeSpec`:
//! every later phase's exit-code assertions run through `Command::cargo_bin`,
//! so prove the mechanism works before anything depends on it.

use assert_cmd::Command;

#[test]
fn binary_runs_and_prints_its_version() {
    Command::cargo_bin("diskrement")
        .expect("binary builds")
        .assert()
        .success()
        .stdout(predicates::str::contains(env!("CARGO_PKG_VERSION")));
}
