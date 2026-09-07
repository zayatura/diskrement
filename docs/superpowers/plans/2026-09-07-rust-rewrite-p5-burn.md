# Rust Rewrite P5 — the stager, the disc metadata, and the simulated `burn` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land P5 — the disc stager, the `/.diskrement` metadata writer
(`project.json` / `manifest.json` / `pointers.json` / the embedded
`catalog.sqlite`), the simulated burner, and `Pipeline::burn` with its
commit-after-success ordering — ending with a Rust `burn` whose staged disc
trees are byte-identical to the harvested corpus's and whose committed catalog
is row-for-row identical to what the Scala build wrote at the same moment.

**Architecture:** Spec §5.3/§5.5. A new `burn` module holds the disc value types
(`DiscIdentity`, `DiscMetadata`, `DiscImage`, `BurnResult`), `BurnError`, the
JSON DTOs, the snapshot writer, the stager, and a one-variant closed
`Burner` enum whose second arm P8 adds. `Pipeline::burn` is the thin edge: read
the staged scan and plan, run the guards, build the `CatalogUpdate`, snapshot
the catalog and apply the update **to the copy**, stage the disc, and only then
commit to the live catalog. The stager copies in-process (reflink with a
plain-copy fallback), not through `cp`.

**Tech Stack:** Rust 2021; `serde`, `serde_json`, `rusqlite` (bundled), `sha2`,
`rayon`, `reflink-copy`, `walkdir`, `chrono`, `clap` 4 derive, `tempfile`,
`assert_cmd`, `predicates` — all already workspace dependencies. **No new
dependencies.**

**Spec:** `docs/superpowers/specs/2026-09-05-rust-rewrite-design.md` — §3.1 (the
on-disc `/.diskrement` layout and the catalog schema are frozen formats), §3.2
(CLI surface and exit codes), §4 (the corpus and the render oracle), §5.3
(layout), §5.5 (pipeline/errors/burn subsystems), §8.1 (the differential task
every phase from P3 carries), §9 (the P5 row). The Scala sources are the
behavioral oracle: **read the file named in each task before porting it.**

---

## Global Constraints

- **Branch:** `feat/rewrite-p5`, forked from `feat/rust-rewrite`. Never commit
  directly to `feat/rust-rewrite`. Merge back at the end with an explicit
  `Merge feat/rewrite-p5: <summary>` commit and delete the phase branch.

```bash
git switch feat/rust-rewrite && git switch -c feat/rewrite-p5
```

- **Do not touch** `core/`, `shell/`, `cli/`, `build.sbt`, `project/`,
  `fixtures/`, `scripts/harvest-fixtures.sh`. The Scala tree must stay
  buildable until P10, and the corpus is a frozen, spec-level artifact. Tests
  read `fixtures/` and **copy before opening** — never open a fixture database
  through `Catalog::open` (it migrates and writes) and never write inside
  `fixtures/`.
- **After every task:**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

  All clean, with no drop in test count. Baseline at plan start:
  **278 passed, 3 ignored** (the three `#[ignore]`d differential entry points).
- **Commit scopes:** `feat(burn):`, `test(burn):`, `feat(pipeline):`,
  `feat(cli):`, `feat(difftest):`.
- **User-visible strings and exit codes are contract** (§3.2). Copy them
  verbatim from the Scala source named in each task; the only sanctioned
  divergence is clap's own usage/argument-error wording.
- **Time formatting only through `diskrement_shell::catalog::time`**
  (`format`/`parse`). Never `chrono`'s serde or `Display` for stored values.
- **Commit after success.** `burn` writes the disc tree first and commits to the
  live catalog only after the burner returns. No `PLANNED` rows are ever
  persisted; the staged plan in the work dir is the recovery unit, and a crash
  before the commit leaves the catalog untouched (CLAUDE.md invariant).
- **The embedded `catalog.sqlite` reflects post-burn state.** It is built by
  `snapshot_to` onto a work-dir copy and then `apply_update_to` **that copy**,
  so each disc is self-describing. Never file-swap a SQLite file under a live
  connection; the double-apply reaches the same end state through the proven
  path (CLAUDE.md invariant — the earlier pre-burn approach was deliberately
  reversed).
- **One disc per cycle.** Each scan→plan→burn writes exactly one disc/session.
- **The catalog is append-only.** `burn` adds rows; it never updates or deletes.
- **`ioctl(FICLONE)` does not preserve mtime** (CLAUDE.md). Every file the
  stager clones must have its mtime and permissions restored afterwards, as
  `fs::snapshot::reflink_tree` already does.
- **Appending `burn` to `runner::RUST_IMPLEMENTED` happens in the same commit
  that lands it in the binary** (Task 13). Otherwise the Rust and interleaved
  differential modes keep skipping every scenario that uses it, and a
  fully-skipped run still exits green.

---

## Scala oracle files (read before porting each area)

| Area | File |
|---|---|
| Disc value types, `BurnError` | `shell/src/main/scala/diskrement/shell/burn/Burn.scala` |
| JSON DTOs | `shell/src/main/scala/diskrement/shell/burn/Json.scala` |
| `/.diskrement` writer | `shell/src/main/scala/diskrement/shell/burn/SnapshotWriter.scala` |
| Stager | `shell/src/main/scala/diskrement/shell/burn/Stager.scala` |
| Simulated burner | `shell/src/main/scala/diskrement/shell/burn/SimulatedBurner.scala` |
| `burn` orchestration, `toDiscWrite`, `manifestHash` | `shell/src/main/scala/diskrement/shell/pipeline/PipelineLive.scala` (`def burn`, `existingSessions`, `toDiscWrite`, `manifestHash`) |
| Pipeline error ADT | `shell/src/main/scala/diskrement/shell/pipeline/PipelineError.scala` |
| Burn rendering, exit ladder | `cli/src/main/scala/diskrement/cli/Render.scala` (`def burn`, `def handle`) |
| Confirmation prompt | `cli/src/main/scala/diskrement/cli/Prompt.scala` |
| Backend resolution | `cli/src/main/scala/diskrement/cli/BackendResolver.scala` |
| `burn` command wiring | `cli/src/main/scala/diskrement/cli/Commands.scala`, `Runner.scala` (the `DiskrementCommand.Burn` case) |

## Corpus oracles used by this phase

| Fixture | Used by | What it pins |
|---|---|---|
| `fixtures/discs/disc-{1,2,3,4}/.diskrement/manifest.json` | Task 3 | the four manifest shapes and, through the hash, the compact JSON byte encoding |
| `fixtures/catalog/11-after-finalize.sqlite` | Task 3 | every session's `manifest_hash` as the Scala build computed it |
| `fixtures/catalog/10-after-burn4.sqlite` + `fixtures/discs/disc-4/.diskrement/pointers.json` | Task 4 | the non-empty pointer rendering, byte for byte |
| `fixtures/work/01-after-plan1` + `fixtures/catalog/02-after-scan1.sqlite` → `fixtures/discs/disc-1`, `fixtures/catalog/03-after-burn1.sqlite` | Task 9 | burn cycle 1 end to end: disc tree bytes and committed rows |
| `fixtures/work/02-after-plan3` + `fixtures/catalog/06-after-scan3.sqlite` → `fixtures/discs/disc-3`, `fixtures/catalog/07-after-burn3.sqlite` | Task 9 | burn cycle 3: the modify+delete manifest and its tombstone rows |
| `fixtures/run-manifest.txt` steps 04/08/11/15/19 | Task 10 | the `burn` stdout wording |

**Two facts about the corpus that this phase depends on — do not re-derive:**

1. **Pointers are one cycle behind.** `archivePointers` is read from the *live*
   catalog before the update commits, so `discs/disc-{1,2,3}/.diskrement/pointers.json`
   are all `{"pointers" : []}` and only `disc-4`'s (the seal burn, read at the
   `10-after-burn4` state) is non-empty. That is why Task 4 exists as a separate
   oracle rather than being folded into Task 9.
2. **`discs/disc-N` is the last session staged for that disc, not the cumulative
   disc.** `disc-4/data/` is empty because the final `plan --finalize` + burn
   re-staged disc 4 with a file-less seal session (`sessionOrdinal` 2, empty
   lists). `fixtures/README.md` says so; believe it rather than "fixing" it.

---

## File Structure

**Created:**

| Path | Responsibility |
|---|---|
| `crates/diskrement-shell/src/burn/mod.rs` | disc value types, `BurnError`, the `Burner` enum, module wiring |
| `crates/diskrement-shell/src/burn/json.rs` | the four on-disc DTOs, the zio-json-compatible pretty printer, `manifest_hash` |
| `crates/diskrement-shell/src/burn/snapshot_writer.rs` | writes one disc's `/.diskrement` directory |
| `crates/diskrement-shell/src/burn/stager.rs` | stages `data/` + re-hashes, the integrity gate |
| `crates/diskrement-shell/src/burn/simulated.rs` | the stage-to-directory burner |
| `crates/diskrement-shell/tests/fixture_manifest_hash.rs` | manifest-hash corpus oracle |
| `crates/diskrement-shell/tests/fixture_pointers.rs` | pointer-rendering corpus oracle |
| `crates/diskrement-shell/tests/fixture_burn.rs` | the phase exit bar: replayed burn vs corpus |
| `crates/diskrement-cli/src/prompt.rs` | the `[y/N]` confirmation |
| `crates/diskrement-cli/src/backend.rs` | pure burner/device resolution |

**Modified:**

| Path | Change |
|---|---|
| `crates/diskrement-shell/src/lib.rs` | `pub mod burn;` |
| `crates/diskrement-shell/src/error.rs` | `PipelineError::SnapshotMissing`, `PipelineError::Burn`, exit mapping |
| `crates/diskrement-shell/src/pipeline/reports.rs` | `DiscBurn`, `BurnReport` |
| `crates/diskrement-shell/src/pipeline/mod.rs` | `Pipeline::burn` |
| `crates/diskrement-shell/tests/support/mod.rs` | `copy_tree`, `fixture_text` helpers |
| `crates/diskrement-cli/src/args.rs` | `burner: Option<BurnerKind>`, the `Burn` subcommand |
| `crates/diskrement-cli/src/render.rs` | `burn_report` |
| `crates/diskrement-cli/src/main.rs` | module decls, the `Burn` dispatch arm, backend refusal |
| `crates/diskrement-cli/tests/binary.rs` | the `p5` end-to-end module |
| `crates/diskrement-difftest/src/runner.rs` | `RUST_IMPLEMENTED` += `"burn"` |
| `crates/diskrement-difftest/src/corpus.rs` | burn scenarios |
| `crates/diskrement-difftest/src/state_diff.rs` | embedded `catalog.sqlite` comparison |

---

### Task 1: Burn value types and the burn error contract

**Files:**
- Create: `crates/diskrement-shell/src/burn/mod.rs`
- Modify: `crates/diskrement-shell/src/lib.rs`
- Modify: `crates/diskrement-shell/src/error.rs`

Read `shell/src/main/scala/diskrement/shell/burn/Burn.scala` and
`shell/src/main/scala/diskrement/shell/pipeline/PipelineError.scala` first, then
`cli/src/main/scala/diskrement/cli/Render.scala`'s `def handle`.

`BurnError` gets **only** the variants P5 can raise (`HashMismatch`,
`SourceFileMissing`, `Io`) plus `Aborted`, which is in early because it carries
the exit-0 half of the contract. The eleven xorriso/media variants
(`ToolMissing`, `NotBlank`, `NoMedia`, `WrongDisc`, …) land in P8 alongside the
code that raises them; porting them now would be eleven variants nothing can
construct.

Note that `PipelineError.Burn`'s rendered message is `cause.getMessage` — so
`Burn` is `#[error(transparent)]`. And note that `BurnIoError`'s Scala message
is the bare string `burn I/O error`, with the cause attached but **not**
concatenated: the user really does see only that. Port it verbatim; do not
"improve" it by interpolating the cause.

- [ ] **Step 1: Write the failing tests**

Append to `crates/diskrement-shell/src/error.rs`'s `mod tests`:

```rust
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
        assert_eq!(PipelineError::SnapshotMissing("photos".into()).exit_code(), 2);
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p diskrement-shell --lib error::`
Expected: FAIL — `error[E0433]: failed to resolve: could not find 'burn' in the crate root`.

- [ ] **Step 3: Create the burn module**

Create `crates/diskrement-shell/src/burn/mod.rs`:

```rust
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
```

`RelPath` and `ContentHash` must render bare in those messages. Check
`crates/diskrement-core/src/model.rs`: if either lacks a `Display` impl that
writes only the inner string, add one there in this task — `{rel_path}` must
produce `a.bin`, never `RelPath("a.bin")`.

- [ ] **Step 4: Create the three placeholder submodules**

The module tree must compile before Tasks 2/5/6/7 fill it. Create each with its
doc comment only:

`crates/diskrement-shell/src/burn/json.rs`:

```rust
//! The `/.diskrement` JSON DTOs (`Json.scala`) and the manifest hash.
```

`crates/diskrement-shell/src/burn/snapshot_writer.rs`:

```rust
//! Writes one disc's `/.diskrement` directory (`SnapshotWriter.scala`).
```

`crates/diskrement-shell/src/burn/stager.rs`:

```rust
//! Stages one disc tree and re-hashes it (`Stager.scala`).
```

`crates/diskrement-shell/src/burn/simulated.rs`:

```rust
//! The stage-to-directory burner (`SimulatedBurner.scala`).
```

The `Burner` enum in `mod.rs` references `simulated::SimulatedBurner`, so
`simulated.rs` needs the type now. Put this in it instead of the bare comment:

```rust
//! The stage-to-directory burner (`SimulatedBurner.scala`): the disc "image"
//! is a work-dir tree. Kept as the test/dev backend; selected with
//! `--burner simulated` / `DISKREMENT_BURNER=simulated`.

use crate::burn::{stager, BurnError, BurnResult, DiscImage};
use crate::progress::ProgressSink;

pub struct SimulatedBurner<'a> {
    progress: &'a dyn ProgressSink,
    parallelism: usize,
}

impl<'a> SimulatedBurner<'a> {
    pub fn new(progress: &'a dyn ProgressSink, parallelism: usize) -> Self {
        SimulatedBurner {
            progress,
            parallelism,
        }
    }

    pub fn burn(&self, image: &DiscImage) -> Result<BurnResult, BurnError> {
        stager::stage(image, self.progress, self.parallelism)
    }
}
```

And give `stager.rs` a signature stub so the crate links (Task 6 implements it):

```rust
//! Stages one disc tree and re-hashes it (`Stager.scala`).

use crate::burn::{BurnError, BurnResult, DiscImage};
use crate::progress::ProgressSink;

pub fn stage(
    _image: &DiscImage,
    _progress: &dyn ProgressSink,
    _parallelism: usize,
) -> Result<BurnResult, BurnError> {
    unimplemented!("Task 6")
}
```

- [ ] **Step 5: Wire the module and extend `PipelineError`**

In `crates/diskrement-shell/src/lib.rs`, add `pub mod burn;` in alphabetical
position (before `pub mod catalog;`).

In `crates/diskrement-shell/src/error.rs`, add to `PipelineError`:

```rust
    #[error("source snapshot missing for {0} (run scan)")]
    SnapshotMissing(String),
    /// Renders its cause's message and nothing else (`PipelineError.Burn`).
    #[error(transparent)]
    Burn(#[from] crate::burn::BurnError),
```

