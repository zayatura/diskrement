//! Scenario 12: a source tree shaped like a real one rather than like the
//! content recipe — read-only files, a git-style object store, empty
//! directories, a FIFO, a symlink, spaces and non-ASCII names — goes through
//! a full cycle, and a name that cannot be archived is refused up front.

use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::PermissionsExt as _;

use crate::{disc, support};

fn read_only(path: &std::path::Path) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o444)).unwrap();
}

#[test]
fn a_messy_tree_scans_plans_burns_and_verifies() {
    let Some(s) = support::scratch_or_skip("s12 realistic tree") else {
        return;
    };
    let cli = s.cli();
    support::write(&s.source, "notes/plain.txt", 64);
    support::write(&s.source, "repo/.git/objects/ab/cdef0123", 128);
    read_only(&s.source.join("repo/.git/objects/ab/cdef0123"));
    support::write(&s.source, "photos/ünïcödé name.jpg", 256);
    read_only(&s.source.join("photos/ünïcödé name.jpg"));
    support::write(&s.source, ".hidden", 16);
    std::fs::create_dir_all(s.source.join("empty/dir")).unwrap();
    std::os::unix::fs::symlink("notes/plain.txt", s.source.join("link.txt")).unwrap();
    let mkfifo = std::process::Command::new("mkfifo")
        .arg(s.source.join("pipe"))
        .status()
        .expect("mkfifo runs");
    assert!(mkfifo.success(), "mkfifo");

    cli.init(&s, "p");
    let sc = cli.run(&["scan", "p"]);
    assert_eq!(sc.code, 0, "{}{}", sc.stdout, sc.stderr);
    // Regular files only: the symlink, the FIFO and the directories are not
    // archived.
    assert!(sc.out_has("Scanned: 4 added, 0 modified, 0 deleted, 0 unchanged."));

    let pl = cli.run(&["plan", "p"]);
    assert_eq!(pl.code, 0, "{}{}", pl.stdout, pl.stderr);
    let bu = cli.run(&["burn", "p", "--yes"]);
    assert_eq!(bu.code, 0, "{}{}", bu.stdout, bu.stderr);

    let staged = disc::data_files(&s.disc_dir(1));
    assert_eq!(
        staged,
        support::set(&[
            ".hidden",
            "notes/plain.txt",
            "photos/ünïcödé name.jpg",
            "repo/.git/objects/ab/cdef0123",
        ])
    );
    let mode = std::fs::metadata(s.disc_dir(1).join("data/repo/.git/objects/ab/cdef0123"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o444, "a read-only file keeps its mode");

    let ve = cli.run(&["verify", "p"]);
    assert_eq!(ve.code, 0, "{}{}", ve.stdout, ve.stderr);

    // The next cycle fast-paths the read-only files like any other.
    let sc2 = cli.run(&["scan", "p"]);
    assert_eq!(sc2.code, 0, "{}{}", sc2.stdout, sc2.stderr);
    assert!(sc2.out_has("Scanned: 0 added, 0 modified, 0 deleted, 4 unchanged."));
    assert!(sc2.out_has("Hashed 0 of 4 files"));
}

#[test]
fn a_file_name_that_is_not_utf8_is_refused_at_scan_with_exit_3() {
    let Some(s) = support::scratch_or_skip("s12 non-utf8 name") else {
        return;
    };
    let cli = s.cli();
    support::write(&s.source, "fine.txt", 16);
    let bad = std::ffi::OsStr::from_bytes(b"bad\xffname.txt");
    std::fs::write(s.source.join(bad), b"bytes").unwrap();

    cli.init(&s, "p");
    let sc = cli.run(&["scan", "p"]);
    assert_eq!(sc.code, 3, "{}{}", sc.stdout, sc.stderr);
    assert!(sc.err_has("not valid UTF-8"), "{}", sc.stderr);
    assert!(
        sc.err_has("bad\u{FFFD}name.txt"),
        "names the file: {}",
        sc.stderr
    );
    assert!(
        !s.work.join("staged/scan.json").exists(),
        "a refused scan stages nothing"
    );
}
