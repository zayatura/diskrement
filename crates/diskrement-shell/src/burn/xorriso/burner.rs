//! The real burner (`XorrisoBurner.scala`).
//!
//! Prompt for the disc, inspect it (strict identity check), stage with
//! re-hash, check capacity, burn, sanity-check the result, eject. Any failure
//! leaves the catalog uncommitted, because `Pipeline::burn` commits only after
//! this returns.

use crate::burn::cmd::Exec;
use crate::burn::media::{MediaInspector, MediaStatus};
use crate::burn::xorriso::{args, parse};
use crate::burn::{
    stager, BurnDevice, BurnError, BurnOperator, BurnResult, DiscIdentity, DiscImage,
};
use crate::progress::ProgressSink;

/// Filesystem metadata, session gap and the ISO's own structures. The Scala
/// constant, kept exactly: capacity refusals must trigger in the same places.
const FIXED_OVERHEAD_BYTES: u64 = 32 * 1024 * 1024;

pub struct XorrisoBurner<'a> {
    device: BurnDevice,
    exec: &'a dyn Exec,
    operator: &'a dyn BurnOperator,
    progress: &'a dyn ProgressSink,
    parallelism: usize,
}

impl<'a> XorrisoBurner<'a> {
    pub fn new(
        device: BurnDevice,
        exec: &'a dyn Exec,
        operator: &'a dyn BurnOperator,
        progress: &'a dyn ProgressSink,
        parallelism: usize,
    ) -> Self {
        XorrisoBurner {
            device,
            exec,
            operator,
            progress,
            parallelism,
        }
    }

    pub fn burn(&self, image: &DiscImage) -> Result<BurnResult, BurnError> {
        self.preflight()?;
        let free_bytes = self.await_correct_media(image)?;
        let staged = stager::stage(image, self.progress, self.parallelism)?;
        check_capacity(staged.image_bytes, free_bytes)?;
        self.write(image)?;
        self.post_burn_check(&image.metadata.identity)?;
        self.eject();
        Ok(staged)
    }

    /// xorriso 1.5 is the floor: `-compliance iso_9660_level=3` and the
    /// osirrox flags this code depends on are not all present before it.
    fn preflight(&self) -> Result<(), BurnError> {
        let out = self.exec.run_quiet(&args::version())?;
        match parse::version(&out.lines) {
            None => Err(BurnError::ToolMissing(
                "cannot parse `xorriso --version` output".to_string(),
            )),
            Some((major, minor, raw)) => {
                if major > 1 || (major == 1 && minor >= 5) {
                    Ok(())
                } else {
                    Err(BurnError::ToolTooOld(raw))
                }
            }
        }
    }

    /// Prompt-and-inspect until the right disc is in the drive; return its free
    /// space. A decline is `Aborted`; the wrong disc re-prompts forever, which
    /// is what an operator swapping discs actually wants.
    fn await_correct_media(&self, image: &DiscImage) -> Result<u64, BurnError> {
        let id = &image.metadata.identity;
        let expected = if id.session_ordinal == 1 {
            format!(
                "a blank disc for '{}' disc {}",
                id.project_name, id.disc_ordinal
            )
        } else {
            format!(
                "disc {} of '{}' (last burned session {})",
                id.disc_ordinal,
                id.project_name,
                id.session_ordinal - 1
            )
        };
        let seal_note = if image.close_disc {
            " This burn will FINALIZE the disc."
        } else {
            ""
        };
        let prompt = format!(
            "Insert {expected} into {} and confirm.{seal_note}",
            self.device.xorriso_address()
        );

        loop {
            if !self.operator.confirm_insert(&prompt) {
                return Err(BurnError::Aborted);
            }
            match self.check_media(id)? {
                Ok(free_bytes) => return Ok(free_bytes),
                Err(found) => self.operator.progress(&format!(
                    "Not the expected disc: found {found}. Swap discs and confirm again."
                )),
            }
        }
    }

