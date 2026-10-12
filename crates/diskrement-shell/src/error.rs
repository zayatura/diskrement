//! Expected, recoverable failures. One `thiserror` enum per subsystem
//! (spec §5.5).

/// Persistence failures the caller is expected to handle.
///
/// The Scala equivalent was a sealed `CatalogError` reached through
/// `refineOrDie`: expected faults became typed errors and anything else killed
/// the fiber. `Unexpected` is that second case — it stays a value here, and
/// maps to the same exit code an unexpected failure had (5).
#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("project already exists: {0}")]
    DuplicateProject(String),
    #[error("project not found: {0}")]
    ProjectNotFound(String),
    #[error("{0}")]
    IllegalStatusTransition(String),
    #[error("migration failed: {0}")]
    MigrationFailed(#[source] rusqlite::Error),
    #[error("constraint violation: {0}")]
    ConstraintViolation(String),
    #[error("unreadable timestamp in the catalog: {0}")]
    BadTimestamp(#[from] crate::catalog::time::TimeError),
    #[error(transparent)]
    Unexpected(#[from] rusqlite::Error),
}

impl CatalogError {
    /// Classify a SQLite failure the way `CatalogStoreLive.refineOrDie` did.
    ///
    /// The Scala matched the English text of the driver's message for
    /// `"constraint"`; SQLite hands us a result code instead, so this cannot
    /// drift with a driver's wording or a locale.
    pub(crate) fn from_sqlite(e: rusqlite::Error) -> Self {
        match &e {
            rusqlite::Error::SqliteFailure(f, message)
                if f.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                CatalogError::ConstraintViolation(message.clone().unwrap_or_else(|| f.to_string()))
            }
            _ => CatalogError::Unexpected(e),
        }
    }
}

/// Whether a SQLite failure is specifically a UNIQUE violation.
///
/// Distinguishing this is what turns a duplicate `project.name` into
/// `DuplicateProject` rather than a generic constraint error.
pub(crate) fn is_unique_violation(e: &rusqlite::Error) -> bool {
    matches!(
        e,
        rusqlite::Error::SqliteFailure(f, _)
            if f.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
    )
}

/// `PipelineError.scala`. The exit ladder is the stable contract
/// (spec §3.2) and is pinned by the CLI's ExitMapping port.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("project not found: {0}")]
    ProjectNotFound(String),
    #[error("no staged plan for {0} (run plan)")]
    NoStagedPlan(String),
    #[error("source snapshot missing for {0} (run plan)")]
    SnapshotMissing(String),
    /// `plan` alone is the whole recovery: it scans afresh and re-reads the
    /// view and the disc states. Never advise `reset` — it deletes
    /// `workDir/staging`, the staged-disc verifier's evidence.
    #[error("staged plan is stale: {0} (re-run plan)")]
    StalePlan(String),
    /// A `--disc` or `mark-dead` argument that names no disc in this project.
    /// Carries the argument as typed (normalised to an upper-case letter), so
    /// `-1` and `9C` are echoed back. A negative `--disc` reaches the domain
    /// (zio-cli let it through), and the exit ladder — not clap's argument
    /// parser — is the contract for what happens next.
    #[error("no disc with ordinal {0}")]
    UnknownDisc(String),
    /// A `--disc` naming a disc marked dead. A *sweep* skips dead discs
    /// quietly; naming one is an assertion of intent, and answering it with a
    /// reassuring exit 0 would be a lie. Carries the disc's name (`3`, `3B`).
    #[error("disc {0} is marked dead")]
    DiscIsDead(String),
    /// A `disc mark-dead` naming a disc that already carries a `MARKED_DEAD`
    /// event. Carries the disc's name (`3`, `3B`).
    #[error("disc {0} is already marked dead")]
    DiscAlreadyDead(String),
    /// Surfaced by `plan` in its report and refused by `burn`: the
    /// two-stage oversize check.
    #[error(
        "oversized files cannot be placed: {}",
        .0.iter().map(diskrement_core::RelPath::as_str).collect::<Vec<_>>().join(", ")
    )]
    OversizedFiles(Vec<diskrement_core::RelPath>),
    // The field is `src`, not `source`: thiserror reserves a field named
    // `source` for the error chain and would demand `PathBuf: Error`.
    #[error("work dir {work} is not on the same filesystem as source {src}")]
    NotSameFilesystem {
        src: std::path::PathBuf,
        work: std::path::PathBuf,
    },
    #[error("work dir is not CoW-capable: {work_dir} [{detail}]")]
    NotCowCapable {
        work_dir: std::path::PathBuf,
        detail: String,
    },
    /// Source file names that cannot be recorded as text. An environment
    /// problem the operator fixes by renaming, so exit 3.
    #[error(
        "file names are not valid UTF-8 and cannot be archived (rename them): {}",
        .0.join(", ")
    )]
    NonUtf8FileNames(Vec<String>),
    /// Renders its cause's message and nothing else (`PipelineError.Burn`).
    #[error(transparent)]
    Burn(#[from] crate::burn::BurnError),
    /// A multi-disc burn that committed some discs and then failed. The cause
    /// decides the exit code — an aborted prompt still exits 0 — and also the
    /// advice: a stale plan needs a re-plan (never `reset`, which deletes the
    /// staging the verifier needs), anything else a re-run of `burn`.
    ///
    /// - `committed`: discs the catalog now holds, in burn order.
    /// - `unrecorded`: the failing disc, when the failure came after media was
    ///   (or may have been) written — the catalog does not know it, so it is
    ///   neither committed nor "not written".
    /// - `pending`: the discs not yet attempted. It never includes the
    ///   failing disc when that disc is `unrecorded`, and it is empty when the
    ///   failure came after every disc had committed (only the work-dir
    ///   cleanup failed).
    #[error("{cause} — {}", partial_burn_tail(.committed, .unrecorded, .pending, .cause))]
    PartialBurn {
        committed: Vec<String>,
        unrecorded: Option<UnrecordedDisc>,
        pending: Vec<String>,
        cause: Box<PipelineError>,
    },
    /// Unexpected-but-handled failure: rendered as
    /// `internal error: <detail> (this is a bug)`, exit 5.
    #[error("internal error: {0} (this is a bug)")]
    Failed(String),
}

