//! What a finished run left behind, and whether two runs left the same thing.
//!
//! Comparison rules are spec §4's, unchanged and deliberately not re-derived:
//! catalog and scancache **logically** (row dumps, never bytes, because SQLite
//! file layout is not stable across library builds); staged plans, manifests and
//! pointers **semantically** (parsed-value equality, because field order is not
//! an oracle); `data/` payloads **byte-for-byte**, since those bytes are the
//! whole point of the tool.

use std::collections::BTreeMap;
use std::path::Path;

use diskrement_shell::testutil::{dump_tables, CATALOG_TABLES, SCANCACHE_TABLES};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::normalize::Normalizer;
use crate::runner::RunOutcome;
use crate::scenario::Expect;

#[derive(Debug, Clone)]
pub struct StepSnapshot {
    pub label: String,
    pub exit: i32,
    /// `None` for `Expect::ExitOnly` steps, where spec §3.2 lets clap and
    /// zio-cli phrase argument errors differently.
    pub stdout: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub scenario: String,
    pub steps: Vec<StepSnapshot>,
    pub catalog: Vec<String>,
    pub scancache: Vec<String>,
    /// `staged/*.json` in the work dir, by filename.
    pub staged: BTreeMap<String, Value>,
    /// `.diskrement/*.json` from every staged disc, keyed `disc-N/<file>`.
    pub disc_json: BTreeMap<String, Value>,
    /// `data/` payloads from every staged disc, keyed `disc-N/<relpath>`, as
    /// sha256 hex — the bytes must match exactly, but printing them on failure
    /// would be useless.
    pub disc_data: BTreeMap<String, String>,
    /// Each staged disc's embedded `catalog.sqlite`, dumped logically and
    /// keyed `disc-N`.
    ///
    /// It is a *copy* of the catalog compared above, so a drift shows up
    /// twice — deliberately: the embedded snapshot is written by a different
    /// path (`snapshot_to` + `apply_update_to` on a work copy), and comparing
    /// it separately says which of the two paths drifted.
    pub disc_catalogs: BTreeMap<String, Vec<String>>,
}

