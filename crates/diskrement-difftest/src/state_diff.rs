//! What a run has left behind — at the end, or between two commands — and
//! whether two runs left the same thing.
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
use crate::runner::{RunOutcome, Scratch, StepOutcome};
use crate::scenario::Expect;

#[derive(Debug, Clone)]
pub struct StepSnapshot {
    pub label: String,
    pub exit: i32,
    /// `None` for `Expect::ExitOnly` steps, where spec §3.2 lets clap and
    /// zio-cli phrase argument errors differently.
    pub stdout: Option<String>,
}

/// What a run left on disk, with every volatile value normalized.
///
/// Split out of `Snapshot` because the sweep captures it *between* commands,
/// where no `RunOutcome` exists yet.
#[derive(Debug, Clone, Default)]
pub struct StateSnapshot {
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

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub scenario: String,
    pub steps: Vec<StepSnapshot>,
    pub state: StateSnapshot,
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

/// Capture one command's outcome.
pub fn step_snapshot(step: &StepOutcome, norm: &mut Normalizer) -> StepSnapshot {
    StepSnapshot {
        label: step.label.clone(),
        exit: step.exit,
        stdout: match step.expect {
            Expect::Full => Some(norm.text(&step.stdout)),
            Expect::ExitOnly => None,
        },
    }
}

/// Capture everything currently on disk in `scratch`.
///
/// **Only call this with no child process running.** It opens SQLite with
/// `immutable=1`, which reads the main database file and ignores any `-wal`;
/// that is complete only once the writing process has exited and SQLite has
/// checkpointed. Between two command invocations, which is where the sweep
/// calls it, that always holds.
///
/// SQLite drops the `-wal` when the last connection closes, so a cleanly
/// exited child leaves a complete main file; a killed one does not. Violating
/// this is silent — you read a stale database and report a difference that is
/// not there, or miss one that is.
///
/// Takes the run's own `Normalizer` so that instant ordinals stay consistent
/// across every value the run produces.
pub fn state_snapshot(scratch: &Scratch, norm: &mut Normalizer) -> StateSnapshot {
    let catalog = dump_normalized(&scratch.catalog, CATALOG_TABLES, norm);
    let scancache = dump_normalized(&scratch.scancache, SCANCACHE_TABLES, norm);

    let mut staged = BTreeMap::new();
    let staged_dir = scratch.work.join("staged");
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
    let staging = scratch.work.join("staging");
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

    StateSnapshot {
        catalog,
        scancache,
        staged,
        disc_json,
        disc_data,
        disc_catalogs,
    }
}

/// Capture a finished run: its command outcomes and its end state.
pub fn snapshot(run: &RunOutcome, norm: &mut Normalizer) -> Snapshot {
    let steps = run.steps.iter().map(|s| step_snapshot(s, norm)).collect();
    let state = state_snapshot(&run.scratch, norm);
    Snapshot {
        scenario: run.scenario.clone(),
        steps,
        state,
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

/// Every way one step's outcome disagrees. `index` only names the step in the
/// message, and the two callers number on different bases: `differences`
/// passes a position in `RunOutcome.steps`, which holds command outcomes only,
/// while the sweep passes a position in the generated `Scenario.steps`, which
/// also counts `write`/`delete` — so the number lines up with the line the
/// counterexample prints. Neither is wrong for its own report, so the number is
/// the caller's to choose; just never compare a `step N` from one against a
/// `step N` from the other.
pub fn step_differences(index: usize, a: &StepSnapshot, b: &StepSnapshot, out: &mut Vec<String>) {
    if a.exit != b.exit {
        out.push(format!(
            "step {index} ({}) exit code: {} vs {}",
            a.label, a.exit, b.exit
        ));
    }
    if a.stdout != b.stdout {
        out.push(format!(
            "step {index} ({}) stdout:\n--- a ---\n{}\n--- b ---\n{}",
            a.label,
            a.stdout.as_deref().unwrap_or("<not compared>"),
            b.stdout.as_deref().unwrap_or("<not compared>")
        ));
    }
}

/// Every way two end states disagree, under spec §4's rules.
pub fn state_differences(a: &StateSnapshot, b: &StateSnapshot, out: &mut Vec<String>) {
    diff_lines("catalog", &a.catalog, &b.catalog, out);
    diff_lines("scancache", &a.scancache, &b.scancache, out);
    diff_maps("staged json", &a.staged, &b.staged, out);
    diff_maps("disc json", &a.disc_json, &b.disc_json, out);
    diff_maps("disc data", &a.disc_data, &b.disc_data, out);
    diff_maps("disc catalogs", &a.disc_catalogs, &b.disc_catalogs, out);
}

/// Every way two snapshots disagree. Empty means they are equivalent.
pub fn differences(a: &Snapshot, b: &Snapshot) -> Vec<String> {
    let mut out = Vec::new();
    for (i, (x, y)) in a.steps.iter().zip(b.steps.iter()).enumerate() {
        step_differences(i, x, y, &mut out);
    }
    if a.steps.len() != b.steps.len() {
        out.push(format!(
            "step counts differ: {} vs {}",
            a.steps.len(),
            b.steps.len()
        ));
    }
    state_differences(&a.state, &b.state, &mut out);
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

    fn state(lines: &[&str]) -> StateSnapshot {
        StateSnapshot {
            catalog: lines.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    fn snap(lines: &[&str]) -> Snapshot {
        Snapshot {
            scenario: "t".into(),
            steps: vec![StepSnapshot {
                label: "scan p".into(),
                exit: 0,
                stdout: Some("Scanned: 1 added".into()),
            }],
            state: state(lines),
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

    #[test]
    fn state_differences_compare_without_a_finished_run() {
        // The sweep diffs between commands, where there is no RunOutcome yet —
        // only a live scratch tree, so the state half has to stand alone. It
        // also has to *append*: the sweep pushes step diffs and then state
        // diffs into one vector, so an implementation that replaced the
        // accumulator would break it while every other test still passed.
        let mut out = vec!["pre-existing".to_string()];
        state_differences(
            &state(&["disc: id=1 status=\"OPEN\""]),
            &state(&["disc: id=1 status=\"FINALIZED\""]),
            &mut out,
        );
        assert_eq!(out.len(), 2, "must append, not replace: {out:?}");
        assert!(
            out[1].contains("catalog") && out[1].contains("OPEN") && out[1].contains("FINALIZED"),
            "{}",
            out[1]
        );
    }

    #[test]
    fn step_differences_compare_one_step_alone() {
        let a = StepSnapshot {
            label: "scan p".into(),
            exit: 0,
            stdout: Some("Scanned: 1 added".into()),
        };
        let b = StepSnapshot {
            label: "scan p".into(),
            exit: 0,
            stdout: Some("Scanned: 2 added".into()),
        };
        // Also pins append-not-replace semantics — see the state-half test above.
        let mut out = vec!["pre-existing".to_string()];
        step_differences(0, &a, &b, &mut out);
        assert_eq!(out.len(), 2, "must append, not replace: {out:?}");
        assert!(
            out[1].contains("1 added") && out[1].contains("2 added"),
            "{}",
            out[1]
        );
    }
}