and extend `exit_code`:

```rust
            PipelineError::ProjectNotFound(_)
            | PipelineError::NoStagedScan(_)
            | PipelineError::NoStagedPlan(_)
            | PipelineError::SnapshotMissing(_)
            | PipelineError::StalePlan(_) => 2,
            PipelineError::NotSameFilesystem { .. }
            | PipelineError::NotCowCapable { .. }
            | PipelineError::OversizedFiles(_) => 3,
            // An aborted burn is a successful run that wrote nothing; every
            // other burn failure is 6 (media/burner).
            PipelineError::Burn(crate::burn::BurnError::Aborted) => 0,
            PipelineError::Burn(_) => 6,
            PipelineError::Failed(_) => 5,
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p diskrement-shell --lib error::`
Expected: PASS, including the pre-existing `ExitMappingSpec` port tests.

- [ ] **Step 7: Verify and commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/diskrement-shell/src/burn crates/diskrement-shell/src/lib.rs crates/diskrement-shell/src/error.rs crates/diskrement-core/src/model.rs
git commit -m "feat(burn): disc value types and the burn error contract"
```

---

### Task 2: The JSON DTOs, the zio-json pretty printer, and the manifest hash

**Files:**
- Modify: `crates/diskrement-shell/src/burn/json.rs`

Read `shell/src/main/scala/diskrement/shell/burn/Json.scala` and
`PipelineLive.manifestHash` first.

Two encodings, and the difference matters:

- **Compact** feeds `sha256` → `session.manifest_hash`, a value stored in every
  catalog session row. It is a **byte-level** dependency: a different field
  order or a space after a colon changes the hash, and the catalog then differs
  from what the Scala build would have written. Task 3 pins it against the
  corpus.
- **Pretty** is what lands on the disc. zio-json's `toJsonPretty` writes a
  two-space indent and `" : "` between key and value, where serde_json writes
  `": "`, and emits no trailing newline. `fixtures/README.md` records
  `manifest.json` and `pointers.json` as byte-stable, so matching the format
  exactly turns them into byte oracles rather than merely semantic ones.

- [ ] **Step 1: Write the failing tests**

Append to `crates/diskrement-shell/src/burn/json.rs`:

```rust
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
            rendered.contains("\"createdAt\" : \"2026-09-05T20:41:06.623959846Z\""),
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
        assert!(rendered.contains("\"discPath\" : \"/data/a.bin\""), "{rendered}");
    }

    #[test]
    fn an_empty_pointer_list_renders_as_the_fixture_shape() {
        assert_eq!(
            to_pretty(&PointersJson::from(&[][..])),
            "{\n  \"pointers\" : []\n}"
        );
    }
}
```

The `2026-09-05T20:41:06.623959846Z` literal above is the value of
`Utc.timestamp_opt(1_757_100_066, 623_959_846)`. If it does not match, fix the
**expected string** to whatever that instant formats to through
`catalog::time::format` — the point of the test is the field name, the
`" : "` separator and the round trip, not that particular second.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p diskrement-shell --lib burn::json`
Expected: FAIL — `cannot find type 'ManifestJson' in this scope`.

- [ ] **Step 3: Implement the DTOs, the formatter and the hash**

Replace the contents of `crates/diskrement-shell/src/burn/json.rs` (keeping the
test module at the bottom):

```rust
//! The `/.diskrement` JSON DTOs (`Json.scala`) and the manifest hash.
//!
//! Two encodings live here and they are not interchangeable. The **compact**
//! encoding is hashed into `session.manifest_hash`, so its field order and its
//! whitespace are part of the catalog's content; the **pretty** encoding is
//! what lands on the disc, and it reproduces zio-json's `toJsonPretty` exactly
//! so the corpus's `manifest.json` and `pointers.json` stay byte oracles.

use std::io;

use chrono::{DateTime, Utc};
use diskrement_core::{
    ContentHash, ManifestChangeKind, ManifestEntry, RelPath, SessionManifest,
};
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
```

If `ManifestChangeKind` is not already imported where the test needs it, adjust
the imports; the `unused_imports` lint is denied.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p diskrement-shell --lib burn::json`
Expected: PASS, 8 tests.

- [ ] **Step 5: Verify and commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/diskrement-shell/src/burn/json.rs
git commit -m "feat(burn): the disc metadata DTOs, zio-json pretty output, and the manifest hash"
```

---

### Task 3: The manifest-hash corpus oracle

**Files:**
- Create: `crates/diskrement-shell/tests/fixture_manifest_hash.rs`
- Modify: `crates/diskrement-shell/tests/support/mod.rs`

`session.manifest_hash` is a value the Scala build computed and stored in every
catalog. If the Rust compact encoding differs by one byte, every session row a
Rust `burn` writes differs from what Scala would have written — and nothing else
in the test suite would notice, because both sides would be internally
consistent. The corpus closes this: each of the four fixture discs carries its
`manifest.json`, and `catalog/11-after-finalize.sqlite` carries the hash the
Scala build computed for that same manifest.

This also covers all four manifest shapes: initial add (disc 1), add session
(disc 2), modify+delete (disc 3), file-less seal (disc 4).

- [ ] **Step 1: Add the fixture text helper**

In `crates/diskrement-shell/tests/support/mod.rs`, above the `pub use` line:

```rust
/// Read a fixture file as text. Never writes, so no `immutable=1` dance is
/// needed — unlike the SQLite fixtures.
pub fn fixture_text(rel: &str) -> String {
    let path = fixtures_dir().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}
```

- [ ] **Step 2: Write the failing test**

Create `crates/diskrement-shell/tests/fixture_manifest_hash.rs`:

```rust
//! `session.manifest_hash` is the sha256 of the *compact* manifest JSON, so the
//! Rust encoder's field order and whitespace are catalog content (spec §3.1).
//!
//! Both halves come out of the corpus: the manifest from the disc tree the
//! Scala build staged, the hash from the catalog it committed. Nothing here is
//! a value typed into this file and asserted against itself.

mod support;

use diskrement_shell::burn::json::{manifest_hash, ManifestJson};
use support::{fixture_text, open_fixture};

/// The catalog holding every session the harvest produced.
const FINAL_CATALOG: &str = "catalog/11-after-finalize.sqlite";

fn hash_from_catalog(disc_ordinal: u32, session_ordinal: u32) -> String {
    let conn = open_fixture(FINAL_CATALOG);
    conn.query_row(
        "select s.manifest_hash
           from session s join disc d on s.disc_id = d.id
          where d.ordinal = ?1 and s.ordinal = ?2",
        (disc_ordinal, session_ordinal),
        |row| row.get(0),
    )
    .unwrap_or_else(|e| panic!("disc {disc_ordinal} session {session_ordinal}: {e}"))
}

fn check(disc: u32) {
    let raw = fixture_text(&format!("discs/disc-{disc}/.diskrement/manifest.json"));
    let dto: ManifestJson =
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("disc {disc} manifest.json: {e}"));
    let manifest = dto
        .to_manifest()
        .unwrap_or_else(|e| panic!("disc {disc} manifest.json: {e}"));
    assert_eq!(manifest.disc_ordinal, disc, "fixture names its own disc");

    let expected = hash_from_catalog(disc, manifest.session_ordinal);
    assert_eq!(
        manifest_hash(&manifest),
        expected,
        "disc {disc}: the Rust manifest hash differs from the one the Scala \
         build stored. The compact JSON encoding has drifted — check field \
         order and whitespace, not the hash function."
    );
}

/// Disc 1: the initial-add shape.
#[test]
fn the_initial_add_manifest_hashes_to_the_stored_value() {
    check(1);
}

/// Disc 2: the spill remainder, another add session.
#[test]
fn the_second_add_manifest_hashes_to_the_stored_value() {
    check(2);
}

/// Disc 3: the modify+delete shape — the only one with a non-empty `modified`
/// and a non-empty `deleted`.
#[test]
fn the_modify_and_delete_manifest_hashes_to_the_stored_value() {
    check(3);
}

/// Disc 4: the file-less seal session (`plan --finalize`), all three lists
/// empty and `sessionOrdinal` 2.
#[test]
fn the_seal_manifest_hashes_to_the_stored_value() {
    check(4);
}
```

- [ ] **Step 3: Run the test to verify it fails for the right reason**

Run: `cargo test -p diskrement-shell --test fixture_manifest_hash`
Expected: it must **pass**. This is a characterization test over already-landed
code, so a failure here is a real encoding bug in Task 2, not a missing feature.
If it fails, fix `json.rs` — never adjust the expectation.

To prove the test has teeth, temporarily change `ManifestJson`'s field order
(move `deleted` above `added`), re-run, and confirm all four tests fail. Then
revert.

- [ ] **Step 4: Verify and commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/diskrement-shell/tests/fixture_manifest_hash.rs crates/diskrement-shell/tests/support/mod.rs
git commit -m "test(burn): pin the manifest hash against every corpus manifest shape"
```

---

### Task 4: The pointer-rendering corpus oracle

**Files:**
- Create: `crates/diskrement-shell/tests/fixture_pointers.rs`

`archive_pointers` was ported and fixture-tested in P2; what is untested is the
*rendering* — field names, the uppercase `kind` spelling, and the order rows
appear in the file. Only `discs/disc-4` has a non-empty `pointers.json`, and it
was read from the live catalog at the `10-after-burn4` state (the seal burn's
starting point), so that pair is an exact oracle.

- [ ] **Step 1: Write the failing test**

Create `crates/diskrement-shell/tests/fixture_pointers.rs`:

```rust
//! `pointers.json` as the Scala build wrote it, byte for byte.
//!
//! Pointers are read from the *live* catalog before the burn's update commits,
//! so they lag one cycle: discs 1-3 all carry `[]`, and only disc 4 — staged by
//! the final `plan --finalize` burn, whose starting catalog was
//! `10-after-burn4` — has entries. That pair is the only non-empty rendering
//! the corpus contains, which is why it gets a test of its own rather than
//! riding along with the burn replay.

mod support;

use diskrement_shell::burn::json::{to_pretty, PointersJson};
use diskrement_shell::catalog::Catalog;
use support::{fixture_copy, fixture_text};

/// The live catalog immediately before the seal burn that staged `disc-4`.
const CATALOG_BEFORE_SEAL_BURN: &str = "catalog/10-after-burn4.sqlite";

#[test]
fn rendering_the_pointers_reproduces_the_file_on_disc_four() {
    let (_tmp, path) = fixture_copy(CATALOG_BEFORE_SEAL_BURN);
    let catalog = Catalog::open(&path).expect("the fixture catalog is already current");
    let project = catalog
        .find_project("p")
        .expect("readable")
        .expect("the harvest project is named p");

    let pointers = catalog
        .archive_pointers(project.id)
        .expect("readable");

    assert_eq!(
        to_pretty(&PointersJson::from(&pointers)),
        fixture_text("discs/disc-4/.diskrement/pointers.json"),
        "the rendered pointers differ from the file the Scala build wrote"
    );
}