/// Open a database that the run just wrote, read-only and without disturbing it.
///
/// `immutable=1` is what the corpus tests use to avoid creating `-wal`/`-shm`
/// files. Here the file is in a temp dir that is about to be deleted, so the
/// motive is different — a read must not change what the *other* comparison
/// steps will see — but the mechanism is the same one.
fn open_ro(path: &Path) -> Option<Connection> {
    if !path.is_file() {
        return None;
    }
    let uri = format!("file:{}?immutable=1", path.display());
    Connection::open_with_flags(
        &uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .ok()
}

fn dump_normalized(path: &Path, tables: &[&str], norm: &mut Normalizer) -> Vec<String> {
    let Some(conn) = open_ro(path) else {
        return Vec::new();
    };
    let mut lines: Vec<String> = dump_tables(&conn, tables)
        .iter()
        .map(|l| norm.text(l))
        .collect();
    // Re-sort *after* normalizing: normalization changes a line's sort key, and
    // two runs whose rows differ only in a timestamp would otherwise sort
    // differently and diff as a false positive.
    lines.sort();
    lines
}

fn read_json(path: &Path, norm: &mut Normalizer) -> Option<Value> {
    let raw = std::fs::read_to_string(path).ok()?;
    let parsed: Value = serde_json::from_str(&raw).ok()?;
    Some(norm.json(&parsed))
}

fn walk_files(root: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// Capture everything a finished run left on disk.
///
/// Takes the run's own `Normalizer`, already advanced by the command stdout, so
/// that instant ordinals are consistent across the whole snapshot.
pub fn snapshot(run: &RunOutcome, norm: &mut Normalizer) -> Snapshot {
    let steps = run
        .steps
        .iter()
        .map(|s| StepSnapshot {
            label: s.label.clone(),
            exit: s.exit,
            stdout: match s.expect {
                Expect::Full => Some(norm.text(&s.stdout)),
                Expect::ExitOnly => None,
            },
        })
        .collect();

    let catalog = dump_normalized(&run.scratch.catalog, CATALOG_TABLES, norm);
    let scancache = dump_normalized(&run.scratch.scancache, SCANCACHE_TABLES, norm);

    let mut staged = BTreeMap::new();
    let staged_dir = run.scratch.work.join("staged");
    for path in walk_files(&staged_dir) {
        if path.extension().is_some_and(|e| e == "json") {
            let name = path
                .strip_prefix(&staged_dir)
                .expect("walked from staged_dir")
                .display()
                .to_string();
            if let Some(v) = read_json(&path, norm) {
                staged.insert(name, v);
            }
        }
    }

    let mut disc_json = BTreeMap::new();
    let mut disc_data = BTreeMap::new();
    let mut disc_catalogs = BTreeMap::new();
    let staging = run.scratch.work.join("staging");
    let mut discs: Vec<std::path::PathBuf> = std::fs::read_dir(&staging)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default();
    discs.sort();
    for disc in discs {
        let disc_name = disc
            .file_name()
            .expect("a directory has a name")
            .to_string_lossy()
            .into_owned();
        let meta = disc.join(".diskrement");
        for path in walk_files(&meta) {
            if path.extension().is_some_and(|e| e == "json") {
                let name = path
                    .file_name()
                    .expect("a file has a name")
                    .to_string_lossy();
                if let Some(v) = read_json(&path, norm) {
                    disc_json.insert(format!("{disc_name}/{name}"), v);
                }
            }
        }
        // The embedded catalog.sqlite is a *copy* of the catalog compared
        // above, so a drift shows up twice — deliberately: it is written by a
        // different path (`snapshot_to` + `apply_update_to` on a work copy),
        // and comparing it separately says which of the two paths drifted.
        let embedded = meta.join("catalog.sqlite");
        if embedded.is_file() {
            disc_catalogs.insert(
                disc_name.clone(),
                dump_normalized(&embedded, CATALOG_TABLES, norm),
            );
        }
        let data = disc.join("data");
        for path in walk_files(&data) {
            let rel = path
                .strip_prefix(&data)
                .expect("walked from data")
                .display()
                .to_string();
            let bytes = std::fs::read(&path).unwrap_or_default();
            let hex = format!("{:x}", Sha256::digest(&bytes));
            disc_data.insert(format!("{disc_name}/{rel}"), hex);
        }
    }

    Snapshot {
        scenario: run.scenario.clone(),
        steps,
        catalog,
        scancache,
        staged,
        disc_json,
        disc_data,
        disc_catalogs,
    }
}

fn diff_lines(what: &str, a: &[String], b: &[String], out: &mut Vec<String>) {
    for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
        if x != y {
            out.push(format!("{what} diverges at line {i}:\n  a: {x}\n  b: {y}"));
            return;
        }
    }
    if a.len() != b.len() {
        out.push(format!(
            "{what} row counts differ: {} vs {}",
            a.len(),
            b.len()
        ));
    }
}

fn diff_maps<T: PartialEq + std::fmt::Debug>(
    what: &str,
    a: &BTreeMap<String, T>,
    b: &BTreeMap<String, T>,
    out: &mut Vec<String>,
) {
    for key in a
        .keys()
        .chain(b.keys())
        .collect::<std::collections::BTreeSet<_>>()
    {
        match (a.get(key), b.get(key)) {
            (Some(x), Some(y)) if x != y => {
                out.push(format!("{what} '{key}' differs:\n  a: {x:?}\n  b: {y:?}"))
            }
            (None, Some(_)) => out.push(format!("{what} '{key}' exists only in run b")),
            (Some(_), None) => out.push(format!("{what} '{key}' exists only in run a")),
            _ => {}
        }
    }
}

/// Every way two snapshots disagree. Empty means they are equivalent.
pub fn differences(a: &Snapshot, b: &Snapshot) -> Vec<String> {
    let mut out = Vec::new();
    for (i, (x, y)) in a.steps.iter().zip(b.steps.iter()).enumerate() {
        if x.exit != y.exit {
            out.push(format!(
                "step {i} ({}) exit code: {} vs {}",
                x.label, x.exit, y.exit
            ));
        }
        if x.stdout != y.stdout {
            out.push(format!(
                "step {i} ({}) stdout:\n--- a ---\n{}\n--- b ---\n{}",
                x.label,
                x.stdout.as_deref().unwrap_or("<not compared>"),
                y.stdout.as_deref().unwrap_or("<not compared>")
            ));
        }
    }
    if a.steps.len() != b.steps.len() {
        out.push(format!(
            "step counts differ: {} vs {}",
            a.steps.len(),
            b.steps.len()
        ));
    }
    diff_lines("catalog", &a.catalog, &b.catalog, &mut out);
    diff_lines("scancache", &a.scancache, &b.scancache, &mut out);
    diff_maps("staged json", &a.staged, &b.staged, &mut out);
    diff_maps("disc json", &a.disc_json, &b.disc_json, &mut out);
    diff_maps("disc data", &a.disc_data, &b.disc_data, &mut out);
    diff_maps(
        "disc catalogs",
        &a.disc_catalogs,
        &b.disc_catalogs,
        &mut out,
    );
    out
}

/// Assert two snapshots agree, printing every difference at once.
pub fn assert_equivalent(a: &Snapshot, b: &Snapshot, context: &str) {
    let diffs = differences(a, b);
    assert!(
        diffs.is_empty(),
        "{context}: {} difference(s)\n\n{}",
        diffs.len(),
        diffs.join("\n\n")
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(lines: &[&str]) -> Snapshot {
        Snapshot {
            scenario: "t".into(),
            steps: vec![StepSnapshot {
                label: "scan p".into(),
                exit: 0,
                stdout: Some("Scanned: 1 added".into()),
            }],
            catalog: lines.iter().map(|s| s.to_string()).collect(),
            scancache: Vec::new(),
            staged: Default::default(),
            disc_json: Default::default(),
            disc_data: Default::default(),
            disc_catalogs: Default::default(),
        }
    }

    #[test]
    fn identical_snapshots_compare_equal() {
        let a = snap(&["disc: id=1 status=\"OPEN\""]);
        let b = snap(&["disc: id=1 status=\"OPEN\""]);
        assert_eq!(differences(&a, &b), Vec::<String>::new());
    }

    #[test]
    fn a_row_difference_is_reported_with_both_sides() {
        let a = snap(&["disc: id=1 status=\"OPEN\""]);
        let b = snap(&["disc: id=1 status=\"FINALIZED\""]);
        let diffs = differences(&a, &b);
        assert_eq!(diffs.len(), 1);
        assert!(diffs[0].contains("catalog"), "{}", diffs[0]);
        assert!(diffs[0].contains("OPEN"), "{}", diffs[0]);
        assert!(diffs[0].contains("FINALIZED"), "{}", diffs[0]);
    }

    #[test]
    fn a_row_count_difference_is_reported() {
        let a = snap(&["disc: id=1 status=\"OPEN\""]);
        let b = snap(&["disc: id=1 status=\"OPEN\"", "disc: id=2 status=\"OPEN\""]);
        assert_eq!(differences(&a, &b).len(), 1);
        assert!(differences(&a, &b)[0].contains("1 vs 2"));
    }

    #[test]
    fn an_exit_only_step_ignores_stdout_but_not_the_code() {
        let mut a = snap(&[]);
        let mut b = snap(&[]);
        a.steps[0].stdout = None;
        b.steps[0].stdout = None;
        assert!(differences(&a, &b).is_empty());
        b.steps[0].exit = 2;
        assert_eq!(differences(&a, &b).len(), 1);
    }
}
