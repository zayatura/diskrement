//! The spawned-binary harness. The analogue of `BuildSmokeSpec`: every
//! exit-code assertion below runs through `Command::cargo_bin`, so the first
//! test proves the mechanism works.

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

mod init_plan_status {
    use super::*;

    fn cmd() -> Command {
        Command::cargo_bin("diskrement").expect("binary builds")
    }

    /// `scan` was merged into `plan` (#4); it is now an unknown subcommand,
    /// which is clap's argument error, exit 1.
    #[test]
    fn scan_is_no_longer_a_command() {
        cmd()
            .args(["scan", "p"])
            .assert()
            .failure()
            .code(1)
            .stderr(contains("unrecognized subcommand"));
    }

    /// The happy path, end to end, in a temp dir: init → plan → status.
    #[test]
    fn init_plan_status_round_trip() {
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
            .args(["plan", "p"])
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

        // The staged scan, the staged plan and the sidecar exist where the
        // contract says.
        assert!(work.join("staged/scan.json").is_file());
        assert!(work.join("staged/plan.json").is_file());
        assert!(root.join("catalog/cat.p.scancache.sqlite").is_file());

        // A second plan scans afresh; the sidecar answers every fingerprint.
        base(&catalog)
            .args(["plan", "p"])
            .assert()
            .success()
            .stdout(contains(
                "Scanned: 2 added, 0 modified, 0 deleted, 0 unchanged.",
            ))
            .stdout(contains("Hashed 0 of 2 files (2 fingerprints reused)."));
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
    fn an_argument_parse_error_exits_1_with_clap_wording() {
        cmd()
            .args(["plan"])
            .assert()
            .failure()
            .code(1) // the contract's argument-error exit; wording may drift (§3.2)
            .stderr(contains("error:"));
    }
}

mod plan {
    use super::*;

    fn cmd() -> Command {
        Command::cargo_bin("diskrement").expect("binary builds")
    }

    /// A scratch project with `sizes` written into the source tree, already
    /// init'd; `plan` scans. Returns the temp dir, the catalog path and the
    /// work dir.
    fn initialized_project(
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
        (dir, catalog, work)
    }

    #[test]
    fn plan_stages_a_plan_and_prints_the_summary() {
        let (_dir, catalog, work) = initialized_project(&[("a.bin", 800)]);
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
        let (_dir, catalog, _work) = initialized_project(&[("a.bin", 800)]);
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
        let (_dir, catalog, _work) = initialized_project(&[("a.bin", 800)]);
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
        let (_dir, catalog, _work) = initialized_project(&[("a.bin", 800), ("b.bin", 800)]);
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
        let (_dir, catalog, _work) = initialized_project(&[("a.bin", 800), ("b.bin", 800)]);
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
        let (_dir, catalog, _work) = initialized_project(&[("big.bin", 800), ("small.bin", 100)]);
        cmd()
            .arg("--catalog")
            .arg(&catalog)
            .args(["plan", "p"])
            .env("DISKREMENT_DISC_BYTES", "500")
            .assert()
            .success()
            .stdout(contains("Oversized (cannot be placed):").and(contains("big.bin")));
    }

