//! Scenario 6: a file larger than the disc. `plan` reports it and still
//! exits 0; `burn` refuses with exit 3 (environment/oversize, per the exit
//! contract in `render.rs`); nothing is staged.

use crate::support;

#[test]
fn a_file_larger_than_a_disc_is_reported_and_burn_refuses() {
    let Some(s) = support::scratch_or_skip("s06 oversized file") else {
        return;
    };
    let cli = s.cli_with_disc_bytes(1000);
    support::write(&s.source, "huge.bin", 5000);

    cli.init(&s, "p");

    let plan = cli.run(&["plan", "p"]);
    assert_eq!(plan.code, 0);
    assert!(plan.out_has("Oversized (cannot be placed):"));
    assert!(plan.out_has("huge.bin"));

    let burn = cli.run(&["burn", "p", "--yes"]);
    assert_eq!(burn.code, 3);
    assert!(burn.err_has("oversized files cannot be placed"));

    assert!(
        !s.disc_dir(1).exists(),
        "a refused burn must not leave a staged disc behind"
    );
}
