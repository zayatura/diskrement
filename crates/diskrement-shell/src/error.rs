//! Expected, recoverable failures. One `thiserror` enum per subsystem
//! (spec §5.5); `BurnError` arrives in a later phase.

/// Persistence failures the caller is expected to handle.
///
/// The Scala equivalent was a sealed `CatalogError` reached through
/// `refineOrDie`: expected faults became typed errors and anything else killed
/// the fiber. `Unexpected` is that second case — it stays a value here, and P6
/// maps it to the same exit code an unexpected failure had (5).
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

/// `PipelineError.scala` — the variants P3 can produce. Later phases append
/// the plan/burn/verify variants; the exit ladder is the stable contract
/// (spec §3.2) and is pinned by the CLI's ExitMapping port.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("project not found: {0}")]
    ProjectNotFound(String),
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
    /// Unexpected-but-handled failure: rendered as
    /// `internal error: <detail> (this is a bug)`, exit 5.
    #[error("internal error: {0} (this is a bug)")]
    Failed(String),
}

impl PipelineError {
    pub fn exit_code(&self) -> i32 {
        match self {
            PipelineError::ProjectNotFound(_) => 2,
            PipelineError::NotSameFilesystem { .. } | PipelineError::NotCowCapable { .. } => 3,
            PipelineError::Failed(_) => 5,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_are_the_contract_for_the_p3_variants() {
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
}