/// The lag is deliberate and load-bearing: if `archive_pointers` were read
/// after the update committed, disc 4 would list a@3 as superseded by itself.
/// Discs 1-3 pin the empty shape.
#[test]
fn the_first_three_discs_carry_an_empty_pointer_file() {
    for disc in 1..=3 {
        assert_eq!(
            fixture_text(&format!("discs/disc-{disc}/.diskrement/pointers.json")),
            "{\n  \"pointers\" : []\n}",
            "disc {disc}"
        );
    }
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test -p diskrement-shell --test fixture_pointers`
Expected: PASS. A failure is a rendering bug in Task 2 or an ordering bug in
`archive_pointers` — fix the code, not the fixture.

If `archive_pointers` takes arguments beyond `project.id`, check its signature
in `crates/diskrement-shell/src/catalog/read.rs:233` and pass what it wants.

- [ ] **Step 3: Verify and commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/diskrement-shell/tests/fixture_pointers.rs
git commit -m "test(burn): pin pointers.json against the corpus rendering"
```

---

### Task 5: The `/.diskrement` snapshot writer

**Files:**
- Modify: `crates/diskrement-shell/src/burn/snapshot_writer.rs`

Read `shell/src/main/scala/diskrement/shell/burn/SnapshotWriter.scala` first.
Four artifacts, in this order: `project.json`, `manifest.json`, `pointers.json`,
and a copy of the already-prepared catalog snapshot as `catalog.sqlite`.

- [ ] **Step 1: Write the failing test**

Append to `crates/diskrement-shell/src/burn/snapshot_writer.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
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
        assert!(project.starts_with("{\n  \"projectName\" : \"p\","), "{project}");
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p diskrement-shell --lib burn::snapshot_writer`
Expected: FAIL — `cannot find function 'write' in this scope`.

- [ ] **Step 3: Implement the writer**

Replace the contents of `crates/diskrement-shell/src/burn/snapshot_writer.rs`
(keeping the test module at the bottom):

```rust
//! Writes one disc's `/.diskrement` directory (`SnapshotWriter.scala`).
//!
//! The catalog snapshot is *copied*, not re-derived: the pipeline has already
//! built it on a work-dir copy and applied this burn's update to it, so what
//! lands on the disc is the post-burn state and the disc is self-describing
//! (CLAUDE.md invariant).

use std::path::Path;

use crate::burn::json::{to_pretty, ManifestJson, PointersJson, ProjectJson};
use crate::burn::{BurnError, DiscIdentity, DiscMetadata};

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
```

The unused `DiscIdentity` import above is a mistake waiting to happen — drop it
if the compiler flags it; `-D warnings` will.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p diskrement-shell --lib burn::snapshot_writer`
Expected: PASS, 4 tests.

- [ ] **Step 5: Verify and commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/diskrement-shell/src/burn/snapshot_writer.rs
git commit -m "feat(burn): write the /.diskrement metadata for one disc"
```

---

### Task 6: The stager

**Files:**
- Modify: `crates/diskrement-shell/src/burn/stager.rs`

Read `shell/src/main/scala/diskrement/shell/burn/Stager.scala` first, then
`crates/diskrement-shell/src/fs/snapshot.rs` (the in-process reflink idiom this
follows) and `crates/diskrement-shell/src/fs/scanner.rs:138-167` (the rayon pool
idiom this follows).

Order of operations, from the Scala:

1. clear and recreate `<staging>/data`
2. check every planned file exists under the snapshot root
3. pre-create every destination directory
4. copy every file
5. re-hash every staged file against its scan hash, under a progress phase
6. write `/.diskrement`
7. sum every regular file under the staging dir

**Two things not to lose in the port.** Step 3 is not an artifact of `cp
--parents -p`: pre-creating directories is what keeps staged directory
permissions and mtimes independent of the *source* directories', so a read-only
source directory cannot break the copy and directory mtimes do not depend on
copy order. Only **file** metadata is preserved from the source. And step 5 is
the integrity gate — it always runs, on every file, even though the copy just
came from a snapshot that was itself hashed.

The `cp` subprocess and its English-stderr sniffing are replaced by
`reflink_copy::reflink` plus the errno test `fs::snapshot::clone_unsupported`,
exactly as P3 replaced them for the source snapshot. The one-warning-per-burn
behavior stays: the first unsupported clone warns and every later file in that
burn goes straight to a plain copy.

- [ ] **Step 1: Write the failing tests**

Append to `crates/diskrement-shell/src/burn/stager.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone as _, Utc};
    use diskrement_core::{ContentHash, RelPath, ScannedFile, SessionManifest};
    use std::path::{Path, PathBuf};

    use crate::burn::{DiscIdentity, DiscMetadata};
    use crate::fs::hasher;
    use crate::progress::NoopSink;

    struct Fixture {
        _dir: tempfile::TempDir,
        snapshot: PathBuf,
        staging: PathBuf,
        catalog_snapshot: PathBuf,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::TempDir::new().unwrap();
        let snapshot = dir.path().join("snapshot");
        let staging = dir.path().join("staging/disc-1");
        let catalog_snapshot = dir.path().join("burn-catalog.sqlite");
        std::fs::create_dir_all(&snapshot).unwrap();
        std::fs::write(&catalog_snapshot, b"catalog bytes").unwrap();
        Fixture {
            _dir: dir,
            snapshot,
            staging,
            catalog_snapshot,
        }
    }

    /// Write `rel` under the snapshot root and return the `ScannedFile` the
    /// planner would have produced for it.
    fn source(fx: &Fixture, rel: &str, body: &[u8]) -> ScannedFile {
        let abs = fx.snapshot.join(rel);
        std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
        std::fs::write(&abs, body).unwrap();
        ScannedFile {
            rel_path: RelPath(rel.to_string()),
            content_hash: hasher::hash(&abs).unwrap(),
            size_bytes: body.len() as u64,
            mtime: Utc.timestamp_opt(1_757_100_000, 0).unwrap(),
        }
    }

    fn image(fx: &Fixture, files: Vec<ScannedFile>) -> DiscImage {
        DiscImage {
            staging_dir: fx.staging.clone(),
            snapshot_root: fx.snapshot.clone(),
            files,
            metadata: DiscMetadata {
                identity: DiscIdentity {
                    project_name: "p".to_string(),
                    disc_ordinal: 1,
                    session_ordinal: 1,
                    created_at: Utc.timestamp_opt(1_757_100_066, 0).unwrap(),
                },
                manifest: SessionManifest {
                    project_name: "p".to_string(),
                    disc_ordinal: 1,
                    session_ordinal: 1,
                    added: Vec::new(),
                    modified: Vec::new(),
                    deleted: Vec::new(),
                },
                catalog_snapshot: fx.catalog_snapshot.clone(),
                pointers: Vec::new(),
            },
            close_disc: false,
        }
    }

    #[test]
    fn stages_every_file_under_data_preserving_structure_and_bytes() {
        let fx = fixture();
        let files = vec![
            source(&fx, "a.bin", b"aaaa"),
            source(&fx, "nested/deep/b.bin", b"bbbbbb"),
        ];
        let out = stage(&image(&fx, files), &NoopSink, 2).unwrap();

        assert_eq!(std::fs::read(fx.staging.join("data/a.bin")).unwrap(), b"aaaa");
        assert_eq!(
            std::fs::read(fx.staging.join("data/nested/deep/b.bin")).unwrap(),
            b"bbbbbb"
        );
        assert_eq!(out.staging_dir, fx.staging);
    }

    #[test]
    fn writes_the_metadata_directory_too() {
        let fx = fixture();
        let files = vec![source(&fx, "a.bin", b"aaaa")];
        stage(&image(&fx, files), &NoopSink, 1).unwrap();

        for name in ["project.json", "manifest.json", "pointers.json", "catalog.sqlite"] {
            assert!(
                fx.staging.join(".diskrement").join(name).is_file(),
                "missing {name}"
            );
        }
    }

    /// `imageBytes` counts every regular file under the staging dir, metadata
    /// included — not just the payload.
    #[test]
    fn image_bytes_sums_the_whole_staged_tree() {
        let fx = fixture();
        let files = vec![source(&fx, "a.bin", b"aaaa")];
        let out = stage(&image(&fx, files), &NoopSink, 1).unwrap();

        let mut expected = 0u64;
        for entry in walkdir::WalkDir::new(&fx.staging) {
            let entry = entry.unwrap();
            if entry.file_type().is_file() {
                expected += entry.metadata().unwrap().len();
            }
        }
        assert_eq!(out.image_bytes, expected);
        assert!(out.image_bytes > 4, "metadata is counted too");
    }

    /// File mtimes come from the source. `ioctl(FICLONE)` stamps the clone with
    /// the current time, so a port that forgets to restore them passes every
    /// byte-comparison test and still breaks nothing here — except this.
    #[test]
    fn staged_files_keep_the_source_mtime() {
        let fx = fixture();
        let files = vec![source(&fx, "a.bin", b"aaaa")];
        let src = fx.snapshot.join("a.bin");
        let stamp = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
        std::fs::File::options()
            .write(true)
            .open(&src)
            .unwrap()
            .set_modified(stamp)
            .unwrap();

        stage(&image(&fx, files), &NoopSink, 1).unwrap();

        let staged = std::fs::metadata(fx.staging.join("data/a.bin")).unwrap();
        assert_eq!(staged.modified().unwrap(), stamp);
    }

    /// Staging is idempotent: the previous tree is removed, so a file that is
    /// no longer planned does not survive into the new session.
    #[test]
    fn restaging_clears_the_previous_tree() {
        let fx = fixture();
        let a = source(&fx, "a.bin", b"aaaa");
        let b = source(&fx, "b.bin", b"bbbb");
        stage(&image(&fx, vec![a.clone(), b]), &NoopSink, 1).unwrap();
        assert!(fx.staging.join("data/b.bin").exists());

        stage(&image(&fx, vec![a]), &NoopSink, 1).unwrap();
        assert!(!fx.staging.join("data/b.bin").exists(), "stale file survived");
        assert!(fx.staging.join("data/a.bin").exists());
    }

    #[test]
    fn a_file_missing_from_the_snapshot_fails_before_anything_is_copied() {
        let fx = fixture();
        let mut files = vec![source(&fx, "a.bin", b"aaaa")];
        files.push(ScannedFile {
            rel_path: RelPath("gone.bin".to_string()),
            content_hash: ContentHash("00".repeat(32)),
            size_bytes: 4,
            mtime: Utc.timestamp_opt(1_757_100_000, 0).unwrap(),
        });

        let err = stage(&image(&fx, files), &NoopSink, 1).unwrap_err();
        match err {
            BurnError::SourceFileMissing(p) => assert_eq!(p.0, "gone.bin"),
            other => panic!("expected SourceFileMissing, got {other:?}"),
        }
        assert!(
            !fx.staging.join("data/a.bin").exists(),
            "the existence check runs before the copy"
        );
    }

    /// The integrity gate: a staged file whose content does not match the hash
    /// the scan recorded stops the burn.
    #[test]
    fn a_hash_mismatch_stops_the_burn() {
        let fx = fixture();
        let mut f = source(&fx, "a.bin", b"aaaa");
        let real = f.content_hash.clone();
        f.content_hash = ContentHash("11".repeat(32));

        let err = stage(&image(&fx, vec![f]), &NoopSink, 1).unwrap_err();
        match err {
            BurnError::HashMismatch {
                rel_path,
                expected,
                actual,
            } => {
                assert_eq!(rel_path.0, "a.bin");
                assert_eq!(expected.0, "11".repeat(32));
                assert_eq!(actual, real);
            }
            other => panic!("expected HashMismatch, got {other:?}"),
        }
    }

    /// A disc with no files is legal: `plan --finalize` seals a disc with a
    /// file-less session, and the corpus's `disc-4/data/` is empty.
    #[test]
    fn a_seal_session_stages_an_empty_data_directory() {
        let fx = fixture();
        let out = stage(&image(&fx, Vec::new()), &NoopSink, 1).unwrap();

        assert!(fx.staging.join("data").is_dir());
        assert_eq!(std::fs::read_dir(fx.staging.join("data")).unwrap().count(), 0);
        assert!(fx.staging.join(".diskrement/manifest.json").is_file());
        assert!(out.image_bytes > 0, "metadata still counts");
    }

    /// Progress is reported once per file, with the phase label the Scala
    /// stager used.
    #[test]
    fn reports_a_staging_phase_over_every_file() {
        use std::sync::Mutex;

        #[derive(Default)]
        struct Rec(Mutex<Vec<String>>);
        impl crate::progress::ProgressSink for Rec {
            fn start(&self, label: &str, total_bytes: u64, total_files: usize) {
                self.0
                    .lock()
                    .unwrap()
                    .push(format!("start:{label}:{total_bytes}:{total_files}"));
            }
            fn advance(&self, delta_bytes: u64, delta_files: usize) {
                self.0
                    .lock()
                    .unwrap()
                    .push(format!("advance:{delta_bytes}:{delta_files}"));
            }
            fn done(&self) {
                self.0.lock().unwrap().push("done".to_string());
            }
        }

        let fx = fixture();
        let files = vec![source(&fx, "a.bin", b"aaaa"), source(&fx, "b.bin", b"bbbbbb")];
        let rec = Rec::default();
        stage(&image(&fx, files), &rec, 1).unwrap();

        let events = rec.0.lock().unwrap().clone();
        assert_eq!(events[0], "start:staging:10:2");
        assert_eq!(events.iter().filter(|e| e.starts_with("advance:")).count(), 2);
        assert_eq!(events.last().unwrap(), "done");
    }

    /// `done` still fires when a hash fails mid-phase.
    #[test]
    fn the_staging_phase_closes_even_when_a_hash_mismatches() {
        use std::sync::Mutex;

        #[derive(Default)]
        struct Rec(Mutex<Vec<String>>);
        impl crate::progress::ProgressSink for Rec {
            fn start(&self, _l: &str, _b: u64, _f: usize) {
                self.0.lock().unwrap().push("start".to_string());
            }
            fn advance(&self, _b: u64, _f: usize) {}
            fn done(&self) {
                self.0.lock().unwrap().push("done".to_string());
            }
        }

        let fx = fixture();
        let mut f = source(&fx, "a.bin", b"aaaa");
        f.content_hash = ContentHash("11".repeat(32));
        let rec = Rec::default();
        let _ = stage(&image(&fx, vec![f]), &rec, 1);

        assert_eq!(rec.0.lock().unwrap().clone(), vec!["start", "done"]);
    }

    /// The plain-copy fallback produces the same tree as the reflink path, so
    /// the stager works on tmpfs and any other filesystem that refuses clones.
    #[test]
    fn the_plain_copy_fallback_produces_the_same_tree() {
        let fx = fixture();
        let files = vec![source(&fx, "a.bin", b"aaaa"), source(&fx, "n/b.bin", b"bb")];
        let img = image(&fx, files);

        stage_with(&img, &NoopSink, 1, |_src: &Path, _dst: &Path| {
            Err(std::io::Error::from_raw_os_error(95)) // EOPNOTSUPP
        })
        .unwrap();

        assert_eq!(std::fs::read(fx.staging.join("data/a.bin")).unwrap(), b"aaaa");
        assert_eq!(std::fs::read(fx.staging.join("data/n/b.bin")).unwrap(), b"bb");
    }

    /// A clone failure that is *not* a capability signal is a real error and
    /// must not be swallowed by the fallback.
    #[test]
    fn a_non_capability_clone_failure_is_an_error() {
        let fx = fixture();
        let files = vec![source(&fx, "a.bin", b"aaaa")];
        let err = stage_with(&image(&fx, files), &NoopSink, 1, |_s: &Path, _d: &Path| {
            Err(std::io::Error::from_raw_os_error(5)) // EIO
        })
        .unwrap_err();
        assert!(matches!(err, BurnError::Io(_)), "{err:?}");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p diskrement-shell --lib burn::stager`
Expected: FAIL — `unimplemented!("Task 6")` panics, and `stage_with` is not
defined.

- [ ] **Step 3: Implement the stager**

Replace the contents of `crates/diskrement-shell/src/burn/stager.rs` (keeping
the test module at the bottom):

```rust
//! Stages one disc tree and re-hashes it (`Stager.scala`).
//!
//! `<staging>/data` is the payload and `<staging>/.diskrement` the metadata;
//! for the real burner (P8) this staged tree is exactly what xorriso masters,
//! so the simulated and real backends share it.
//!
//! The copy is `Stager.scala`'s `cp --reflink=always --parents -p` batches,
//! translated the way `fs::snapshot` translated the source snapshot's: an
//! in-process clone per file with the mtime and permissions restored
//! afterwards, and the "this filesystem cannot clone" test moved from
//! English `cp` stderr to `ioctl(FICLONE)`'s errnos. The batching went with
//! the subprocess; there is no process to amortise any more.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::burn::{snapshot_writer, BurnError, BurnResult, DiscImage};
use crate::fs::hasher;
use crate::fs::snapshot::clone_unsupported;
use crate::progress::{phase, ProgressSink};

pub fn stage(
    image: &DiscImage,
    progress: &dyn ProgressSink,
    parallelism: usize,
) -> Result<BurnResult, BurnError> {
    stage_with(image, progress, parallelism, |src, dst| {
        reflink_copy::reflink(src, dst).map_err(std::io::Error::from)
    })
}

/// `stage` with the clone step injected, so the fallback and the hard-failure
/// classification are testable on a filesystem that happily reflinks.
pub fn stage_with(
    image: &DiscImage,
    progress: &dyn ProgressSink,
    parallelism: usize,
    clone: impl Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<BurnResult, BurnError> {
    let data_dir = image.staging_dir.join("data");
    clear_and_create(&data_dir)?;
    check_all_exist(image)?;
    create_dirs(&data_dir, image)?;
    copy_all(image, &data_dir, clone)?;

    let total_bytes: u64 = image.files.iter().map(|f| f.size_bytes).sum();
    let verified: Result<(), BurnError> = phase(
        progress,
        "staging",
        total_bytes,
        image.files.len(),
        || {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(parallelism.max(1))
                .build()
                .map_err(|e| BurnError::Io(std::io::Error::other(e)))?;
            pool.install(|| {
                image
                    .files
                    .par_iter()
                    .map(|f| {
                        let staged = data_dir.join(&f.rel_path.0);
                        let actual = hasher::hash(&staged).map_err(BurnError::Io)?;
                        if actual != f.content_hash {
                            return Err(BurnError::HashMismatch {
                                rel_path: f.rel_path.clone(),
                                expected: f.content_hash.clone(),
                                actual,
                            });
                        }
                        progress.advance(f.size_bytes, 1);
                        Ok(())
                    })
                    .collect::<Result<(), BurnError>>()
            })
        },
    );
    verified?;

    snapshot_writer::write(&image.staging_dir.join(".diskrement"), &image.metadata)?;
    let image_bytes = total_staged_bytes(&image.staging_dir)?;
    Ok(BurnResult {
        staging_dir: image.staging_dir.clone(),
        image_bytes,
    })
}

/// Remove any previous staging for this disc and create an empty `data/`.
/// Re-staging must not leak a file the new session no longer plans.
fn clear_and_create(data_dir: &Path) -> Result<(), BurnError> {
    let staging_dir = data_dir
        .parent()
        .expect("data_dir was built by joining onto the staging dir");
    if staging_dir.exists() {
        std::fs::remove_dir_all(staging_dir).map_err(BurnError::Io)?;
    }
    std::fs::create_dir_all(data_dir).map_err(BurnError::Io)
}

fn check_all_exist(image: &DiscImage) -> Result<(), BurnError> {
    for f in &image.files {
        if !image.snapshot_root.join(&f.rel_path.0).exists() {
            return Err(BurnError::SourceFileMissing(f.rel_path.clone()));
        }
    }
    Ok(())
}

/// Pre-create every destination directory.
///
/// Not a `cp --parents` workaround: creating them here is what keeps staged
/// directory permissions and mtimes independent of the *source* directories'.
/// A read-only source directory would otherwise break a later write into it,
/// and directory mtimes would depend on copy order. Only FILE metadata is
/// preserved from the source.
fn create_dirs(data_dir: &Path, image: &DiscImage) -> Result<(), BurnError> {
    let mut parents: BTreeSet<PathBuf> = BTreeSet::new();
    for f in &image.files {
        if let Some(parent) = data_dir.join(&f.rel_path.0).parent() {
            parents.insert(parent.to_path_buf());
        }
    }
    for parent in parents {
        std::fs::create_dir_all(&parent).map_err(BurnError::Io)?;
    }
    Ok(())
}

/// Clone every planned file, falling back to a plain copy for the rest of the
/// burn once the filesystem says it cannot clone. Sequential: the copy is
/// I/O-bound and the Scala version was too.
fn copy_all(
    image: &DiscImage,
    data_dir: &Path,
    clone: impl Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<(), BurnError> {
    let mut fallback = false;
    for f in &image.files {
        let src = image.snapshot_root.join(&f.rel_path.0);
        let dst = data_dir.join(&f.rel_path.0);
        if !fallback {
            match clone(&src, &dst) {
                Ok(()) => {
                    copy_file_metadata(&src, &dst)?;
                    continue;
                }
                Err(e) if clone_unsupported(&e) => {
                    eprintln!(
                        "reflink clone unsupported here; falling back to plain copies for \
                         this burn (staging will read and write every byte)"
                    );
                    fallback = true;
                }
                Err(e) => return Err(BurnError::Io(e)),
            }
        }
        // `fs::copy` carries permissions; the mtime still needs restoring.
        std::fs::copy(&src, &dst).map_err(BurnError::Io)?;
        copy_file_metadata(&src, &dst)?;
    }
    Ok(())
}

/// `ioctl(FICLONE)` stamps the clone with the current time and `fs::copy` does
/// the same, but the staged tree must carry the source's file mtimes — that is
/// what `cp -p` gave the Scala stager (CLAUDE.md).
fn copy_file_metadata(src: &Path, dst: &Path) -> Result<(), BurnError> {
    let meta = std::fs::metadata(src).map_err(BurnError::Io)?;
    std::fs::set_permissions(dst, meta.permissions()).map_err(BurnError::Io)?;
    let mtime = meta.modified().map_err(BurnError::Io)?;
    std::fs::File::options()
        .write(true)
        .open(dst)
        .map_err(BurnError::Io)?
        .set_modified(mtime)
        .map_err(BurnError::Io)
}

/// Every regular file under the staged tree, metadata included.
fn total_staged_bytes(staging_dir: &Path) -> Result<u64, BurnError> {
    let mut total = 0u64;
    for entry in walkdir::WalkDir::new(staging_dir).follow_links(false) {
        let entry = entry.map_err(|e| BurnError::Io(std::io::Error::other(e)))?;
        if entry.file_type().is_file() {
            total += entry.metadata().map_err(|e| {
                BurnError::Io(std::io::Error::other(e))
            })?.len();
        }
    }
    Ok(total)
}
```

`reflink_copy::reflink` returns `std::io::Error` already in the version pinned
here; if the `map_err(std::io::Error::from)` is redundant, drop it —
`clippy::useless_conversion` will say so.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p diskrement-shell --lib burn::stager`
Expected: PASS, 12 tests.

- [ ] **Step 5: Verify and commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/diskrement-shell/src/burn/stager.rs
git commit -m "feat(burn): stage a disc tree with reflink copies and a re-hash gate"
```

---

### Task 7: The burn report types

**Files:**
- Modify: `crates/diskrement-shell/src/pipeline/reports.rs`

Read `shell/src/main/scala/diskrement/shell/pipeline/Pipeline.scala` for
`DiscBurn` and `BurnReport`.

- [ ] **Step 1: Add the types**

Append to `crates/diskrement-shell/src/pipeline/reports.rs`:

```rust
use std::path::PathBuf;

/// What one disc's burn produced (`Pipeline.scala`'s `DiscBurn`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscBurn {
    pub ordinal: u32,
    pub label: String,
    pub finalized: bool,
    pub staging_dir: PathBuf,
    pub image_bytes: u64,
}

/// The result of one burn cycle: exactly one disc, plus what still remains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BurnReport {
    pub disc: DiscBurn,
    pub remaining: RemainingEstimate,
}
```

- [ ] **Step 2: Verify it compiles and commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/diskrement-shell/src/pipeline/reports.rs
git commit -m "feat(pipeline): the burn report types"
```

---

### Task 8: `Pipeline::burn`

**Files:**
- Modify: `crates/diskrement-shell/src/pipeline/mod.rs`

Read `PipelineLive.burn`, `existingSessions`, and `toDiscWrite` in
`shell/src/main/scala/diskrement/shell/pipeline/PipelineLive.scala` first.

**Ordering is the whole design.** The disc is staged first and the live catalog
is committed only after the burner returns; the embedded snapshot is the same
update applied to a *copy* first, so the disc knows about its own session. A
crash anywhere before the commit leaves the catalog untouched and the staged
plan intact, which is the recovery unit.

**The dead-disc guard cannot be pushed into the catalog.** Deadness is derived
from `disc_event`, so `disc.status` still reads `OPEN` for a disc that no longer
physically exists. Without the guard the burner would happily commit a session
onto it.

`now` is a parameter, not `Utc::now()`. The Scala pipeline took it from ZIO's
injectable `Clock`; the Rust CLI supplies it, as it already does for
`ProjectInit::created_at`. That is what lets Task 9 replay a corpus burn and
compare committed rows exactly instead of normalizing timestamps away.

The burner is a parameter rather than a `Pipeline` field: `burn` is the only
operation that needs one, and threading it through `Pipeline::new` would force
`init`/`scan`/`plan`/`status` to construct a burner they never use. P8 swaps
`Burner::Simulated` for `Burner::Real` at the call site.

- [ ] **Step 1: Write the failing tests**

Create a test module at the bottom of
`crates/diskrement-shell/src/pipeline/mod.rs` (or extend the existing one):

```rust
#[cfg(test)]
mod burn_tests {
    use super::*;
    use chrono::{TimeZone as _, Utc};
    use diskrement_core::{DiscType, RelPath, ScannedFile};

    use crate::burn::{simulated::SimulatedBurner, Burner};
    use crate::catalog::model::ProjectInit;
    use crate::progress::NoopSink;

    struct Env {
        _dir: tempfile::TempDir,
        catalog: Catalog,
        work: PathBuf,
        scan_cache: PathBuf,
        source: PathBuf,
    }

    fn now() -> chrono::DateTime<Utc> {
        Utc.timestamp_opt(1_757_100_066, 623_959_846).unwrap()
    }

    /// A project whose snapshot already holds `files`, with `scan.json` and
    /// `plan.json` staged — the state `burn` expects to find.
    fn staged_env(files: &[(&str, &[u8])], finalize: bool) -> Env {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let work = root.join("work");
        let source = root.join("source");
        let snapshot = work.join("snapshot");
        std::fs::create_dir_all(&snapshot).unwrap();
        std::fs::create_dir_all(&source).unwrap();

        let mut scanned: Vec<ScannedFile> = Vec::new();
        for (rel, body) in files {
            let abs = snapshot.join(rel);
            std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
            std::fs::write(&abs, body).unwrap();
            scanned.push(ScannedFile {
                rel_path: RelPath((*rel).to_string()),
                content_hash: crate::fs::hasher::hash(&abs).unwrap(),
                size_bytes: body.len() as u64,
                mtime: Utc.timestamp_opt(1_757_000_000, 0).unwrap(),
            });
        }

        let mut catalog = Catalog::open(&root.join("catalog.sqlite")).unwrap();
        catalog
            .create_project(&ProjectInit {
                name: "p".to_string(),
                source_root: source.display().to_string(),
                work_dir: work.display().to_string(),
                default_disc_type: DiscType::BdR,
                margin_percent: 2.0,
                copies: 1,
                created_at: now(),
            })
            .unwrap();

        staged::write_scan(&work, &scanned).unwrap();
        let report = crate::pipeline::planning::plan_from(
            &diskrement_core::CatalogView::empty(),
            &scanned,
            &[],
            DiscType::BdR,
            diskrement_core::Capacity::new(1_000_000),
            finalize,
        );
        staged::write_plan(&work, &report).unwrap();

        Env {
            _dir: dir,
            catalog,
            work: work.clone(),
            scan_cache: root.join("scancache.sqlite"),
            source,
        }
    }

    fn run_burn(env: &mut Env) -> Result<BurnReport, PipelineError> {
        let sink = NoopSink;
        let burner = Burner::Simulated(SimulatedBurner::new(&sink, 2));
        let mut pipeline = Pipeline::new(
            &mut env.catalog,
            PipelinePaths {
                work_dir: env.work.clone(),
                scan_cache: env.scan_cache.clone(),
            },
            &sink,
        );
        pipeline.burn("p", now(), &burner)
    }

    #[test]
    fn burning_stages_the_disc_and_commits_one_session() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], false);
        let report = run_burn(&mut env).unwrap();

        assert_eq!(report.disc.ordinal, 1);
        assert_eq!(report.disc.label, "p disc 1");
        assert!(!report.disc.finalized);
        assert!(env.work.join("staging/disc-1/data/a.bin").is_file());
        assert!(env.work.join("staging/disc-1/.diskrement/catalog.sqlite").is_file());

        let sessions = env
            .catalog
            .sessions_by_state(ProjectId(1), SessionState::Burned)
            .unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].disc_ordinal, 1);
        assert_eq!(sessions[0].ordinal, 1);
    }

    /// The staged plan is consumed, the snapshot released, and the temporary
    /// burn catalog removed — the whole work-dir cleanup in one place.
    #[test]
    fn a_successful_burn_clears_the_staged_state() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], false);
        run_burn(&mut env).unwrap();

        assert!(staged::read_scan(&env.work).unwrap().is_none());
        assert!(staged::read_plan(&env.work).unwrap().is_none());
        assert!(!env.work.join("snapshot").exists(), "snapshot released");
        assert!(!env.work.join("burn-catalog.sqlite").exists());
        assert!(env.work.join("staging/disc-1").is_dir(), "staging survives");
    }

    /// The embedded snapshot is post-burn: it already knows about the session
    /// being written, so a single disc describes the whole archive.
    #[test]
    fn the_embedded_catalog_already_contains_this_burns_session() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], false);
        run_burn(&mut env).unwrap();

        let embedded = rusqlite::Connection::open_with_flags(
            env.work.join("staging/disc-1/.diskrement/catalog.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let sessions: i64 = embedded
            .query_row("select count(*) from session", [], |r| r.get(0))
            .unwrap();
        assert_eq!(sessions, 1, "the disc knows about its own session");
    }

    #[test]
    fn burning_without_a_staged_scan_is_a_state_error() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], false);
        staged::clear(&env.work).unwrap();
        let err = run_burn(&mut env).unwrap_err();
        assert!(matches!(err, PipelineError::NoStagedScan(_)), "{err}");
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn burning_without_a_staged_plan_is_a_state_error() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], false);
        std::fs::remove_file(staged::plan_path(&env.work)).unwrap();
        let err = run_burn(&mut env).unwrap_err();
        assert!(matches!(err, PipelineError::NoStagedPlan(_)), "{err}");
    }

    #[test]
    fn burning_without_a_snapshot_is_a_state_error() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], false);
        std::fs::remove_dir_all(env.work.join("snapshot")).unwrap();
        let err = run_burn(&mut env).unwrap_err();
        assert!(matches!(err, PipelineError::SnapshotMissing(_)), "{err}");
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn burning_an_unknown_project_is_a_lookup_error() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], false);
        let sink = NoopSink;
        let burner = Burner::Simulated(SimulatedBurner::new(&sink, 1));
        let mut pipeline = Pipeline::new(
            &mut env.catalog,
            PipelinePaths {
                work_dir: env.work.clone(),
                scan_cache: env.scan_cache.clone(),
            },
            &sink,
        );
        let err = pipeline.burn("nosuch", now(), &burner).unwrap_err();
        assert!(matches!(err, PipelineError::ProjectNotFound(_)), "{err}");
    }

    /// A failed burn commits nothing: the catalog is untouched and the staged
    /// plan survives, so the operator can retry.
    #[test]
    fn a_failed_burn_leaves_the_catalog_and_the_staged_plan_alone() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], false);
        // Delete the source file after planning: the stager's existence check
        // fails, standing in for any mid-burn failure.
        std::fs::remove_file(env.work.join("snapshot/a.bin")).unwrap();

        let err = run_burn(&mut env).unwrap_err();
        assert!(matches!(err, PipelineError::Burn(_)), "{err}");
        assert_eq!(err.exit_code(), 6);

        assert!(staged::read_plan(&env.work).unwrap().is_some(), "plan survives");
        let sessions = env
            .catalog
            .sessions_by_state(ProjectId(1), SessionState::Burned)
            .unwrap();
        assert!(sessions.is_empty(), "nothing committed");
    }

    /// `plan` records oversized files and still succeeds; `burn` is the half
    /// that refuses them.
    #[test]
    fn oversized_files_are_refused_at_burn_time() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], false);
        let mut report = staged::read_plan(&env.work).unwrap().unwrap();
        report.oversized.push(ScannedFile {
            rel_path: RelPath("huge.bin".to_string()),
            content_hash: diskrement_core::ContentHash("00".repeat(32)),
            size_bytes: 1 << 40,
            mtime: now(),
        });
        staged::write_plan(&env.work, &report).unwrap();

        let err = run_burn(&mut env).unwrap_err();
        assert!(matches!(err, PipelineError::OversizedFiles(_)), "{err}");
        assert_eq!(err.exit_code(), 3);
    }

    /// A disc marked dead after the plan was staged. The catalog cannot catch
    /// this — deadness is derived from `disc_event`, so the disc row still
    /// reads OPEN — and without the guard the burn would write a session onto
    /// a disc that no longer exists.
    #[test]
    fn a_disc_marked_dead_after_planning_makes_the_plan_stale() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], false);
        run_burn(&mut env).unwrap();

        // Re-stage a plan for the same (now burned) disc, then kill the disc.
        let scanned = vec![ScannedFile {
            rel_path: RelPath("a.bin".to_string()),
            content_hash: crate::fs::hasher::hash(&env.work.join("staging/disc-1/data/a.bin"))
                .unwrap(),
            size_bytes: 4,
            mtime: now(),
        }];
        std::fs::create_dir_all(env.work.join("snapshot")).unwrap();
        std::fs::write(env.work.join("snapshot/a.bin"), b"aaaa").unwrap();
        staged::write_scan(&env.work, &scanned).unwrap();
        let mut report = crate::pipeline::planning::plan_from(
            &diskrement_core::CatalogView::empty(),
            &scanned,
            &[],
            DiscType::BdR,
            diskrement_core::Capacity::new(1_000_000),
            false,
        );
        report.disc.ordinal = 1;
        report.disc.is_new = false;
        staged::write_plan(&env.work, &report).unwrap();

        env.catalog
            .mark_dead(ProjectId(1), 1, now())
            .expect("marking disc 1 dead");

        let err = run_burn(&mut env).unwrap_err();
        match &err {
            PipelineError::StalePlan(detail) => {
                assert!(detail.contains("disc 1"), "{detail}");
                assert!(detail.contains("marked dead"), "{detail}");
            }
            other => panic!("expected StalePlan, got {other}"),
        }
        // The advice is always "re-run plan": `reset` would delete the staging
        // the staged-disc verifier needs.
        assert!(err.to_string().contains("re-run plan"), "{err}");
        assert!(!err.to_string().contains("reset"), "{err}");
    }

    #[test]
    fn a_finalizing_plan_marks_the_disc_finalized() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], true);
        let report = run_burn(&mut env).unwrap();
        assert!(report.disc.finalized);

        let states = env.catalog.read_disc_states(ProjectId(1)).unwrap();
        assert_eq!(states.len(), 1);
        assert_eq!(states[0].status, diskrement_core::DiscStatus::Finalized);
    }

    /// Session ordinals count per disc, from the highest already committed.
    #[test]
    fn a_second_session_on_the_same_disc_gets_ordinal_two() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], false);
        run_burn(&mut env).unwrap();

        let scanned = vec![ScannedFile {
            rel_path: RelPath("b.bin".to_string()),
            content_hash: {
                std::fs::create_dir_all(env.work.join("snapshot")).unwrap();
                std::fs::write(env.work.join("snapshot/b.bin"), b"bbbb").unwrap();
                crate::fs::hasher::hash(&env.work.join("snapshot/b.bin")).unwrap()
            },
            size_bytes: 4,
            mtime: now(),
        }];
        staged::write_scan(&env.work, &scanned).unwrap();
        let view = env.catalog.read_view(ProjectId(1)).unwrap();
        let states = env.catalog.read_disc_states(ProjectId(1)).unwrap();
        let report = crate::pipeline::planning::plan_from(
            &view,
            &scanned,
            &states,
            DiscType::BdR,
            diskrement_core::Capacity::new(1_000_000),
            false,
        );
        staged::write_plan(&env.work, &report).unwrap();

        run_burn(&mut env).unwrap();
        let sessions = env
            .catalog
            .sessions_by_state(ProjectId(1), SessionState::Burned)
            .unwrap();
        let mut ordinals: Vec<u32> = sessions
            .iter()
            .filter(|s| s.disc_ordinal == 1)
            .map(|s| s.ordinal)
            .collect();
        ordinals.sort_unstable();
        assert_eq!(ordinals, vec![1, 2]);
    }

    /// The `source` field exists only so the project row is honest; nothing in
    /// `burn` reads it. Asserting it keeps the fixture from rotting silently.
    #[test]
    fn burn_reads_the_snapshot_not_the_source_root() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], false);
        std::fs::remove_dir_all(&env.source).unwrap();
        run_burn(&mut env).expect("burn works from the snapshot alone");
    }
}
```

Check `mark_dead`'s real signature in
`crates/diskrement-shell/src/catalog/store.rs:149` and adapt the call; it may
take a reason or a `LostVersion` argument.

Check `plan_from`'s real signature in
`crates/diskrement-shell/src/pipeline/planning.rs` and adapt; Task 4 of the P4
plan defined it and the argument order above is from memory of that signature.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p diskrement-shell --lib pipeline::burn_tests`
Expected: FAIL — `no method named 'burn' found for struct 'Pipeline'`.

