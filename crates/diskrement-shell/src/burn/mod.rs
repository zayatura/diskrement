//! Everything about putting one disc's worth of files somewhere durable:
//! the disc value types, the `/.diskrement` metadata, the stager, and the
//! burners (spec §5.3).
//!
//! `Burner` is a closed enum rather than a trait object: there are exactly two
//! backends, ever, and the split is the real-vs-simulated one the Scala build
//! expressed as two `ZLayer`s.

pub mod cmd;
pub mod json;
pub mod media;
pub mod reader;
pub mod simulated;
pub mod snapshot_writer;
pub mod stager;
pub mod verifier;
pub mod xorriso;

#[cfg(any(test, feature = "testutil"))]
pub mod testkit;

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use diskrement_core::{ContentHash, DiscKey, RelPath, ScannedFile, SessionManifest, MAX_COPIES};

use crate::catalog::model::ArchivePointer;

/// Who this disc is, as written to `/.diskrement/project.json`.
///
/// `copies` is the project's copy count, so a single disc can say "I am 1B of
/// 2" without the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscIdentity {
    pub project_name: String,
    pub disc_ordinal: u32,
    pub copy_set: u32,
    pub copies: u32,
    pub session_ordinal: u32,
    pub created_at: DateTime<Utc>,
}

impl DiscIdentity {
    pub fn disc(&self) -> DiscKey {
        DiscKey::new(self.disc_ordinal, self.copy_set)
    }

    /// `3` or `3B` — see `DiscKey::name`.
    ///
    /// Identities are also read off media, where a corrupt `project.json` can
    /// carry a copy set with no letter (`>= MAX_COPIES`). `DiscKey::name`
    /// panics on that; here it renders as `<ordinal>?<copy set>` (`3?26`), so
    /// the burner describes the wrong disc and re-prompts instead of dying.
    pub fn disc_name(&self) -> String {
        if self.copies > 1 && self.copy_set >= MAX_COPIES {
            return format!("{}?{}", self.disc_ordinal, self.copy_set);
        }
        self.disc().name(self.copies)
    }

    /// The same physical disc series member: project, ordinal and copy set.
    /// The session is deliberately not compared.
    pub fn same_disc(&self, other: &DiscIdentity) -> bool {
        self.project_name == other.project_name && self.disc() == other.disc()
    }
}

/// Scala's case-class `toString`, reproduced because it is interpolated into
/// `PostBurnCheckFailed`'s detail and into the verifier's re-prompt line
/// (spec §3.2: output wording is contract outside clap's argument errors).
/// At `copies = 1` it reads as it always has; otherwise the ordinal slot
/// carries the disc name (`1B`).
impl std::fmt::Display for DiscIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "DiscIdentity({},{},{},{})",
            self.project_name,
            self.disc_name(),
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

