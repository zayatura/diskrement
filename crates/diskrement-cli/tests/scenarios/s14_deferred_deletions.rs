//! Scenario 14 (issue #9): at copies=2 a deletion-only cycle writes no disc
//! in either set. 1A/1B are sealed first, so the old fallback would have
//! opened an untwinned 2A; instead the deletion is recorded by the next cycle
//! that burns anyway (discs 2A and 2B), and both twins' manifests list it.

use crate::{disc, support};

#[test]
fn a_deletion_only_cycle_writes_no_disc_and_the_next_burn_records_it() {
    let Some(s) = support::scratch_or_skip("s14 deferred deletions") else {
        return;
    };
    let cli = s.cli();
    support::write(&s.source, "a.txt", 128);
    support::write(&s.source, "gone.txt", 128);
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
    assert_eq!(init.code, 0, "{}{}", init.stdout, init.stderr);
    for step in [
        &["scan", "p"][..],
        &["plan", "p", "--finalize"],
        &["burn", "p", "--yes"],
    ] {
        let r = cli.run(step);
        assert_eq!(r.code, 0, "{step:?}: {}{}", r.stdout, r.stderr);
    }

    // Deletion-only cycle: nothing burned, nothing recorded.
    support::delete(&s.source, "gone.txt");
    let sc = cli.run(&["scan", "p"]);
    assert!(
        sc.out_has("Scanned: 0 added, 0 modified, 1 deleted, 1 unchanged."),
        "{}",
        sc.stdout
    );
    let pl = cli.run(&["plan", "p"]);
    assert_eq!(pl.code, 0, "{}{}", pl.stdout, pl.stderr);
    assert!(
        pl.out_has("Nothing to burn: 1 deletion waits for the next disc."),
        "{}",
        pl.stdout
    );
    let b = cli.run(&["burn", "p", "--yes"]);
    assert_eq!(b.code, 0, "{}{}", b.stdout, b.stderr);
    assert!(
        b.out_has("Nothing burned: 1 deletion will be recorded with the next disc."),
        "{}",
        b.stdout
    );
    for name in ["1A", "1B"] {
        assert_eq!(
            disc::manifest(&s.disc_dir_named(name)).session_ordinal,
            1,
            "{name}"
        );
    }
    assert!(!s.disc_dir_named("2A").exists());
    assert!(!s.disc_dir_named("2B").exists());
    let h = cli.run(&["history", "p", "gone.txt"]);
    assert_eq!(h.code, 0, "{}{}", h.stdout, h.stderr);
    assert!(h.out_has("CURRENT"), "{}", h.stdout);
    assert!(!h.out_has("DELETED"), "not recorded yet: {}", h.stdout);

    // The next real cycle carries it on both twins and records it once.
    support::write(&s.source, "b.txt", 128);
    let sc2 = cli.run(&["scan", "p"]);
    assert!(
        sc2.out_has("Scanned: 1 added, 0 modified, 1 deleted, 1 unchanged."),
        "{}",
        sc2.stdout
    );
    let pl2 = cli.run(&["plan", "p"]);
    assert_eq!(pl2.code, 0, "{}{}", pl2.stdout, pl2.stderr);
    let b2 = cli.run(&["burn", "p", "--yes"]);
    assert_eq!(b2.code, 0, "{}{}", b2.stdout, b2.stderr);
    assert!(b2.out_has("Wrote disc 2A"), "{}", b2.stdout);
    assert!(b2.out_has("Wrote disc 2B"), "{}", b2.stdout);
    for name in ["1A", "1B"] {
        assert_eq!(
            disc::manifest(&s.disc_dir_named(name)).session_ordinal,
            1,
            "{name} is untouched"
        );
    }
    for name in ["2A", "2B"] {
        let m = disc::manifest(&s.disc_dir_named(name));
        assert_eq!(m.session_ordinal, 1, "{name}");
        assert_eq!(
            disc::manifest_deleted(&m),
            support::set(&["gone.txt"]),
            "{name}"
        );
    }
    let h2 = cli.run(&["history", "p", "gone.txt"]);
    assert!(h2.out_has("DELETED"), "{}", h2.stdout);
}
