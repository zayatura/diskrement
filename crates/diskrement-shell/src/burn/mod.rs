//! Everything about putting one disc's worth of files somewhere durable:
//! the disc value types, the `/.diskrement` metadata, the stager, and the
//! burners (spec §5.3).
//!
//! `Burner` is a closed enum rather than a trait object: there are exactly two
//! backends, ever, and the split is the real-vs-simulated one the Scala build
//! expressed as two `ZLayer`s. P5 lands the simulated arm; P8 adds `Real`.

pub mod json;
pub mod simulated;
pub mod snapshot_writer;
pub mod stager;
pub mod verifier;

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
/// P5 defines only what P5 can raise, plus `Aborted`, which carries the exit-0
/// contract. P8 adds the xorriso and media variants with the code that raises
/// them.
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