- [ ] **Step 3: Implement `Pipeline::burn`**

Add to `impl<'a> Pipeline<'a>` in `crates/diskrement-shell/src/pipeline/mod.rs`,
after `plan`:

```rust
    /// `burn`: write the staged plan's one disc, then commit its one session.
    ///
    /// The ordering is the design (`PipelineLive.burn`). The disc tree is
    /// staged **before** anything reaches the live catalog, and the snapshot
    /// embedded on the disc is the same update applied to a work-dir copy, so
    /// the disc knows about its own session. A crash before the commit leaves
    /// the catalog untouched and the staged plan intact — the staged plan is
    /// the recovery unit, and no `PLANNED` row is ever persisted.
    ///
    /// `now` is supplied by the caller rather than read from the clock, as
    /// `ProjectInit::created_at` already is: it keeps the pipeline
    /// deterministic and lets the corpus replay compare committed timestamps
    /// exactly.
    pub fn burn(
        &mut self,
        name: &str,
        now: DateTime<Utc>,
        burner: &Burner<'_>,
    ) -> Result<BurnReport, PipelineError> {
        let p = self.project(name)?;
        let scanned = staged::read_scan(&self.paths.work_dir)
            .map_err(Self::failed)?
            .ok_or_else(|| PipelineError::NoStagedScan(name.to_string()))?;
        let report = staged::read_plan(&self.paths.work_dir)
            .map_err(Self::failed)?
            .ok_or_else(|| PipelineError::NoStagedPlan(name.to_string()))?;
        if !report.oversized.is_empty() {
            return Err(PipelineError::OversizedFiles(
                report.oversized.iter().map(|f| f.rel_path.clone()).collect(),
            ));
        }

        // The catalog cannot catch this: deadness is derived from `disc_event`,
        // so the disc row itself still reads OPEN. Without this guard the
        // burner would commit a session onto a disc that no longer physically
        // exists.
        let states = self.catalog.read_disc_states(p.id).map_err(Self::failed)?;
        if states
            .iter()
            .any(|s| s.ordinal == report.disc.ordinal && s.status == DiscStatus::Dead)
        {
            return Err(PipelineError::StalePlan(format!(
                "disc {} was marked dead after this plan was staged",
                report.disc.ordinal
            )));
        }

        let snap_root = self.paths.work_dir.join("snapshot");
        if !snap_root.exists() {
            return Err(PipelineError::SnapshotMissing(name.to_string()));
        }

        let view = self.catalog.read_view(p.id).map_err(Self::failed)?;
        let change_set = diff::diff(&view, &scanned);
        let sess_ord = self.next_session_ordinal(p.id, report.disc.ordinal)?;
        let manifest = manifest::build(
            name,
            sess_ord,
            &report.disc,
            &change_set,
            report.tombstones.clone(),
        );
        let label = format!("{name} disc {}", report.disc.ordinal);
        let update = CatalogUpdate {
            project_id: p.id,
            finalize_disc_ordinals: Vec::new(),
            discs: vec![disc_write(
                &label,
                &report,
                sess_ord,
                now,
                &change_set,
                &manifest,
            )],
        };

        let tmp_catalog = self.paths.work_dir.join("burn-catalog.sqlite");
        if let Some(parent) = tmp_catalog.parent() {
            std::fs::create_dir_all(parent).map_err(Self::failed)?;
        }
        let _ = std::fs::remove_file(&tmp_catalog);
        self.catalog
            .snapshot_to(&tmp_catalog)
            .map_err(Self::failed)?;
        // Materialize the post-burn state into the snapshot this disc embeds.
        // Applies to the copy only; the live catalog is committed after the
        // disc is staged.
        Catalog::apply_update_to(&tmp_catalog, &update).map_err(Self::failed)?;

        let pointers = self.catalog.archive_pointers(p.id).map_err(Self::failed)?;
        let image = DiscImage {
            staging_dir: self
                .paths
                .work_dir
                .join("staging")
                .join(format!("disc-{}", report.disc.ordinal)),
            snapshot_root: snap_root.clone(),
            files: report.disc.files.clone(),
            metadata: DiscMetadata {
                identity: DiscIdentity {
                    project_name: name.to_string(),
                    disc_ordinal: report.disc.ordinal,
                    session_ordinal: sess_ord,
                    created_at: now,
                },
                manifest,
                catalog_snapshot: tmp_catalog.clone(),
                pointers,
            },
            close_disc: report.disc.will_finalize,
        };
        let burned = burner.burn(&image)?;

        // The crash-safe commit point: the disc exists, so the catalog may now
        // learn about it. The committed state equals the embedded snapshot.
        self.catalog.apply_update(&update).map_err(Self::failed)?;
        snapshot::release(&SnapshotHandle { root: snap_root });
        staged::clear(&self.paths.work_dir).map_err(Self::failed)?;
        let _ = std::fs::remove_file(&tmp_catalog);

        Ok(BurnReport {
            disc: DiscBurn {
                ordinal: report.disc.ordinal,
                label,
                finalized: report.disc.will_finalize,
                staging_dir: burned.staging_dir,
                image_bytes: burned.image_bytes,
            },
            remaining: report.remaining,
        })
    }

    /// One past the highest session ordinal already committed for this disc
    /// (`PipelineLive.existingSessions`). Both BURNED and VERIFIED count.
    fn next_session_ordinal(
        &self,
        project: ProjectId,
        disc_ordinal: u32,
    ) -> Result<u32, PipelineError> {
        let mut highest = 0u32;
        for state in [SessionState::Burned, SessionState::Verified] {
            for s in self
                .catalog
                .sessions_by_state(project, state)
                .map_err(Self::failed)?
            {
                if s.disc_ordinal == disc_ordinal {
                    highest = highest.max(s.ordinal);
                }
            }
        }
        Ok(highest + 1)
    }
```

