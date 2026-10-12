//! Scenario 13: two copy sets. Every cycle writes one disc per set; a burn
//! that fails on the second set keeps the first and resumes; a dead twin is
//! restored onto a fresh disc of its own set while the other set carries on.

use crate::{disc, support};

fn init_two_copies(cli: &support::Cli, s: &support::Scratch) {
    let r = cli.run(&[
        "init",
        "p",
        "--source",
        &s.source.display().to_string(),
        "--work-dir",
        &s.work.display().to_string(),
        "--copies",
        "2",
    ]);
    assert_eq!(r.code, 0, "init failed: {}{}", r.stdout, r.stderr);
}

fn plan_burn(cli: &support::Cli) -> support::Output {
    let pl = cli.run(&["plan", "p"]);
    assert_eq!(pl.code, 0, "plan: {}{}", pl.stdout, pl.stderr);
    cli.run(&["burn", "p", "--yes"])
}

fn names(m: &[diskrement_shell::burn::json::ManifestEntryJson]) -> Vec<&str> {
    m.iter().map(|e| e.rel_path.as_str()).collect()
}

#[test]
fn each_cycle_writes_one_disc_per_set_and_a_dead_twin_is_restored() {
    let Some(s) = support::scratch_or_skip("s13 two copies") else {
        return;
    };
    let cli = s.cli();
    support::write(&s.source, "a.txt", 128);
    support::write(&s.source, "b.txt", 128);
    init_two_copies(&cli, &s);

    // Cycle 1: both sets receive a and b. 1A records the versions; 1B
    // records copies of them.
    let b1 = plan_burn(&cli);
    assert_eq!(b1.code, 0, "{}{}", b1.stdout, b1.stderr);
    assert!(b1.out_has("Wrote disc 1A"), "{}", b1.stdout);
    assert!(b1.out_has("Wrote disc 1B"), "{}", b1.stdout);
    assert!(
        b1.out_has("Copies: every current file has 2 copies."),
        "{}",
        b1.stdout
    );
    for name in ["1A", "1B"] {
        assert_eq!(
            disc::data_files(&s.disc_dir_named(name)),
            support::set(&["a.txt", "b.txt"]),
            "disc {name}"
        );
    }
    let m1b = disc::manifest(&s.disc_dir_named("1B"));
    assert_eq!(m1b.copy_set, 1);
    assert!(m1b.added.is_empty(), "1B adds nothing; 1A did");
    assert_eq!(names(&m1b.copied), vec!["a.txt", "b.txt"]);
    let id1b = disc::project_json(&s.disc_dir_named("1B"));
    assert_eq!((id1b.copy_set, id1b.copies), (1, 2));

    let st = cli.run(&["status", "p"]);
    assert!(st.out_has("#1A"), "{}", st.stdout);
    assert!(st.out_has("#1B"), "{}", st.stdout);
    assert!(st.out_has("set B: complete"), "{}", st.stdout);

    // Cycle 2: b changes, and 1B's burn fails — a regular file sits where its
    // staging directory goes. 1A is committed anyway.
    support::modify(&s.source, "b.txt", 128);
    let blocker = s.disc_dir_named("1B");
    std::fs::remove_dir_all(&blocker).unwrap();
    std::fs::write(&blocker, b"not a directory").unwrap();
    let b2 = plan_burn(&cli);
    assert_eq!(b2.code, 6, "{}{}", b2.stdout, b2.stderr);
    assert!(
        b2.err_has("1A committed; 1B not written (re-run burn)"),
        "{}",
        b2.stderr
    );
    let st = cli.run(&["status", "p"]);
    assert!(st.out_has("set B: 1 files"), "{}", st.stdout);

    // Re-running burn alone writes only what is left.
    std::fs::remove_file(&blocker).unwrap();
    let b3 = cli.run(&["burn", "p", "--yes"]);
    assert_eq!(b3.code, 0, "{}{}", b3.stdout, b3.stderr);
    assert!(b3.out_has("Wrote disc 1B"), "{}", b3.stdout);
    assert!(!b3.out_has("Wrote disc 1A"), "{}", b3.stdout);
    assert!(cli.run(&["status", "p"]).out_has("set B: complete"));

    // 1B dies. Every version on it is still on 1A: nothing is lost or
    // re-burned; the current ones are short a copy.
    let dead = cli.run(&["disc", "mark-dead", "p", "1B", "--yes"]);
    assert_eq!(dead.code, 0, "{}{}", dead.stdout, dead.stderr);
    assert!(dead.out_has("Disc 1B marked DEAD."), "{}", dead.stdout);
    assert!(
        dead.out_has("No versions permanently lost."),
        "{}",
        dead.stdout
    );
    assert!(dead.out_has("No live files to re-burn."), "{}", dead.stdout);
    assert!(
        dead.out_has("Short of a copy — restored on next scan/plan/burn (2):"),
        "{}",
        dead.stdout
    );
    assert!(
        dead.out_has("Superseded versions still held on another disc: 1"),
        "{}",
        dead.stdout
    );
    // Set B is short both files, but each still has its copy on 1A.
    let st = cli.run(&["status", "p"]);
    assert!(st.out_has("set B: 2 files"), "{}", st.stdout);
    assert!(st.out_has("set A: complete"), "{}", st.stdout);
    assert!(!st.out_has("No live copy"), "{}", st.stdout);

    // Recovery: set B's next disc, 2B, carries the two catch-up copies; set A
    // has nothing to do.
    let pl = cli.run(&["plan", "p"]);
    assert!(
        pl.out_has("Scanned: 0 added, 0 modified, 0 deleted, 2 unchanged."),
        "{}",
        pl.stdout
    );
    assert!(
        pl.out_has("Plan: disc 2B (new disc, BD_R) — 2 files (0 new, 2 catch-up)"),
        "{}",
        pl.stdout
    );
    assert!(!pl.out_has("disc 1A"), "{}", pl.stdout);
    let b4 = cli.run(&["burn", "p", "--yes"]);
    assert_eq!(b4.code, 0, "{}{}", b4.stdout, b4.stderr);
    assert!(b4.out_has("Wrote disc 2B"), "{}", b4.stdout);
    assert_eq!(
        disc::data_files(&s.disc_dir_named("2B")),
        support::set(&["a.txt", "b.txt"])
    );

    let v = cli.run(&["verify", "p"]);
    assert_eq!(v.code, 0, "{}{}", v.stdout, v.stderr);
    assert!(v.line_starts_with("disc 1A:"), "{}", v.stdout);
    assert!(v.line_starts_with("disc 2B:"), "{}", v.stdout);
    assert!(!v.line_starts_with("disc 1B:"), "{}", v.stdout);
    let named = cli.run(&["verify", "p", "--disc", "1B"]);
    assert_eq!(named.code, 2);
    assert!(named.err_has("disc 1B is marked dead"), "{}", named.stderr);
}
