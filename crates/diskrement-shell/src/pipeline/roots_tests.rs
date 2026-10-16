//! full plan → burn → verify cycle whose discs fill by date across roots
//! (project-roots spec §3.3, §4, §5).

use super::*;
use crate::burn::simulated::SimulatedBurner;
use crate::burn::verifier::StagingVerifier;
use crate::progress::NoopSink;
use chrono::TimeZone;
use diskrement_core::RootName;

struct Env {
    _dir: tempfile::TempDir,
    catalog: Catalog,
    lib: PathBuf,
    work: PathBuf,
}

fn at(offset: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(1_757_000_000 + offset, 0).unwrap()
}

fn name(s: &str) -> RootName {
    s.parse().unwrap()
}

/// Three Immich-style libraries under `lib/` (`<user>/YYYY/YYYY-MM-DD/<file>`).
/// Every file is 100_000 bytes, so two fit the 250_000 usable bytes that a
/// 99.999% margin leaves (see `tiny_project_init` in `tests`).
fn env() -> Env {
    let dir = tempfile::TempDir::new().unwrap();
    let lib = dir.path().join("lib");
    let work = dir.path().join("work");
    for rel in [
        "u1/2021/2021-03-01/a.jpg",
        "u1/2023/2023-07-01/d.jpg",
        "u2/2020/2020-01-01/b.jpg",
        "u3/2022/2022-05-05/c.jpg",
    ] {
        let p = lib.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let body: Vec<u8> = rel.bytes().cycle().take(100_000).collect();
        std::fs::write(&p, body).unwrap();
    }
    std::fs::create_dir_all(&work).unwrap();
    Env {
        catalog: Catalog::open(&dir.path().join("cat.sqlite")).unwrap(),
        _dir: dir,
        lib,
        work,
    }
}

fn three_roots(e: &Env) -> Sources {
    Sources::rooted(vec![
        (name("family/anna"), e.lib.join("u1")),
        (name("family/bela"), e.lib.join("u2")),
        (name("others/cecil"), e.lib.join("u3")),
    ])
    .unwrap()
}

fn init_of(e: &Env, sources: Sources, copies: u32) -> ProjectInit {
    ProjectInit {
        name: "p".into(),
        sources,
        work_dir: e.work.display().to_string(),
        default_disc_type: DiscType::BdR,
        margin_percent: 99.999,
        copies,
        created_at: at(0),
    }
}

fn pipeline(e: &mut Env) -> Pipeline<'_> {
    let paths = PipelinePaths {
        work_dir: e.work.clone(),
        scan_cache: e.work.join("scancache.sqlite"),
    };
    Pipeline::new(&mut e.catalog, paths, &NoopSink)
}

