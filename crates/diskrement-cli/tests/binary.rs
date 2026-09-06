//! The spawned-binary harness. This is the P0 analogue of `BuildSmokeSpec`:
//! every later phase's exit-code assertions run through `Command::cargo_bin`,
//! so prove the mechanism works before anything depends on it.

use assert_cmd::Command;
use predicates::prelude::*;
use predicates::str::contains;

#[test]
fn binary_runs_and_prints_its_version() {
    // `--version` explicitly: a bare invocation is a missing-subcommand
    // argument error now that the real command surface exists (exit 1).
    Command::cargo_bin("diskrement")
        .expect("binary builds")
        .arg("--version")
        .assert()
        .success()
        .stdout(contains(env!("CARGO_PKG_VERSION")));
}

mod p3 {
    use super::*;

    fn cmd() -> Command {
        Command::cargo_bin("diskrement").expect("binary builds")
    }

    /// The happy path, end to end, in a temp dir: init → scan → status.
    #[test]
    fn init_scan_status_round_trip() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src/sub")).unwrap();
        std::fs::write(root.join("src/a.txt"), b"hello").unwrap();
        std::fs::write(root.join("src/sub/b.txt"), b"world").unwrap();
        let catalog = root.join("catalog/cat.sqlite");
        std::fs::create_dir_all(catalog.parent().unwrap()).unwrap();
        let work = root.join("work");
        let base = |catalog: &std::path::Path| {
            let mut c = cmd();
            c.arg("--catalog").arg(catalog);
            c
        };

        base(&catalog)
            .args(["init", "p", "--source"])
            .arg(root.join("src"))
            .arg("--work-dir")
            .arg(&work)
            .assert()
            .success()
            .stdout(contains("Created project p (id 1)."));

        base(&catalog)
            .args(["scan", "p"])
            .env("DISKREMENT_HASH_PARALLELISM", "2")
            .assert()
            .success()
            .stdout(contains(
                "Scanned: 2 added, 0 modified, 0 deleted, 0 unchanged.",
            ))
            .stdout(contains("Hashed 2 of 2 files (0 fingerprints reused)."));

        base(&catalog)
            .args(["status", "p"])
            .assert()
            .success()
            .stdout(
                contains("Project p  (source")
                    .and(contains("Discs:"))
                    .and(contains("Current files: 0      Last burn: never")),
            );

        // The staged scan and the sidecar exist where the contract says.
        assert!(work.join("staged/scan.json").is_file());
        assert!(root.join("catalog/cat.p.scancache.sqlite").is_file());

        // A second scan reuses the staged scan: no Hashed line.
        base(&catalog)
            .args(["scan", "p"])
            .assert()
            .success()
            .stdout(contains(
                "Scanned: 2 added, 0 modified, 0 deleted, 0 unchanged.",
            ))
            .stdout(contains("Hashed").not());
    }

    #[test]
    fn an_unknown_project_exits_2_with_the_contract_message() {
        let dir = tempfile::TempDir::new().unwrap();
        let catalog = dir.path().join("cat.sqlite");
        // The catalog does not exist yet; the binary must create it (empty)
        // and report the lookup failure, like the Scala Runner does.
        cmd()
            .arg("--catalog")
            .arg(&catalog)
            .args(["status", "nosuch"])
            .assert()
            .failure()
            .code(2)
            .stderr(contains("project not found: nosuch"));
        assert!(catalog.is_file(), "the binary creates an empty catalog");
    }

    #[test]
    fn scan_on_an_unknown_project_also_exits_2() {
        let dir = tempfile::TempDir::new().unwrap();
        cmd()
            .arg("--catalog")
            .arg(dir.path().join("cat.sqlite"))
            .args(["scan", "nosuch"])
            .assert()
            .failure()
            .code(2)
            .stderr(contains("project not found: nosuch"));
    }

    #[test]
    fn an_argument_parse_error_exits_1_with_clap_wording() {
        cmd()
            .args(["scan"])
            .assert()
            .failure()
            .code(1) // the contract's argument-error exit; wording may drift (§3.2)
            .stderr(contains("error:"));
    }
}
