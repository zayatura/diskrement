//! Test-only helpers shared by the burn unit tests and by
//! `tests/xorriso_image.rs`.
//!
//! Gated on `cfg(test)` *or* the `testutil` feature so both can reach it: the
//! in-crate unit tests take the first arm, the integration tests the second
//! (`diskrement-shell` is its own dev-dependency with that feature on).
//! Nothing here ships in the binary.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use diskrement_core::{ContentHash, RelPath, ScannedFile, SessionManifest};
use sha2::{Digest, Sha256};

use crate::burn::cmd::CmdOutput;
use crate::burn::drive::{DriveState, DriveWatch};
use crate::burn::{BurnError, BurnOperator, DiscIdentity, DiscImage, DiscMetadata};
use crate::progress::ProgressSink;

/// A `BurnOperator` with canned answers and a recorded transcript
/// (`ScriptedOperator` in `XorrisoBurnerSpec.scala`). Running out of answers
/// means "no", so a re-prompt loop always terminates.
pub struct ScriptedOperator {
    answers: RefCell<std::collections::VecDeque<bool>>,
    log: RefCell<Vec<String>>,
}

impl ScriptedOperator {
    pub fn new(answers: &[bool]) -> Self {
        ScriptedOperator {
            answers: RefCell::new(answers.iter().copied().collect()),
            log: RefCell::new(Vec::new()),
        }
    }

    /// Every prompt and progress line, in order.
    pub fn log(&self) -> Vec<String> {
        self.log.borrow().clone()
    }

    /// Whether any recorded line contains `needle` — the usual assertion.
    pub fn said(&self, needle: &str) -> bool {
        self.log.borrow().iter().any(|l| l.contains(needle))
    }
}

impl BurnOperator for ScriptedOperator {
    fn confirm_insert(&self, message: &str) -> bool {
        self.log.borrow_mut().push(message.to_string());
        self.answers.borrow_mut().pop_front().unwrap_or(false)
    }

    fn progress(&self, line: &str) {
        self.log.borrow_mut().push(line.to_string());
    }
}

/// A `ProgressSink` that records every call as a string, in order:
/// `start label bytes files`, `start_bytes label bytes`, `advance bytes files`,
/// `done`.
#[derive(Default)]
pub struct RecordingSink(std::sync::Mutex<Vec<String>>);

impl RecordingSink {
    pub fn events(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }

    /// The bytes of every `advance`, summed.
    pub fn advanced_bytes(&self) -> u64 {
        self.events()
            .iter()
            .filter_map(|e| e.strip_prefix("advance "))
            .map(|rest| rest.split(' ').next().unwrap().parse::<u64>().unwrap())
            .sum()
    }

    /// The files of every `advance`, summed.
    pub fn advanced_files(&self) -> usize {
        self.events()
            .iter()
            .filter_map(|e| e.strip_prefix("advance "))
            .map(|rest| rest.split(' ').nth(1).unwrap().parse::<usize>().unwrap())
            .sum()
    }

    fn push(&self, event: String) {
        self.0.lock().unwrap().push(event);
    }
}

impl ProgressSink for RecordingSink {
    fn start(&self, label: &str, total_bytes: u64, total_files: usize) {
        self.push(format!("start {label} {total_bytes} {total_files}"));
    }
    fn start_bytes(&self, label: &str, total_bytes: u64) {
        self.push(format!("start_bytes {label} {total_bytes}"));
    }
    fn advance(&self, delta_bytes: u64, delta_files: usize) {
        self.push(format!("advance {delta_bytes} {delta_files}"));
    }
    fn done(&self) {
        self.push("done".to_string());
    }
}

