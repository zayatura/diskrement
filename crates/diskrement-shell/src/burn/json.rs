//! The `/.diskrement` JSON DTOs (`Json.scala`) and the manifest hash.
//!
//! Two encodings live here and they are not interchangeable. The **compact**
//! encoding is hashed into `session.manifest_hash`, so its field order and its
//! whitespace are part of the catalog's content; the **pretty** encoding is
//! what lands on the disc, and it reproduces zio-json's `toJsonPretty` exactly
//! so the corpus's `manifest.json` and `pointers.json` stay byte oracles.

use std::io;

use chrono::{DateTime, Utc};
use diskrement_core::{ContentHash, ManifestEntry, RelPath, SessionManifest};
use serde::{Deserialize, Serialize};
use serde_json::ser::{Formatter, PrettyFormatter};
use sha2::{Digest, Sha256};

use crate::burn::DiscIdentity;
use crate::catalog::model::ArchivePointer;
use crate::catalog::time;

// --- the pretty printer -------------------------------------------------

/// zio-json's `toJsonPretty`: serde_json's pretty printer with `" : "` where
/// it writes `": "`. Every other method delegates, so indentation, array
/// layout and the empty-collection shapes stay serde_json's — which already
/// agree with zio-json's (`[]`, `{}`).
struct ZioPretty<'a> {
    inner: PrettyFormatter<'a>,
}

impl ZioPretty<'_> {
    fn new() -> Self {
        ZioPretty {
            inner: PrettyFormatter::with_indent(b"  "),
        }
    }
}

impl Formatter for ZioPretty<'_> {
    fn begin_object_value<W>(&mut self, w: &mut W) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        w.write_all(b" : ")
    }

    fn begin_array<W>(&mut self, w: &mut W) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        self.inner.begin_array(w)
    }
    fn end_array<W>(&mut self, w: &mut W) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        self.inner.end_array(w)
    }
    fn begin_array_value<W>(&mut self, w: &mut W, first: bool) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        self.inner.begin_array_value(w, first)
    }
    fn end_array_value<W>(&mut self, w: &mut W) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        self.inner.end_array_value(w)
    }
    fn begin_object<W>(&mut self, w: &mut W) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        self.inner.begin_object(w)
    }
    fn end_object<W>(&mut self, w: &mut W) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        self.inner.end_object(w)
    }
    fn begin_object_key<W>(&mut self, w: &mut W, first: bool) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        self.inner.begin_object_key(w, first)
    }
    fn end_object_value<W>(&mut self, w: &mut W) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        self.inner.end_object_value(w)
    }
}

/// Render `value` the way zio-json's `toJsonPretty` would. No trailing newline.
pub fn to_pretty<T: Serialize>(value: &T) -> String {
    let mut buf = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, ZioPretty::new());
    value
        .serialize(&mut ser)
        .expect("these DTOs are plain data and cannot fail to serialize");
    String::from_utf8(buf).expect("serde_json emits UTF-8")
}

// --- project.json -------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectJson {
    pub project_name: String,
    pub disc_ordinal: u32,
    pub session_ordinal: u32,
    pub created_at: String,
}

impl ProjectJson {
    pub fn from(id: &DiscIdentity) -> Self {
        ProjectJson {
            project_name: id.project_name.clone(),
            disc_ordinal: id.disc_ordinal,
            session_ordinal: id.session_ordinal,
            created_at: time::format(&id.created_at),
        }
    }

    pub fn to_identity(&self) -> Result<DiscIdentity, String> {
        let created_at: DateTime<Utc> = time::parse(&self.created_at)
            .map_err(|e| format!("bad createdAt in project.json: {e}"))?;
        Ok(DiscIdentity {
            project_name: self.project_name.clone(),
            disc_ordinal: self.disc_ordinal,
            session_ordinal: self.session_ordinal,
            created_at,
        })
    }
}

// --- manifest.json ------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestEntryJson {
    pub rel_path: String,
    pub content_hash: String,
    pub size_bytes: u64,
}

impl ManifestEntryJson {
    fn from(e: &ManifestEntry) -> Self {
        ManifestEntryJson {
            rel_path: e.rel_path.0.clone(),
            content_hash: e.content_hash.0.clone(),
            size_bytes: e.size_bytes,
        }
    }

