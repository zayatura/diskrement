//! Scenario 4: three cycles under a 1000-byte capacity override, each
//! producing one disc. Cycle 3 modifies `a` and deletes `b`, landing the new
//! `a` on disc 3.

use crate::{disc, support};

#[test]
fn three_cycles_span_discs_one_two_and_three_with_correct_history() {
    let Some(s) = support::scratch_or_skip("s04 three-disc incremental") else {
        return;
    };
    let cli = s.cli_with_disc_bytes(1000);
    support::write(&s.source, "a.bin", 800);

    cli.init(&s, "p");

    // Cycle 1 -> disc 1 [a]
    cli.run(&["plan", "p"]);
    cli.run(&["burn", "p", "--yes"]);

    // Cycle 2 -> disc 2 [b]
    support::write(&s.source, "b.bin", 800);
    let pl2 = cli.run(&["plan", "p"]);
    assert_eq!(pl2.code, 0);
    assert!(pl2.out_has("new disc"));
    cli.run(&["burn", "p", "--yes"]);

    // Cycle 3 -> disc 3 [a'], plus b's tombstone
    support::modify(&s.source, "a.bin", 800);
    support::delete(&s.source, "b.bin");
    let pl3 = cli.run(&["plan", "p"]);
    assert_eq!(pl3.code, 0);
    assert!(pl3.out_has("Scanned: 0 added, 1 modified, 1 deleted, 0 unchanged."));
    assert!(pl3.out_has("new disc"));
    cli.run(&["burn", "p", "--yes"]);

    assert_eq!(disc::data_files(&s.disc_dir(1)), support::set(&["a.bin"]));
    assert_eq!(disc::data_files(&s.disc_dir(2)), support::set(&["b.bin"]));
    assert_eq!(disc::data_files(&s.disc_dir(3)), support::set(&["a.bin"]));

    let m3 = disc::manifest(&s.disc_dir(3));
    assert_eq!(m3.disc_ordinal, 3);
    assert_eq!(disc::manifest_modified(&m3), support::set(&["a.bin"]));
    assert_eq!(disc::manifest_deleted(&m3), support::set(&["b.bin"]));

    let ha = cli.run(&["history", "p", "a.bin"]);
    assert_eq!(ha.code, 0);
    assert!(ha.out_has("SUPERSEDED"));
    assert!(ha.out_has("CURRENT"));

    let hb = cli.run(&["history", "p", "b.bin"]);
    assert_eq!(hb.code, 0);
    assert!(hb.out_has("DELETED"));

    let status = cli.run(&["status", "p"]);
    assert_eq!(status.code, 0);
    assert!(status.out_has("Current files: 1"));

    let verify = cli.run(&["verify", "p"]);
    assert_eq!(verify.code, 0);
    assert!(verify.out_has("files OK"));
    assert!(!verify.out_has("MISMATCH"));
}
