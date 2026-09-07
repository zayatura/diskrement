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

mod p4 {
    use super::*;

    fn cmd() -> Command {
        Command::cargo_bin("diskrement").expect("binary builds")
    }

    /// A scratch project with `sizes` written into the source tree, already
    /// init'd and scanned. Returns the temp dir, the catalog path and the work
    /// dir.
    fn scanned_project(
        sizes: &[(&str, usize)],
    ) -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let src = root.join("src");
        std::fs::create_dir_all(&src).unwrap();
        for (name, size) in sizes {
            std::fs::write(src.join(name), vec![b'x'; *size]).unwrap();
        }
        let catalog = root.join("catalog/cat.sqlite");
        std::fs::create_dir_all(catalog.parent().unwrap()).unwrap();
        let work = root.join("work");
        cmd()
            .arg("--catalog")
            .arg(&catalog)
            .args(["init", "p", "--source"])
            .arg(&src)
            .arg("--work-dir")
            .arg(&work)
            .assert()
            .success();
        cmd()
            .arg("--catalog")
            .arg(&catalog)
            .args(["scan", "p"])
            .assert()
            .success();
        (dir, catalog, work)
    }

    #[test]
    fn plan_stages_a_plan_and_prints_the_summary() {
        let (_dir, catalog, work) = scanned_project(&[("a.bin", 800)]);
        cmd()
            .arg("--catalog")
            .arg(&catalog)
            .args(["plan", "p"])
            .assert()
            .success()
            .stdout(
                contains("Plan: disc 1 (new disc, BD_R) —")
                    .and(contains("left OPEN"))
                    .and(contains("This disc completes the set.")),
            );
        assert!(work.join("staged/plan.json").is_file());
        // The staged scan survives: `plan` does not consume it.
        assert!(work.join("staged/scan.json").is_file());
    }

    #[test]
    fn plan_finalize_seals_the_disc() {
        let (_dir, catalog, _work) = scanned_project(&[("a.bin", 800)]);
        cmd()
            .arg("--catalog")
            .arg(&catalog)
            .args(["plan", "p", "--finalize"])
            .assert()
            .success()
            .stdout(contains("will FINALIZE"));
    }

    #[test]
    fn plan_disc_type_overrides_the_projects_default() {
        let (_dir, catalog, _work) = scanned_project(&[("a.bin", 800)]);
        cmd()
            .arg("--catalog")
            .arg(&catalog)
            .args(["plan", "p", "--disc-type", "DVD"])
            .assert()
            .success()
            .stdout(contains("(new disc, DVD)"));
    }

    /// The shrink-only capacity override, end to end. Two 800-byte files
    /// against a 1000-byte disc: the plan spills and seals.
    #[test]
    fn the_disc_bytes_override_shrinks_capacity_and_spills() {
        let (_dir, catalog, _work) = scanned_project(&[("a.bin", 800), ("b.bin", 800)]);
        cmd()
            .arg("--catalog")
            .arg(&catalog)
            .args(["plan", "p"])
            .env("DISKREMENT_DISC_BYTES", "1000")
            .assert()
            .success()
            .stdout(contains("will FINALIZE").and(contains(
                "~1 more files (~1 more discs) remain after this one.",
            )));
    }

    /// A value at or above the margin-adjusted capacity is ignored, so the
    /// override can never plan past the safety margin.
    #[test]
    fn the_disc_bytes_override_never_grows_capacity() {
        let (_dir, catalog, _work) = scanned_project(&[("a.bin", 800), ("b.bin", 800)]);
        cmd()
            .arg("--catalog")
            .arg(&catalog)
            .args(["plan", "p"])
            .env("DISKREMENT_DISC_BYTES", "999999999999")
            .assert()
            .success()
            .stdout(contains("This disc completes the set."));
    }

    #[test]
    fn an_oversized_file_is_listed_but_the_plan_still_succeeds() {
        let (_dir, catalog, _work) = scanned_project(&[("big.bin", 800), ("small.bin", 100)]);
        cmd()
            .arg("--catalog")
            .arg(&catalog)
            .args(["plan", "p"])
            .env("DISKREMENT_DISC_BYTES", "500")
            .assert()
            .success()
            .stdout(contains("Oversized (cannot be placed):").and(contains("big.bin")));
    }

    #[test]
    fn plan_without_a_staged_scan_exits_2_and_says_to_run_scan() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        let catalog = root.join("catalog/cat.sqlite");
        std::fs::create_dir_all(catalog.parent().unwrap()).unwrap();
        cmd()
            .arg("--catalog")
            .arg(&catalog)
            .args(["init", "p", "--source"])
            .arg(root.join("src"))
            .arg("--work-dir")
            .arg(root.join("work"))
            .assert()
            .success();
        cmd()
            .arg("--catalog")
            .arg(&catalog)
            .args(["plan", "p"])
            .assert()
            .code(2)
            .stderr(contains("no staged scan for p (run scan)"));
    }

    #[test]
    fn plan_on_an_unknown_project_exits_2() {
        let dir = tempfile::TempDir::new().unwrap();
        let catalog = dir.path().join("catalog/cat.sqlite");
        std::fs::create_dir_all(catalog.parent().unwrap()).unwrap();
        cmd()
            .arg("--catalog")
            .arg(&catalog)
            .args(["plan", "nosuch"])
            .assert()
            .code(2)
            .stderr(contains("project not found: nosuch"));
    }
}