/// Operator interaction during real burns and verifies: insert-disc
/// confirmations and live progress lines. The CLI supplies a console
/// implementation; tests script one.
///
/// `&self` rather than `&mut self`: a scripted implementation mutates through
/// interior mutability, and the burner holds the operator immutably alongside
/// everything else it borrows.
pub trait BurnOperator {
    /// Show `message` and wait. `false` means abort (burn) or skip (verify).
    fn confirm_insert(&self, message: &str) -> bool;
    fn progress(&self, line: &str);
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
/// `ImageFile` is constructed only by the image-file integration tests.
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
    /// The media inspection (`-toc`) failed in a way `parse` does not
    /// classify, or printed nothing it could parse, in `burn` or `verify`.
    /// Worded exactly like `BurnFailed`, but in a burn it comes before any
    /// write, so a partial burn calls the disc "not written" (#17).
    #[error("xorriso failed (exit {exit_code}): {}", output_tail.join(" | "))]
    InspectFailed {
        exit_code: i32,
        output_tail: Vec<String>,
    },
    /// An I/O failure around the burn command itself (setting up its pipe,
    /// spawning it, reading its output, waiting for it), as opposed to `Io`
    /// before it. xorriso may
    /// already have written to the media, so a partial burn must not call
    /// this disc "not written".
    #[error("burn I/O error while xorriso was writing the disc: {0}")]
    WriteInterrupted(#[source] std::io::Error),
    #[error("post-burn sanity check failed: {0}")]
    PostBurnCheckFailed(String),
    #[error("could not mount the disc for verification: {0}")]
    MountFailed(String),
    #[error("aborted by operator")]
    Aborted,
}

/// The burn backends. `--burner real|simulated` selects between them, and the
/// verifier arm moves with it (spec §5.2).
pub enum Burner<'a> {
    Simulated(simulated::SimulatedBurner<'a>),
    Real(xorriso::burner::XorrisoBurner<'a>),
}

impl Burner<'_> {
    pub fn burn(&self, image: &DiscImage) -> Result<BurnResult, BurnError> {
        match self {
            Burner::Simulated(b) => b.burn(image),
            Burner::Real(b) => b.burn(image),
        }
    }

    /// Eject the disc once the pipeline is done with it: after the commit,
    /// or after the read-back under `burn --verify`. Best-effort; the
    /// simulated backend and an image file have no tray.
    pub fn eject(&self) {
        if let Burner::Real(b) = self {
            b.eject();
        }
    }

    /// Whether a disc's staging tree must outlive its commit. The simulated
    /// backend's tree *is* the disc `StagingVerifier` reads back; a real disc
    /// carries its own record, so its tree is only scratch space, and a kept
    /// tree pins source extents the source has since deleted (issue #3).
    pub fn retains_staging(&self) -> bool {
        matches!(self, Burner::Simulated(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(ordinal: u32, copy_set: u32, copies: u32) -> DiscIdentity {
        DiscIdentity {
            project_name: "photos".to_string(),
            disc_ordinal: ordinal,
            copy_set,
            copies,
            session_ordinal: 2,
            created_at: crate::catalog::time::parse("2026-08-30T10:00:00Z").unwrap(),
        }
    }

    #[test]
    fn twins_are_different_discs() {
        assert!(!id(1, 0, 2).same_disc(&id(1, 1, 2)));
        assert!(id(1, 1, 2).same_disc(&id(1, 1, 2)));
    }

    /// The simulated tree is the disc `StagingVerifier` reads back; a real
    /// disc carries its own record, so its tree is scratch space.
    #[test]
    fn only_the_simulated_backend_retains_staging() {
        use crate::burn::cmd::CmdOutput;
        use crate::burn::testkit::ScriptedOperator;
        use crate::progress::NoopSink;

        let exec = |_: &[String], _: &mut dyn FnMut(&str)| -> Result<CmdOutput, BurnError> {
            unreachable!("asking the policy runs nothing")
        };
        let op = ScriptedOperator::new(&[]);
        let real = Burner::Real(xorriso::burner::XorrisoBurner::new(
            BurnDevice::RealDevice("/dev/sr0".to_string()),
            &exec,
            &op,
            &NoopSink,
            1,
        ));
        let simulated = Burner::Simulated(simulated::SimulatedBurner::new(&NoopSink, 1));

        assert!(simulated.retains_staging());
        assert!(!real.retains_staging());
    }

    /// Only the real backend has a tray to open.
    #[test]
    fn only_the_real_backend_ejects() {
        use crate::burn::testkit::{ScriptedExec, ScriptedOperator};
        use crate::progress::NoopSink;

        let script = ScriptedExec::new();
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
        let op = ScriptedOperator::new(&[]);
        Burner::Simulated(simulated::SimulatedBurner::new(&NoopSink, 1)).eject();
        assert!(!script.ran("-eject"));
        Burner::Real(xorriso::burner::XorrisoBurner::new(
            BurnDevice::RealDevice("/dev/sr0".to_string()),
            &exec,
            &op,
            &NoopSink,
            1,
        ))
        .eject();
        assert!(script.ran("-eject"));
    }

    /// A corrupt `project.json` read off media must not panic the burner:
    /// it is just the wrong disc, to be described and re-prompted.
    #[test]
    fn a_copy_set_without_a_letter_still_has_a_name() {
        assert_eq!(id(3, 26, 2).disc_name(), "3?26");
        assert_eq!(id(3, u32::MAX, 2).disc_name(), format!("3?{}", u32::MAX));
        assert_eq!(
            id(1, 30, 2).to_string(),
            "DiscIdentity(photos,1?30,2,2026-08-30T10:00:00Z)"
        );
    }

    #[test]
    fn the_identity_displays_with_the_disc_name() {
        assert_eq!(
            id(1, 0, 1).to_string(),
            "DiscIdentity(photos,1,2,2026-08-30T10:00:00Z)"
        );
        assert_eq!(
            id(1, 1, 2).to_string(),
            "DiscIdentity(photos,1B,2,2026-08-30T10:00:00Z)"
        );
    }
}
