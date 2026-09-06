//! The staged scan (`StagedStore.scala`, scan half): `<work>/staged/scan.json`
//! is the hand-off from `scan` to `plan` and the recovery unit's first
//! artifact. Field spellings are the Scala zio-json codec's, byte-compatible.

use std::io::Write;
use std::path::{Path, PathBuf};

use diskrement_core::{RelPath, ScannedFile};
use serde::{Deserialize, Serialize};

use crate::catalog::time;

#[derive(Serialize, Deserialize)]
struct ScanJsonEntry {
    #[serde(rename = "relPath")]
    rel_path: String,
    #[serde(rename = "contentHash")]
    content_hash: String,
    #[serde(rename = "sizeBytes")]
    size_bytes: u64,
    mtime: String,
}

/// `<workDir>/staged/scan.json`.
pub fn scan_path(work_dir: &Path) -> PathBuf {
    work_dir.join("staged").join("scan.json")
}

/// Compact JSON, like zio-json's `.toJson`.
pub fn write_scan(work_dir: &Path, files: &[ScannedFile]) -> std::io::Result<()> {
    let entries: Vec<ScanJsonEntry> = files
        .iter()
        .map(|f| ScanJsonEntry {
            rel_path: f.rel_path.as_str().to_string(),
            content_hash: f.content_hash.as_str().to_string(),
            size_bytes: f.size_bytes,
            mtime: time::format(&f.mtime),
        })
        .collect();
    std::fs::create_dir_all(scan_path(work_dir).parent().expect("staged has a parent"))?;
    let mut out = std::fs::File::create(scan_path(work_dir))?;
    out.write_all(
        serde_json::to_string(&entries)
            .expect("serializable")
            .as_bytes(),
    )?;
    Ok(())
}

/// `Ok(None)` when nothing is staged; a decode failure is the caller's
/// defect (`corrupt scan.json: …` in the Scala `RuntimeException`).
pub fn read_scan(work_dir: &Path) -> Result<Option<Vec<ScannedFile>>, serde_json::Error> {
    let raw = match std::fs::read_to_string(scan_path(work_dir)) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(serde_json::Error::io(e)),
    };
    let entries: Vec<ScanJsonEntry> = serde_json::from_str(&raw)?;
    let mut files = Vec::with_capacity(entries.len());
    for e in entries {
        let mtime = time::parse(&e.mtime)
            .map_err(|err| <serde_json::Error as serde::de::Error>::custom(err.to_string()))?;
        files.push(ScannedFile {
            rel_path: RelPath(e.rel_path),
            content_hash: diskrement_core::ContentHash(e.content_hash),
            size_bytes: e.size_bytes,
            mtime,
        });
    }
    Ok(Some(files))
}

#[cfg(test)]
mod tests {
    use super::*;
    use diskrement_core::ContentHash;

    fn sf(rel: &str) -> ScannedFile {
        ScannedFile {
            rel_path: RelPath(rel.into()),
            content_hash: ContentHash("deadbeef".into()),
            size_bytes: 10,
            mtime: crate::catalog::time::parse("2026-06-01T00:00:00Z").unwrap(),
        }
    }

    #[test]
    fn round_trips_a_staged_scan() {
        let dir = tempfile::TempDir::new().unwrap();
        let files = vec![sf("a.txt"), sf("sub/b.txt")];
        write_scan(dir.path(), &files).unwrap();
        assert_eq!(read_scan(dir.path()).unwrap().unwrap(), files);
    }

    #[test]
    fn read_returns_none_when_nothing_is_staged() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(read_scan(dir.path()).unwrap().is_none());
    }

    #[test]
    fn a_corrupt_staged_scan_is_a_decode_error() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("staged")).unwrap();
        std::fs::write(dir.path().join("staged/scan.json"), "{not json").unwrap();
        let err = read_scan(dir.path()).unwrap_err();
        // The category (syntax/data/eof) depends on where the parse dies;
        // what matters is that it is a decode failure, not an I/O one.
        assert!(!err.is_io(), "{err}");
    }

    #[test]
    fn the_json_field_spellings_match_the_scala_codec() {
        let dir = tempfile::TempDir::new().unwrap();
        write_scan(dir.path(), &[sf("a.txt")]).unwrap();
        let raw = std::fs::read_to_string(dir.path().join("staged/scan.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let e = &v[0];
        assert_eq!(e["relPath"], "a.txt");
        assert_eq!(e["contentHash"], "deadbeef");
        assert_eq!(e["sizeBytes"], 10);
        assert_eq!(e["mtime"], "2026-06-01T00:00:00Z");
    }
}
