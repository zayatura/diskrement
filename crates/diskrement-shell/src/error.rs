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
    #[error("no staged scan for {0} (run scan)")]
    NoStagedScan(String),
    #[error("no staged plan for {0} (run plan)")]
    NoStagedPlan(String),
    #[error("source snapshot missing for {0} (run scan)")]
    SnapshotMissing(String),
    /// `plan` alone is the whole recovery: the staged scan and the snapshot
    /// both survive, and `plan` re-reads the view and the disc states. Never
    /// advise `reset` — it deletes `workDir/staging`, the staged-disc
    /// verifier's evidence.
    #[error("staged plan is stale: {0} (re-run plan)")]
    StalePlan(String),
    /// A `--disc` ordinal that names no disc in this project.
    ///
    /// The ordinal is `i64` because zio-cli let a negative value through to
    /// the domain, and the exit ladder — not clap's argument parser — is the
    /// contract for what happens next.
    #[error("no disc with ordinal {0}")]
    UnknownDisc(i64),
    /// A `--disc` ordinal that names a disc marked dead. A *sweep* skips dead
    /// discs quietly; naming one is an assertion of intent, and answering it
    /// with a reassuring exit 0 would be a lie.
    #[error("disc {0} is marked dead")]
    DiscIsDead(i64),
    /// A `disc mark-dead` naming a disc that already carries a `MARKED_DEAD`
    /// event.
    ///
    /// The ordinal is `u32`, not the `i64` `UnknownDisc` and `DiscIsDead`
    /// carry: `disc mark-dead`'s ordinal is validated in the *parser*
    /// (`Commands.scala`), so a non-positive value never reaches the domain.
    /// `verify --disc`'s does, which is why that one stays signed.
    #[error("disc {0} is already marked dead")]
    DiscAlreadyDead(u32),
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
    /// Unexpected-but-handled failure: rendered as
    /// `internal error: <detail> (this is a bug)`, exit 5.
    #[error("internal error: {0} (this is a bug)")]
    Failed(String),
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
            | PipelineError::NoStagedScan(_)
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

    #[test]
    fn staged_state_errors_are_exit_2_and_oversize_is_exit_3() {
        assert_eq!(PipelineError::NoStagedScan("p".into()).exit_code(), 2);
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
            PipelineError::NoStagedScan("photos".into()).to_string(),
            "no staged scan for photos (run scan)"
        );
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
            "source snapshot missing for photos (run scan)"
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
        assert_eq!(PipelineError::UnknownDisc(9).exit_code(), 2);
        assert_eq!(PipelineError::DiscIsDead(3).exit_code(), 2);
        assert_eq!(
            PipelineError::UnknownDisc(99).to_string(),
            "no disc with ordinal 99"
        );
        assert_eq!(
            PipelineError::DiscIsDead(3).to_string(),
            "disc 3 is marked dead"
        );
    }

    /// zio-cli let a negative `--disc` reach the domain, so the contract
    /// answer is exit 2 with the ordinal echoed back — not an argument error.
    /// Verified against the staged Scala launcher.
    #[test]
    fn a_negative_ordinal_is_a_lookup_failure_not_an_argument_error() {
        let e = PipelineError::UnknownDisc(-1);
        assert_eq!(e.to_string(), "no disc with ordinal -1");
        assert_eq!(e.exit_code(), 2);
    }

    /// A second `disc mark-dead` on the same disc. The catalog would refuse it
    /// too, but as `IllegalStatusTransition` — which travels as `Failed` and
    /// exits 5. The Scala build answers 2, so the pipeline pre-checks and
    /// raises this instead.
    #[test]
    fn marking_a_dead_disc_again_is_exit_2_with_its_own_message() {
        assert_eq!(PipelineError::DiscAlreadyDead(1).exit_code(), 2);
        assert_eq!(
            PipelineError::DiscAlreadyDead(1).to_string(),
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
            PipelineError::UnknownDisc(3).to_string(),
            PipelineError::DiscIsDead(3).to_string(),
            PipelineError::DiscAlreadyDead(3).to_string(),
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
            PipelineError::UnknownDisc(3).exit_code(),
            PipelineError::DiscIsDead(3).exit_code(),
            PipelineError::DiscAlreadyDead(3).exit_code(),
        ]
        .iter()
        .all(|c| *c == 2));
    }

    /// Every media/burner failure is exit 6, and every message is the Scala
    /// one verbatim (`Burn.scala`). The abort is the single exception, and it
    /// is already pinned above.
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