    /// `Ok(free)` when this is the disc to write; `Err(description)` when it is
    /// not, worded for the operator.
    fn check_media(&self, id: &DiscIdentity) -> Result<Result<u64, String>, BurnError> {
        let status = MediaInspector::new(self.device.clone(), self.exec).inspect()?;
        Ok(match (status, id.session_ordinal) {
            (MediaStatus::NoMedia, _) => Err("no disc in the drive".to_string()),
            (MediaStatus::Blank(free), 1) => Ok(free),
            (MediaStatus::Blank(_), _) => {
                Err("a blank disc (expected written media to append to)".to_string())
            }
            (MediaStatus::Appendable { .. }, 1) => {
                Err("written media (expected a blank disc)".to_string())
            }
            (
                MediaStatus::Appendable {
                    free_bytes,
                    identity: Some(ref found),
                    ..
                },
                n,
            ) if found.project_name == id.project_name
                && found.disc_ordinal == id.disc_ordinal
                && found.session_ordinal + 1 == n =>
            {
                Ok(free_bytes)
            }
            (
                MediaStatus::Appendable {
                    identity: Some(found),
                    ..
                },
                _,
            ) => Err(format!(
                "disc '{}' #{} at session {}",
                found.project_name, found.disc_ordinal, found.session_ordinal
            )),
            (MediaStatus::Appendable { identity: None, .. }, _) => {
                Err("a written non-diskrement disc".to_string())
            }
            (MediaStatus::Closed(_), _) => Err("a finalized (closed) disc".to_string()),
            (MediaStatus::Unsuitable, _) => Err(
                "unsuitable media (e.g. a POW-formatted BD-R) — use a plain unformatted BD-R"
                    .to_string(),
            ),
        })
    }

    fn write(&self, image: &DiscImage) -> Result<(), BurnError> {
        let id = &image.metadata.identity;
        let volid = args::volid(&id.project_name, id.disc_ordinal);
        let command = args::burn(
            &self.device,
            &image.staging_dir,
            id.session_ordinal == 1,
            image.close_disc,
            &volid,
        );
        let result = self.exec.run(&command, &mut |line| {
            // xorriso's percentage lines; everything else is noise the
            // operator does not need mid-burn.
            if line.contains("UPDATE") {
                self.operator.progress(line);
            }
        })?;
        if result.exit_code != 0 {
            return Err(BurnError::BurnFailed {
                exit_code: result.exit_code,
                output_tail: result.tail(10),
            });
        }
        Ok(())
    }

    fn post_burn_check(&self, id: &DiscIdentity) -> Result<(), BurnError> {
        let found = MediaInspector::new(self.device.clone(), self.exec).read_identity()?;
        let matches = found.as_ref().is_some_and(|f| {
            f.project_name == id.project_name
                && f.disc_ordinal == id.disc_ordinal
                && f.session_ordinal == id.session_ordinal
        });
        if matches {
            return Ok(());
        }
        Err(BurnError::PostBurnCheckFailed(format!(
            "expected identity ({}, disc {}, session {}) readable from the disc, found: {}",
            id.project_name,
            id.disc_ordinal,
            id.session_ordinal,
            match &found {
                Some(f) => format!("Some({f})"),
                None => "None".to_string(),
            }
        )))
    }

    /// Best-effort: the burn already succeeded, and an eject hiccup must not
    /// turn a written disc into a failed run.
    fn eject(&self) {
        if let BurnDevice::ImageFile(_) = self.device {
            return;
        }
        let ok = self
            .exec
            .run_quiet(&args::eject(&self.device))
            .map(|r| r.exit_code == 0)
            .unwrap_or(false);
        if !ok {
            self.operator.progress("note: eject failed");
        }
    }
}