    /// `plan` needs no earlier step: on an empty source it scans, finds
    /// nothing, and says so (#4).
    #[test]
    fn plan_on_a_fresh_project_scans_by_itself() {
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
        let out = cmd()
            .arg("--catalog")
            .arg(&catalog)
            .args(["plan", "p"])
            .assert()
            .success()
            .stdout(
                contains("Scanned: 0 added, 0 modified, 0 deleted, 0 unchanged.\n")
                    .and(contains("Hashed 0 of 0 files (0 fingerprints reused).\n"))
                    .and(contains("Nothing to burn: no changes to plan.")),
            )
            .get_output()
            .stdout
            .clone();
        let s = String::from_utf8(out).unwrap();
        assert!(
            s.find("Scanned:").unwrap() < s.find("Nothing to burn").unwrap(),
            "the scan block comes before the plan block: {s}"
        );
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

/// Shared fixtures for the project-driving modules: `project` builds a
/// scratch project, `base`/`cmd` run the binary without ever touching a real
/// drive, and `burned_project` starts from one burned disc.
fn cmd() -> Command {
    let mut c = Command::cargo_bin("diskrement").expect("binary builds");
    // Never touch a real drive from a test.
    c.env("DISKREMENT_BURNER", "simulated");
    c.env("DISKREMENT_HASH_PARALLELISM", "2");
    c.env_remove("DISKREMENT_DISC_BYTES");
    c
}

struct Project {
    _dir: tempfile::TempDir,
    catalog: std::path::PathBuf,
    work: std::path::PathBuf,
    source: std::path::PathBuf,
}

fn project() -> Project {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    let source = root.join("src");
    let work = root.join("work");
    let catalog = root.join("catalog/cat.sqlite");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(catalog.parent().unwrap()).unwrap();
    std::fs::write(source.join("a.bin"), vec![b'a'; 800]).unwrap();
    Project {
        _dir: dir,
        catalog,
        work,
        source,
    }
}

fn base(p: &Project) -> Command {
    let mut c = cmd();
    c.arg("--catalog").arg(&p.catalog);
    c
}

fn init_and_plan(p: &Project) {
    base(p)
        .args(["init", "p", "--source"])
        .arg(&p.source)
        .arg("--work-dir")
        .arg(&p.work)
        .assert()
        .success();
    base(p).args(["plan", "p"]).assert().success();
}

struct Fixture {
    _dir: tempfile::TempDir,
    catalog: std::path::PathBuf,
    work: std::path::PathBuf,
}

/// init → plan → burn, leaving one burned disc with two files staged.
/// Every assertion that starts from a burn starts here.
fn burned_project() -> Fixture {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let source = root.join("source");
    std::fs::create_dir_all(source.join("nested")).unwrap();
    std::fs::write(source.join("a.bin"), vec![b'a'; 800]).unwrap();
    std::fs::write(source.join("nested/c.bin"), vec![b'c'; 100]).unwrap();
    let catalog = root.join("catalog/cat.sqlite");
    std::fs::create_dir_all(catalog.parent().unwrap()).unwrap();
    let work = root.join("work");

    let run = |c: &std::path::Path| {
        let mut x = cmd();
        x.arg("--catalog").arg(c);
        x
    };
    run(&catalog)
        .args(["init", "p", "--source"])
        .arg(&source)
        .arg("--work-dir")
        .arg(&work)
        .assert()
        .success();
    run(&catalog).args(["plan", "p"]).assert().success();
    run(&catalog)
        .args(["burn", "p", "--yes"])
        .assert()
        .success();

    Fixture {
        _dir: dir,
        catalog,
        work,
    }
}

mod copies {
    use super::*;

    /// One version, two copies, one of them on a dead disc: `history` prints
    /// the version once with both copies under it (#7).
    #[test]
    fn history_lists_a_versions_copies_under_it() {
        let p = project();
        base(&p)
            .args(["init", "p", "--source"])
            .arg(&p.source)
            .arg("--work-dir")
            .arg(&p.work)
            .args(["--copies", "2"])
            .assert()
            .success();
        base(&p).args(["plan", "p"]).assert().success();
        base(&p).args(["burn", "p", "--yes"]).assert().success();
        base(&p)
            .args(["disc", "mark-dead", "p", "1B", "--yes"])
            .assert()
            .success();

        // `a.bin` is 800 bytes of `a`.
        let hash = "0868513521f77faae1cf5200d9ff9603007fa04e0caef9f81183b44d46cbba86";
        base(&p)
            .args(["history", "p", "a.bin"])
            .assert()
            .success()
            .stdout(format!(
                "a.bin\n\
                 \x20 v1  0.0 GB  {hash}\n\
                 \x20     disc 1A / session 1   CURRENT\n\
                 \x20     disc 1B / session 1   CURRENT (on a dead disc)\n"
            ));
    }

    #[test]
    fn copies_outside_one_to_twenty_six_is_a_usage_error() {
        for n in ["0", "27"] {
            let p = project();
            base(&p)
                .args(["init", "p", "--source"])
                .arg(&p.source)
                .arg("--work-dir")
                .arg(&p.work)
                .args(["--copies", n])
                .assert()
                .failure()
                .code(1);
        }
    }

    /// At copies=2 a bare `1` means 1A, and the prompt must say so before
    /// anything irreversible happens. A child with no stdin declines.
    #[test]
    fn mark_dead_confirms_with_the_resolved_disc_name() {
        let p = project();
        base(&p)
            .args(["init", "p", "--source"])
            .arg(&p.source)
            .arg("--work-dir")
            .arg(&p.work)
            .args(["--copies", "2"])
            .assert()
            .success();

        base(&p)
            .args(["disc", "mark-dead", "p", "1"])
            .assert()
            .success()
            .stdout(contains(
                "Mark disc 1A of p DEAD? Superseded versions held only on it are \
                 lost forever; its live files will re-plan.",
            ))
            .stdout(contains("Aborted."));
    }
}

mod burn {
    use super::*;

    #[test]
    fn burn_writes_the_disc_and_reports_it() {
        let p = project();
        init_and_plan(&p);

        base(&p)
            .args(["burn", "p", "--yes"])
            .assert()
            .success()
            .stdout(contains("✔ Wrote disc 1  —  label: \"p disc 1\""))
            .stdout(contains(
                "Status: left OPEN — may receive more sessions later.",
            ))
            .stdout(contains("No more changes — the set is up to date."));

        assert!(p.work.join("staging/disc-1/data/a.bin").is_file());
        assert!(p
            .work
            .join("staging/disc-1/.diskrement/manifest.json")
            .is_file());
        assert!(p
            .work
            .join("staging/disc-1/.diskrement/catalog.sqlite")
            .is_file());
    }

    /// A second burn with nothing staged is the state error, exit 2.
    #[test]
    fn burning_twice_fails_with_the_staged_state_error() {
        let p = project();
        init_and_plan(&p);
        base(&p).args(["burn", "p", "--yes"]).assert().success();

        base(&p)
            .args(["burn", "p", "--yes"])
            .assert()
            .code(2)
            .stderr(contains("no staged plan for p (run plan)"));
    }

    #[test]
    fn burning_an_unknown_project_exits_two() {
        let p = project();
        init_and_plan(&p);
        base(&p)
            .args(["burn", "nosuch", "--yes"])
            .assert()
            .code(2)
            .stderr(contains("project not found: nosuch"));
    }

    /// Without `--yes` and with no terminal, the read hits EOF and the burn is
    /// refused: exit 0, nothing written.
    #[test]
    fn declining_the_prompt_is_a_successful_no_op() {
        let p = project();
        init_and_plan(&p);

        base(&p)
            .args(["burn", "p"])
            .write_stdin("n\n")
            .assert()
            .success()
            .stdout(contains("Aborted."));

        assert!(!p.work.join("staging").exists(), "nothing was staged");
        // The plan survives, so the operator can retry.
        assert!(p.work.join("staged/plan.json").is_file());
    }

    /// A bad `--burner` value is an argument error (exit 1), the one place
    /// spec §3.2 lets clap and zio-cli word things differently.
    #[test]
    fn a_bogus_burner_flag_is_an_argument_error() {
        let p = project();
        base(&p)
            .args(["--burner", "pretend", "burn", "p", "--yes"])
            .assert()
            .code(1);
    }

    /// The full cycle the corpus records: spill across two discs, one disc per
    /// burn, with the remaining-discs line changing between them.
    #[test]
    fn two_cycles_spill_across_two_discs() {
        let p = project();
        std::fs::write(p.source.join("b.bin"), vec![b'b'; 800]).unwrap();
        let with_shrink = |p: &Project| {
            let mut c = base(p);
            c.env("DISKREMENT_DISC_BYTES", "1000");
            c
        };

        with_shrink(&p)
            .args(["init", "p", "--source"])
            .arg(&p.source)
            .arg("--work-dir")
            .arg(&p.work)
            .assert()
            .success();
        with_shrink(&p).args(["plan", "p"]).assert().success();
        with_shrink(&p)
            .args(["burn", "p", "--yes"])
            .assert()
            .success()
            .stdout(contains("✔ Wrote disc 1"))
            .stdout(contains("more discs of changes remain"));

        with_shrink(&p).args(["plan", "p"]).assert().success();
        with_shrink(&p)
            .args(["burn", "p", "--yes"])
            .assert()
            .success()
            .stdout(contains("✔ Wrote disc 2"))
            .stdout(contains("No more changes — the set is up to date."));

        assert!(p.work.join("staging/disc-1/data/a.bin").is_file());
        assert!(p.work.join("staging/disc-2/data/b.bin").is_file());
    }
}

mod verify_history_reset {
    use super::*;

    fn cmd() -> Command {
        let mut c = Command::cargo_bin("diskrement").expect("binary builds");
        c.env("DISKREMENT_BURNER", "simulated");
        c.env_remove("DISKREMENT_DEVICE");
        c.env_remove("DISKREMENT_DISC_BYTES");
        c
    }

    fn run(f: &Fixture, args: &[&str]) -> assert_cmd::assert::Assert {
        let mut c = cmd();
        c.arg("--catalog").arg(&f.catalog);
        c.args(args).assert()
    }

    #[test]
    fn verify_of_a_clean_burn_exits_0_and_names_every_disc() {
        let f = burned_project();
        run(&f, &["verify", "p"])
            .success()
            .stdout("disc 1: 2 files OK ✓\n");
    }

    /// Result-driven exit: corruption found is a successful run with a bad
    /// outcome, and its code is 4, not one of the error-ladder codes.
    #[test]
    fn a_corrupted_staged_file_exits_4_and_lists_the_path() {
        let f = burned_project();
        std::fs::write(f.work.join("staging/disc-1/data/a.bin"), b"CORRUPT").unwrap();
        run(&f, &["verify", "p"])
            .failure()
            .code(4)
            .stdout("disc 1: 2 checked, 1 MISMATCH:\n  a.bin\n");
    }

    #[test]
    fn verifying_a_named_disc_that_does_not_exist_exits_2() {
        let f = burned_project();
        run(&f, &["verify", "p", "--disc", "99"])
            .failure()
            .code(2)
            .stderr(contains("no disc with ordinal 99"));
    }

    #[test]
    fn a_set_letter_past_the_projects_copies_is_an_unknown_disc() {
        let f = burned_project();
        run(&f, &["verify", "p", "--disc", "1B"])
            .failure()
            .code(2)
            .stderr(contains("no disc with ordinal 1B"));
    }

    /// The parser must not turn a negative ordinal into an argument error:
    /// the Scala build answers it from the domain, with exit 2.
    #[test]
    fn a_negative_disc_ordinal_is_a_lookup_failure_not_an_argument_error() {
        let f = burned_project();
        run(&f, &["verify", "p", "--disc", "-1"])
            .failure()
            .code(2)
            .stderr(contains("no disc with ordinal -1"));
    }

    #[test]
    fn verify_before_any_burn_says_there_is_nothing_to_check() {
        let dir = tempfile::TempDir::new().unwrap();
        let catalog = dir.path().join("cat.sqlite");
        let source = dir.path().join("source");
        let work = dir.path().join("work");
        std::fs::create_dir_all(&source).unwrap();
        let mut c = cmd();
        c.arg("--catalog")
            .arg(&catalog)
            .args(["init", "p", "--source"])
            .arg(&source)
            .arg("--work-dir")
            .arg(&work)
            .assert()
            .success();
        let mut c = cmd();
        c.arg("--catalog")
            .arg(&catalog)
            .args(["verify", "p"])
            .assert()
            .success()
            .stdout("Nothing to verify: no burned discs.\n");
    }

    #[test]
    fn history_prints_one_line_per_version() {
        let f = burned_project();
        run(&f, &["history", "p", "nested/c.bin"]).success().stdout(
            contains("nested/c.bin\n")
                .and(contains("  v1  disc 1 / session 1   0.0 GB  "))
                .and(contains("  CURRENT")),
        );
    }

    #[test]
    fn history_of_an_unknown_path_says_so_and_exits_0() {
        let f = burned_project();
        run(&f, &["history", "p", "nope.bin"])
            .success()
            .stdout("No history for nope.bin.\n");
    }

    /// `reset` destroys the staging trees, so the next verify skips the disc
    /// rather than reporting it clean — and the catalog still knows the disc
    /// was burned.
    #[test]
    fn reset_clears_the_work_dir_and_the_next_verify_skips() {
        let f = burned_project();
        assert!(f.work.join("staging/disc-1").is_dir());
        run(&f, &["reset", "p"]).success().stdout("Reset p.\n");
        assert!(!f.work.join("staging").exists());
        assert!(!f.work.join("snapshot").exists());
        run(&f, &["verify", "p"])
            .success()
            .stdout("disc 1: skipped (not checked)\n");
        run(&f, &["status", "p"])
            .success()
            .stdout(contains("  #1   "));
    }

    /// `reset` discards the staged plan; the next `plan` simply scans again.
    #[test]
    fn planning_after_a_reset_scans_again() {
        let f = burned_project();
        run(&f, &["plan", "p"]).success();
        run(&f, &["reset", "p"]).success();
        assert!(!f.work.join("staged").exists());
        run(&f, &["plan", "p"]).success().stdout(contains(
            "Scanned: 0 added, 0 modified, 0 deleted, 2 unchanged.",
        ));
    }

    #[test]
    fn the_three_new_commands_exit_2_on_an_unknown_project() {
        let dir = tempfile::TempDir::new().unwrap();
        let catalog = dir.path().join("cat.sqlite");
        for args in [
            vec!["verify", "nosuch"],
            vec!["history", "nosuch", "a.bin"],
            vec!["reset", "nosuch"],
        ] {
            let mut c = cmd();
            c.arg("--catalog")
                .arg(&catalog)
                .args(&args)
                .assert()
                .failure()
                .code(2)
                .stderr(contains("project not found: nosuch"));
        }
    }
}

/// `disc mark-dead`, the lost report, and the recovery cycle.
mod mark_dead {
    use super::*;

    /// The content hashes of `b.bin`'s two versions, which the lost report and
    /// `history` print verbatim. Written out, not computed at test time: a
    /// helper that hashed the same bytes the fixture wrote would agree with
    /// itself no matter what the binary printed.
    const B1: &str = "b9749f157163b4bbb7faec0a55cfb88c69968a25eac4cfe13e45d6286874709a";
    const B2: &str = "49a7906775663af7970f00f946d1d540822233477e75f5b2912ca497780a01bc";

    fn cmd() -> Command {
        let mut c = Command::cargo_bin("diskrement").expect("binary builds");
        c.env("DISKREMENT_BURNER", "simulated");
        c.env_remove("DISKREMENT_DEVICE");
        c.env_remove("DISKREMENT_DISC_BYTES");
        c
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        catalog: std::path::PathBuf,
        source: std::path::PathBuf,
    }

    fn run(f: &Fixture, args: &[&str]) -> assert_cmd::assert::Assert {
        let mut c = cmd();
        c.arg("--catalog").arg(&f.catalog);
        c.args(args).assert()
    }

    /// Two burned sessions on one open disc: `a.bin` current from session 1,
    /// `b.bin` superseded in session 2. Disc 1 therefore holds one of each
    /// casualty class, which is what makes both halves of the report non-empty.
    fn two_session_disc() -> Fixture {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        let source = root.join("source");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("a.bin"), vec![b'a'; 800]).unwrap();
        std::fs::write(source.join("b.bin"), vec![b'b'; 800]).unwrap();
        let catalog = root.join("catalog/cat.sqlite");
        std::fs::create_dir_all(catalog.parent().unwrap()).unwrap();
        let work = root.join("work");

        let f = Fixture {
            _dir: dir,
            catalog,
            source,
        };
        let mut c = cmd();
        c.arg("--catalog")
            .arg(&f.catalog)
            .args(["init", "p", "--source"])
            .arg(&f.source)
            .arg("--work-dir")
            .arg(&work)
            .assert()
            .success();
        run(&f, &["plan", "p"]).success();
        run(&f, &["burn", "p", "--yes"]).success();

        std::fs::write(f.source.join("b.bin"), vec![b'B'; 800]).unwrap();
        run(&f, &["plan", "p"]).success();
        run(&f, &["burn", "p", "--yes"]).success();
        f
    }

    #[test]
    fn mark_dead_prints_both_casualty_blocks_and_exits_0() {
        let f = two_session_disc();
        run(&f, &["disc", "mark-dead", "p", "1", "--yes"])
            .success()
            .stdout(format!(
                "Disc 1 marked DEAD.\n\
                 Permanently lost (1):\n\
                 \x20 b.bin  0.0 GB  {B1}  (was session 1)\n\
                 Re-burn on next plan/burn (2):\n\
                 \x20 a.bin\n\
                 \x20 b.bin\n"
            ));
    }

    /// The disc becomes DEAD in `status` — the one place the CLI shows the
    /// derived state — while the row behind it is untouched (asserted in the
    /// shell's pipeline `mark_dead_tests`, which can see SQL). Its files are
    /// still current versions, now with no live copy (issue #6).
    #[test]
    fn status_reports_the_disc_dead_and_its_files_as_having_no_live_copy() {
        let f = two_session_disc();
        run(&f, &["disc", "mark-dead", "p", "1", "--yes"]).success();
        run(&f, &["status", "p"]).success().stdout(
            contains("  #1   DEAD")
                .and(contains(
                    "No live copy: 2 files (0.0 GB) — re-burn on next plan/burn.\n",
                ))
                .and(contains("Current files: 2")),
        );
    }

    #[test]
    fn marking_the_same_disc_twice_is_exit_2() {
        let f = two_session_disc();
        run(&f, &["disc", "mark-dead", "p", "1", "--yes"]).success();
        run(&f, &["disc", "mark-dead", "p", "1", "--yes"])
            .failure()
            .code(2)
            .stderr(contains("disc 1 is already marked dead"));
    }

    #[test]
    fn an_ordinal_no_disc_has_is_exit_2() {
        let f = two_session_disc();
        run(&f, &["disc", "mark-dead", "p", "9", "--yes"])
            .failure()
            .code(2)
            .stderr(contains("no disc with ordinal 9"));
    }

    #[test]
    fn mark_dead_on_an_unknown_project_is_exit_2() {
        let f = two_session_disc();
        run(&f, &["disc", "mark-dead", "nosuch", "1", "--yes"])
            .failure()
            .code(2)
            .stderr(contains("project not found: nosuch"));
    }

    /// The parse-time boundary, through the real binary: exit **1**, and the
    /// catalog is never opened. Compare `verify --disc -1`, which is exit 2 —
    /// the two ordinals are deliberately different, and this is the assertion
    /// that keeps them from drifting together.
    #[test]
    fn a_non_positive_or_out_of_range_ordinal_is_an_argument_error() {
        let f = two_session_disc();
        for bad in ["0", "-1", "4294967297", "abc"] {
            run(&f, &["disc", "mark-dead", "p", bad, "--yes"])
                .failure()
                .code(1);
        }
        run(&f, &["verify", "p", "--disc", "-1"])
            .failure()
            .code(2)
            .stderr(contains("no disc with ordinal -1"));
    }

    /// Refusing the prompt is a successful no-op: exit 0, nothing appended,
    /// and the disc still reads OPEN.
    #[test]
    fn declining_the_prompt_leaves_the_disc_alive() {
        let f = two_session_disc();
        let mut c = cmd();
        c.arg("--catalog")
            .arg(&f.catalog)
            .args(["disc", "mark-dead", "p", "1"])
            .write_stdin("n\n")
            .assert()
            .success()
            .stdout(
                "Mark disc 1 of p DEAD? Superseded versions held only on it are lost \
                 forever; its live files will re-plan. [y/N] Aborted.\n",
            );

        run(&f, &["status", "p"])
            .success()
            .stdout(contains("  #1   OPEN"));
    }

    /// The whole point of the phase: the live files come back. After the
    /// coaster, `plan` scans both paths as *added* and opens a fresh disc
    /// 2, and the recovery burn restores them — while `history` keeps both
    /// dead copies visible as LOST.
    #[test]
    fn the_live_files_re_plan_onto_a_fresh_disc_and_history_shows_lost_per_copy() {
        let f = two_session_disc();
        run(&f, &["disc", "mark-dead", "p", "1", "--yes"]).success();

        run(&f, &["plan", "p"])
            .success()
            .stdout(contains(
                "Scanned: 2 added, 0 modified, 0 deleted, 0 unchanged.",
            ))
            .stdout(contains("Plan: disc 2 (new disc, BD_R) — 2 files"));
        run(&f, &["burn", "p", "--yes"])
            .success()
            .stdout(contains("Wrote disc 2"));

        // LOST is per physical copy: v2 and v3 share one content hash.
        run(&f, &["history", "p", "b.bin"])
            .success()
            .stdout(format!(
                "b.bin\n\
             \x20 v1  disc 1 / session 1   0.0 GB  {B1}  LOST\n\
             \x20 v2  disc 1 / session 2   0.0 GB  {B2}  LOST\n\
             \x20 v3  disc 2 / session 1   0.0 GB  {B2}  CURRENT\n"
            ));

        // The sweep skips the dead disc silently; naming it is an error.
        run(&f, &["verify", "p"])
            .success()
            .stdout("disc 2: 2 files OK ✓\n");
        run(&f, &["verify", "p", "--disc", "1"])
            .failure()
            .code(2)
            .stderr(contains("disc 1 is marked dead"));
    }

    /// Every disc dead: the sweep has nothing left to check and says so,
    /// rather than reporting a clean run over an empty list.
    #[test]
    fn a_project_whose_only_disc_died_has_nothing_to_verify() {
        let f = two_session_disc();
        run(&f, &["disc", "mark-dead", "p", "1", "--yes"]).success();
        run(&f, &["verify", "p"])
            .success()
            .stdout("Nothing to verify: no burned discs.\n");
    }
}

/// The real backend, driven as far as a machine with no drive can go.
mod real_backend {
    use super::*;

