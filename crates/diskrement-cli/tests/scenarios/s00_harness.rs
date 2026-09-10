//! `HarnessSpec` and `BuildSmokeSpec`: prove the harness before anything
//! depends on it.

use crate::support::{self, BIN};

#[test]
fn the_binary_under_test_exists_and_help_exits_zero() {
    // `BuildSmokeSpec` asked whether `sbt cli/stage` had run. Cargo builds
    // the binary for the test, so all that is left to check is that the
    // path cargo handed us runs.
    assert!(
        std::path::Path::new(BIN).is_file(),
        "CARGO_BIN_EXE_diskrement does not point at a file: {BIN}"
    );
    let out = std::process::Command::new(BIN)
        .arg("--help")
        .output()
        .expect("the binary runs");
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn init_in_an_isolated_scratch_succeeds_and_leaves_nothing_behind() {
    let Some(s) = support::scratch_or_skip("harness") else {
        return;
    };
    let root = s.root.clone();
    let r = s.cli().init(&s, "proj");
    assert!(r.out_has("Created project proj"), "stdout: {}", r.stdout);
    assert!(root.is_dir(), "the scratch exists while the test holds it");
    assert!(
        s.catalog.is_file(),
        "the catalog landed at --catalog's path"
    );

    drop(s);
    assert!(!root.exists(), "the scratch is gone once dropped");
}

#[test]
fn the_tree_writer_is_deterministic_and_version_distinct() {
    let Some(s) = support::scratch_or_skip("harness") else {
        return;
    };
    support::write(&s.source, "a.txt", 128);
    let v0 = std::fs::read(s.source.join("a.txt")).unwrap();
    support::write(&s.source, "a.txt", 128);
    assert_eq!(v0, std::fs::read(s.source.join("a.txt")).unwrap());

    support::modify(&s.source, "a.txt", 128);
    let v1 = std::fs::read(s.source.join("a.txt")).unwrap();
    assert_eq!(v1.len(), 128);
    assert_ne!(
        v0, v1,
        "modify must produce different bytes at the same size"
    );

    // Same size, different path: distinct content, so a hash collision
    // between two scenario files cannot happen by accident.
    support::write(&s.source, "b.txt", 128);
    assert_ne!(v0, std::fs::read(s.source.join("b.txt")).unwrap());

    support::delete(&s.source, "a.txt");
    assert!(!s.source.join("a.txt").exists());
}

#[test]
fn the_runner_isolates_the_catalog_and_the_home_dir() {
    let Some(s) = support::scratch_or_skip("harness") else {
        return;
    };
    s.cli().init(&s, "p");
    // Nothing was written to the redirected XDG location: --catalog won.
    let xdg = s.home.join(".local/share");
    let stray: Vec<_> = walkdir::WalkDir::new(&xdg)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(|e| e.path().display().to_string())
        .collect();
    assert!(
        stray.is_empty(),
        "the CLI wrote outside --catalog: {stray:?}"
    );
}
