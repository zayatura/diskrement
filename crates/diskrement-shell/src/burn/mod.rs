//! Everything about putting one disc's worth of files somewhere durable:
//! the disc value types, the `/.diskrement` metadata, the stager, and the
//! burners (spec §5.3).
//!
//! `Burner` is a closed enum rather than a trait object: there are exactly two
//! backends, ever, and the split is the real-vs-simulated one the Scala build
//! expressed as two `ZLayer`s. P5 lands the simulated arm; P8 adds `Real`.

pub mod cmd;
pub mod json;
pub mod media;
pub mod simulated;
pub mod snapshot_writer;
pub mod stager;
pub mod verifier;
pub mod xorriso;

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use diskrement_core::{ContentHash, RelPath, ScannedFile, SessionManifest};

use crate::catalog::model::ArchivePointer;

/// Who this disc is, as written to `/.diskrement/project.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscIdentity {
    pub project_name: String,
    pub disc_ordinal: u32,
    pub session_ordinal: u32,
    pub created_at: DateTime<Utc>,
}

/// Scala's case-class `toString`, reproduced because it is interpolated into
/// `PostBurnCheckFailed`'s detail and into the verifier's re-prompt line
/// (spec §3.2: output wording is contract outside clap's argument errors).
impl std::fmt::Display for DiscIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "DiscIdentity({},{},{},{})",
            self.project_name,
            self.disc_ordinal,
            self.session_ordinal,
            crate::catalog::time::format(&self.created_at)
        )
    }
}

/// The whole `/.diskrement` payload for one disc.
///
/// `catalog_snapshot` is a path rather than bytes: it is the work-dir copy the
/// pipeline has already brought to post-burn state, and the writer copies it
/// into place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscMetadata {
    pub identity: DiscIdentity,
    pub manifest: SessionManifest,
    pub catalog_snapshot: PathBuf,
    pub pointers: Vec<ArchivePointer>,
}

/// One disc, ready to be written: where it is staged, where its bytes come
/// from, which files it holds, and whether this session seals it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscImage {
    pub staging_dir: PathBuf,
    pub snapshot_root: PathBuf,
    pub files: Vec<ScannedFile>,
    pub metadata: DiscMetadata,
    pub close_disc: bool,
}

/// What a burn produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BurnResult {
    pub staging_dir: PathBuf,
    pub image_bytes: u64,
}

/// Where a burn physically goes: a real optical drive, or (tests only) a plain
/// file addressed through xorriso's `stdio:` pseudo-drive.
///
/// `ImageFile` has no constructor until P8 wires the image-file integration
/// tests; it is here because `BurnDevice` is the CLI backend resolver's return
/// type and splitting the enum across two phases would churn that signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BurnDevice {
    RealDevice(String),
    ImageFile(PathBuf),
}

impl BurnDevice {
    pub fn xorriso_address(&self) -> String {
        match self {
            BurnDevice::RealDevice(p) => p.clone(),
            BurnDevice::ImageFile(p) => format!("stdio:{}", p.display()),
        }
    }
}

/// Expected burn failures.
///
/// One variant per failure the operator can act on. The exit ladder maps every
/// one of them to 6 except `Aborted`, which is a successful run that wrote
/// nothing and exits 0 (spec §3.2).
#[derive(Debug, thiserror::Error)]
pub enum BurnError {
    #[error("content hash mismatch for {rel_path}: expected {expected}, got {actual}")]
    HashMismatch {
        rel_path: RelPath,
        expected: ContentHash,
        actual: ContentHash,
    },
    #[error("source file missing from snapshot: {0}")]
    SourceFileMissing(RelPath),
    /// The message is deliberately bare: `BurnError.BurnIoError`'s Scala
    /// message is the constant string, and the CLI renders the message alone,
    /// so interpolating the cause here would be a user-visible divergence.
    #[error("burn I/O error")]
    Io(#[source] std::io::Error),
    #[error("xorriso is not available: {0} (install the libisoburn/xorriso package)")]
    ToolMissing(String),
    #[error("xorriso {0} is too old (need 1.5 or newer)")]
    ToolTooOld(String),
    #[error("burner device not found: {0}")]
    DeviceNotFound(String),
    #[error(
        "no permission to open {0} — add your user to the 'optical' group \
         (sudo usermod -aG optical <user>) and log in again"
    )]
    DevicePermissionDenied(String),
    /// Constructed nowhere in this crate, exactly as in the Scala build — it
    /// is pinned by `ExitMappingSpec` as the representative exit-6 failure and
    /// is kept for that reason alone. `NotBlank`, `DiscClosed`, `ForeignDisc`
    /// and `WrongDisc` are the four Scala variants nothing constructs *and*
    /// nothing pins; they are deliberately not ported.
    #[error("no disc in {0}")]
    NoMedia(String),
    #[error("insufficient media capacity: need {needed_bytes} bytes, only {free_bytes} free")]
    InsufficientCapacity { needed_bytes: u64, free_bytes: u64 },
    #[error("xorriso failed (exit {exit_code}): {}", output_tail.join(" | "))]
    BurnFailed {
        exit_code: i32,
        output_tail: Vec<String>,
    },
    #[error("post-burn sanity check failed: {0}")]
    PostBurnCheckFailed(String),
    #[error("could not mount the disc for verification: {0}")]
    MountFailed(String),
    #[error("aborted by operator")]
    Aborted,
}

/// The burn backends. One variant now; P8 adds `Real`.
pub enum Burner<'a> {
    Simulated(simulated::SimulatedBurner<'a>),
}

impl Burner<'_> {
    pub fn burn(&self, image: &DiscImage) -> Result<BurnResult, BurnError> {
        match self {
            Burner::Simulated(b) => b.burn(image),
        }
    }
}