and this free function at module scope:

```rust
/// `PipelineLive.toDiscWrite`: split the disc's files into added vs modified
/// using the change set, and stamp every one with its `/data/`-rooted disc
/// path. `copy_index` is 0 until `copies > 1` is activated.
fn disc_write(
    label: &str,
    report: &PlanReport,
    sess_ord: u32,
    now: DateTime<Utc>,
    cs: &ChangeSet,
    manifest: &SessionManifest,
) -> DiscWrite {
    let added_paths: std::collections::HashSet<&RelPath> =
        cs.added.iter().map(|f| &f.rel_path).collect();
    let (added, modified): (Vec<&ScannedFile>, Vec<&ScannedFile>) = report
        .disc
        .files
        .iter()
        .partition(|f| added_paths.contains(&f.rel_path));
    let fw = |f: &ScannedFile| FileWrite {
        rel_path: f.rel_path.clone(),
        content_hash: f.content_hash.clone(),
        size_bytes: f.size_bytes,
        mtime: f.mtime,
        disc_path: format!("/data/{}", f.rel_path.0),
        copy_index: 0,
    };
    DiscWrite {
        ordinal: report.disc.ordinal,
        is_new: report.disc.is_new,
        disc_type: report.disc_type,
        usable_bytes: report.disc.usable_bytes,
        status: if report.disc.will_finalize {
            DiscStatus::Finalized
        } else {
            DiscStatus::Open
        },
        label: label.to_string(),
        session: SessionWrite {
            ordinal: sess_ord,
            burned_at: now,
            manifest_hash: json::manifest_hash(manifest),
            added: added.into_iter().map(fw).collect(),
            modified: modified.into_iter().map(fw).collect(),
            deleted: report.tombstones.clone(),
        },
    }
}
```

Add the imports this needs at the top of the file: `chrono::{DateTime, Utc}`,
`diskrement_core::{manifest, ChangeSet, DiscStatus, RelPath, ScannedFile,
SessionManifest}`, `crate::burn::{json, Burner, DiscIdentity, DiscImage,
DiscMetadata}`, `crate::catalog::model::{CatalogUpdate, DiscWrite, FileWrite,
ProjectId, SessionState, SessionWrite}`, `crate::fs::snapshot::{self,
SnapshotHandle}`, and `crate::pipeline::reports::{BurnReport, DiscBurn,
PlanReport}`. Adjust to whatever is already imported; `-D warnings` rejects
duplicates.

**Note on `DiscStatus::Dead`:** confirm the spelling of the derived-dead variant
in `crates/diskrement-core/src/planner.rs`. If `read_disc_states` reports
deadness some other way (a separate field rather than a `Dead` status), follow
what P2 landed — the guard's job is to detect it, not to dictate its shape.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p diskrement-shell --lib pipeline::burn_tests`
Expected: PASS, 13 tests.

- [ ] **Step 5: Verify and commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/diskrement-shell/src/pipeline/mod.rs
git commit -m "feat(pipeline): burn the staged plan and commit after success"
```

---

### Task 9: The fixture burn oracle — the phase exit bar

**Files:**
- Create: `crates/diskrement-shell/tests/fixture_burn.rs`
- Modify: `crates/diskrement-shell/tests/support/mod.rs`

This is spec §9's P5 exit bar: *staged disc trees diff clean against the fixture
disc trees.* Two moments in the harvest are fully replayable, because the corpus
kept the whole work dir at each:

| Work dir | Pre-burn catalog | Produces | Post-burn catalog |
|---|---|---|---|
| `work/01-after-plan1` | `catalog/02-after-scan1.sqlite` | `discs/disc-1` | `catalog/03-after-burn1.sqlite` |
| `work/02-after-plan3` | `catalog/06-after-scan3.sqlite` | `discs/disc-3` | `catalog/07-after-burn3.sqlite` |

Because `now` is injected, no normalization is needed anywhere: feed the burn
the `createdAt` the Scala build stamped into that disc's `project.json`, and
every derived timestamp must come out identical.

`fixtures/README.md`'s comparison rules apply: `data/`, `manifest.json` and
`pointers.json` are byte-stable and compare byte for byte; `project.json`
carries the `createdAt` we just pinned, so it compares byte for byte too; the
embedded `catalog.sqlite` is compared **logically** (row dumps), never as bytes.

- [ ] **Step 1: Add the tree-copy helper**

In `crates/diskrement-shell/tests/support/mod.rs`, above the `pub use` line:

```rust
/// Recursively copy a fixture directory into `dest`, preserving file mtimes.
///
/// Mtimes matter because the staged tree inherits them from the snapshot; a
/// copy that dropped them would still pass a byte comparison, but it would
/// quietly stop testing the metadata half of the stager.
pub fn copy_tree(src: &Path, dest: &Path) {
    std::fs::create_dir_all(dest).expect("creating the destination");
    for entry in walkdir::WalkDir::new(src).follow_links(false) {
        let entry = entry.expect("readable fixture entry");
        let rel = entry
            .path()
            .strip_prefix(src)
            .expect("walked from src");
        let target = dest.join(rel);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target).expect("creating a directory");
        } else if entry.file_type().is_file() {
            std::fs::copy(entry.path(), &target).expect("copying a file");
            let mtime = entry
                .metadata()
                .expect("readable metadata")
                .modified()
                .expect("a modification time");
            std::fs::File::options()
                .write(true)
                .open(&target)
                .expect("reopening the copy")
                .set_modified(mtime)
                .expect("restoring the mtime");
        }
    }
}