    /// Is real `xorriso` installed? The burn path preflights it, so a machine
    /// without it can only test the tool-missing branch.
    fn has_xorriso() -> bool {
        std::process::Command::new("xorriso")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// `--burner real` asks for a disc. With no stdin, the answer is "no", and
    /// an aborted burn is a successful run that wrote nothing: exit 0,
    /// `Aborted.` on stderr, and a catalog with no discs in it.
    #[test]
    fn a_real_burn_asks_for_the_disc_and_an_unanswered_prompt_aborts() {
        if !has_xorriso() {
            eprintln!(
                "SKIP [a_real_burn_asks_for_the_disc_and_an_unanswered_prompt_aborts]: \
                 xorriso is not installed; the burn path preflights it. Install the \
                 libisoburn/xorriso package and re-run."
            );
            return;
        }
        let p = project();
        init_and_plan(&p);

        Command::cargo_bin("diskrement")
            .expect("binary builds")
            .arg("--catalog")
            .arg(&p.catalog)
            .args([
                "--burner", "real", "--device", "/dev/sr0", "burn", "p", "--yes",
            ])
            .env_remove("DISKREMENT_BURNER")
            .env_remove("DISKREMENT_DEVICE")
            .assert()
            .code(0)
            .stdout(contains(
                "Insert a blank disc for 'p' disc 1 into /dev/sr0 and confirm. [y/N]",
            ))
            .stderr(contains("Aborted."));

        // Commit-after-success: the aborted burn committed nothing.
        base(&p)
            .args(["status", "p"])
            .assert()
            .success()
            .stdout(contains("Last burn: never"));
        // And the staged plan survives, so the operator can retry.
        assert!(p.work.join("staged/plan.json").is_file());
    }

    /// Without xorriso on `PATH` the same run fails at the preflight, with the
    /// message that names the package to install — exit 6, the media/burner
    /// rung.
    #[test]
    fn a_real_burn_without_xorriso_says_what_to_install() {
        let p = project();
        init_and_plan(&p);

        Command::cargo_bin("diskrement")
            .expect("binary builds")
            .arg("--catalog")
            .arg(&p.catalog)
            .args(["--burner", "real", "burn", "p", "--yes"])
            .env_remove("DISKREMENT_BURNER")
            // An empty PATH reaches only the child: assert_cmd invokes the
            // binary under test by absolute path.
            .env("PATH", "/nonexistent")
            .assert()
            .code(6)
            .stderr(contains("xorriso is not available"))
            .stderr(contains("install the libisoburn/xorriso package"));
    }

    /// `verify` on the real backend asks for each disc and skips the ones the
    /// operator does not produce. A skipped disc is reported as skipped, and
    /// the run still exits 0 — nothing was found wrong, because nothing was
    /// checked.
    #[test]
    fn a_real_verify_asks_for_each_disc_and_skips_the_ones_not_produced() {
        let f = burned_project();

        Command::cargo_bin("diskrement")
            .expect("binary builds")
            .env("DISKREMENT_BURNER", "real")
            .env("DISKREMENT_DEVICE", "/dev/sr0")
            .arg("--catalog")
            .arg(&f.catalog)
            .args(["verify", "p"])
            .assert()
            .code(0)
            .stdout(contains(
                "Insert disc 1 of 'p' into /dev/sr0 to verify it (or decline to skip this disc).",
            ))
            .stdout(contains("disc 1: skipped (not checked)"));
    }
}

/// A catch-up copy too large for the plan's disc type (#12).
mod unplaceable_catch_up {
    use super::*;

