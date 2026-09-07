//! Writes one disc's `/.diskrement` directory (`SnapshotWriter.scala`).
//!
//! The catalog snapshot is *copied*, not re-derived: the pipeline has already
//! built it on a work-dir copy and applied this burn's update to it, so what
//! lands on the disc is the post-burn state and the disc is self-describing
//! (CLAUDE.md invariant).

use std::path::Path;

use crate::burn::json::{to_pretty, ManifestJson, PointersJson, ProjectJson};
use crate::burn::{BurnError, DiscMetadata};

pub fn write(diskrement_dir: &Path, meta: &DiscMetadata) -> Result<(), BurnError> {
    std::fs::create_dir_all(diskrement_dir).map_err(BurnError::Io)?;
    write_json(
        diskrement_dir,
        "project.json",
        &to_pretty(&ProjectJson::from(&meta.identity)),
    )?;
    write_json(
        diskrement_dir,
        "manifest.json",
        &to_pretty(&ManifestJson::from(&meta.manifest)),
    )?;
    write_json(
        diskrement_dir,
        "pointers.json",
        &to_pretty(&PointersJson::from(&meta.pointers)),
    )?;
    // `Files.copy(..., REPLACE_EXISTING)`: a disc can be re-staged (the
    // harvest stages disc 4 twice), so this must overwrite.
    std::fs::copy(
        &meta.catalog_snapshot,
        diskrement_dir.join("catalog.sqlite"),
    )
    .map_err(BurnError::Io)?;
    Ok(())
}

fn write_json(dir: &Path, name: &str, body: &str) -> Result<(), BurnError> {
    std::fs::write(dir.join(name), body.as_bytes()).map_err(BurnError::Io)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::burn::DiscIdentity;
    use chrono::TimeZone as _;
    use diskrement_core::SessionManifest;

    fn metadata(catalog_snapshot: std::path::PathBuf) -> DiscMetadata {
        DiscMetadata {
            identity: DiscIdentity {
                project_name: "p".to_string(),
                disc_ordinal: 2,
                session_ordinal: 1,
                created_at: chrono::Utc.timestamp_opt(1_757_100_066, 0).unwrap(),
            },
            manifest: SessionManifest {
                project_name: "p".to_string(),
                disc_ordinal: 2,
                session_ordinal: 1,
                added: Vec::new(),
                modified: Vec::new(),
                deleted: Vec::new(),
            },
            catalog_snapshot,
            pointers: Vec::new(),
        }
    }

    #[test]
    fn writes_all_four_artifacts() {
        let dir = tempfile::TempDir::new().unwrap();
        let snap = dir.path().join("burn-catalog.sqlite");
        std::fs::write(&snap, b"not really sqlite, but bytes are bytes").unwrap();
        let meta_dir = dir.path().join("staging/disc-2/.diskrement");

        write(&meta_dir, &metadata(snap.clone())).unwrap();

        assert!(meta_dir.join("project.json").is_file());
        assert!(meta_dir.join("manifest.json").is_file());
        assert!(meta_dir.join("pointers.json").is_file());
        assert_eq!(
            std::fs::read(meta_dir.join("catalog.sqlite")).unwrap(),
            std::fs::read(&snap).unwrap()
        );
    }

    #[test]
    fn the_json_it_writes_is_the_pretty_encoding_with_no_trailing_newline() {
        let dir = tempfile::TempDir::new().unwrap();
        let snap = dir.path().join("burn-catalog.sqlite");
        std::fs::write(&snap, b"x").unwrap();
        let meta_dir = dir.path().join(".diskrement");

        write(&meta_dir, &metadata(snap)).unwrap();

        let pointers = std::fs::read_to_string(meta_dir.join("pointers.json")).unwrap();
        assert_eq!(pointers, "{\n  \"pointers\" : []\n}");
        let project = std::fs::read_to_string(meta_dir.join("project.json")).unwrap();
        assert!(
            project.starts_with("{\n  \"projectName\" : \"p\","),
            "{project}"
        );
        assert!(!project.ends_with('\n'), "no trailing newline: {project:?}");
    }

    /// Re-staging a disc overwrites rather than failing: the harvest's disc 4
    /// is staged twice (recovery burn, then the seal burn).
    #[test]
    fn rewriting_an_existing_metadata_dir_replaces_it() {
        let dir = tempfile::TempDir::new().unwrap();
        let snap = dir.path().join("burn-catalog.sqlite");
        std::fs::write(&snap, b"first").unwrap();
        let meta_dir = dir.path().join(".diskrement");

        write(&meta_dir, &metadata(snap.clone())).unwrap();
        std::fs::write(&snap, b"second").unwrap();
        write(&meta_dir, &metadata(snap)).unwrap();

        assert_eq!(
            std::fs::read(meta_dir.join("catalog.sqlite")).unwrap(),
            b"second"
        );
    }

    #[test]
    fn a_missing_catalog_snapshot_is_a_burn_io_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let err = write(
            &dir.path().join(".diskrement"),
            &metadata(dir.path().join("nope.sqlite")),
        )
        .unwrap_err();
        assert!(matches!(err, BurnError::Io(_)), "{err:?}");
    }
}
