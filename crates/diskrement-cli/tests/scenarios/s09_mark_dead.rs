//! Scenario 9: the coaster recovery flow. A topped-off disc dies; its live
//! content re-plans onto a fresh disc, the version that was superseded
//! *on that disc* is permanently lost, and verify stops looking at the dead
//! disc.
//!
//! CLAUDE.md: `LOST` is per physical copy, not per content hash, and `DEAD`
//! is derived from `disc_event` rather than stored.

use crate::{disc, support};

#[test]
fn a_dead_discs_live_content_replans_and_the_superseded_version_is_lost() {
    let Some(s) = support::scratch_or_skip("s09 mark-dead -> re-plan") else {
        return;
    };
    let cli = s.cli();
    support::write(&s.source, "a.txt", 128);
    support::write(&s.source, "b.txt", 128);

    cli.init(&s, "p");

    // Cycle 1: a + b to disc 1, left open.
    cli.run(&["plan", "p"]);
    cli.run(&["burn", "p", "--yes"]);

    // Cycle 2: modify b. Session 2 tops off disc 1, superseding b@v0 in
    // place — which is what makes the loss below unrecoverable.
    support::modify(&s.source, "b.txt", 128);
    cli.run(&["plan", "p"]);
    cli.run(&["burn", "p", "--yes"]);

    // The coaster event.
    let dead = cli.run(&["disc", "mark-dead", "p", "1", "--yes"]);
    assert_eq!(dead.code, 0);
    assert!(dead.out_has("Disc 1 marked DEAD."));
    assert!(dead.out_has("Permanently lost (1):"));
    // Specifically b.txt's superseded version, not merely "b.txt appears
    // somewhere" — it also appears in the re-burn list below.
    assert!(
        dead.stdout
            .lines()
            .any(|l| l.trim_start().starts_with("b.txt") && l.contains("(was session 1)")),
        "the lost entry is not b.txt's session-1 version: {}",
        dead.stdout
    );
    assert!(dead.out_has("Re-burn on next plan/burn (2):"));

    let status = cli.run(&["status", "p"]);
    assert!(status.out_has("DEAD"));
    // a.txt and b.txt are still current, but neither has a copy left.
    assert!(status.out_has("No live copy: 2 files"), "{}", status.stdout);
    assert!(status.out_has("Current files: 2"), "{}", status.stdout);

    // Recovery cycle: everything live re-plans onto a fresh disc 2. The
    // dead disc's content is gone from the burned view, so both files read
    // as added rather than unchanged.
    let pl = cli.run(&["plan", "p"]);
    assert_eq!(pl.code, 0);
    assert!(pl.out_has("Scanned: 2 added, 0 modified, 0 deleted, 0 unchanged."));
    assert!(pl.out_has("new disc"));

    let b2 = cli.run(&["burn", "p", "--yes"]);
    assert_eq!(b2.code, 0);
    assert!(b2.out_has("Wrote disc 2"));
    assert_eq!(
        disc::data_files(&s.disc_dir(2)),
        support::set(&["a.txt", "b.txt"])
    );
    let status = cli.run(&["status", "p"]);
    assert!(!status.out_has("No live copy"), "{}", status.stdout);

    let verify = cli.run(&["verify", "p"]);
    assert_eq!(verify.code, 0);
    // Matched on the line prefix: a bare "disc 1" also matches "disc 10".
    assert!(
        !verify.line_starts_with("disc 1:"),
        "verify must skip the dead disc: {}",
        verify.stdout
    );
    assert!(verify.line_starts_with("disc 2:"));
    assert!(verify.out_has("files OK"));

    let hb = cli.run(&["history", "p", "b.txt"]);
    assert_eq!(hb.code, 0);
    assert!(hb.out_has("LOST"));
}

#[test]
fn a_declined_confirmation_aborts_cleanly_and_marks_nothing() {
    let Some(s) = support::scratch_or_skip("s09 declined mark-dead") else {
        return;
    };
    let cli = s.cli();
    support::write(&s.source, "a.txt", 128);

    cli.init(&s, "p");
    cli.run(&["plan", "p"]);
    cli.run(&["burn", "p", "--yes"]);

    // No --yes: stdin is /dev/null, so the prompt reads EOF and declines.
    let dead = cli.run(&["disc", "mark-dead", "p", "1"]);
    assert_eq!(dead.code, 0);
    assert!(dead.out_has("Aborted."));

    let status = cli.run(&["status", "p"]);
    assert!(!status.out_has("DEAD"));
}
