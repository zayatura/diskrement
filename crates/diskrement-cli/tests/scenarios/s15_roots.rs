//! Scenario 15: three Immich-style libraries as named roots of one project.
//! Discs fill by date across the libraries rather than one library after
//! another, in a copies=2 project like the real photo backup. Each disc
//! carries the roots as readable top-level directories, and its embedded
//! catalog records them. Unlike s05, which file lands on
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
        "u1/2022/2022-03-01/a.jpg",
        "u1/2023/2023-07-01/d.jpg",
        "u2/2020/2020-01-01/b.jpg",
        "u3/2021/2021-05-05/c.jpg",
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
        "--copies",
        "2",
    ]);
    assert_eq!(r.code, 0, "init failed: {}{}", r.stdout, r.stderr);

    // Cycle 1: the two oldest files, b (2020) and c (2021). They sit in
    // different groups (`family/`, `others/`), so this split is wrong under
    // path order (anna's a and d first), group-first or round-robin order
    // (a and b), and basename order (a and b).
    let pl1 = cli.run(&["plan", "fam"]);
    assert_eq!(pl1.code, 0, "{}{}", pl1.stdout, pl1.stderr);
    assert!(pl1.out_has("Scanned: 4 added"), "{}", pl1.stdout);
    let b1 = cli.run(&["burn", "fam", "--yes"]);
    assert_eq!(b1.code, 0, "{}{}", b1.stdout, b1.stderr);
    let oldest = support::set(&[
        "family/bela/2020/2020-01-01/b.jpg",
        "others/cecil/2021/2021-05-05/c.jpg",
    ]);
    for name in ["1A", "1B"] {
        assert_eq!(
            disc::data_files(&s.disc_dir_named(name)),
            oldest,
            "disc {name}"
        );
    }

    // Cycle 2: the rest.
    let pl2 = cli.run(&["plan", "fam"]);
    assert_eq!(pl2.code, 0, "{}{}", pl2.stdout, pl2.stderr);
    let b2 = cli.run(&["burn", "fam", "--yes"]);
    assert_eq!(b2.code, 0, "{}{}", b2.stdout, b2.stderr);
    let newest = support::set(&[
        "family/anna/2022/2022-03-01/a.jpg",
        "family/anna/2023/2023-07-01/d.jpg",
    ]);
    for name in ["2A", "2B"] {
        assert_eq!(
            disc::data_files(&s.disc_dir_named(name)),
            newest,
            "disc {name}"
        );
    }

    // The disc describes its own layout: its embedded catalog has the roots.
    // `immutable=1`, so reading it writes nothing beside it.
    let embedded = rusqlite::Connection::open_with_flags(
        format!(
            "file:{}?immutable=1",
            s.disc_dir_named("2B")
                .join(".diskrement/catalog.sqlite")
                .display()
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
    assert_eq!(st.code, 0, "{}{}", st.stdout, st.stderr);
    assert!(st.out_has("(3 roots, BD_R"), "{}", st.stdout);
    for (name, dir) in [
        ("family/anna", "u1"),
        ("family/bela", "u2"),
        ("others/cecil", "u3"),
    ] {
        let line = format!("  {name} → {}", s.source.join(dir).display());
        assert!(st.line_starts_with(&line), "{}", st.stdout);
    }

    let v = cli.run(&["verify", "fam"]);
    assert_eq!(v.code, 0, "{}{}", v.stdout, v.stderr);
    for name in ["1A", "1B", "2A", "2B"] {
        assert!(v.line_starts_with(&format!("disc {name}:")), "{}", v.stdout);
    }
    let h = cli.run(&["history", "fam", "family/bela/2020/2020-01-01/b.jpg"]);
    assert_eq!(h.code, 0, "{}{}", h.stdout, h.stderr);
    assert!(h.out_has("  v1  "), "{}", h.stdout);
    assert!(h.out_has("disc 1A / session 1   CURRENT"), "{}", h.stdout);
    assert!(h.out_has("disc 1B / session 1   CURRENT"), "{}", h.stdout);
}