/// Copy a whole fixture work dir into a temp dir and return both.
pub fn fixture_work_copy(rel: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::TempDir::new().expect("a writable temp directory");
    let dest = dir.path().join("work");
    copy_tree(&fixtures_dir().join(rel), &dest);
    (dir, dest)
}
```

`walkdir` is a `diskrement-shell` dependency but not a dev-dependency; if the
test crate cannot see it, add `walkdir.workspace = true` to
`crates/diskrement-shell/Cargo.toml`'s `[dev-dependencies]`.

- [ ] **Step 2: Write the failing test**

Create `crates/diskrement-shell/tests/fixture_burn.rs`:

```rust
//! Replay a harvested burn through the real pipeline and diff everything it
//! produced against what the Scala build produced at the same moment
//! (spec §4, §9's P5 row).
//!
//! Every input comes out of `fixtures/`: the pre-burn catalog, the whole work
//! dir (snapshot + staged scan + staged plan), and the `createdAt` the Scala
//! build stamped on the disc. Because the pipeline takes `now` as a parameter,
//! the comparison needs no normalization at all — committed timestamps are
//! compared as values, not as shapes.

mod support;

use std::path::{Path, PathBuf};

use diskrement_shell::burn::json::ProjectJson;
use diskrement_shell::burn::{simulated::SimulatedBurner, Burner};
use diskrement_shell::catalog::{time, Catalog};
use diskrement_shell::pipeline::{Pipeline, PipelinePaths};
use diskrement_shell::progress::NoopSink;
use support::{
    assert_dumps_match, dump_catalog, fixture_text, fixtures_dir, fixture_work_copy,
    open_fixture,
};

struct Replay {
    _tmp: tempfile::TempDir,
    work: PathBuf,
    catalog_path: PathBuf,
    staged_disc: PathBuf,
}

/// Run one harvested burn cycle against copies of the corpus.
fn replay(work_fixture: &str, pre_catalog: &str, disc_ordinal: u32) -> Replay {
    let (tmp, work) = fixture_work_copy(work_fixture);
    let catalog_path = tmp.path().join("catalog.sqlite");
    std::fs::copy(fixtures_dir().join(pre_catalog), &catalog_path)
        .unwrap_or_else(|e| panic!("copying {pre_catalog}: {e}"));

    // The instant the Scala build used for this burn, read back out of the
    // disc it wrote.
    let raw = fixture_text(&format!("discs/disc-{disc_ordinal}/.diskrement/project.json"));
    let project_json: ProjectJson = serde_json::from_str(&raw).expect("project.json parses");
    let now = time::parse(&project_json.created_at).expect("a frozen-format timestamp");

    let mut catalog = Catalog::open(&catalog_path).expect("the fixture catalog is already current");
    let sink = NoopSink;
    let burner = Burner::Simulated(SimulatedBurner::new(&sink, 2));
    {
        let mut pipeline = Pipeline::new(
            &mut catalog,
            PipelinePaths {
                work_dir: work.clone(),
                scan_cache: tmp.path().join("scancache.sqlite"),
            },
            &sink,
        );
        let report = pipeline
            .burn("p", now, &burner)
            .expect("the harvested cycle burns");
        assert_eq!(report.disc.ordinal, disc_ordinal);
    }
    drop(catalog);

    let staged_disc = work.join("staging").join(format!("disc-{disc_ordinal}"));
    Replay {
        _tmp: tmp,
        work,
        catalog_path,
        staged_disc,
    }
}

/// Every file under `dir`, relative and sorted.
fn rel_files(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(dir).follow_links(false) {
        let entry = entry.expect("readable entry");
        if entry.file_type().is_file() {
            out.push(
                entry
                    .path()
                    .strip_prefix(dir)
                    .expect("walked from dir")
                    .display()
                    .to_string(),
            );
        }
    }
    out.sort();
    out
}

