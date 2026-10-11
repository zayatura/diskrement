//! Test-only helpers shared by the burn unit tests and by
//! `tests/xorriso_image.rs`.
//!
//! Gated on `cfg(test)` *or* the `testutil` feature so both can reach it: the
//! in-crate unit tests take the first arm, the integration tests the second
//! (`diskrement-shell` is its own dev-dependency with that feature on).
//! Nothing here ships in the binary.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use diskrement_core::{ContentHash, RelPath, ScannedFile, SessionManifest};
use sha2::{Digest, Sha256};

use crate::burn::cmd::CmdOutput;
use crate::burn::{BurnError, BurnOperator, DiscIdentity, DiscImage, DiscMetadata};

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

    pub fn version(mut self, v: &str) -> Self {
        self.version = v.to_string();
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

#[cfg(test)]
mod tests {
    use super::*;

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
