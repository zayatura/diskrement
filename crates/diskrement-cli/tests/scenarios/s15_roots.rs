//! Scenario 15: three Immich-style libraries as named roots of one project.
//! Discs fill by date across the libraries rather than one library after
//! another. Each disc carries the roots as readable top-level directories,
//! and its embedded catalog records them. Unlike s05, which file lands on
//! which disc *is* the contract here (project-roots spec §5).

use crate::{disc, support};

#[test]
fn a_rooted_project_fills_discs_by_date_across_its_roots() {
    let Some(s) = support::scratch_or_skip("s15 named roots") else {
        return;
    };
    // 400-byte files under a 1000-byte cap: two per disc.
    let cli = s.cli_with_disc_bytes(1000);
    for f in [
        "u1/2021/2021-03-01/a.jpg",
        "u1/2023/2023-07-01/d.jpg",
        "u2/2020/2020-01-01/b.jpg",
        "u3/2022/2022-05-05/c.jpg",
    ] {
        support::write(&s.source, f, 400);
    }
    let root = |name: &str, dir: &str| format!("{name}={}", s.source.join(dir).display());
    let work = s.work.display().to_string();
    let (anna, bela, cecil) = (
        root("family/anna", "u1"),
        root("family/bela", "u2"),
        root("others/cecil", "u3"),
    );
    let r = cli.run(&[
        "init",
        "fam",
        "--work-dir",
        &work,
        "--root",
        &anna,
        "--root",
        &bela,
        "--root",
        &cecil,
    ]);
    assert_eq!(r.code, 0, "init failed: {}{}", r.stdout, r.stderr);

    // Cycle 1: the two oldest files, from two different libraries.
    let pl1 = cli.run(&["plan", "fam"]);
    assert_eq!(pl1.code, 0, "{}{}", pl1.stdout, pl1.stderr);
    assert!(pl1.out_has("Scanned: 4 added"), "{}", pl1.stdout);
    let b1 = cli.run(&["burn", "fam", "--yes"]);
    assert_eq!(b1.code, 0, "{}{}", b1.stdout, b1.stderr);
    assert_eq!(
        disc::data_files(&s.disc_dir(1)),
        support::set(&[
            "family/anna/2021/2021-03-01/a.jpg",
            "family/bela/2020/2020-01-01/b.jpg",
        ])
    );

    // Cycle 2: the rest.
    let pl2 = cli.run(&["plan", "fam"]);
    assert_eq!(pl2.code, 0, "{}{}", pl2.stdout, pl2.stderr);
    let b2 = cli.run(&["burn", "fam", "--yes"]);
    assert_eq!(b2.code, 0, "{}{}", b2.stdout, b2.stderr);
    assert_eq!(
        disc::data_files(&s.disc_dir(2)),
        support::set(&[
            "family/anna/2023/2023-07-01/d.jpg",
            "others/cecil/2022/2022-05-05/c.jpg",
        ])
    );

    // The disc describes its own layout: its embedded catalog has the roots.
    // `immutable=1`, so reading it writes nothing beside it.
    let embedded = rusqlite::Connection::open_with_flags(
        format!(
            "file:{}?immutable=1",
            s.disc_dir(2).join(".diskrement/catalog.sqlite").display()
        ),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .unwrap();
    let names: Vec<String> = embedded
        .prepare("select name from project_root order by name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(names, ["family/anna", "family/bela", "others/cecil"]);

    let st = cli.run(&["status", "fam"]);
    assert!(st.out_has("(3 roots, BD_R"), "{}", st.stdout);
    assert!(st.out_has("  family/bela → "), "{}", st.stdout);
    assert_eq!(cli.run(&["verify", "fam"]).code, 0);
    let h = cli.run(&["history", "fam", "family/bela/2020/2020-01-01/b.jpg"]);
    assert_eq!(h.code, 0, "{}{}", h.stdout, h.stderr);
    assert!(h.out_has("  v1  disc 1 / session 1"), "{}", h.stdout);
}
