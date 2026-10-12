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
        self.write(image, staged.image_bytes)?;
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
                id.project_name,
                id.disc_name()
            )
        } else {
            format!(
                "disc {} of '{}' (last burned session {})",
                id.disc_name(),
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
            ) if found.same_disc(id) && found.session_ordinal + 1 == n => Ok(free_bytes),
            (
                MediaStatus::Appendable {
                    identity: Some(found),
                    ..
                },
                _,
            ) => Err(format!(
                "disc '{}' #{} at session {}",
                found.project_name,
                found.disc_name(),
                found.session_ordinal
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

    fn write(&self, image: &DiscImage, image_bytes: u64) -> Result<(), BurnError> {
        let id = &image.metadata.identity;
        let volid = args::volid(&id.project_name, &id.disc_name());
        let command = args::burn(
            &self.device,
            &image.staging_dir,
            id.session_ordinal == 1,
            image.close_disc,
            &volid,
        );
        let closing_note = if image.close_disc {
            format!(
                "Writing done; closing the session and finalizing disc {}. This can take a minute.",
                id.disc_name()
            )
        } else {
            format!(
                "Writing done; closing the session on disc {}.",
                id.disc_name()
            )
        };
        self.progress
            .start_bytes(&format!("burning disc {}", id.disc_name()), image_bytes);
        let mut writing = WriteProgress::Before;
        let result = self.exec.run(&command, &mut |line| {
            // Only the `Writing:` percentages and the end of writing reach the
            // operator; the rest of xorriso's chatter stays in `result.lines`.
            writing.observe(line, image_bytes, self.progress, || {
                self.operator.progress(&closing_note)
            });
        });
        if writing != WriteProgress::Ended {
            self.progress.done();
        }
        // `Exec` reports pipe and `wait()` failures as plain `Io`; here they
        // come after xorriso may have touched the media. A tool that cannot
        // be launched at all is still `ToolMissing`.
        let result = result.map_err(|e| match e {
            BurnError::Io(io) => BurnError::WriteInterrupted(io),
            other => other,
        })?;
        if result.exit_code != 0 {
            return Err(BurnError::BurnFailed {
                exit_code: result.exit_code,
                output_tail: result.tail(10),
            });
        }
        Ok(())
    }

    /// Every failure here comes after a successful write, so each one is a
    /// `PostBurnCheckFailed`: the disc is written but not confirmed.
    fn post_burn_check(&self, id: &DiscIdentity) -> Result<(), BurnError> {
        let found = MediaInspector::new(self.device.clone(), self.exec)
            .read_identity()
            .map_err(|e| {
                // `Io`'s own message is the bare "burn I/O error"; name the
                // underlying error instead.
                BurnError::PostBurnCheckFailed(match e {
                    BurnError::Io(io) => format!("could not read the identity back: {io}"),
                    other => format!("could not read the identity back: {other}"),
                })
            })?;
        let matches = found
            .as_ref()
            .is_some_and(|f| f.same_disc(id) && f.session_ordinal == id.session_ordinal);
        if matches {
            return Ok(());
        }
        Err(BurnError::PostBurnCheckFailed(format!(
            "expected identity ({}, disc {}, session {}) readable from the disc, found: {}",
            id.project_name,
            id.disc_name(),
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

/// Where a burn is, as read from xorriso's output line by line.
///
/// Writing ends at 100% or at libburn's closing/fixating message, whichever
/// comes first; after that, xorriso closes the session (and the disc, when
/// sealing) without printing percentages, which is the pause the operator is
/// told about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteProgress {
    Before,
    /// `reported` bytes have been passed to the sink so far.
    Writing {
        reported: u64,
    },
    Ended,
}

impl WriteProgress {
    fn observe(
        &mut self,
        line: &str,
        image_bytes: u64,
        sink: &dyn ProgressSink,
        announce_closing: impl FnOnce(),
    ) {
        if *self == WriteProgress::Ended {
            return;
        }
        let reported = match *self {
            WriteProgress::Writing { reported } => reported,
            _ => 0,
        };
        if let Some(per_mille) = parse::writing_per_mille(line) {
            let target = image_bytes * u64::from(per_mille) / 1000;
            if target > reported {
                sink.advance(target - reported, 0);
            }
            *self = WriteProgress::Writing {
                reported: target.max(reported),
            };
            if per_mille == 1000 {
                self.end(image_bytes, sink, announce_closing);
            }
        } else if matches!(self, WriteProgress::Writing { .. }) && is_closing(line) {
            self.end(image_bytes, sink, announce_closing);
        }
    }

    /// Fill the bar (the data is all written), close the phase, say why the
    /// output is about to go quiet.
    fn end(&mut self, image_bytes: u64, sink: &dyn ProgressSink, announce_closing: impl FnOnce()) {
        if let WriteProgress::Writing { reported } = *self {
            if image_bytes > reported {
                sink.advance(image_bytes - reported, 0);
            }
        }
        sink.done();
        announce_closing();
        *self = WriteProgress::Ended;
    }
}

/// libburn's pacifier while it closes a track or session, or fixates a disc.
/// The exact wording varies by medium and version, so this matches loosely.
fn is_closing(line: &str) -> bool {
    let line = line.to_ascii_lowercase();
    line.contains("closing") || line.contains("fixating")
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
    use crate::burn::testkit::{
        image_of, RecordingSink, ScriptedExec as Script, ScriptedOperator, APPENDABLE, BLANK,
    };
    use crate::progress::NoopSink;

    const IDENTITY_1: &str = r#"{"projectName":"photos","discOrdinal":1,"sessionOrdinal":1,"createdAt":"2026-08-30T10:00:00Z"}"#;
    const IDENTITY_2: &str = r#"{"projectName":"photos","discOrdinal":1,"sessionOrdinal":2,"createdAt":"2026-08-30T10:00:00Z"}"#;
    /// The disc 1 twin from set B of a two-copy project, at session 1.
    const IDENTITY_1B: &str = r#"{"projectName":"photos","discOrdinal":1,"copySet":1,"copies":2,"sessionOrdinal":1,"createdAt":"2026-08-30T10:00:00Z"}"#;
    const IDENTITY_7: &str = r#"{"projectName":"photos","discOrdinal":1,"sessionOrdinal":7,"createdAt":"2026-08-30T10:00:00Z"}"#;

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

    /// The twin from the other copy set is the same project, the same ordinal
    /// and the previous session: it is still the wrong disc, and re-prompts.
    #[test]
    fn a_top_off_refuses_the_twin_from_another_copy_set() {
        let dir = tempfile::TempDir::new().unwrap();
        let script = Script::new()
            .toc(&[APPENDABLE])
            .identities(&[Some(IDENTITY_1B)]);
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
        let op = ScriptedOperator::new(&[true, false]);
        let image = image_of(dir.path(), 2, &[("a.txt", "hello")], false);
        assert_eq!(image.metadata.identity.copy_set, 0);
        let err = XorrisoBurner::new(
            BurnDevice::RealDevice("/dev/sr0".to_string()),
            &exec,
            &op,
            &NoopSink,
            2,
        )
        .burn(&image)
        .unwrap_err();
        assert!(matches!(err, BurnError::Aborted), "{err:?}");
        assert!(
            op.said(
                "Not the expected disc: found disc 'photos' #1B at session 1. \
                 Swap discs and confirm again."
            ),
            "{:?}",
            op.log()
        );
        assert!(!script.ran("-commit"), "nothing is written to the twin");
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

    /// A pipe or `wait()` failure while xorriso writes is not a pre-write
    /// `Io`: the disc may be spent, and the partial-burn report depends on
    /// telling the two apart (#11).
    #[test]
    fn an_io_failure_during_the_write_is_write_interrupted() {
        let dir = tempfile::TempDir::new().unwrap();
        let script = Script::new().toc(&[BLANK]).identities(&[]);
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| {
            if c.iter().any(|a| a == "-commit") {
                return Err(BurnError::Io(std::io::Error::other("broken pipe")));
            }
            script.respond(c, l)
        };
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
        assert!(matches!(err, BurnError::WriteInterrupted(_)), "{err:?}");
        assert_eq!(
            err.to_string(),
            "burn I/O error while xorriso was writing the disc: broken pipe"
        );
    }

    /// The identity read-back runs after a successful write, so its I/O
    /// failure is a failed post-burn check, never a pre-write `Io` (#11).
    #[test]
    fn an_io_failure_reading_the_identity_back_fails_the_post_burn_check() {
        let dir = tempfile::TempDir::new().unwrap();
        let script = Script::new().toc(&[BLANK]).identities(&[]);
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| {
            if c.iter().any(|a| a == "-extract") {
                return Err(BurnError::Io(std::io::Error::other("read error")));
            }
            script.respond(c, l)
        };
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
            "post-burn sanity check failed: could not read the identity back: read error"
        );
    }

    /// A tool that cannot be launched never touched the media, so it keeps
    /// its own variant through the write's `Io` mapping.
    #[test]
    fn a_missing_tool_at_the_write_stays_tool_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let script = Script::new().toc(&[BLANK]).identities(&[]);
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| {
            if c.iter().any(|a| a == "-commit") {
                return Err(BurnError::ToolMissing("cannot launch xorriso".into()));
            }
            script.respond(c, l)
        };
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
        assert!(matches!(err, BurnError::ToolMissing(_)), "{err:?}");
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

    const FILES_ADDED: &str = "xorriso : UPDATE :       1 files added in 1 seconds";
    const AT_0: &str = "xorriso : UPDATE : Writing:          0s    0.0%   fifo   0%  buf   0%";
    const AT_50: &str =
        "xorriso : UPDATE : Writing:     848140s   50.0%   fifo  99%  buf  50%  3.1xB ";
    const AT_100: &str =
        "xorriso : UPDATE : Writing:    1696280s  100.0%   fifo   0%  buf   0%  3.1xB ";

    /// Burn the one-file test image with `lines` as xorriso's output.
    fn burn_printing(lines: &[&str], close_disc: bool) -> (RecordingSink, ScriptedOperator, u64) {
        let dir = tempfile::TempDir::new().unwrap();
        let script = Script::new()
            .toc(&[BLANK])
            .identities(&[Some(IDENTITY_1)])
            .burn_lines(lines);
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
        let op = ScriptedOperator::new(&[true]);
        let sink = RecordingSink::default();
        let result = XorrisoBurner::new(
            BurnDevice::RealDevice("/dev/sr0".to_string()),
            &exec,
            &op,
            &sink,
            2,
        )
        .burn(&image_of(dir.path(), 1, &[("a.txt", "hello")], close_disc))
        .expect("the burn succeeds");
        (sink, op, result.image_bytes)
    }

    /// The burn's own phase follows staging's: sized to the staged image, fed
    /// by xorriso's `Writing:` percentages, and closed.
    #[test]
    fn writing_lines_drive_a_bytes_phase_sized_to_the_image() {
        let (sink, op, image_bytes) = burn_printing(&[FILES_ADDED, AT_0, AT_50, AT_100], false);
        let events = sink.events();
        let burn: Vec<&String> = events
            .iter()
            .skip_while(|e| !e.starts_with("start_bytes"))
            .collect();
        assert_eq!(
            burn.first().map(|e| e.as_str()),
            Some(format!("start_bytes burning disc 1 {image_bytes}").as_str())
        );
        assert_eq!(burn.last().map(|e| e.as_str()), Some("done"));
        assert_eq!(
            burn.iter().filter(|e| e.starts_with("advance")).count(),
            2,
            "0% advances nothing; 50% and 100% do"
        );
        let staged: u64 = events
            .iter()
            .take_while(|e| !e.starts_with("start_bytes"))
            .filter_map(|e| e.strip_prefix("advance "))
            .map(|r| r.split(' ').next().unwrap().parse::<u64>().unwrap())
            .sum();
        assert_eq!(
            sink.advanced_bytes() - staged,
            image_bytes,
            "100% is the whole image"
        );
        assert!(
            !op.said("UPDATE"),
            "raw xorriso lines are not echoed: {:?}",
            op.log()
        );
    }

    /// After 100% xorriso closes the session (and the disc, when sealing),
    /// which prints no percentages; the operator is told once. The closing
    /// line is the real drive's (Pioneer BDR-UD04, BD-R, 2026-10-11), which
    /// repeats it about once a second for the ~11 s the close took.
    #[test]
    fn the_closing_pause_is_announced_once() {
        let closing = "xorriso : UPDATE : Closing track/session. Working since 1292 seconds";
        let (_, op, _) = burn_printing(&[AT_50, AT_100, closing, closing], true);
        let notes: Vec<String> = op
            .log()
            .into_iter()
            .filter(|l| l.contains("closing the session"))
            .collect();
        assert_eq!(
            notes,
            vec![
                "Writing done; closing the session and finalizing disc 1. This can take a minute."
            ]
        );
    }

    /// A closing message ends the phase even if the last percentage seen was
    /// below 100, and later `Writing:` lines do not reopen it.
    #[test]
    fn a_closing_message_before_100_percent_still_ends_the_phase_once() {
        let closing = "xorriso : UPDATE : Closing track/session. Working since 0 seconds";
        let (sink, op, _) = burn_printing(&[AT_50, closing, AT_100], false);
        let events = sink.events();
        let burn: Vec<&String> = events
            .iter()
            .skip_while(|e| !e.starts_with("start_bytes"))
            .collect();
        assert_eq!(burn.iter().filter(|e| e.as_str() == "done").count(), 1);
        assert_eq!(burn.last().map(|e| e.as_str()), Some("done"));
        assert!(op.said("Writing done; closing the session on disc 1."));
    }

    /// A burn whose output never reaches 100% (or never prints a percentage)
    /// still closes its phase, and says nothing about closing.
    #[test]
    fn a_burn_without_100_percent_still_closes_the_phase_quietly() {
        let (sink, op, _) = burn_printing(&[FILES_ADDED, AT_50], false);
        let events = sink.events();
        let burn: Vec<&String> = events
            .iter()
            .skip_while(|e| !e.starts_with("start_bytes"))
            .collect();
        assert!(!burn.is_empty(), "the burn opened its phase: {events:?}");
        assert_eq!(burn.last().map(|e| e.as_str()), Some("done"));
        assert!(!op.said("closing the session"));
    }
}