/// A disc a `PartialBurn` wrote, or may have written, without recording it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnrecordedDisc {
    pub name: String,
    /// `true` when the burner reported success and only the commit failed;
    /// `false` when the burner failed after it may already have touched the
    /// media (a failed xorriso run, a failed post-burn check).
    pub surely_written: bool,
}

impl UnrecordedDisc {
    pub fn written(name: impl Into<String>) -> Self {
        UnrecordedDisc {
            name: name.into(),
            surely_written: true,
        }
    }

    pub fn maybe_written(name: impl Into<String>) -> Self {
        UnrecordedDisc {
            name: name.into(),
            surely_written: false,
        }
    }
}

/// What to do after a `PartialBurn`, decided by its cause.
fn partial_burn_advice(cause: &PipelineError) -> &'static str {
    match cause {
        // The staged plan no longer holds, or its snapshot is gone. A fresh
        // `plan` rebuilds both, and turns the committed discs' files into
        // catch-up copies for the lagging sets.
        PipelineError::StalePlan(_) | PipelineError::SnapshotMissing(_) => "re-run plan, then burn",
        _ => "re-run burn",
    }
}

/// Everything a `PartialBurn` says after its cause:
/// `"1A committed; 1B not written (re-run burn)"`. Parts that are empty are
/// left out. The single source of the advice text: the error's `Display` and
/// the CLI's `Aborted.` line both use it.
pub fn partial_burn_tail(
    committed: &[String],
    unrecorded: &Option<UnrecordedDisc>,
    pending: &[String],
    cause: &PipelineError,
) -> String {
    let mut parts = vec![format!("{} committed", committed.join(", "))];
    match unrecorded {
        Some(d) if d.surely_written => {
            parts.push(format!("{} written but not recorded", d.name));
        }
        Some(d) => parts.push(format!(
            "{} may have been written but was not recorded",
            d.name
        )),
        None => {}
    }
    if !pending.is_empty() {
        parts.push(format!("{} not written", pending.join(", ")));
    }
    // After every disc committed, only the work-dir cleanup failed: there is
    // nothing to re-burn, and "re-run burn" would only find no staged plan.
    let advice = if pending.is_empty() && unrecorded.is_none() {
        "nothing is left to write"
    } else {
        partial_burn_advice(cause)
    };
    format!("{} ({advice})", parts.join("; "))
}