/// Every file staged on disc `name`, relative to its `data/`, sorted.
fn staged(e: &Env, name: &str) -> Vec<String> {
    let data = e.work.join(format!("staging/disc-{name}/data"));
    let mut files: Vec<String> = walkdir::WalkDir::new(&data)
        .into_iter()
        .map(Result::unwrap)
        .filter(|x| x.file_type().is_file())
        .map(|x| {
            x.path()
                .strip_prefix(&data)
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    files.sort();
    files
}

#[test]
fn a_rooted_cycle_fills_each_disc_with_the_oldest_files_across_roots() {
    let mut e = env();
    let pi = init_of(&e, three_roots(&e), 2);
    pipeline(&mut e).init(&pi).unwrap();
    let burner = Burner::Simulated(SimulatedBurner::new(&NoopSink, 2));

    // Cycle 1: the two oldest files. By rel_path alone this disc would hold
    // both of family/anna's files instead.
    let out = pipeline(&mut e).plan("p", None, false, false).unwrap();
    assert_eq!(out.scan.added, 4);
    pipeline(&mut e).burn("p", at(1), &burner).unwrap();
    let oldest = vec![
        "family/anna/2021/2021-03-01/a.jpg",
        "family/bela/2020/2020-01-01/b.jpg",
    ];
    assert_eq!(staged(&e, "1A"), oldest);
    assert_eq!(staged(&e, "1B"), oldest);

    // The disc's embedded catalog names the roots: it describes its own layout.
    // Opened `immutable=1`, so reading it leaves no `-wal`/`-shm` in the staged
    // tree.
    let embedded = rusqlite::Connection::open_with_flags(
        format!(
            "file:{}?immutable=1",
            e.work
                .join("staging/disc-1B/.diskrement/catalog.sqlite")
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

    // Cycle 2: the rest.
    pipeline(&mut e).plan("p", None, false, false).unwrap();
    pipeline(&mut e).burn("p", at(2), &burner).unwrap();
    let newest = vec![
        "family/anna/2023/2023-07-01/d.jpg",
        "others/cecil/2022/2022-05-05/c.jpg",
    ];
    assert_eq!(staged(&e, "2A"), newest);
    assert_eq!(staged(&e, "2B"), newest);

    let verifier = Verifier::Staging(StagingVerifier::new(e.work.join("staging"), &NoopSink));
    let reports = pipeline(&mut e).verify("p", None, &verifier).unwrap();
    assert_eq!(reports.len(), 4);
    assert!(
        reports
            .iter()
            .all(|r| r.mismatches.is_empty() && !r.skipped),
        "{reports:?}"
    );
}

/// `init` refuses roots whose paths overlap, writes no project, and exits 3.
fn refused_overlap(e: &mut Env, roots: Vec<(RootName, PathBuf)>) -> PipelineError {
    let pi = init_of(e, Sources::rooted(roots).unwrap(), 1);
    let err = pipeline(e).init(&pi).unwrap_err();
    assert_eq!(err.exit_code(), 3, "{err}");
    assert!(matches!(
        pipeline(e).status("p"),
        Err(PipelineError::ProjectNotFound(_))
    ));
    err
}

#[test]
fn init_refuses_two_roots_at_the_same_directory() {
    let mut e = env();
    let u1 = e.lib.join("u1");
    let err = refused_overlap(
        &mut e,
        vec![(name("anna"), u1.clone()), (name("bela"), u1.clone())],
    );
    assert!(
        matches!(&err, PipelineError::OverlappingRoots { a, b } if *a == u1 && *b == u1),
        "{err:?}"
    );
}

#[test]
fn init_refuses_a_root_inside_another() {
    let mut e = env();
    let (lib, u1) = (e.lib.clone(), e.lib.join("u1"));
    let err = refused_overlap(
        &mut e,
        vec![(name("all"), lib.clone()), (name("anna"), u1.clone())],
    );
    // Roots sort by name, so `all` comes first.
    assert!(
        matches!(&err, PipelineError::OverlappingRoots { a, b } if *a == lib && *b == u1),
        "{err:?}"
    );
}

#[test]
fn init_resolves_symlinks_before_comparing_roots() {
    let mut e = env();
    let (u1, alias) = (e.lib.join("u1"), e.lib.join("alias"));
    std::os::unix::fs::symlink(&u1, &alias).unwrap();
    let err = refused_overlap(
        &mut e,
        vec![(name("anna"), u1.clone()), (name("alias"), alias.clone())],
    );
    // The error names the paths as given, not as resolved: `alias` is a
    // symlink, so its canonical form is `u1`'s. Roots sort by name, so
    // `alias` comes first.
    assert!(
        matches!(&err, PipelineError::OverlappingRoots { a, b } if *a == alias && *b == u1),
        "{err:?}"
    );
}

#[test]
fn sibling_roots_whose_paths_share_a_prefix_do_not_overlap() {
    let mut e = env();
    std::fs::create_dir_all(e.lib.join("u10")).unwrap();
    let sources = Sources::rooted(vec![
        (name("a"), e.lib.join("u1")),
        (name("b"), e.lib.join("u10")),
    ])
    .unwrap();
    let pi = init_of(&e, sources, 1);
    pipeline(&mut e).init(&pi).unwrap();
}

#[test]
fn a_missing_root_fails_init_naming_it() {
    let mut e = env();
    let gone = e.lib.join("gone");
    let sources = Sources::rooted(vec![
        (name("anna"), e.lib.join("u1")),
        (name("gone"), gone.clone()),
    ])
    .unwrap();
    let pi = init_of(&e, sources, 1);
    let err = pipeline(&mut e).init(&pi).unwrap_err();
    assert_eq!(err.exit_code(), 5);
    assert!(
        err.to_string().contains(&gone.display().to_string()),
        "{err}"
    );
}

#[test]
fn a_root_gone_after_init_fails_plan_naming_it() {
    let mut e = env();
    let pi = init_of(&e, three_roots(&e), 1);
    pipeline(&mut e).init(&pi).unwrap();
    std::fs::remove_dir_all(e.lib.join("u3")).unwrap();
    let err = pipeline(&mut e).plan("p", None, false, false).unwrap_err();
    assert_eq!(err.exit_code(), 5);
    let msg = err.to_string();
    assert!(msg.contains("source root does not exist"), "{msg}");
    assert!(
        msg.contains(&e.lib.join("u3").display().to_string()),
        "{msg}"
    );
}

#[test]
fn init_refuses_a_root_on_another_filesystem_naming_it() {
    let Ok(other) = tempfile::TempDir::new_in("/dev/shm") else {
        eprintln!("SKIP: /dev/shm is not available as a second filesystem");
        return;
    };
    let mut e = env();
    let dev = |p: &Path| std::fs::metadata(p).unwrap().dev();
    if dev(other.path()) == dev(&e.work) {
        eprintln!("SKIP: /dev/shm is on the work dir's filesystem");
        return;
    }
    let sources = Sources::rooted(vec![
        (name("anna"), e.lib.join("u1")),
        (name("elsewhere"), other.path().to_path_buf()),
    ])
    .unwrap();
    let pi = init_of(&e, sources, 1);
    let err = pipeline(&mut e).init(&pi).unwrap_err();
    assert_eq!(err.exit_code(), 3);
    assert!(
        matches!(&err, PipelineError::NotSameFilesystem { src, .. } if src == other.path()),
        "{err:?}"
    );
}