    fn run(
        catalog: &std::path::Path,
        args: &[&str],
        disc_bytes: Option<&str>,
    ) -> assert_cmd::assert::Assert {
        let mut c = cmd();
        if let Some(n) = disc_bytes {
            c.env("DISKREMENT_DISC_BYTES", n);
        }
        c.arg("--catalog").arg(catalog).args(args).assert()
    }

    /// Set B's only copy of `a.bin` dies, and the disc type now planned is too
    /// small for it. The plan skips it rather than refusing, `burn` exits 0
    /// with nothing to write, and neither claims the sets are up to date.
    #[test]
    fn a_catch_up_copy_too_large_is_skipped_and_the_set_reads_incomplete() {
        let p = project();
        base(&p)
            .args(["init", "p", "--source"])
            .arg(&p.source)
            .arg("--work-dir")
            .arg(&p.work)
            .args(["--copies", "2"])
            .assert()
            .success();
        run(&p.catalog, &["plan", "p"], None).success();
        run(&p.catalog, &["burn", "p", "--yes"], None).success();
        run(&p.catalog, &["disc", "mark-dead", "p", "1B", "--yes"], None).success();

        run(&p.catalog, &["plan", "p"], Some("500"))
            .success()
            .stdout(contains("Catch-up copies too large for BD_R (skipped;"))
            .stdout(contains("  a.bin  0.0 GB"))
            .stdout(contains("Oversized").not());
        run(&p.catalog, &["burn", "p", "--yes"], None)
            .success()
            .stdout(contains("Nothing burned: no disc in this plan."))
            .stdout(contains("Copies: set B is missing 1 files (0.0 GB)."))
            .stdout(contains("Copy sets are incomplete. Run: plan → burn"))
            .stdout(contains("up to date").not());
    }
}