/// `e`'s message followed by each of its causes, joined with `": "`.
///
/// A cause whose text the message already contains is skipped, since
/// `#[error("... {0}")]` variants often embed their source.
pub fn with_causes(e: &(dyn std::error::Error + 'static)) -> String {
    let mut out = e.to_string();
    let mut cause = e.source();
    while let Some(c) = cause {
        let text = c.to_string();
        if !out.contains(&text) {
            out.push_str(": ");
            out.push_str(&text);
        }
        cause = c.source();
    }
    out
}

impl PipelineError {
    /// `Failed` carrying `e`'s whole cause chain, so an unexpected failure
    /// still names the file and the OS error behind it.
    pub fn failed(e: &(dyn std::error::Error + 'static)) -> Self {
        PipelineError::Failed(with_causes(e))
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            PipelineError::ProjectNotFound(_)
            | PipelineError::NoStagedPlan(_)
            | PipelineError::SnapshotMissing(_)
            | PipelineError::UnknownDisc(_)
            | PipelineError::DiscIsDead(_)
            | PipelineError::DiscAlreadyDead(_)
            | PipelineError::StalePlan(_) => 2,
            PipelineError::NotSameFilesystem { .. }
            | PipelineError::NotCowCapable { .. }
            | PipelineError::NonUtf8FileNames(_)
            | PipelineError::OversizedFiles(_) => 3,
            // An aborted burn is a successful run that wrote nothing; every
            // other burn failure is 6 (media/burner).
            PipelineError::Burn(crate::burn::BurnError::Aborted) => 0,
            PipelineError::Burn(_) => 6,
            PipelineError::PartialBurn { cause, .. } => cause.exit_code(),
            PipelineError::Failed(_) => 5,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diskrement_core::RelPath;

    #[test]
    fn exit_codes_are_the_contract_for_the_project_and_filesystem_variants() {
        assert_eq!(
            PipelineError::ProjectNotFound("photos".into()).exit_code(),
            2
        );
        assert_eq!(
            PipelineError::NotSameFilesystem {
                src: "/s".into(),
                work: "/w".into()
            }
            .exit_code(),
            3
        );
        assert_eq!(
            PipelineError::NotCowCapable {
                work_dir: "/w".into(),
                detail: "d".into()
            }
            .exit_code(),
            3
        );
        assert_eq!(PipelineError::Failed("x".into()).exit_code(), 5);
    }

    #[test]
    fn messages_match_the_scala_render_verbatim() {
        assert_eq!(
            PipelineError::ProjectNotFound("photos".into()).to_string(),
            "project not found: photos"
        );
        assert_eq!(
            PipelineError::NotSameFilesystem {
                src: "/srv/src".into(),
                work: "/srv/work".into()
            }
            .to_string(),
            "work dir /srv/work is not on the same filesystem as source /srv/src"
        );
        assert_eq!(
            PipelineError::NotCowCapable {
                work_dir: "/w".into(),
                detail: "clone said no".into()
            }
            .to_string(),
            "work dir is not CoW-capable: /w [clone said no]"
        );
        assert_eq!(
            PipelineError::Failed("project already exists: p".into()).to_string(),
            "internal error: project already exists: p (this is a bug)"
        );
    }

    fn partial(cause: PipelineError) -> PipelineError {
        PipelineError::PartialBurn {
            committed: vec!["1A".into()],
            unrecorded: None,
            pending: vec!["1B".into()],
            cause: Box::new(cause),
        }
    }

    #[test]
    fn a_partial_burn_carries_its_causes_exit_code_and_names_both_halves() {
        let media = partial(PipelineError::Burn(crate::burn::BurnError::NoMedia(
            "/dev/sr0".into(),
        )));
        assert_eq!(media.exit_code(), 6);
        assert_eq!(
            media.to_string(),
            "no disc in /dev/sr0 — 1A committed; 1B not written (re-run burn)"
        );
        assert_eq!(
            partial(PipelineError::Burn(crate::burn::BurnError::Aborted)).exit_code(),
            0
        );
    }

    #[test]
    fn a_partial_burn_on_a_stale_plan_advises_re_planning_never_reset() {
        let stale = partial(PipelineError::StalePlan("disc 1B was marked dead".into()));
        assert_eq!(stale.exit_code(), 2);
        let text = stale.to_string();
        assert!(
            text.ends_with("1A committed; 1B not written (re-run plan, then burn)"),
            "{text}"
        );
        assert!(!text.contains("reset"), "{text}");
        let missing = partial(PipelineError::SnapshotMissing("p".into())).to_string();
        assert!(missing.ends_with("(re-run plan, then burn)"), "{missing}");
        assert!(!missing.contains("reset"), "{missing}");
    }

    #[test]
    fn a_disc_written_but_not_recorded_is_not_called_unwritten() {
        let err = PipelineError::PartialBurn {
            committed: vec!["1A".into()],
            unrecorded: Some(UnrecordedDisc::written("1B")),
            pending: vec!["1C".into()],
            cause: Box::new(PipelineError::Failed("disk full".into())),
        };
        assert_eq!(err.exit_code(), 5);
        assert_eq!(
            err.to_string(),
            "internal error: disk full (this is a bug) — 1A committed; \
             1B written but not recorded; 1C not written (re-run burn)"
        );
    }

    #[test]
    fn a_disc_the_burner_may_have_written_says_so() {
        let err = PipelineError::PartialBurn {
            committed: vec!["1A".into()],
            unrecorded: Some(UnrecordedDisc::maybe_written("1B")),
            pending: vec![],
            cause: Box::new(PipelineError::Burn(
                crate::burn::BurnError::PostBurnCheckFailed("unreadable".into()),
            )),
        };
        assert_eq!(err.exit_code(), 6);
        assert_eq!(
            err.to_string(),
            "post-burn sanity check failed: unreadable — 1A committed; \
             1B may have been written but was not recorded (re-run burn)"
        );
    }

    #[test]
    fn the_partial_burn_tail_is_the_display_after_the_cause() {
        let cause = PipelineError::Burn(crate::burn::BurnError::Aborted);
        let tail = partial_burn_tail(&["1A".into()], &None, &["1B".into()], &cause);
        assert_eq!(tail, "1A committed; 1B not written (re-run burn)");
        assert_eq!(
            partial(cause).to_string(),
            format!("aborted by operator — {tail}")
        );
    }

    #[test]
    fn a_failure_after_every_disc_committed_lists_nothing_as_unwritten() {
        let err = PipelineError::PartialBurn {
            committed: vec!["1A".into(), "1B".into()],
            unrecorded: None,
            pending: vec![],
            cause: Box::new(PipelineError::Failed("cannot clear".into())),
        };
        assert_eq!(
            err.to_string(),
            "internal error: cannot clear (this is a bug) — 1A, 1B committed (nothing is left to write)"
        );
    }

    #[test]
    fn staged_state_errors_are_exit_2_and_oversize_is_exit_3() {
        assert_eq!(PipelineError::NoStagedPlan("p".into()).exit_code(), 2);
        assert_eq!(PipelineError::StalePlan("d".into()).exit_code(), 2);
        assert_eq!(
            PipelineError::OversizedFiles(vec![RelPath("huge.bin".into())]).exit_code(),
            3
        );
    }

    #[test]
    fn failed_renders_the_cause_chain_without_repeating_an_embedded_source() {
        let io = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "a.bin: denied");
        let snap = crate::fs::snapshot::SnapshotError::Io(io);
        assert_eq!(
            PipelineError::failed(&snap).to_string(),
            "internal error: snapshot I/O error: a.bin: denied (this is a bug)"
        );
        // `ConstraintViolation`'s message already embeds its detail, and it
        // has no source: nothing is appended.
        let embedded = CatalogError::ConstraintViolation("unique".into());
        assert_eq!(with_causes(&embedded), "constraint violation: unique");
    }

    #[test]
    fn non_utf8_file_names_are_an_exit_3_environment_error_that_says_rename() {
        let e = PipelineError::NonUtf8FileNames(vec!["a\u{FFFD}".into(), "b\u{FFFD}".into()]);
        assert_eq!(e.exit_code(), 3);
        assert_eq!(
            e.to_string(),
            "file names are not valid UTF-8 and cannot be archived (rename them): \
             a\u{FFFD}, b\u{FFFD}"
        );
    }

    #[test]
    fn the_staged_state_messages_match_the_scala_render_verbatim() {
        assert_eq!(
            PipelineError::NoStagedPlan("photos".into()).to_string(),
            "no staged plan for photos (run plan)"
        );
        assert_eq!(
            PipelineError::OversizedFiles(vec![
                RelPath("huge.bin".into()),
                RelPath("also/huge.bin".into())
            ])
            .to_string(),
            "oversized files cannot be placed: huge.bin, also/huge.bin"
        );
    }

    /// The one assertion `ExitMappingSpec` makes negatively: `reset` deletes
    /// `workDir/staging`, which is the staged-disc verifier's evidence, so the
    /// advertised recovery from a stale plan is `plan` and only `plan`.
    #[test]
    fn stale_plan_advises_re_running_plan_and_never_mentions_reset() {
        let msg =
            PipelineError::StalePlan("disc 3 was marked dead after this plan was staged".into())
                .to_string();
        assert_eq!(
            msg,
            "staged plan is stale: disc 3 was marked dead after this plan was \
             staged (re-run plan)"
        );
        assert!(msg.contains("re-run plan"), "{msg}");
        assert!(!msg.contains("reset"), "{msg}");
    }

    #[test]
    fn burn_errors_are_exit_6_and_an_abort_is_exit_0() {
        use crate::burn::BurnError;
        use diskrement_core::{ContentHash, RelPath};

        assert_eq!(
            PipelineError::Burn(BurnError::HashMismatch {
                rel_path: RelPath("a.bin".into()),
                expected: ContentHash("aa".into()),
                actual: ContentHash("bb".into()),
            })
            .exit_code(),
            6
        );
        assert_eq!(
            PipelineError::Burn(BurnError::SourceFileMissing(RelPath("a.bin".into()))).exit_code(),
            6
        );
        // An aborted burn is a successful run that wrote nothing.
        assert_eq!(PipelineError::Burn(BurnError::Aborted).exit_code(), 0);
        assert_eq!(
            PipelineError::SnapshotMissing("photos".into()).exit_code(),
            2
        );
    }

    #[test]
    fn burn_error_messages_match_the_scala_render_verbatim() {
        use crate::burn::BurnError;
        use diskrement_core::{ContentHash, RelPath};

        assert_eq!(
            PipelineError::SnapshotMissing("photos".into()).to_string(),
            "source snapshot missing for photos (run plan)"
        );
        // `PipelineError.Burn` renders its cause's message, so the wrapper adds
        // nothing of its own.
        assert_eq!(
            PipelineError::Burn(BurnError::HashMismatch {
                rel_path: RelPath("nested/a.bin".into()),
                expected: ContentHash("1111".into()),
                actual: ContentHash("2222".into()),
            })
            .to_string(),
            "content hash mismatch for nested/a.bin: expected 1111, got 2222"
        );
        assert_eq!(
            PipelineError::Burn(BurnError::SourceFileMissing(RelPath("gone.bin".into())))
                .to_string(),
            "source file missing from snapshot: gone.bin"
        );
        // Deliberately terse: `BurnIoError`'s Scala message never interpolates
        // its cause, and the CLI prints the message alone.
        assert_eq!(
            PipelineError::Burn(BurnError::Io(std::io::Error::other("disk on fire"))).to_string(),
            "burn I/O error"
        );
        assert_eq!(
            PipelineError::Burn(BurnError::Aborted).to_string(),
            "aborted by operator"
        );
    }

    #[test]
    fn disc_state_errors_are_exit_2_with_their_messages() {
        assert_eq!(PipelineError::UnknownDisc("9".into()).exit_code(), 2);
        assert_eq!(PipelineError::DiscIsDead("3".into()).exit_code(), 2);
        assert_eq!(
            PipelineError::UnknownDisc("99".into()).to_string(),
            "no disc with ordinal 99"
        );
        assert_eq!(
            PipelineError::DiscIsDead("3".into()).to_string(),
            "disc 3 is marked dead"
        );
    }

    /// zio-cli let a negative `--disc` reach the domain, so the contract
    /// answer is exit 2 with the ordinal echoed back — not an argument error.
    /// Verified against the staged Scala launcher.
    #[test]
    fn a_negative_ordinal_is_a_lookup_failure_not_an_argument_error() {
        let e = PipelineError::UnknownDisc("-1".into());
        assert_eq!(e.to_string(), "no disc with ordinal -1");
        assert_eq!(e.exit_code(), 2);
    }

    /// A second `disc mark-dead` on the same disc. The catalog would refuse it
    /// too, but as `IllegalStatusTransition` — which travels as `Failed` and
    /// exits 5. The Scala build answers 2, so the pipeline pre-checks and
    /// raises this instead.
    #[test]
    fn marking_a_dead_disc_again_is_exit_2_with_its_own_message() {
        assert_eq!(PipelineError::DiscAlreadyDead("1".into()).exit_code(), 2);
        assert_eq!(
            PipelineError::DiscAlreadyDead("1".into()).to_string(),
            "disc 1 is already marked dead"
        );
    }

    /// Three exit-2 disc errors, three distinct sentences. `DiscIsDead` is
    /// what a *named* `verify` target answers; `DiscAlreadyDead` is what a
    /// repeated mark-dead answers; `UnknownDisc` is neither. Collapsing any
    /// two would lose the operator's only clue about which mistake they made.
    #[test]
    fn the_three_disc_errors_read_differently_at_the_same_exit_code() {
        let messages = [
            PipelineError::UnknownDisc("3".into()).to_string(),
            PipelineError::DiscIsDead("3".into()).to_string(),
            PipelineError::DiscAlreadyDead("3".into()).to_string(),
        ];
        assert_eq!(
            messages,
            [
                "no disc with ordinal 3",
                "disc 3 is marked dead",
                "disc 3 is already marked dead",
            ]
        );
        assert!([
            PipelineError::UnknownDisc("3".into()).exit_code(),
            PipelineError::DiscIsDead("3".into()).exit_code(),
            PipelineError::DiscAlreadyDead("3".into()).exit_code(),
        ]
        .iter()
        .all(|c| *c == 2));
    }

    /// Every media/burner failure is exit 6, and every message is the Scala
    /// one verbatim (`Burn.scala`), except `WriteInterrupted`, which has no
    /// Scala counterpart (#11); `InspectFailed` reuses `BurnFailed`'s words
    /// (#17). The abort is the single exception to exit 6,
    /// and it is already pinned above.
    #[test]
    fn the_media_and_tool_errors_are_exit_6_with_their_scala_messages() {
        use crate::burn::BurnError;

        let cases: Vec<(BurnError, &str)> = vec![
            (
                BurnError::ToolMissing("cannot launch xorriso: nope".into()),
                "xorriso is not available: cannot launch xorriso: nope \
                 (install the libisoburn/xorriso package)",
            ),
            (
                BurnError::ToolTooOld("1.4.2".into()),
                "xorriso 1.4.2 is too old (need 1.5 or newer)",
            ),
            (
                BurnError::DeviceNotFound("/dev/sr9".into()),
                "burner device not found: /dev/sr9",
            ),
            (
                BurnError::DevicePermissionDenied("/dev/sr0".into()),
                "no permission to open /dev/sr0 — add your user to the 'optical' group \
                 (sudo usermod -aG optical <user>) and log in again",
            ),
            (BurnError::NoMedia("/dev/sr0".into()), "no disc in /dev/sr0"),
            (
                BurnError::InsufficientCapacity {
                    needed_bytes: 1_000,
                    free_bytes: 10,
                },
                "insufficient media capacity: need 1000 bytes, only 10 free",
            ),
            (
                BurnError::BurnFailed {
                    exit_code: 5,
                    output_tail: vec!["FAILURE : write error".into(), "aborting".into()],
                },
                "xorriso failed (exit 5): FAILURE : write error | aborting",
            ),
            (
                BurnError::InspectFailed {
                    exit_code: 5,
                    output_tail: vec!["FAILURE : odd drive".into()],
                },
                "xorriso failed (exit 5): FAILURE : odd drive",
            ),
            (
                BurnError::WriteInterrupted(std::io::Error::other("broken pipe")),
                "burn I/O error while xorriso was writing the disc: broken pipe",
            ),
            (
                BurnError::PostBurnCheckFailed("expected …, found: None".into()),
                "post-burn sanity check failed: expected …, found: None",
            ),
            (
                BurnError::MountFailed("udisksctl gave no mount point".into()),
                "could not mount the disc for verification: udisksctl gave no mount point",
            ),
        ];

        for (err, message) in cases {
            assert_eq!(err.to_string(), message);
            assert_eq!(
                PipelineError::Burn(err).exit_code(),
                6,
                "every burn failure but an abort is 6"
            );
        }
    }
}