    fn to_entry(&self) -> ManifestEntry {
        ManifestEntry {
            rel_path: RelPath(self.rel_path.clone()),
            content_hash: ContentHash(self.content_hash.clone()),
            size_bytes: self.size_bytes,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestJson {
    pub project_name: String,
    pub disc_ordinal: u32,
    pub session_ordinal: u32,
    pub added: Vec<ManifestEntryJson>,
    pub modified: Vec<ManifestEntryJson>,
    pub deleted: Vec<String>,
}

impl ManifestJson {
    pub fn from(m: &SessionManifest) -> Self {
        ManifestJson {
            project_name: m.project_name.clone(),
            disc_ordinal: m.disc_ordinal,
            session_ordinal: m.session_ordinal,
            added: m.added.iter().map(ManifestEntryJson::from).collect(),
            modified: m.modified.iter().map(ManifestEntryJson::from).collect(),
            deleted: m.deleted.iter().map(|p| p.0.clone()).collect(),
        }
    }

    /// The inverse, for the corpus oracle now and `DiscReader` at P8.
    pub fn to_manifest(&self) -> Result<SessionManifest, String> {
        Ok(SessionManifest {
            project_name: self.project_name.clone(),
            disc_ordinal: self.disc_ordinal,
            session_ordinal: self.session_ordinal,
            added: self.added.iter().map(ManifestEntryJson::to_entry).collect(),
            modified: self
                .modified
                .iter()
                .map(ManifestEntryJson::to_entry)
                .collect(),
            deleted: self.deleted.iter().map(|p| RelPath(p.clone())).collect(),
        })
    }
}

/// `sha256` of the **compact** manifest encoding (`PipelineLive.manifestHash`),
/// lowercase hex. Stored as `session.manifest_hash`, so this function's output
/// is catalog content and its input encoding is frozen.
pub fn manifest_hash(m: &SessionManifest) -> String {
    let json = serde_json::to_string(&ManifestJson::from(m))
        .expect("the manifest DTO is plain data and cannot fail to serialize");
    format!("{:x}", Sha256::digest(json.as_bytes()))
}

// --- pointers.json ------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchivePointerJson {
    pub rel_path: String,
    pub content_hash: String,
    pub size_bytes: u64,
    pub disc_ordinal: u32,
    pub session_ordinal: u32,
    pub disc_path: String,
    pub kind: String,
}

impl ArchivePointerJson {
    fn from(p: &ArchivePointer) -> Self {
        ArchivePointerJson {
            rel_path: p.rel_path.0.clone(),
            content_hash: p.content_hash.0.clone(),
            size_bytes: p.size_bytes,
            disc_ordinal: p.disc_ordinal,
            session_ordinal: p.session_ordinal,
            disc_path: p.disc_path.clone(),
            // `ManifestChangeKind.toString` — the frozen uppercase spelling
            // (spec §3.1). Nothing but this test catches a serde mistake here.
            kind: p.kind.as_str().to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PointersJson {
    pub pointers: Vec<ArchivePointerJson>,
}

impl PointersJson {
    pub fn from(ps: &[ArchivePointer]) -> Self {
        PointersJson {
            pointers: ps.iter().map(ArchivePointerJson::from).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone as _;
    use diskrement_core::{ContentHash, ManifestChangeKind, ManifestEntry, RelPath};

    fn manifest() -> SessionManifest {
        SessionManifest {
            project_name: "p".to_string(),
            disc_ordinal: 3,
            session_ordinal: 1,
            added: Vec::new(),
            modified: vec![ManifestEntry {
                rel_path: RelPath("a.bin".to_string()),
                content_hash: ContentHash(
                    "e148cf0e8c66a7c18ea6be52a5215dd2a770d74b6c014d51d1e1e0463e882e77".to_string(),
                ),
                size_bytes: 800,
            }],
            deleted: vec![RelPath("b.bin".to_string())],
        }
    }

    /// Byte-for-byte `fixtures/discs/disc-3/.diskrement/manifest.json`, which
    /// is zio-json `toJsonPretty` output: two-space indent, `" : "` separator,
    /// `[]` for an empty array, no trailing newline.
    #[test]
    fn pretty_output_matches_zio_json() {
        let expected = "{\n  \"projectName\" : \"p\",\n  \"discOrdinal\" : 3,\n  \
             \"sessionOrdinal\" : 1,\n  \"added\" : [],\n  \"modified\" : [\n    {\n      \
             \"relPath\" : \"a.bin\",\n      \"contentHash\" : \
             \"e148cf0e8c66a7c18ea6be52a5215dd2a770d74b6c014d51d1e1e0463e882e77\",\n      \
             \"sizeBytes\" : 800\n    }\n  ],\n  \"deleted\" : [\n    \"b.bin\"\n  ]\n}";
        assert_eq!(to_pretty(&ManifestJson::from(&manifest())), expected);
    }

    /// The compact encoding is what the manifest hash is taken over, so its
    /// field order and its lack of whitespace are both contract.
    #[test]
    fn compact_output_has_scala_field_order_and_no_whitespace() {
        let compact = serde_json::to_string(&ManifestJson::from(&manifest())).unwrap();
        assert_eq!(
            compact,
            "{\"projectName\":\"p\",\"discOrdinal\":3,\"sessionOrdinal\":1,\
             \"added\":[],\"modified\":[{\"relPath\":\"a.bin\",\"contentHash\":\
             \"e148cf0e8c66a7c18ea6be52a5215dd2a770d74b6c014d51d1e1e0463e882e77\",\
             \"sizeBytes\":800}],\"deleted\":[\"b.bin\"]}"
        );
    }

    #[test]
    fn the_manifest_hash_is_the_sha256_of_the_compact_encoding() {
        let compact = serde_json::to_string(&ManifestJson::from(&manifest())).unwrap();
        let mut d = sha2::Sha256::new();
        sha2::Digest::update(&mut d, compact.as_bytes());
        assert_eq!(manifest_hash(&manifest()), format!("{:x}", d.finalize()));
    }

    #[test]
    fn project_json_round_trips_through_the_frozen_timestamp_format() {
        let id = DiscIdentity {
            project_name: "p".to_string(),
            disc_ordinal: 1,
            session_ordinal: 1,
            created_at: Utc.timestamp_opt(1_757_100_066, 623_959_846).unwrap(),
        };
        let rendered = to_pretty(&ProjectJson::from(&id));
        assert!(
            rendered.contains("\"createdAt\" : \"2025-09-05T19:21:06.623959846Z\""),
            "{rendered}"
        );
        let parsed: ProjectJson = serde_json::from_str(&rendered).unwrap();
        assert_eq!(parsed.to_identity().unwrap(), id);
    }

    #[test]
    fn a_bad_created_at_is_reported_rather_than_panicking() {
        let p = ProjectJson {
            project_name: "p".to_string(),
            disc_ordinal: 1,
            session_ordinal: 1,
            created_at: "not a timestamp".to_string(),
        };
        let err = p.to_identity().unwrap_err();
        assert!(err.starts_with("bad createdAt in project.json:"), "{err}");
    }

    #[test]
    fn a_manifest_json_converts_back_to_its_manifest() {
        let m = manifest();
        assert_eq!(ManifestJson::from(&m).to_manifest().unwrap(), m);
    }

    #[test]
    fn pointer_kinds_render_as_the_frozen_uppercase_names() {
        let p = ArchivePointer {
            rel_path: RelPath("a.bin".to_string()),
            content_hash: ContentHash("aa".to_string()),
            size_bytes: 4,
            disc_ordinal: 1,
            session_ordinal: 2,
            disc_path: "/data/a.bin".to_string(),
            kind: ManifestChangeKind::Deleted,
        };
        let rendered = to_pretty(&PointersJson::from(std::slice::from_ref(&p)));
        assert!(rendered.contains("\"kind\" : \"DELETED\""), "{rendered}");
        assert!(
            rendered.contains("\"discPath\" : \"/data/a.bin\""),
            "{rendered}"
        );
    }

    #[test]
    fn an_empty_pointer_list_renders_as_the_fixture_shape() {
        assert_eq!(
            to_pretty(&PointersJson::from(&[][..])),
            "{\n  \"pointers\" : []\n}"
        );
    }
}