/// Compare a staged disc tree against the corpus's, following
/// `fixtures/README.md`'s rules: payload and JSON byte for byte, the embedded
/// catalog logically.
fn assert_disc_matches(staged: &Path, disc_ordinal: u32) {
    let expected_root = fixtures_dir().join(format!("discs/disc-{disc_ordinal}"));

    // Payload: same set of files, same bytes.
    assert_eq!(
        rel_files(&staged.join("data")),
        rel_files(&expected_root.join("data")),
        "disc {disc_ordinal}: the staged payload has different files"
    );
    for rel in rel_files(&expected_root.join("data")) {
        assert_eq!(
            std::fs::read(staged.join("data").join(&rel)).unwrap(),
            std::fs::read(expected_root.join("data").join(&rel)).unwrap(),
            "disc {disc_ordinal}: data/{rel} differs"
        );
    }

    // Metadata JSON: byte-stable, `createdAt` included, because `now` was
    // pinned to the value the fixture carries.
    for name in ["project.json", "manifest.json", "pointers.json"] {
        assert_eq!(
            std::fs::read_to_string(staged.join(".diskrement").join(name)).unwrap(),
            std::fs::read_to_string(expected_root.join(".diskrement").join(name)).unwrap(),
            "disc {disc_ordinal}: .diskrement/{name} differs"
        );
    }

    // The embedded catalog: logically, never as bytes — SQLite file layout is
    // not stable across library builds, and xerial's SQLite is not
    // libsqlite3-sys's.
    let ours = rusqlite::Connection::open_with_flags(
        staged.join(".diskrement/catalog.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("the embedded catalog opens");
    let theirs = open_fixture(&format!("discs/disc-{disc_ordinal}/.diskrement/catalog.sqlite"));
    assert_dumps_match(&dump_catalog(&theirs), &dump_catalog(&ours));
}

/// Cycle 1: two 800-byte files spill across a 1000-byte disc, so disc 1 holds
/// `a.bin` alone and is sealed. The initial-add manifest shape.
#[test]
fn burning_cycle_one_reproduces_the_scala_disc_and_catalog() {
    let r = replay("work/01-after-plan1", "catalog/02-after-scan1.sqlite", 1);
    assert_disc_matches(&r.staged_disc, 1);

    let ours = rusqlite::Connection::open_with_flags(
        &r.catalog_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("the committed catalog opens");
    let theirs = open_fixture("catalog/03-after-burn1.sqlite");
    assert_dumps_match(&dump_catalog(&theirs), &dump_catalog(&ours));
}

/// Cycle 3: `a.bin` superseded and `b.bin` deleted — the only harvested cycle
/// with a non-empty `modified` list and a tombstone.
#[test]
fn burning_cycle_three_reproduces_the_scala_disc_and_catalog() {
    let r = replay("work/02-after-plan3", "catalog/06-after-scan3.sqlite", 3);
    assert_disc_matches(&r.staged_disc, 3);

    let ours = rusqlite::Connection::open_with_flags(
        &r.catalog_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("the committed catalog opens");
    let theirs = open_fixture("catalog/07-after-burn3.sqlite");
    assert_dumps_match(&dump_catalog(&theirs), &dump_catalog(&ours));
}

/// The embedded snapshot and the committed catalog are two applications of one
/// update and must agree — that equality is the whole reason the double-apply
/// exists instead of a file swap.
#[test]
fn the_embedded_snapshot_equals_the_committed_catalog() {
    let r = replay("work/01-after-plan1", "catalog/02-after-scan1.sqlite", 1);

    let committed = rusqlite::Connection::open_with_flags(
        &r.catalog_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let embedded = rusqlite::Connection::open_with_flags(
        r.staged_disc.join(".diskrement/catalog.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_dumps_match(&dump_catalog(&committed), &dump_catalog(&embedded));
}

/// Burning leaves the work dir in the state the next cycle expects: staging
/// kept (the staged-disc verifier reads it), staged JSON and snapshot gone.
#[test]
fn the_replayed_burn_leaves_the_work_dir_as_the_scala_build_did() {
    let r = replay("work/01-after-plan1", "catalog/02-after-scan1.sqlite", 1);
    assert!(r.staged_disc.is_dir(), "staging survives the burn");
    assert!(!r.work.join("staged/scan.json").exists());
    assert!(!r.work.join("staged/plan.json").exists());
    assert!(!r.work.join("snapshot").exists());
    assert!(!r.work.join("burn-catalog.sqlite").exists());
}
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p diskrement-shell --test fixture_burn -- --nocapture`
Expected: PASS, 4 tests.

**If the catalog dumps differ only in a timestamp column**, do not normalize it
away — find where a fresh `Utc::now()` leaked into the write path (the likely
suspects are `apply_update`'s `first_seen_at` handling and anything in
`mark_dead`) and thread `now` through instead. A normalization rule here would
be a hole in the phase's own exit bar.

**If they differ in `manifest_hash`**, Task 3 should have caught it; re-run that
test and fix the compact encoding.

**If the payload bytes differ**, the stager copied the wrong file — check that
`rel_path` is joined onto the snapshot root, not the source root.

- [ ] **Step 4: Verify and commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/diskrement-shell/tests/fixture_burn.rs crates/diskrement-shell/tests/support/mod.rs crates/diskrement-shell/Cargo.toml
git commit -m "test(burn): replay two harvested burns and diff against the corpus"
```

---

### Task 10: `render::burn_report`

**Files:**
- Modify: `crates/diskrement-cli/src/render.rs`

Read `cli/src/main/scala/diskrement/cli/Render.scala`'s `def burn`, and check it
against `fixtures/run-manifest.txt` steps 04, 08, 11, 15 and 19 — that file is
the cross-implementation render oracle and it is the authority on spacing.

The exact shape, from step 04:

```
✔ Wrote disc 1  —  label: "p disc 1"
Status: FINALIZED — set this disc aside, it is sealed.
~1 more discs of changes remain. Run: scan → plan → burn
```

Note **two spaces** on each side of the em dash in the first line, and a plain
em dash with single spaces in the second.

- [ ] **Step 1: Write the failing tests**

Append to `crates/diskrement-cli/src/render.rs` (create the `mod tests` block if
there is none):

```rust
#[cfg(test)]
mod burn_render_tests {
    use super::*;
    use diskrement_shell::pipeline::reports::{BurnReport, DiscBurn, RemainingEstimate};

    fn report(finalized: bool, more_discs: usize) -> BurnReport {
        BurnReport {
            disc: DiscBurn {
                ordinal: 1,
                label: "p disc 1".to_string(),
                finalized,
                staging_dir: "/w/staging/disc-1".into(),
                image_bytes: 4096,
            },
            remaining: RemainingEstimate {
                more_discs,
                more_files: 1,
                more_bytes: 800,
            },
        }
    }

    /// `fixtures/run-manifest.txt` step 04, byte for byte.
    #[test]
    fn a_finalized_disc_with_more_to_come_matches_the_render_oracle() {
        assert_eq!(
            burn_report(&report(true, 1)),
            "✔ Wrote disc 1  —  label: \"p disc 1\"\n\
             Status: FINALIZED — set this disc aside, it is sealed.\n\
             ~1 more discs of changes remain. Run: scan → plan → burn"
        );
    }

    /// `fixtures/run-manifest.txt` step 19: the set is complete.
    #[test]
    fn an_open_disc_with_nothing_left_matches_the_render_oracle() {
        assert_eq!(
            burn_report(&report(false, 0)),
            "✔ Wrote disc 1  —  label: \"p disc 1\"\n\
             Status: left OPEN — may receive more sessions later.\n\
             No more changes — the set is up to date."
        );
    }

    /// The first line's spacing is easy to lose in a reformat and the oracle
    /// is byte-exact, so pin it on its own.
    #[test]
    fn the_wrote_line_uses_two_spaces_around_the_dash() {
        let line = burn_report(&report(false, 0))
            .lines()
            .next()
            .unwrap()
            .to_string();
        assert_eq!(line, "✔ Wrote disc 1  —  label: \"p disc 1\"");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p diskrement-cli --bin diskrement burn_render_tests`
Expected: FAIL — `cannot find function 'burn_report' in this scope`.

- [ ] **Step 3: Implement the renderer**

Append to the non-test part of `crates/diskrement-cli/src/render.rs`:

```rust
/// `Render.burn`. One disc per cycle, so there is exactly one block.
pub fn burn_report(r: &BurnReport) -> String {
    let d = &r.disc;
    let status = if d.finalized {
        "Status: FINALIZED — set this disc aside, it is sealed."
    } else {
        "Status: left OPEN — may receive more sessions later."
    };
    let next = if r.remaining.more_discs == 0 {
        "No more changes — the set is up to date.".to_string()
    } else {
        format!(
            "~{} more discs of changes remain. Run: scan → plan → burn",
            r.remaining.more_discs
        )
    };
    format!(
        "✔ Wrote disc {}  —  label: \"{}\"\n{status}\n{next}",
        d.ordinal, d.label
    )
}
```

and extend the `use` line to import `BurnReport` from
`diskrement_shell::pipeline::reports`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p diskrement-cli --bin diskrement burn_render_tests`
Expected: PASS, 3 tests.

- [ ] **Step 5: Verify and commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/diskrement-cli/src/render.rs
git commit -m "feat(cli): render the burn report"
```

---

### Task 11: The `burn` command, the prompt, and backend resolution

**Files:**
- Create: `crates/diskrement-cli/src/prompt.rs`
- Create: `crates/diskrement-cli/src/backend.rs`
- Modify: `crates/diskrement-cli/src/args.rs`
- Modify: `crates/diskrement-cli/src/main.rs`

Read `cli/src/main/scala/diskrement/cli/Prompt.scala`,
`BackendResolver.scala`, and `Runner.scala`'s `DiskrementCommand.Burn` case.

Three things to get right:

- **The prompt is skipped by `--yes`, and a refusal is exit 0** printing
  `Aborted.` — not a `BurnError::Aborted`, which is P8's path for an operator
  aborting a *real* burn mid-flight. EOF or a read error counts as "no".
- **`burn` never re-plans.** The staged plan carries the disc type chosen at
  `plan` time; re-planning here would silently overwrite a
  `plan --disc-type ...` choice with the project default.
- **Backend resolution's default is `real`.** Spec §9 gives the real burner to
  P8, so the `Real` arm fails fast with a clear message. What must not happen is
  the Rust binary silently simulating a burn the Scala binary would have sent to
  `/dev/sr0` — nothing in the differential harness would catch that, because
  every scenario sets `DISKREMENT_BURNER=simulated`.

Note the deliberate asymmetry in the Scala resolver, and preserve it: the
**flag** is an enumeration, so `--burner bogus` is an argument error, while an
**unrecognised env value** falls through to the default rather than failing.

- [ ] **Step 1: Write the failing tests**

Create `crates/diskrement-cli/src/backend.rs` with tests only for now:

```rust
//! Pure burner/device resolution (`BackendResolver.scala`).

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn the_default_is_the_real_burner_on_dev_sr0() {
        assert_eq!(
            resolve(None, None, &no_env),
            Backend::Real(BurnDevice::RealDevice("/dev/sr0".to_string()))
        );
    }

    #[test]
    fn the_flag_beats_the_environment() {
        let env = |k: &str| (k == "DISKREMENT_BURNER").then(|| "real".to_string());
        assert_eq!(
            resolve(Some(BurnerKind::Simulated), None, &env),
            Backend::Simulated
        );
    }

    #[test]
    fn the_environment_selects_the_simulated_burner() {
        let env = |k: &str| (k == "DISKREMENT_BURNER").then(|| "  SIMULATED \n".to_string());
        assert_eq!(resolve(None, None, &env), Backend::Simulated);
    }

    /// An unrecognised env value falls through to the default rather than
    /// failing — the flag is an enumeration, the environment is not.
    #[test]
    fn an_unknown_environment_value_falls_back_to_the_default() {
        let env = |k: &str| (k == "DISKREMENT_BURNER").then(|| "pretend".to_string());
        assert_eq!(
            resolve(None, None, &env),
            Backend::Real(BurnDevice::RealDevice("/dev/sr0".to_string()))
        );
    }

    #[test]
    fn the_device_comes_from_the_flag_then_the_environment_then_the_default() {
        let env = |k: &str| (k == "DISKREMENT_DEVICE").then(|| "/dev/sr9".to_string());
        assert_eq!(
            resolve(None, Some("/dev/sr1"), &env),
            Backend::Real(BurnDevice::RealDevice("/dev/sr1".to_string()))
        );
        assert_eq!(
            resolve(None, None, &env),
            Backend::Real(BurnDevice::RealDevice("/dev/sr9".to_string()))
        );
    }

    /// An empty `DISKREMENT_DEVICE` is ignored (`filter(_.nonEmpty)`).
    #[test]
    fn an_empty_device_variable_is_ignored() {
        let env = |k: &str| (k == "DISKREMENT_DEVICE").then(String::new);
        assert_eq!(
            resolve(None, None, &env),
            Backend::Real(BurnDevice::RealDevice("/dev/sr0".to_string()))
        );
    }
}
```

Create `crates/diskrement-cli/src/prompt.rs` with tests only for now:

```rust
//! The `[y/N]` burn confirmation (`Prompt.scala`).

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assume_yes_never_reads() {
        assert!(confirm_from("Burn?", true, || panic!("must not read")));
    }

    #[test]
    fn only_y_and_yes_mean_yes() {
        for answer in ["y", "Y", "yes", "YES", " yes \n"] {
            assert!(
                confirm_from("Burn?", false, || Some(answer.to_string())),
                "{answer:?} should confirm"
            );
        }
        for answer in ["n", "no", "", "sure", "yep"] {
            assert!(
                !confirm_from("Burn?", false, || Some(answer.to_string())),
                "{answer:?} should refuse"
            );
        }
    }

    /// EOF or a read error is "no": an unattended run must never burn.
    #[test]
    fn eof_is_a_refusal() {
        assert!(!confirm_from("Burn?", false, || None));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p diskrement-cli --bin diskrement backend:: prompt::`
Expected: FAIL — the modules are not declared and the functions do not exist.

- [ ] **Step 3: Implement the prompt and the resolver**

Prepend to `crates/diskrement-cli/src/prompt.rs`:

```rust
use std::io::Write as _;

/// Ask, then read one line. `assume_yes` skips the read entirely.
pub fn confirm(message: &str, assume_yes: bool) -> bool {
    confirm_from(message, assume_yes, || {
        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line),
        }
    })
}

/// `confirm` with the read injected.
///
/// Anything but `y`/`yes` (case-insensitive, trimmed) refuses, and so does EOF
/// or a read error — an unattended run must never burn by accident.
pub fn confirm_from(message: &str, assume_yes: bool, read_line: impl FnOnce() -> Option<String>) -> bool {
    if assume_yes {
        return true;
    }
    print!("{message} [y/N] ");
    let _ = std::io::stdout().flush();
    match read_line() {
        Some(line) => {
            let t = line.trim().to_lowercase();
            t == "y" || t == "yes"
        }
        None => false,
    }
}
```

Prepend to `crates/diskrement-cli/src/backend.rs`:

```rust
use diskrement_shell::burn::BurnDevice;

use crate::args::BurnerKind;

/// Which backend a run should use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Backend {
    Simulated,
    Real(BurnDevice),
}

/// Precedence: flag > environment > default (the real burner on `/dev/sr0`).
///
/// Deliberately asymmetric, matching `BackendResolver.scala`: the flag is an
/// enumeration, so a bad value is an argument error, while an unrecognised
/// `DISKREMENT_BURNER` falls through to the default instead of failing.
pub fn resolve(
    burner_flag: Option<BurnerKind>,
    device_flag: Option<&str>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Backend {
    let kind = burner_flag.or_else(|| {
        match env("DISKREMENT_BURNER")
            .map(|v| v.trim().to_lowercase())
            .as_deref()
        {
            Some("simulated") => Some(BurnerKind::Simulated),
            Some("real") => Some(BurnerKind::Real),
            _ => None,
        }
    });
    match kind.unwrap_or(BurnerKind::Real) {
        BurnerKind::Simulated => Backend::Simulated,
        BurnerKind::Real => {
            let device = device_flag
                .map(str::to_string)
                .or_else(|| env("DISKREMENT_DEVICE").filter(|v| !v.is_empty()))
                .unwrap_or_else(|| "/dev/sr0".to_string());
            Backend::Real(BurnDevice::RealDevice(device))
        }
    }
}
```

- [ ] **Step 4: Add the command surface**

In `crates/diskrement-cli/src/args.rs`, add the enum and change the global flag:

```rust
/// The burn backend, as a flag. zio-cli's `Options.enumeration` rejected an
/// unknown value as an argument error and clap's `ValueEnum` does the same.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
#[value(rename_all = "lower")]
pub enum BurnerKind {
    Real,
    Simulated,
}
```

Change the global option from `pub burner: Option<String>` to:

```rust
    #[arg(long, global = true, value_enum)]
    pub burner: Option<BurnerKind>,
```

and add the subcommand to `enum Command`:

```rust
    /// Burn the staged plan (with confirmation).
    Burn {
        name: String,
        #[arg(long, default_value_t = false)]
        yes: bool,
    },
```

- [ ] **Step 5: Wire the dispatch**

In `crates/diskrement-cli/src/main.rs`, add `mod backend;` and `mod prompt;` to
the module list, add `Command::Burn { name, .. } => name` to the
`project_name` match, and add the dispatch arm:

```rust
        Command::Burn { name, yes } => {
            // Burn the plan exactly as staged: it carries the disc type chosen
            // at `plan` time, and re-planning here would silently discard a
            // `plan --disc-type ...` choice. `render::burn_report` then reports
            // what was actually written.
            let sink = progress::ConsoleProgress::new();
            let burner = match backend::resolve(cli.burner, cli.device.as_deref(), &env_lookup) {
                backend::Backend::Simulated => {
                    let par = diskrement_shell::fs::scanner::parallelism_from_env();
                    for w in &par.warnings {
                        eprintln!("{w}");
                    }
                    Burner::Simulated(SimulatedBurner::new(&sink, par.value))
                }
                // P8 replaces this arm with the xorriso burner. Refusing here
                // rather than falling back to the simulated backend is the
                // point: a silently simulated burn is a divergence nothing in
                // the differential harness would catch.
                backend::Backend::Real(device) => {
                    return Err(PipelineError::Failed(format!(
                        "the real burner is not implemented yet (device {}); \
                         re-run with --burner simulated",
                        device.xorriso_address()
                    )))
                }
            };

            if !prompt::confirm(&format!("Burn the staged plan for {name}?"), yes) {
                say("Aborted.");
                return Ok(());
            }

            let mut catalog = open_catalog(&resolved.catalog)?;
            let p = find_or_fail(&catalog, &name)?;
            let mut pipeline = Pipeline::new(
                &mut catalog,
                PipelinePaths {
                    work_dir: PathBuf::from(&p.work_dir),
                    scan_cache: paths::scan_cache_path(&resolved.catalog, &name),
                },
                &sink,
            );
            let report = pipeline.burn(&name, chrono::Utc::now(), &burner)?;
            say(&render::burn_report(&report));
            Ok(())
        }
```

Add `use diskrement_shell::burn::{simulated::SimulatedBurner, Burner};` to the
imports.

**Order matters here:** the Scala `Runner` resolves the project *before*
prompting, so an unknown project fails with exit 2 without asking anything.
Check `Runner.scala`'s `withProject` and match it — if the project lookup comes
first there, move `open_catalog`/`find_or_fail` above the prompt.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p diskrement-cli`
Expected: PASS, including the nine new backend/prompt tests.

- [ ] **Step 7: Verify and commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/diskrement-cli/src
git commit -m "feat(cli): the burn command, the confirmation prompt, and backend resolution"
```

---

### Task 12: End-to-end `burn` through the spawned binary

**Files:**
- Modify: `crates/diskrement-cli/tests/binary.rs`

The unit tests prove the pieces; this proves the wiring — argument parsing,
path resolution, the prompt, the exit ladder, and the rendered text, through a
real process.

`init` refuses a non-CoW work dir, so these tests need `TMPDIR` on
btrfs/XFS/bcachefs, exactly as the existing `p3` module does.

- [ ] **Step 1: Write the failing tests**

Append to `crates/diskrement-cli/tests/binary.rs`:

```rust
mod p5 {
    use super::*;

    fn cmd() -> Command {
        let mut c = Command::cargo_bin("diskrement").expect("binary builds");
        // Never touch a real drive from a test.
        c.env("DISKREMENT_BURNER", "simulated");
        c.env("DISKREMENT_HASH_PARALLELISM", "2");
        c
    }

    struct Project {
        _dir: tempfile::TempDir,
        catalog: std::path::PathBuf,
        work: std::path::PathBuf,
        source: std::path::PathBuf,
    }

    fn project() -> Project {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let source = root.join("src");
        let work = root.join("work");
        let catalog = root.join("catalog/cat.sqlite");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(catalog.parent().unwrap()).unwrap();
        std::fs::write(source.join("a.bin"), vec![b'a'; 800]).unwrap();
        Project {
            _dir: dir,
            catalog,
            work,
            source,
        }
    }

    fn base(p: &Project) -> Command {
        let mut c = cmd();
        c.arg("--catalog").arg(&p.catalog);
        c
    }

    fn init_scan_plan(p: &Project) {
        base(p)
            .args(["init", "p", "--source"])
            .arg(&p.source)
            .arg("--work-dir")
            .arg(&p.work)
            .assert()
            .success();
        base(p).args(["scan", "p"]).assert().success();
        base(p).args(["plan", "p"]).assert().success();
    }

    #[test]
    fn burn_writes_the_disc_and_reports_it() {
        let p = project();
        init_scan_plan(&p);

        base(&p)
            .args(["burn", "p", "--yes"])
            .assert()
            .success()
            .stdout(contains("✔ Wrote disc 1  —  label: \"p disc 1\""))
            .stdout(contains("Status: left OPEN — may receive more sessions later."))
            .stdout(contains("No more changes — the set is up to date."));

        assert!(p.work.join("staging/disc-1/data/a.bin").is_file());
        assert!(p.work.join("staging/disc-1/.diskrement/manifest.json").is_file());
        assert!(p.work.join("staging/disc-1/.diskrement/catalog.sqlite").is_file());
    }

    /// A second burn with nothing staged is the state error, exit 2.
    #[test]
    fn burning_twice_fails_with_the_staged_state_error() {
        let p = project();
        init_scan_plan(&p);
        base(&p).args(["burn", "p", "--yes"]).assert().success();

        base(&p)
            .args(["burn", "p", "--yes"])
            .assert()
            .code(2)
            .stderr(contains("no staged scan for p (run scan)"));
    }

    #[test]
    fn burning_an_unknown_project_exits_two() {
        let p = project();
        init_scan_plan(&p);
        base(&p)
            .args(["burn", "nosuch", "--yes"])
            .assert()
            .code(2)
            .stderr(contains("project not found: nosuch"));
    }

    /// Without `--yes` and with no terminal, the read hits EOF and the burn is
    /// refused: exit 0, nothing written.
    #[test]
    fn declining_the_prompt_is_a_successful_no_op() {
        let p = project();
        init_scan_plan(&p);

        base(&p)
            .args(["burn", "p"])
            .write_stdin("n\n")
            .assert()
            .success()
            .stdout(contains("Aborted."));

        assert!(!p.work.join("staging").exists(), "nothing was staged");
        // The plan survives, so the operator can retry.
        assert!(p.work.join("staged/plan.json").is_file());
    }

    /// The real burner is P8's. Until then the CLI refuses rather than
    /// silently simulating.
    #[test]
    fn asking_for_the_real_burner_is_refused_for_now() {
        let p = project();
        init_scan_plan(&p);

        let mut c = Command::cargo_bin("diskrement").expect("binary builds");
        c.arg("--catalog")
            .arg(&p.catalog)
            .args(["--burner", "real", "burn", "p", "--yes"])
            .env_remove("DISKREMENT_BURNER")
            .assert()
            .code(5)
            .stderr(contains("the real burner is not implemented yet"));
    }

    /// A bad `--burner` value is an argument error (exit 1), the one place
    /// spec §3.2 lets clap and zio-cli word things differently.
    #[test]
    fn a_bogus_burner_flag_is_an_argument_error() {
        let p = project();
        base(&p)
            .args(["--burner", "pretend", "burn", "p", "--yes"])
            .assert()
            .code(1);
    }

    /// The full cycle the corpus records: spill across two discs, one disc per
    /// burn, with the remaining-discs line changing between them.
    #[test]
    fn two_cycles_spill_across_two_discs() {
        let p = project();
        std::fs::write(p.source.join("b.bin"), vec![b'b'; 800]).unwrap();
        let with_shrink = |p: &Project| {
            let mut c = base(p);
            c.env("DISKREMENT_DISC_BYTES", "1000");
            c
        };

        with_shrink(&p)
            .args(["init", "p", "--source"])
            .arg(&p.source)
            .arg("--work-dir")
            .arg(&p.work)
            .assert()
            .success();
        with_shrink(&p).args(["scan", "p"]).assert().success();
        with_shrink(&p).args(["plan", "p"]).assert().success();
        with_shrink(&p)
            .args(["burn", "p", "--yes"])
            .assert()
            .success()
            .stdout(contains("✔ Wrote disc 1"))
            .stdout(contains("more discs of changes remain"));

        with_shrink(&p).args(["scan", "p"]).assert().success();
        with_shrink(&p).args(["plan", "p"]).assert().success();
        with_shrink(&p)
            .args(["burn", "p", "--yes"])
            .assert()
            .success()
            .stdout(contains("✔ Wrote disc 2"))
            .stdout(contains("No more changes — the set is up to date."));

        assert!(p.work.join("staging/disc-1/data/a.bin").is_file());
        assert!(p.work.join("staging/disc-2/data/b.bin").is_file());
    }
}
```

`write_stdin` needs `assert_cmd`'s `Command`; it is already the import in this
file.

- [ ] **Step 2: Run the tests**

Run: `cargo test -p diskrement-cli --test binary p5`
Expected: PASS, 7 tests.

If `init` fails with `work dir is not CoW-capable`, set `TMPDIR` to a directory
on btrfs/XFS/bcachefs and re-run. That is an environment limitation, not a bug —
report it rather than weakening the test.

The spill test's disc-1/disc-2 assertion depends on which file the planner
places first; if it places `b.bin` on disc 1, swap the two assertions to match
what the planner actually did rather than forcing an order.

- [ ] **Step 3: Verify and commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/diskrement-cli/tests/binary.rs
git commit -m "test(cli): burn end to end through the spawned binary"
```

---

### Task 13: Open the differential gate on `burn`

**Files:**
- Modify: `crates/diskrement-difftest/src/runner.rs`
- Modify: `crates/diskrement-difftest/src/corpus.rs`
- Modify: `crates/diskrement-difftest/src/state_diff.rs`

Spec §8.1: every phase from P3 on extends the harness with the commands it
lands, and the phase cannot land while its own commands disagree with the Scala
binary. **`RUST_IMPLEMENTED` must gain `"burn"` in the same commit that lands
the scenarios** — otherwise the Rust and interleaved modes keep skipping every
scenario that uses it and a fully-skipped run still exits green.

`state_diff.rs` already walks `staging/disc-N` for JSON and payload bytes; its
own comment defers the embedded `catalog.sqlite` to this phase. Add it now, as a
logical dump keyed by disc — it is a copy of the live catalog, so a divergence
between the two implementations' embedded snapshots is a real finding that the
live-catalog comparison alone would not localize.

- [ ] **Step 1: Add the embedded catalog comparison**

In `crates/diskrement-difftest/src/state_diff.rs`, add a field to `Snapshot`:

```rust
    /// Each staged disc's embedded `catalog.sqlite`, dumped logically and
    /// keyed `disc-N`.
    ///
    /// It is a *copy* of the catalog compared above, so a drift shows up
    /// twice — deliberately: the embedded snapshot is written by a different
    /// path (`snapshot_to` + `apply_update_to` on a work copy), and comparing
    /// it separately says which of the two paths drifted.
    pub disc_catalogs: BTreeMap<String, Vec<String>>,
```

and populate it inside the per-disc loop in `snapshot`, replacing the comment
that defers this:

```rust
        let embedded = meta.join("catalog.sqlite");
        if embedded.is_file() {
            disc_catalogs.insert(
                disc_name.clone(),
                dump_normalized(&embedded, CATALOG_TABLES, norm),
            );
        }
```

declaring `let mut disc_catalogs = BTreeMap::new();` next to `disc_json`, adding
it to the constructed `Snapshot`, and extending whatever compares two
`Snapshot`s to compare the new field. Find that comparison — it is either a
`PartialEq` derive or an explicit diff function in this file — and treat a
missing comparison as the bug it would be.

- [ ] **Step 2: Add the burn scenarios**

In `crates/diskrement-difftest/src/corpus.rs`, add to `all()` after
`plan_errors()`:

```rust
        burn_basics(),
        burn_spill_cycles(),
        burn_errors(),
```

and the scenarios themselves:

```rust
/// One full cycle: scan, plan, burn, and the status that reports it. The
/// simplest proof that a Rust burn and a Scala burn leave the same catalog,
/// the same staged disc tree and the same embedded snapshot.
pub fn burn_basics() -> Scenario {
    Scenario {
        name: "burn-basics",
        disc_bytes: None,
        steps: vec![
            write("a.bin", 800),
            write("nested/b.bin", 120),
            cmd(
                "init p",
                &["init", "p", "--source", "{source}", "--work-dir", "{work}"],
            ),
            cmd("scan p", &["scan", "p"]),
            cmd("plan p", &["plan", "p"]),
            cmd("burn p --yes", &["burn", "p", "--yes"]),
            cmd("status p", &["status", "p"]),
            // Nothing changed, so the next cycle has nothing to plan — and the
            // burn that follows must fail identically on both sides.
            cmd("scan p", &["scan", "p"]),
            cmd("plan p", &["plan", "p"]),
        ],
    }
}

/// The spill, burned: one disc per cycle, a second session's ordinal, a
/// superseded file and a tombstone. This is the shape that produced the
/// corpus's disc-1 through disc-3, minus the coaster (P7's `disc mark-dead`).
pub fn burn_spill_cycles() -> Scenario {
    Scenario {
        name: "burn-spill-cycles",
        disc_bytes: Some(1000),
        steps: vec![
            write("a.bin", 800),
            write("b.bin", 800),
            cmd(
                "init p",
                &["init", "p", "--source", "{source}", "--work-dir", "{work}"],
            ),
            // cycle 1: a.bin fills disc 1, which is sealed; b.bin spills.
            cmd("scan p", &["scan", "p"]),
            cmd("plan p", &["plan", "p"]),
            cmd("burn p --yes", &["burn", "p", "--yes"]),
            // cycle 1b: the remainder lands on disc 2, left OPEN.
            cmd("scan p", &["scan", "p"]),
            cmd("plan p", &["plan", "p"]),
            cmd("burn p --yes", &["burn", "p", "--yes"]),
            // cycle 2: supersede a, delete b — the modify+delete manifest.
            modify("a.bin", 800),
            delete("b.bin"),
            cmd("scan p", &["scan", "p"]),
            cmd("plan p", &["plan", "p"]),
            cmd("burn p --yes", &["burn", "p", "--yes"]),
            cmd("status p", &["status", "p"]),
            // Seal the open disc with a file-less session.
            cmd("scan p", &["scan", "p"]),
            cmd("plan p --finalize", &["plan", "p", "--finalize"]),
            cmd("burn p --yes", &["burn", "p", "--yes"]),
            cmd("status p", &["status", "p"]),
        ],
    }
}

/// `burn`'s refusals, compared by agreement. Each is a state or environment
/// error, so both the message and the exit code are contract.
pub fn burn_errors() -> Scenario {
    Scenario {
        name: "burn-errors",
        disc_bytes: Some(500),
        steps: vec![
            write("big.bin", 800),
            write("small.bin", 100),
            cmd(
                "init p",
                &["init", "p", "--source", "{source}", "--work-dir", "{work}"],
            ),
            // Nothing staged yet.
            cmd("burn before scan", &["burn", "p", "--yes"]),
            cmd("scan p", &["scan", "p"]),
            // Scanned but not planned.
            cmd("burn before plan", &["burn", "p", "--yes"]),
            // `plan` records the oversized file and succeeds; `burn` is the
            // half that refuses it (exit 3).
            cmd("plan p", &["plan", "p"]),
            cmd("burn with an oversized file", &["burn", "p", "--yes"]),
            cmd("burn on an unknown project", &["burn", "nosuch", "--yes"]),
        ],
    }
}
```

- [ ] **Step 3: Add the scenario-hygiene test**

In `crates/diskrement-difftest/src/corpus.rs`'s `mod tests`:

```rust
    #[test]
    fn the_burn_scenarios_use_only_commands_p5_lands() {
        for s in [burn_basics(), burn_spill_cycles(), burn_errors()] {
            for c in s.commands() {
                assert!(
                    ["init", "scan", "status", "plan", "burn"].contains(&c),
                    "scenario '{}' uses '{c}', which P5 does not land",
                    s.name
                );
            }
        }
    }

    /// The oversize scenario must actually reach the refusal: a 800-byte file
    /// against a 500-byte disc, with a 100-byte file that still plans.
    #[test]
    fn the_burn_error_scenario_stages_a_plan_that_burn_must_refuse() {
        assert_eq!(burn_errors().disc_bytes, Some(500));
    }
```

- [ ] **Step 4: Open the gate**

In `crates/diskrement-difftest/src/runner.rs`:

```rust
pub const RUST_IMPLEMENTED: &[&str] = &["burn", "frobnicate", "init", "plan", "scan", "status"];
```

- [ ] **Step 5: Run the differential suite in all three modes**

```bash
cargo build -p diskrement-cli
sbt cli/stage
DISKREMENT_SCALA_BIN=$PWD/cli/target/universal/stage/bin/diskrement \
  cargo test -p diskrement-difftest -- --ignored --nocapture
```

Expected: every scenario runs in all three modes and passes. **Read the printed
scenario counts** — a run that skips everything still exits green. If
`DISKREMENT_SCALA_BIN` is unset the suite skips and says so; that is not a pass.

If the scratch root is not on a CoW filesystem the runner skips with that
reason. Set `TMPDIR` to a btrfs/XFS/bcachefs directory and re-run.

**When a diff appears, the Scala binary is right.** Fix the Rust side. Widening
a normalization rule to make a failing diff pass is how this harness stops
working without anyone noticing (CLAUDE.md).

- [ ] **Step 6: Verify and commit**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/diskrement-difftest/src
git commit -m "feat(difftest): teach the harness that burn exists"
```

---

### Task 14: Land the phase

**Files:**
- Modify: `docs/superpowers/plans/2026-09-07-rust-rewrite-p5-burn.md`

- [ ] **Step 1: Run the full verification**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

All clean. Record the new counts; they must exceed the 278 passed / 3 ignored
baseline.

- [ ] **Step 2: Run the differential suite once more, in all three modes**

```bash
sbt cli/stage
DISKREMENT_SCALA_BIN=$PWD/cli/target/universal/stage/bin/diskrement \
  cargo test -p diskrement-difftest -- --ignored --nocapture
```

Check the printed scenario counts, not just the exit code.

- [ ] **Step 3: Confirm the Scala tree still builds**

```bash
sbt clean test
```

P5 touches nothing under `core/`, `shell/`, `cli/`, but the branch must leave
the Scala build green until P10.

- [ ] **Step 4: Append the execution deviations**

Add a `## Execution deviations` section to this plan document recording every
place the implementation diverged from what is written above, and why. Per
CLAUDE.md, deviations live in the plan doc, not in commit messages.

- [ ] **Step 5: Merge and clean up**

```bash
git add docs/superpowers/plans/2026-09-07-rust-rewrite-p5-burn.md
git commit -m "docs(plan): record the P5 execution deviations"
git switch feat/rust-rewrite
git merge --no-ff feat/rewrite-p5 -m "Merge feat/rewrite-p5: the stager, the disc metadata, the embedded catalog snapshot and the simulated burn — staged disc trees byte-identical to the corpus, committed catalogs row-for-row identical, proven against the Scala binary in all three harness modes"
git branch -d feat/rewrite-p5
```

---

## Phase exit bar (spec §9, P5 row)

- [ ] `cargo test --workspace` green, above the 278/3 baseline
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean
- [ ] `cargo fmt --check` clean
- [ ] **Staged disc trees diff clean against the fixture disc trees** — Task 9,
      byte-for-byte on `data/` and all three JSON files, logically on the
      embedded catalog
- [ ] The committed catalog after a replayed burn is row-for-row identical to
      the corpus's post-burn catalog — Task 9
- [ ] `session.manifest_hash` matches the Scala-computed hash for all four
      manifest shapes — Task 3
- [ ] `pointers.json` renders byte-identically to the corpus — Task 4
- [ ] `burn` stdout matches `run-manifest.txt`'s wording — Task 10
- [ ] The differential suite passes in all three modes for every scenario
      reachable with `init`/`scan`/`status`/`plan`/`burn` — Task 13
- [ ] `sbt clean test` still green

## Spec coverage

| Spec requirement | Task |
|---|---|
| §3.1 on-disc `/.diskrement` layout | 2, 5, 9 |
| §3.1 catalog schema unchanged, append-only | 8, 9 |
| §3.2 `burn` command surface, `--yes` | 11, 12 |
| §3.2 exit codes 2/3/5/6 and the aborted-burn 0 | 1, 11, 12 |
| §4 corpus as the oracle, render oracle | 3, 4, 9, 10 |
| §5.3 `burn/` module layout | 1, 2, 5, 6 |
| §5.5 closed `Burner` enum, no trait objects | 1 |
| §5.5 `thiserror` per module, one `exit_code()` | 1 |
| §6 concurrency confined to the shell (rayon re-hash) | 6 |
| §8.1 the phase's differential task | 13 |
| §9 P5 scope and exit bar | all |
| CLAUDE.md: commit-after-success, post-burn embedded snapshot, one disc per cycle, `StalePlan` advice | 8, 9 |
| CLAUDE.md: `FICLONE` does not preserve mtime | 6 |

## Execution deviations

Every place the landed implementation diverges from the task texts above, and
why. Formatting-only reflows imposed by the `cargo fmt --check` gate are not
listed per file; they occurred in most tasks and never changed semantics.

- **Task 1:** trimmed the now-stale trailing clause of an `error.rs` doc
  comment ("`BurnError` arrives in a later phase") — Task 1 itself made it
  false. The conditional `Display` additions for `RelPath`/`ContentHash` were
  no-ops: both impls already existed in `crates/diskrement-core/src/model.rs`.
- **Task 2:** the plan's expected timestamp literal
  `2026-09-05T20:41:06.623959846Z` was arithmetically wrong for
  `Utc.timestamp_opt(1_757_100_066, 623_959_846)`; corrected to
  `2025-09-05T19:21:06.623959846Z` exactly as the plan's own note sanctions
  ("fix the expected string to whatever that instant formats to"). The unused
  parent import of `ManifestChangeKind` was dropped (denied by
  `-Wunused:all` convention; the test module imports it directly).
- **Task 5:** dropped the unused `DiscIdentity` import from the writer
  implementation (the plan itself flags it) and added it inside the test
  module instead, where the tests' `use super::*` needed it after the drop.
- **Task 6:** dropped `map_err(std::io::Error::from)` on the
  `reflink_copy::reflink` call — the pinned reflink-copy already returns
  `io::Result<()>` (the plan's closing note anticipates exactly this).
- **Task 7:** the `use std::path::PathBuf;` line moved into the file's
  existing top-level import block rather than landing mid-file as the plan's
  snippet shows; duplicate imports are denied.
- **Task 8:** the only signature adaptation was the one the plan pre-flags:
  `mark_dead(ProjectId(1), 1, &now())` — the real parameter is
  `at: &DateTime<Utc>`. `plan_from`'s argument order matched as written.
- **Task 9:** no `walkdir` dev-dependency addition was needed (the plan
  anticipated it might be) — `walkdir` is already a regular dependency of
  `diskrement-shell`, visible to integration tests. All four oracle tests
  passed on first run; no debugging was required.
- **Task 10/11:** `burn_report` carried an interim `#[allow(dead_code)]`
  (matching `render.rs`'s existing convention; `-D warnings` forces it) between
  Task 10 and Task 11's dispatch wiring, then the attribute was dropped. The
  `Burn` dispatch arm resolves the project *before* prompting, per the plan's
  own "order matters" note after checking `Runner.scala`'s `withProject` —
  the sketch's literal order had the prompt first.
- **Task 12:** the planner-order caveat did not fire (`a.bin` → disc 1,
  `b.bin` → disc 2, as written).
- **Task 13:** the deferral comment replaced by the embedded-catalog
  comparison was retained in adapted form as the rationale comment the plan's
  snippet carries. `runner.rs`'s `RUST_IMPLEMENTED` doc comment has a
  pre-existing duplicated paragraph (not introduced here, left for its next
  editor).

Known deferred debt observed during review (recorded, deliberately not fixed
here): read-only source files (mode 0444) fail the stager's mtime restore
because permissions are set before the write-fd open — inherited from the
mandated `fs::snapshot::reflink_tree` idiom, fix idiom-wide when touched;
`--yes` lacks Scala's `-y` alias (the plan's sketch omitted it; scenarios use
the long form); `burn_basics`'s trailing comment anticipates a `burn` step the
scenario never issues.

## Execution verification

- `cargo test --workspace`: **348 passed, 0 failed, 3 ignored** (baseline
  278/3; the three ignored are the `#[ignore]`d differential entry points).
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo fmt --check`: clean.
- Differential suite, all three modes: bootstrap (all-scala) **10/10
  reproduced identically**; all-rust **9 compared, 0 failed**; interleaved
  **9 compared, 0 failed**. The single skip is `harvest-replay` in the rust
  and interleaved modes — it needs P7's `disc`/`history`/`verify`, and the
  harness announces the skip loudly rather than passing silently.
- `sbt clean test`: **51 passed, 0 failed, 0 errors** — the Scala oracle tree
  is untouched and green.