/// Is real `xorriso` on this machine? Tests that need it print a reason and
/// return when it is absent (spec §7) rather than failing or, worse, passing
/// silently.
pub fn has_xorriso() -> bool {
    std::process::Command::new("xorriso")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// `-toc` output for a blank disc with room to spare.
pub const BLANK: &[&str] = &["Media status : is blank", "Media space  : 500000s"];

/// `-toc` output for a written disc that can take another session.
pub const APPENDABLE: &[&str] = &[
    "Media status : is written , is appendable",
    "Media summary: 1 session, 57 data blocks, 114k data, 23g free",
    "Media space  : 500000s",
];

/// A disc's `/data` tree as `(path, body)` pairs, for `ScriptedExec::extractions`.
pub type Tree<'a> = &'a [(&'a str, &'a [u8])];

/// A `Tree` the script owns.
type OwnedTree = Vec<(String, Vec<u8>)>;

/// A scripted `Exec` standing in for xorriso.
///
/// `toc` and `identities` are queues consumed per matching invocation,
/// which is how one script expresses "a wrong disc, then the right one"
/// and "the medium says session 1, the burned disc says session 2".
/// Callers wrap it as `|c, l| script.respond(c, l)`.
pub struct ScriptedExec {
    version: String,
    toc: RefCell<VecDeque<Vec<String>>>,
    identities: RefCell<VecDeque<Option<&'static str>>>,
    burn_exit: i32,
    burn_lines: Vec<String>,
    calls: RefCell<Vec<String>>,
    mounts: RefCell<VecDeque<Option<PathBuf>>>,
    trees: RefCell<VecDeque<Option<OwnedTree>>>,
}

impl Default for ScriptedExec {
    fn default() -> Self {
        Self::new()
    }
}

impl ScriptedExec {
    pub fn new() -> Self {
        ScriptedExec {
            version: "xorriso 1.5.8.pl02".to_string(),
            toc: RefCell::new(VecDeque::new()),
            identities: RefCell::new(VecDeque::new()),
            burn_exit: 0,
            burn_lines: vec!["xorriso : UPDATE : 100% done".to_string()],
            calls: RefCell::new(Vec::new()),
            mounts: RefCell::new(VecDeque::new()),
            trees: RefCell::new(VecDeque::new()),
        }
    }

    pub fn toc(mut self, states: &[&[&str]]) -> Self {
        self.toc = RefCell::new(
            states
                .iter()
                .map(|s| s.iter().map(|l| (*l).to_string()).collect())
                .collect(),
        );
        self
    }

    pub fn identities(mut self, ids: &[Option<&'static str>]) -> Self {
        self.identities = RefCell::new(ids.iter().copied().collect());
        self
    }

    /// What each `udisksctl mount` answers, in order: `Some(dir)` mounts the
    /// disc at `dir`, `None` fails as udisks does when it cannot mount.
    pub fn mounts(mut self, points: &[Option<&Path>]) -> Self {
        self.mounts = RefCell::new(points.iter().map(|p| p.map(Path::to_path_buf)).collect());
        self
    }

    /// What each `-extract /data` answers, in order: `Some(files)` writes
    /// that tree (with a restore pacifier line, as xorriso prints one),
    /// `None` fails the way xorriso does. Other extractions are identities.
    pub fn extractions(mut self, trees: &[Option<Tree<'_>>]) -> Self {
        self.trees = RefCell::new(
            trees
                .iter()
                .map(|t| {
                    t.map(|files| {
                        files
                            .iter()
                            .map(|(rel, body)| ((*rel).to_string(), body.to_vec()))
                            .collect()
                    })
                })
                .collect(),
        );
        self
    }

    pub fn version(mut self, v: &str) -> Self {
        self.version = v.to_string();
        self
    }

    /// What the burn command prints, in order, on a successful burn.
    pub fn burn_lines(mut self, lines: &[&str]) -> Self {
        self.burn_lines = lines.iter().map(|l| (*l).to_string()).collect();
        self
    }

    pub fn burn_fails(mut self) -> Self {
        self.burn_exit = 5;
        self.burn_lines = vec!["FAILURE : write error".to_string()];
        self
    }

    pub fn ran(&self, needle: &str) -> bool {
        self.calls.borrow().iter().any(|c| c.contains(needle))
    }

    pub fn respond(
        &self,
        cmd: &[String],
        on_line: &mut dyn FnMut(&str),
    ) -> Result<CmdOutput, BurnError> {
        self.calls.borrow_mut().push(cmd.join(" "));
        let has = |flag: &str| cmd.iter().any(|a| a == flag);
        if cmd.first().is_some_and(|c| c == "udisksctl") {
            if cmd.get(1).is_some_and(|a| a == "mount") {
                let device = cmd.last().expect("a device");
                let point = self
                    .mounts
                    .borrow_mut()
                    .pop_front()
                    .expect("the script ran out of mounts");
                return Ok(match point {
                    Some(dir) => CmdOutput {
                        exit_code: 0,
                        lines: vec![format!("Mounted {device} at {}.", dir.display())],
                    },
                    None => CmdOutput {
                        exit_code: 1,
                        lines: vec![format!("Error mounting {device}: wrong fs type")],
                    },
                });
            }
            // unmount
            return Ok(CmdOutput {
                exit_code: 0,
                lines: Vec::new(),
            });
        }
        if has("--version") {
            return Ok(CmdOutput {
                exit_code: 0,
                lines: vec![self.version.clone()],
            });
        }
        if has("-toc") {
            let lines = self
                .toc
                .borrow_mut()
                .pop_front()
                .expect("the script ran out of media states");
            return Ok(CmdOutput {
                exit_code: 0,
                lines,
            });
        }
        if has("-extract") && cmd.len() >= 2 && cmd[cmd.len() - 2] == "/data" {
            let dest = Path::new(cmd.last().expect("a destination"));
            let tree = self
                .trees
                .borrow_mut()
                .pop_front()
                .expect("the script ran out of /data extractions");
            return Ok(match tree {
                Some(files) => {
                    std::fs::create_dir_all(dest).expect("writable destination");
                    let mut bytes = 0;
                    for (rel, body) in &files {
                        let path = dest.join(rel);
                        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
                        std::fs::write(&path, body).expect("writable destination");
                        bytes += body.len();
                    }
                    on_line(&format!(
                        "xorriso : UPDATE : {} files restored ( {bytes}) in 1 seconds",
                        files.len()
                    ));
                    CmdOutput {
                        exit_code: 0,
                        lines: Vec::new(),
                    }
                }
                None => CmdOutput {
                    exit_code: 5,
                    lines: vec!["FAILURE : cannot read the medium".to_string()],
                },
            });
        }
        if has("-extract") {
            return match self.identities.borrow_mut().pop_front().flatten() {
                Some(json) => {
                    std::fs::write(cmd.last().expect("a destination"), json)
                        .expect("writable destination");
                    Ok(CmdOutput {
                        exit_code: 0,
                        lines: Vec::new(),
                    })
                }
                None => Ok(CmdOutput {
                    exit_code: 5,
                    lines: vec!["FAILURE : cannot find path".to_string()],
                }),
            };
        }
        if has("-commit") {
            for line in &self.burn_lines {
                on_line(line);
            }
            return Ok(CmdOutput {
                exit_code: self.burn_exit,
                lines: self.burn_lines.clone(),
            });
        }
        // -eject and anything else
        Ok(CmdOutput {
            exit_code: 0,
            lines: Vec::new(),
        })
    }
}

pub fn sha256(body: &str) -> ContentHash {
    let mut h = Sha256::new();
    h.update(body.as_bytes());
    ContentHash(format!("{:x}", h.finalize()))
}

/// The fixed instant every burn unit test uses, matching the Scala specs'
/// `Instant.parse("2026-08-30T10:00:00Z")`.
pub fn ts() -> DateTime<Utc> {
    crate::catalog::time::parse("2026-08-30T10:00:00Z").expect("a valid instant")
}

pub fn identity(session_ordinal: u32) -> DiscIdentity {
    DiscIdentity {
        project_name: "photos".to_string(),
        disc_ordinal: 1,
        copy_set: 0,
        copies: 1,
        session_ordinal,
        created_at: ts(),
    }
}

/// A snapshot directory holding `files`, plus the `DiscImage` that stages it.
///
/// The catalog snapshot is a stub file: the stager copies it verbatim into
/// `/.diskrement/catalog.sqlite` and never opens it, so a real database would
/// only slow the test down.
pub fn image_of(
    base: &Path,
    session_ordinal: u32,
    files: &[(&str, &str)],
    close_disc: bool,
) -> DiscImage {
    let snapshot = base.join(format!("snapshot-{session_ordinal}"));
    let mut scanned = Vec::new();
    for (rel, content) in files {
        let path = snapshot.join(rel);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(&path, content).expect("write");
        scanned.push(ScannedFile {
            rel_path: RelPath((*rel).to_string()),
            content_hash: sha256(content),
            size_bytes: content.len() as u64,
            mtime: ts(),
        });
    }
    // The planner sorts by rel path, and the stager's progress accounting
    // assumes that order; keep it here so a multi-file image is deterministic.
    scanned.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));

    let catalog_snapshot: PathBuf = base.join(format!("catalog-{session_ordinal}.sqlite"));
    std::fs::write(&catalog_snapshot, format!("stub-catalog-{session_ordinal}"))
        .expect("write the stub catalog");

    DiscImage {
        staging_dir: base.join(format!("staging-{session_ordinal}")),
        snapshot_root: snapshot,
        files: scanned,
        metadata: DiscMetadata {
            identity: identity(session_ordinal),
            manifest: SessionManifest {
                project_name: "photos".to_string(),
                disc_ordinal: 1,
                copy_set: 0,
                session_ordinal,
                added: Vec::new(),
                modified: Vec::new(),
                copied: Vec::new(),
                deleted: Vec::new(),
            },
            catalog_snapshot,
            pointers: Vec::new(),
        },
        close_disc,
    }
}

/// `-toc` output for a drive xorriso finds empty.
pub const NOT_PRESENT: &[&str] = &["Media status : is not present"];

/// A `DriveWatch` answering from a queue. Running out panics, so a wait loop
/// that polls more than the test expects fails instead of hanging; `failing`
/// answers every poll with `DriveStatusUnavailable`.
pub struct ScriptedWatch {
    states: RefCell<VecDeque<DriveState>>,
    fail: bool,
    pauses: Cell<usize>,
}

impl ScriptedWatch {
    pub fn new(states: &[DriveState]) -> Self {
        ScriptedWatch {
            states: RefCell::new(states.iter().copied().collect()),
            fail: false,
            pauses: Cell::new(0),
        }
    }

    pub fn failing() -> Self {
        ScriptedWatch {
            fail: true,
            ..ScriptedWatch::new(&[])
        }
    }

    pub fn pauses(&self) -> usize {
        self.pauses.get()
    }
}

impl DriveWatch for ScriptedWatch {
    fn state(&self) -> Result<DriveState, BurnError> {
        if self.fail {
            return Err(BurnError::DriveStatusUnavailable {
                device: "/dev/sr0".to_string(),
                reason: "Inappropriate ioctl for device".to_string(),
            });
        }
        Ok(self
            .states
            .borrow_mut()
            .pop_front()
            .expect("the script ran out of drive states"))
    }

    fn pause(&self) {
        self.pauses.set(self.pauses.get() + 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mounts answer in order: `None` fails the way udisks does (no mount
    /// point to parse), `Some(dir)` mounts there. Unmounts always succeed.
    #[test]
    fn scripted_mounts_answer_in_order() {
        let dir = tempfile::TempDir::new().unwrap();
        let script = ScriptedExec::new().mounts(&[None, Some(dir.path())]);
        let mount: Vec<String> = ["udisksctl", "mount", "-b", "/dev/sr0"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let first = script.respond(&mount, &mut |_| {}).unwrap();
        assert_eq!(crate::burn::reader::parse_mount_point(&first.lines), None);
        let second = script.respond(&mount, &mut |_| {}).unwrap();
        assert_eq!(
            crate::burn::reader::parse_mount_point(&second.lines),
            Some(dir.path().to_path_buf())
        );
        assert!(script.ran("udisksctl mount -b /dev/sr0"));
    }

    /// Answers are consumed in order and exhaustion means "no", so a scripted
    /// run can never loop forever on a re-prompt.
    #[test]
    fn the_scripted_operator_answers_in_order_then_refuses() {
        let op = ScriptedOperator::new(&[true, false]);
        assert!(op.confirm_insert("first?"));
        assert!(!op.confirm_insert("second?"));
        assert!(!op.confirm_insert("beyond the script"));
        op.progress("a note");
        assert_eq!(
            op.log(),
            vec![
                "first?".to_string(),
                "second?".to_string(),
                "beyond the script".to_string(),
                "a note".to_string(),
            ]
        );
    }
}