/// One percent of slack plus the fixed overhead — the Scala arithmetic,
/// unchanged.
fn check_capacity(staged_bytes: u64, free_bytes: u64) -> Result<(), BurnError> {
    let needed = staged_bytes + staged_bytes / 100 + FIXED_OVERHEAD_BYTES;
    if needed <= free_bytes {
        Ok(())
    } else {
        Err(BurnError::InsufficientCapacity {
            needed_bytes: needed,
            free_bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    use crate::burn::cmd::CmdOutput;
    use crate::burn::testkit::{image_of, ScriptedOperator};
    use crate::progress::NoopSink;

    const IDENTITY_1: &str = r#"{"projectName":"photos","discOrdinal":1,"sessionOrdinal":1,"createdAt":"2026-08-30T10:00:00Z"}"#;
    const IDENTITY_2: &str = r#"{"projectName":"photos","discOrdinal":1,"sessionOrdinal":2,"createdAt":"2026-08-30T10:00:00Z"}"#;
    const IDENTITY_7: &str = r#"{"projectName":"photos","discOrdinal":1,"sessionOrdinal":7,"createdAt":"2026-08-30T10:00:00Z"}"#;

    const BLANK: &[&str] = &["Media status : is blank", "Media space  : 500000s"];
    const APPENDABLE: &[&str] = &[
        "Media status : is written , is appendable",
        "Media summary: 1 session, 57 data blocks, 114k data, 23g free",
        "Media space  : 500000s",
    ];

    /// A scripted `Exec`.
    ///
    /// `toc` and `identities` are queues consumed per matching invocation,
    /// which is how one script expresses "a wrong disc, then the right one"
    /// and "the medium says session 1, the burned disc says session 2".
    struct Script {
        version: String,
        toc: RefCell<VecDeque<Vec<String>>>,
        identities: RefCell<VecDeque<Option<&'static str>>>,
        burn_exit: i32,
        burn_lines: Vec<String>,
        calls: RefCell<Vec<String>>,
    }

    impl Script {
        fn new() -> Self {
            Script {
                version: "xorriso 1.5.8.pl02".to_string(),
                toc: RefCell::new(VecDeque::new()),
                identities: RefCell::new(VecDeque::new()),
                burn_exit: 0,
                burn_lines: vec!["xorriso : UPDATE : 100% done".to_string()],
                calls: RefCell::new(Vec::new()),
            }
        }

        fn toc(mut self, states: &[&[&str]]) -> Self {
            self.toc = RefCell::new(
                states
                    .iter()
                    .map(|s| s.iter().map(|l| (*l).to_string()).collect())
                    .collect(),
            );
            self
        }

        fn identities(mut self, ids: &[Option<&'static str>]) -> Self {
            self.identities = RefCell::new(ids.iter().copied().collect());
            self
        }

        fn version(mut self, v: &str) -> Self {
            self.version = v.to_string();
            self
        }

        fn burn_fails(mut self) -> Self {
            self.burn_exit = 5;
            self.burn_lines = vec!["FAILURE : write error".to_string()];
            self
        }

        fn ran(&self, needle: &str) -> bool {
            self.calls.borrow().iter().any(|c| c.contains(needle))
        }

        fn respond(
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

    #[test]
    fn session_one_on_blank_media_burns_and_post_checks() {
        let dir = tempfile::TempDir::new().unwrap();
        let script = Script::new().toc(&[BLANK]).identities(&[Some(IDENTITY_1)]);
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
        let op = ScriptedOperator::new(&[true]);
        let burner = XorrisoBurner::new(
            BurnDevice::RealDevice("/dev/sr0".to_string()),
            &exec,
            &op,
            &NoopSink,
            2,
        );

        let result = burner
            .burn(&image_of(dir.path(), 1, &[("a.txt", "hello")], false))
            .expect("the burn succeeds");
        assert!(result.image_bytes > 0);
        assert!(script.ran("-outdev"), "a first session writes to -outdev");
        assert!(
            script.ran("-close off"),
            "an unsealed session leaves it open"
        );
        assert!(script.ran("-eject"), "a real device is ejected afterwards");
    }

    /// The insert prompt is contract: it is what the operator reads, and the
    /// differential harness compares it between the two binaries.
    #[test]
    fn the_insert_prompt_names_the_disc_the_project_and_the_seal() {
        let dir = tempfile::TempDir::new().unwrap();
        let script = Script::new().toc(&[BLANK]).identities(&[Some(IDENTITY_1)]);
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
        let op = ScriptedOperator::new(&[true]);
        XorrisoBurner::new(
            BurnDevice::RealDevice("/dev/sr0".to_string()),
            &exec,
            &op,
            &NoopSink,
            2,
        )
        .burn(&image_of(dir.path(), 1, &[("a.txt", "hello")], true))
        .expect("the burn succeeds");

        assert_eq!(
            op.log()[0],
            "Insert a blank disc for 'photos' disc 1 into /dev/sr0 and confirm. \
             This burn will FINALIZE the disc."
        );
    }

    #[test]
    fn a_top_off_prompt_names_the_last_burned_session() {
        let dir = tempfile::TempDir::new().unwrap();
        let script = Script::new()
            .toc(&[APPENDABLE])
            .identities(&[Some(IDENTITY_1), Some(IDENTITY_2)]);
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
        let op = ScriptedOperator::new(&[true]);
        XorrisoBurner::new(
            BurnDevice::RealDevice("/dev/sr0".to_string()),
            &exec,
            &op,
            &NoopSink,
            2,
        )
        .burn(&image_of(dir.path(), 2, &[("a.txt", "hello")], false))
        .expect("the burn succeeds");

        assert_eq!(
            op.log()[0],
            "Insert disc 1 of 'photos' (last burned session 1) into /dev/sr0 and confirm."
        );
    }

    /// Written media where a blank was asked for: refused, re-prompted, and
    /// accepted once the operator swaps the disc.
    #[test]
    fn session_one_refuses_written_media_and_succeeds_on_the_swap() {
        let dir = tempfile::TempDir::new().unwrap();
        let script = Script::new()
            .toc(&[APPENDABLE, BLANK])
            .identities(&[Some(IDENTITY_1), Some(IDENTITY_1)]);
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
        let op = ScriptedOperator::new(&[true, true]);
        let result = XorrisoBurner::new(
            BurnDevice::RealDevice("/dev/sr0".to_string()),
            &exec,
            &op,
            &NoopSink,
            2,
        )
        .burn(&image_of(dir.path(), 1, &[("a.txt", "hello")], false))
        .expect("the second disc is accepted");
        assert!(result.image_bytes > 0);
        assert!(
            op.said("Not the expected disc: found written media (expected a blank disc)"),
            "{:?}",
            op.log()
        );
    }

    /// A top-off is accepted only when the disc identifies as this project,
    /// this disc, at exactly the previous session ordinal.
    #[test]
    fn a_top_off_requires_the_previous_session_ordinal() {
        let dir = tempfile::TempDir::new().unwrap();
        let script = Script::new()
            .toc(&[APPENDABLE])
            .identities(&[Some(IDENTITY_1), Some(IDENTITY_2)]);
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
        let op = ScriptedOperator::new(&[true]);
        let result = XorrisoBurner::new(
            BurnDevice::RealDevice("/dev/sr0".to_string()),
            &exec,
            &op,
            &NoopSink,
            2,
        )
        .burn(&image_of(dir.path(), 2, &[("a.txt", "hello")], true))
        .expect("the burn succeeds");
        assert!(result.image_bytes > 0);
        assert!(script.ran("-dev"), "an append merges rather than restarts");
        assert!(script.ran("-close on"), "a sealing session closes the disc");
    }

    /// A declined prompt is `Aborted` — exit 0 at the CLI, and nothing burned.
    #[test]
    fn a_declined_prompt_aborts_without_staging() {
        let dir = tempfile::TempDir::new().unwrap();
        let script = Script::new().toc(&[BLANK]).identities(&[]);
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
        let op = ScriptedOperator::new(&[false]);
        let image = image_of(dir.path(), 1, &[("a.txt", "hello")], false);
        let err = XorrisoBurner::new(
            BurnDevice::RealDevice("/dev/sr0".to_string()),
            &exec,
            &op,
            &NoopSink,
            2,
        )
        .burn(&image)
        .unwrap_err();
        assert!(matches!(err, BurnError::Aborted));
        assert!(
            !image.staging_dir.exists(),
            "a declined burn stages nothing"
        );
        assert!(!script.ran("-commit"), "and writes nothing");
    }

    /// 1000 bytes free is less than the fixed 32 MiB overhead, so the burn is
    /// refused after staging and before any write.
    #[test]
    fn insufficient_capacity_refuses_before_writing() {
        let dir = tempfile::TempDir::new().unwrap();
        let script = Script::new()
            .toc(&[&["Media status : is blank", "Media space  : 0s"]])
            .identities(&[]);
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
        let op = ScriptedOperator::new(&[true]);
        let err = XorrisoBurner::new(
            BurnDevice::RealDevice("/dev/sr0".to_string()),
            &exec,
            &op,
            &NoopSink,
            2,
        )
        .burn(&image_of(dir.path(), 1, &[("a.txt", "hello")], false))
        .unwrap_err();
        assert!(
            matches!(err, BurnError::InsufficientCapacity { .. }),
            "{err}"
        );
        assert!(!script.ran("-commit"), "nothing was written");
    }

    #[test]
    fn a_failing_xorriso_surfaces_its_exit_code_and_output_tail() {
        let dir = tempfile::TempDir::new().unwrap();
        let script = Script::new().toc(&[BLANK]).identities(&[]).burn_fails();
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
        let op = ScriptedOperator::new(&[true]);
        let err = XorrisoBurner::new(
            BurnDevice::RealDevice("/dev/sr0".to_string()),
            &exec,
            &op,
            &NoopSink,
            2,
        )
        .burn(&image_of(dir.path(), 1, &[("a.txt", "hello")], false))
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "xorriso failed (exit 5): FAILURE : write error"
        );
    }

    /// The disc must read back as the session we just wrote. A drive that
    /// silently wrote nothing fails here rather than at the next top-off.
    #[test]
    fn a_post_burn_identity_mismatch_fails_the_burn() {
        let dir = tempfile::TempDir::new().unwrap();
        let script = Script::new().toc(&[BLANK]).identities(&[Some(IDENTITY_7)]);
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
        let op = ScriptedOperator::new(&[true]);
        let err = XorrisoBurner::new(
            BurnDevice::RealDevice("/dev/sr0".to_string()),
            &exec,
            &op,
            &NoopSink,
            2,
        )
        .burn(&image_of(dir.path(), 1, &[("a.txt", "hello")], false))
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "post-burn sanity check failed: expected identity (photos, disc 1, session 1) \
             readable from the disc, found: Some(DiscIdentity(photos,1,7,2026-08-30T10:00:00Z))"
        );
    }

    /// A disc that reads back as nothing at all renders `None`, matching the
    /// Scala interpolation of an empty `Option`.
    #[test]
    fn an_unreadable_disc_post_burn_renders_none() {
        let dir = tempfile::TempDir::new().unwrap();
        let script = Script::new().toc(&[BLANK]).identities(&[None]);
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
        let op = ScriptedOperator::new(&[true]);
        let err = XorrisoBurner::new(
            BurnDevice::RealDevice("/dev/sr0".to_string()),
            &exec,
            &op,
            &NoopSink,
            2,
        )
        .burn(&image_of(dir.path(), 1, &[("a.txt", "hello")], false))
        .unwrap_err();
        assert!(err.to_string().ends_with("found: None"), "{err}");
    }

    #[test]
    fn xorriso_older_than_1_5_is_refused_before_anything_else() {
        let dir = tempfile::TempDir::new().unwrap();
        let script = Script::new()
            .version("xorriso 1.4.2")
            .toc(&[])
            .identities(&[]);
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
        let op = ScriptedOperator::new(&[true]);
        let err = XorrisoBurner::new(
            BurnDevice::RealDevice("/dev/sr0".to_string()),
            &exec,
            &op,
            &NoopSink,
            2,
        )
        .burn(&image_of(dir.path(), 1, &[("a.txt", "hello")], false))
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "xorriso 1.4.2 is too old (need 1.5 or newer)"
        );
        assert!(op.log().is_empty(), "the operator was never prompted");
    }

    /// An image file has no tray, so nothing is ejected.
    #[test]
    fn an_image_file_is_never_ejected() {
        let dir = tempfile::TempDir::new().unwrap();
        let script = Script::new().toc(&[BLANK]).identities(&[Some(IDENTITY_1)]);
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
        let op = ScriptedOperator::new(&[true]);
        XorrisoBurner::new(
            BurnDevice::ImageFile(dir.path().join("d.iso")),
            &exec,
            &op,
            &NoopSink,
            2,
        )
        .burn(&image_of(dir.path(), 1, &[("a.txt", "hello")], false))
        .expect("the burn succeeds");
        assert!(!script.ran("-eject"));
    }
}
