# Rust Rewrite — Phase P1: The Pure Functional Core

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port `diskrement.core` — the model types, `diff`, the disc planner, and the manifest builder — into `diskrement-core`, reproducing every assertion in the 51 Scala core tests and adding six that pin decisions this port has to make.

**Architecture:** Pure data and pure functions only. The crate's single dependency is `chrono` with default features disabled, which excludes the `clock` feature, so `Utc::now()` does not compile here — purity is enforced by the dependency graph rather than by discipline, and Task 9 proves it with a deliberate negative-compile check. No I/O, no serde, no binary. The crate and its stub smoke test already exist from P0; this phase fills it in.

**Tech Stack:** Rust stable (pinned by `rust-toolchain.toml`, edition 2021), `chrono` 0.4 with `default-features = false`, `proptest` 1.

**Spec:** `docs/superpowers/specs/2026-09-05-rust-rewrite-design.md` — this plan implements phase P1 (§9); the crate's role is §5.3.

---

## Context for the implementer

You are porting a working Scala 3 program. The sources for this phase are in
`core/src/main/scala/diskrement/core/` and their tests in
`core/src/test/scala/diskrement/core/`. **Read the Scala file before porting
it.** It is the specification, and its comments explain decisions the code alone
does not. That tree is still present and still builds; it is not deleted until
P9.

Five things about this port that are easy to get wrong:

1. **This is not a transliteration.** Write Rust the way Rust is written. Where
   the Scala folds over an immutable accumulator and a `for` loop with a `mut`
   local is clearer, use the loop.

2. **Behavior must match exactly anyway.** Every assertion in the Scala tests
   has a counterpart here, including the ones that look pedantic. The
   floating-point capacity test is pinning a real bug that was found and fixed
   (`DiscType.scala`, the `BigDecimal` comment).

3. **`u64` for byte counts, `u32` for ordinals.** The Scala uses `Long`/`Int`
   for both. Conversion to SQLite's `i64` happens at the catalog boundary in P2,
   not here. Where the Scala relied on signed arithmetic going negative, that is
   called out in the task that ports it — do not paper over it with
   `saturating_sub` and assume the result is equivalent, because in one place it
   is not (Task 5).

4. **Determinism is a property of this crate, not of its callers.** Two of the
   ported property tests exist only to pin it. Prefer `BTreeMap` over `HashMap`
   and sort explicitly even when a container already gives you order.

5. **No `serde` here.** The wire spellings frozen by the compatibility contract
   (spec §3.1) are pinned in this crate as `as_str`/`FromStr` pairs with unit
   tests; the serde attributes that consume them live in `shell` from P2 onward.
   Keeping the strings in core means one place defines them and the pure tests
   catch a typo before any JSON exists.

### What "done" means for this phase

```
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

All three clean. Run all three before every commit; the plan reminds you, but
the habit matters more than the reminder. Run plain `cargo fmt` first — the code
in this plan is written to be readable in a document, not to be byte-identical
to rustfmt's output, and `--check` is a gate rather than a formatter.

### Test counts

Each task states the number the `diskrement-core` lib test binary should report
(`cargo test -p diskrement-core --lib`). P0 left it at **1** (`crate_builds`,
the `SmokeTest` port). The phase ends at **57**.

---

## File structure

| File | Responsibility |
|------|----------------|
| `crates/diskrement-core/src/lib.rs` | Module declarations and public re-exports |
| `crates/diskrement-core/src/model.rs` | `RelPath`, `ContentHash`, `ScannedFile`, `CatalogEntry`, `CatalogView`, `DiscType`, `Capacity` |
| `crates/diskrement-core/src/diff.rs` | `ChangeSet` and `diff()` |
| `crates/diskrement-core/src/planner.rs` | `DiscStatus`, `DiscState`, `PlannedDisc`, `BurnPlan`, `plan()` |
| `crates/diskrement-core/src/manifest.rs` | `ManifestEntry`, `ManifestChangeKind`, `SessionManifest`, `build()` |
| `crates/diskrement-core/src/testutil.rs` | `#[cfg(test)]` only: shared fixtures and proptest strategies |
| `crates/diskrement-core/README.md` | What the crate is and why it has one dependency |

Unit tests live in `#[cfg(test)] mod tests` at the bottom of the file they test.
That is the Rust convention and it keeps a type and its tests in one place.

Two deliberate differences from the Scala layout. `DiscType`/`Capacity` join
`model.rs` and `ChangeSet` joins `diff.rs` rather than each getting its own file
— types live with the function that consumes them, and these files stay small.
And `testutil.rs` has no Scala counterpart beyond `Generators.scala`; it also
absorbs the per-test-file helpers (`sf`, `entry`, `viewOf`) that the Scala
duplicated across five test classes.

Functions are **not** re-exported at the crate root — callers write
`diff::diff(..)`, `planner::plan(..)`, `manifest::build(..)`, which reads
unambiguously and mirrors the Scala `Diff.diff` / `Planner.plan` /
`ManifestBuilder.build`. Types **are** re-exported for convenience.

---

## Task 1: Core model types

Port `core/src/main/scala/diskrement/core/Model.scala`. Read it first.

**Files:**
- Create: `crates/diskrement-core/src/model.rs`
- Create: `crates/diskrement-core/src/testutil.rs`
- Modify: `crates/diskrement-core/src/lib.rs`

- [ ] **Step 1: Write the shared test helpers**

Create `crates/diskrement-core/src/testutil.rs`:

```rust
//! Test-only fixtures shared by the unit test modules, and (from Task 7) the
//! proptest strategies that replace `Generators.scala`.

use chrono::{DateTime, Utc};

use crate::model::{CatalogEntry, CatalogView, ContentHash, RelPath, ScannedFile};

/// The Unix epoch — every ported test's stand-in mtime, matching the Scala
/// tests' `Instant.EPOCH`. Centralised so the constructor appears exactly once:
/// if the pinned `chrono` predates `DateTime::from_timestamp`, change it here to
/// `DateTime::UNIX_EPOCH` and nothing else moves.
pub fn epoch() -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(0, 0).expect("the epoch is a valid timestamp")
}

/// A scanned file whose hash is derived from its path, mirroring the `sf`
/// helper in `PlannerTest.scala` and `ManifestBuilderTest.scala`.
pub fn sf(path: &str, size: u64) -> ScannedFile {
    ScannedFile {
        rel_path: RelPath(path.to_string()),
        content_hash: ContentHash(format!("h-{path}")),
        size_bytes: size,
        mtime: epoch(),
    }
}

/// A scanned file with an explicit hash and the size-1 default from
/// `DiffTest.scala`, whose tests need hash control rather than size control.
pub fn sf_hash(path: &str, hash: &str) -> ScannedFile {
    ScannedFile {
        rel_path: RelPath(path.to_string()),
        content_hash: ContentHash(hash.to_string()),
        size_bytes: 1,
        mtime: epoch(),
    }
}

/// A catalog entry, mirroring the `entry` helper in `DiffTest.scala`.
pub fn entry(path: &str, hash: &str) -> CatalogEntry {
    CatalogEntry {
        rel_path: RelPath(path.to_string()),
        content_hash: ContentHash(hash.to_string()),
        size_bytes: 1,
    }
}

/// A catalog view over the given entries, mirroring `viewOf` in `DiffTest.scala`.
pub fn view_of(entries: &[CatalogEntry]) -> CatalogView {
    CatalogView {
        current: entries
            .iter()
            .map(|e| (e.rel_path.clone(), e.clone()))
            .collect(),
    }
}
```

- [ ] **Step 2: Wire the modules into `lib.rs`**

Replace the whole contents of `crates/diskrement-core/src/lib.rs`:

```rust
//! diskrement's functional core: pure data and pure functions, no I/O.
//!
//! Nothing here touches the filesystem, SQLite, a subprocess, or the clock.
//! The shell crate owns every effect; this crate owns the change-set,
//! planning, and manifest models (spec §5.3).
//!
//! Functions are reached through their modules — `diff::diff`, `planner::plan`,
//! `manifest::build` — mirroring the Scala `Diff.diff` / `Planner.plan` /
//! `ManifestBuilder.build`. Types are re-exported here for convenience.

pub mod model;

#[cfg(test)]
// Helpers land in the task that first needs them, which is sometimes one task
// ahead of their first consumer; the allow keeps that from failing `-D warnings`
// mid-phase.
#[allow(dead_code)]
mod testutil;

pub use model::{CatalogEntry, CatalogView, ContentHash, RelPath, ScannedFile};

/// Temporary P0 scaffolding: gives the `shell -> core` dependency edge
/// something to link against. Task 9 retires it in favour of a real type.
pub struct Probe;

#[cfg(test)]
mod tests {
    #[test]
    fn crate_builds() {
        // Mirrors the Scala `SmokeTest`: proves the crate compiles and its test
        // harness runs.
    }
}
```

- [ ] **Step 3: Write the failing tests**

Create `crates/diskrement-core/src/model.rs` containing only the test module for
now:

```rust
//! Pure domain types shared across the functional core.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{entry, epoch, view_of};

    #[test]
    fn rel_path_orders_lexicographically_by_value() {
        let mut paths = vec![
            RelPath("b/a".to_string()),
            RelPath("a/z".to_string()),
            RelPath("a/a".to_string()),
        ];
        paths.sort();
        let values: Vec<&str> = paths.iter().map(RelPath::as_str).collect();
        assert_eq!(values, vec!["a/a", "a/z", "b/a"]);
    }

    #[test]
    fn an_empty_catalog_view_has_no_current_entries() {
        assert_eq!(CatalogView::empty().current.len(), 0);
    }

    #[test]
    fn catalog_view_lookups_are_by_rel_path() {
        let e = entry("x/y", "ab");
        let view = view_of(&[e.clone()]);
        assert_eq!(view.current.get(&RelPath("x/y".to_string())), Some(&e));
    }

    #[test]
    fn a_scanned_file_carries_path_hash_size_and_mtime() {
        let f = ScannedFile {
            rel_path: RelPath("p".to_string()),
            content_hash: ContentHash("cd".to_string()),
            size_bytes: 42,
            mtime: epoch(),
        };
        assert_eq!(f.size_bytes, 42);
        assert_eq!(f.content_hash, ContentHash("cd".to_string()));
        assert_eq!(f.mtime, epoch());
    }

    #[test]
    fn from_scanned_records_every_scanned_file_as_the_current_version() {
        // This is how `DiffPropertyTest` expresses idempotence, so it needs a
        // test of its own rather than only being exercised through a property.
        let files = vec![
            ScannedFile {
                rel_path: RelPath("a".to_string()),
                content_hash: ContentHash("h1".to_string()),
                size_bytes: 7,
                mtime: epoch(),
            },
            ScannedFile {
                rel_path: RelPath("b".to_string()),
                content_hash: ContentHash("h2".to_string()),
                size_bytes: 9,
                mtime: epoch(),
            },
        ];
        let view = CatalogView::from_scanned(&files);
        assert_eq!(view.current.len(), 2);
        assert_eq!(
            view.current.get(&RelPath("a".to_string())),
            Some(&CatalogEntry {
                rel_path: RelPath("a".to_string()),
                content_hash: ContentHash("h1".to_string()),
                size_bytes: 7,
            })
        );
    }
}
```

- [ ] **Step 4: Run the tests to verify they fail**

```bash
cargo test -p diskrement-core --lib
```

Expected: compilation errors — `cannot find type RelPath in this scope`,
`cannot find type CatalogView in this scope`, and similar. A compile failure
*is* the red state for a Rust port; do not try to get to a running-but-failing
assertion first.

- [ ] **Step 5: Write the implementation**

Insert above the `mod tests` block in `model.rs`:

```rust
use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

/// Path of a file relative to the project's source root, using '/' separators.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RelPath(pub String);

impl RelPath {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RelPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Content hash of a file (SHA-256 hex in production; opaque to the core).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentHash(pub String);

impl ContentHash {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ContentHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A file observed during a scan, always fully hashed by the scanner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedFile {
    pub rel_path: RelPath,
    pub content_hash: ContentHash,
    pub size_bytes: u64,
    pub mtime: DateTime<Utc>,
}

/// The current (live) version of one file as recorded in the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub rel_path: RelPath,
    pub content_hash: ContentHash,
    pub size_bytes: u64,
}

/// Immutable snapshot of the catalog's current view: rel path -> live version.
///
/// A `BTreeMap` rather than a `HashMap`, so key iteration is deterministic.
/// `diff` sorts its outputs explicitly anyway, but a nondeterministic core
/// would be the hardest bug here to see, so both guards stay.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CatalogView {
    pub current: BTreeMap<RelPath, CatalogEntry>,
}

impl CatalogView {
    pub fn empty() -> Self {
        Self::default()
    }

    /// The view that results from recording exactly these files as the current
    /// versions. Used to express diff idempotence (see `DiffPropertyTest`).
    pub fn from_scanned(files: &[ScannedFile]) -> Self {
        CatalogView {
            current: files
                .iter()
                .map(|f| {
                    (
                        f.rel_path.clone(),
                        CatalogEntry {
                            rel_path: f.rel_path.clone(),
                            content_hash: f.content_hash.clone(),
                            size_bytes: f.size_bytes,
                        },
                    )
                })
                .collect(),
        }
    }
}
```

- [ ] **Step 6: Run the tests to verify they pass**

```bash
cargo test -p diskrement-core --lib
```

Expected: `test result: ok. 6 passed`.

- [ ] **Step 7: Verify lint and format**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
```

Expected: clean.

- [ ] **Step 8: Commit**

```bash
git add crates/diskrement-core/src
git commit -m "feat(core): port model types (RelPath, ContentHash, ScannedFile, CatalogView)"
```

---

## Task 2: Disc types and capacity

Port `core/src/main/scala/diskrement/core/DiscType.scala`. **Read its
`BigDecimal` comment before writing the implementation** — it is the reason this
task exists as its own step.

**Files:**
- Modify: `crates/diskrement-core/src/model.rs`
- Modify: `crates/diskrement-core/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Add these inside the existing `mod tests` block in `model.rs`:

```rust
    #[test]
    fn disc_types_expose_nominal_capacities() {
        assert_eq!(DiscType::BdR.nominal_bytes(), 25_000_000_000);
        assert_eq!(DiscType::BdRDl.nominal_bytes(), 50_000_000_000);
        assert_eq!(DiscType::Dvd.nominal_bytes(), 4_700_000_000);
        assert_eq!(DiscType::DvdDl.nominal_bytes(), 8_500_000_000);
    }

    #[test]
    fn capacity_subtracts_the_margin_percent_and_floors() {
        assert_eq!(
            Capacity::from_disc_type(DiscType::BdR, 2.0).usable_bytes,
            24_500_000_000
        );
    }

    #[test]
    fn zero_margin_yields_nominal_capacity() {
        assert_eq!(
            Capacity::from_disc_type(DiscType::Dvd, 0.0).usable_bytes,
            4_700_000_000
        );
    }

    #[test]
    fn capacity_is_floored_exactly_with_no_floating_point_drift() {
        // 8_500_000_000 * 0.93 == 7_905_000_000 exactly. Computed in f64 this
        // floors to 7_904_999_999 — one byte short. Integer math must not.
        assert_eq!(
            Capacity::from_disc_type(DiscType::DvdDl, 7.0).usable_bytes,
            7_905_000_000
        );
    }

    #[test]
    fn a_fractional_margin_percent_is_honoured_exactly() {
        // 25_000_000_000 * (1 - 2.5/100) == 24_375_000_000 exactly.
        assert_eq!(
            Capacity::from_disc_type(DiscType::BdR, 2.5).usable_bytes,
            24_375_000_000
        );
    }

    #[test]
    fn disc_type_wire_names_round_trip() {
        // Frozen by the data-compatibility contract (spec §3.1): these are the
        // strings the Scala build wrote into `project.default_disc_type` and
        // into the staged `plan.json`.
        for (dt, name) in [
            (DiscType::BdR, "BD_R"),
            (DiscType::BdRDl, "BD_R_DL"),
            (DiscType::Dvd, "DVD"),
            (DiscType::DvdDl, "DVD_DL"),
        ] {
            assert_eq!(dt.as_str(), name);
            assert_eq!(name.parse::<DiscType>().expect("parses"), dt);
        }
    }

    #[test]
    fn an_unknown_disc_type_name_is_rejected() {
        assert!("CD_R".parse::<DiscType>().is_err());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p diskrement-core --lib
```

Expected: compilation errors — `failed to resolve: use of undeclared type
DiscType` and `Capacity`.

- [ ] **Step 3: Write the implementation**

Append to `model.rs`, above the `mod tests` block:

```rust
/// Physical optical media types and their nominal byte capacities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiscType {
    BdR,
    BdRDl,
    Dvd,
    DvdDl,
}

impl DiscType {
    pub fn nominal_bytes(self) -> u64 {
        match self {
            DiscType::BdR => 25_000_000_000,
            DiscType::BdRDl => 50_000_000_000,
            DiscType::Dvd => 4_700_000_000,
            DiscType::DvdDl => 8_500_000_000,
        }
    }

    /// The catalog and JSON spelling. Frozen by the data-compatibility
    /// contract; see the spec, §3.1. Do not change these strings.
    pub fn as_str(self) -> &'static str {
        match self {
            DiscType::BdR => "BD_R",
            DiscType::BdRDl => "BD_R_DL",
            DiscType::Dvd => "DVD",
            DiscType::DvdDl => "DVD_DL",
        }
    }
}

impl std::fmt::Display for DiscType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for DiscType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "BD_R" => Ok(DiscType::BdR),
            "BD_R_DL" => Ok(DiscType::BdRDl),
            "DVD" => Ok(DiscType::Dvd),
            "DVD_DL" => Ok(DiscType::DvdDl),
            other => Err(format!("unknown disc type: {other}")),
        }
    }
}

/// Usable bytes on a disc after subtracting filesystem/UDF overhead margin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capacity {
    pub usable_bytes: u64,
}

impl Capacity {
    pub fn new(usable_bytes: u64) -> Self {
        Capacity { usable_bytes }
    }

    /// Nominal capacity reduced by `margin_percent` (e.g. 2.0 == 2%), floored.
    ///
    /// The margin is converted to an exact integer count of thousandths of a
    /// percent before dividing, so the result never drifts by a byte the way
    /// `nominal as f64 * (1.0 - margin / 100.0)` does: DVD-DL at a 7% margin
    /// floors to 7_904_999_999 in f64, one byte short of the exact
    /// 7_905_000_000. The Scala reached for `BigDecimal` to solve the same
    /// problem.
    ///
    /// This quantises the margin to three decimal places. `--margin` is a
    /// user-facing percentage where that is far below anything meaningful (a
    /// thousandth of a percent of a BD-R is 250 KB), and the alternative — a
    /// decimal-arithmetic dependency in the one crate that is meant to have
    /// almost none — costs more than it buys.
    ///
    /// A margin above 100% would compute negative and is clamped to zero; the
    /// value comes from `project.margin_percent` and is nonsense in that range.
    pub fn from_disc_type(disc_type: DiscType, margin_percent: f64) -> Self {
        let margin_thousandths = (margin_percent * 1000.0).round() as i128;
        let usable =
            i128::from(disc_type.nominal_bytes()) * (100_000 - margin_thousandths) / 100_000;
        Capacity {
            usable_bytes: usable.max(0) as u64,
        }
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p diskrement-core --lib
```

Expected: `test result: ok. 13 passed`.

If `capacity_is_floored_exactly_with_no_floating_point_drift` fails with
`7904999999`, the implementation is multiplying by an `f64` factor somewhere —
the whole point of the integer-thousandths form is that no float reaches the
byte-scale multiplication.

- [ ] **Step 5: Extend the re-exports**

In `lib.rs`, replace the `pub use model::{...}` line with:

```rust
pub use model::{
    Capacity, CatalogEntry, CatalogView, ContentHash, DiscType, RelPath, ScannedFile,
};
```

- [ ] **Step 6: Verify lint and format, then commit**

```bash
cargo test -p diskrement-core --lib \
  && cargo clippy --workspace --all-targets -- -D warnings \
  && cargo fmt --check
git add crates/diskrement-core/src
git commit -m "feat(core): port DiscType and exact-floor Capacity"
```

Expected: 13 passed, clippy and rustfmt silent.

---

## Task 3: ChangeSet and diff

Port `core/src/main/scala/diskrement/core/ChangeSet.scala` and `Diff.scala`.

**Files:**
- Create: `crates/diskrement-core/src/diff.rs`
- Modify: `crates/diskrement-core/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/diskrement-core/src/diff.rs` with only the test module:

```rust
//! Pure classification of a scan against the catalog's current view.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CatalogView, ContentHash, RelPath};
    use crate::testutil::{entry, sf_hash, view_of};

    fn paths(files: &[crate::model::ScannedFile]) -> Vec<&str> {
        files.iter().map(|f| f.rel_path.as_str()).collect()
    }

    // --- ChangeSetTest.scala ---

    #[test]
    fn an_empty_change_set_has_no_entries_and_reports_nothing_to_do() {
        assert!(ChangeSet::empty().is_empty());
    }

    #[test]
    fn a_change_set_with_any_added_modified_or_deleted_is_non_empty() {
        let cs = ChangeSet {
            deleted: vec![RelPath("gone".to_string())],
            ..ChangeSet::empty()
        };
        assert!(!cs.is_empty());
    }

    // --- DiffTest.scala ---

    #[test]
    fn a_new_path_is_classified_as_added() {
        let cs = diff(&CatalogView::empty(), &[sf_hash("a", "h1")]);
        assert_eq!(paths(&cs.added), vec!["a"]);
        assert!(cs.modified.is_empty() && cs.deleted.is_empty() && cs.unchanged.is_empty());
    }

    #[test]
    fn the_same_path_with_a_different_hash_is_modified() {
        let cs = diff(&view_of(&[entry("a", "old")]), &[sf_hash("a", "new")]);
        assert_eq!(
            cs.modified
                .iter()
                .map(|f| f.content_hash.clone())
                .collect::<Vec<_>>(),
            vec![ContentHash("new".to_string())]
        );
        assert!(cs.added.is_empty());
    }

    #[test]
    fn the_same_path_and_hash_is_unchanged() {
        let cs = diff(&view_of(&[entry("a", "h")]), &[sf_hash("a", "h")]);
        assert_eq!(paths(&cs.unchanged), vec!["a"]);
        assert!(cs.added.is_empty() && cs.modified.is_empty());
    }

    #[test]
    fn a_catalog_path_absent_from_the_scan_is_deleted() {
        let cs = diff(&view_of(&[entry("a", "h")]), &[]);
        assert_eq!(cs.deleted, vec![RelPath("a".to_string())]);
    }

    #[test]
    fn outputs_are_sorted_by_rel_path_for_determinism() {
        let cs = diff(
            &CatalogView::empty(),
            &[sf_hash("b", "h"), sf_hash("a", "h")],
        );
        assert_eq!(paths(&cs.added), vec!["a", "b"]);
    }

    #[test]
    fn a_mixed_scenario_classifies_every_file_exactly_once() {
        let catalog = view_of(&[entry("keep", "h"), entry("edit", "old"), entry("rm", "h")]);
        let scan = [
            sf_hash("keep", "h"),
            sf_hash("edit", "new"),
            sf_hash("add", "h"),
        ];
        let cs = diff(&catalog, &scan);
        assert_eq!(paths(&cs.added), vec!["add"]);
        assert_eq!(paths(&cs.modified), vec!["edit"]);
        assert_eq!(paths(&cs.unchanged), vec!["keep"]);
        assert_eq!(cs.deleted, vec![RelPath("rm".to_string())]);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p diskrement-core --lib
```

Expected: compilation errors — `cannot find type ChangeSet in this scope`,
`cannot find function diff in this scope`.

- [ ] **Step 3: Write the implementation**

Insert above the `mod tests` block in `diff.rs`:

```rust
use std::collections::HashSet;

use crate::model::{CatalogView, RelPath, ScannedFile};

/// Classification of a scan against the catalog's current view.
///
/// `unchanged` is retained for reporting; it never contributes to a burn.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChangeSet {
    pub added: Vec<ScannedFile>,
    pub modified: Vec<ScannedFile>,
    pub deleted: Vec<RelPath>,
    pub unchanged: Vec<ScannedFile>,
}

impl ChangeSet {
    pub fn empty() -> Self {
        Self::default()
    }

    /// True when there is nothing to back up. `unchanged` is deliberately not
    /// consulted: a scan that found only unchanged files has nothing to burn.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.modified.is_empty() && self.deleted.is_empty()
    }
}

/// Classify `scanned` against the catalog's current view.
///
/// Every output list is sorted by rel path, so the result depends on the *set*
/// of scanned files and not on the order the scanner produced them.
pub fn diff(catalog: &CatalogView, scanned: &[ScannedFile]) -> ChangeSet {
    let mut added = Vec::new();
    let mut modified = Vec::new();
    let mut unchanged = Vec::new();

    for f in scanned {
        match catalog.current.get(&f.rel_path) {
            None => added.push(f.clone()),
            Some(e) if e.content_hash != f.content_hash => modified.push(f.clone()),
            Some(_) => unchanged.push(f.clone()),
        }
    }

    let scanned_paths: HashSet<&RelPath> = scanned.iter().map(|f| &f.rel_path).collect();
    let mut deleted: Vec<RelPath> = catalog
        .current
        .keys()
        .filter(|p| !scanned_paths.contains(*p))
        .cloned()
        .collect();

    added.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    modified.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    unchanged.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    deleted.sort();

    ChangeSet {
        added,
        modified,
        deleted,
        unchanged,
    }
}
```

- [ ] **Step 4: Wire the module into `lib.rs`**

Add `pub mod diff;` above `pub mod model;`, and add the type re-export:

```rust
pub use diff::ChangeSet;
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p diskrement-core --lib
```

Expected: `test result: ok. 21 passed`.

- [ ] **Step 6: Verify lint and format, then commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/diskrement-core/src
git commit -m "feat(core): port ChangeSet and diff classification"
```

---

## Task 4: Planner types

Port the type definitions at the top of
`core/src/main/scala/diskrement/core/Planner.scala` — `DiscStatus`,
`DiscState`, `PlannedDisc`, `BurnPlan` — but not `plan()` itself, which is
Task 5.

**Files:**
- Create: `crates/diskrement-core/src/planner.rs`
- Modify: `crates/diskrement-core/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/diskrement-core/src/planner.rs` with only the test module:

```rust
//! Pure, deterministic ordered-fill disc planner.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::RelPath;
    use crate::testutil::sf;

    #[test]
    fn disc_state_reports_free_bytes() {
        let d = DiscState {
            ordinal: 1,
            status: DiscStatus::Open,
            usable_bytes: 100,
            used_bytes: 30,
        };
        assert_eq!(d.free_bytes(), 70);
    }

    #[test]
    fn an_overfull_disc_reports_zero_free_rather_than_underflowing() {
        // The Scala returned a negative `Long` here. Nothing renders this
        // value, so saturating at zero is a safe choice for `free_bytes` — but
        // it is NOT a safe basis for deciding whether a file fits. See
        // `a_zero_byte_file_does_not_fit_an_overfull_disc` in Task 5.
        let d = DiscState {
            ordinal: 1,
            status: DiscStatus::Open,
            usable_bytes: 100,
            used_bytes: 130,
        };
        assert_eq!(d.free_bytes(), 0);
    }

    #[test]
    fn planned_disc_reports_added_and_total_bytes() {
        let f = sf("a", 25);
        let pd = PlannedDisc {
            ordinal: 2,
            is_new: true,
            will_finalize: false,
            files: vec![f.clone(), f],
            prior_used_bytes: 10,
            usable_bytes: 100,
        };
        assert_eq!(pd.added_bytes(), 50);
        assert_eq!(pd.total_used_bytes(), 60);
    }

    #[test]
    fn burn_plan_groups_discs_tombstones_and_oversized() {
        let plan = BurnPlan {
            discs: vec![],
            tombstones: vec![RelPath("gone".to_string())],
            oversized: vec![],
        };
        assert_eq!(plan.tombstones, vec![RelPath("gone".to_string())]);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p diskrement-core --lib
```

Expected: compilation errors — `cannot find type DiscState in this scope` and
similar.

- [ ] **Step 3: Write the implementation**

Insert above the `mod tests` block in `planner.rs`:

```rust
use crate::model::{RelPath, ScannedFile};

/// Whether a disc can still receive new sessions.
///
/// `Dead` means the physical disc is lost or unreadable (a "coaster"). It is
/// derived from a `MARKED_DEAD` catalog event at read time and never stored in
/// the disc row itself — `disc.status` keeps its
/// `check (status in ('OPEN','FINALIZED'))` constraint. The derivation lives in
/// the catalog query (`CatalogStoreLive.readDiscStates`), so by the time a
/// `DiscState` reaches the planner, `Dead` is a legitimate value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscStatus {
    Open,
    Finalized,
    Dead,
}

/// An existing disc as known to the catalog when planning begins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscState {
    pub ordinal: u32,
    pub status: DiscStatus,
    pub usable_bytes: u64,
    pub used_bytes: u64,
}

impl DiscState {
    /// Remaining space, saturating at zero.
    ///
    /// Reporting only. The planner decides whether a file fits with an
    /// addition rather than this subtraction, because the two disagree when
    /// `used_bytes > usable_bytes` — see `Bin::fits` in Task 5.
    pub fn free_bytes(&self) -> u64 {
        self.usable_bytes.saturating_sub(self.used_bytes)
    }
}

/// One disc as it will be written by the upcoming burn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedDisc {
    pub ordinal: u32,
    pub is_new: bool,
    pub will_finalize: bool,
    pub files: Vec<ScannedFile>,
    pub prior_used_bytes: u64,
    pub usable_bytes: u64,
}

impl PlannedDisc {
    pub fn added_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.size_bytes).sum()
    }

    pub fn total_used_bytes(&self) -> u64 {
        self.prior_used_bytes + self.added_bytes()
    }
}

/// The full plan for the next burn session(s).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BurnPlan {
    pub discs: Vec<PlannedDisc>,
    pub tombstones: Vec<RelPath>,
    pub oversized: Vec<ScannedFile>,
}
```

- [ ] **Step 4: Wire the module into `lib.rs`**

Add `pub mod planner;` after `pub mod model;`, and add:

```rust
pub use planner::{BurnPlan, DiscState, DiscStatus, PlannedDisc};
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p diskrement-core --lib
```

Expected: `test result: ok. 25 passed`.

- [ ] **Step 6: Verify lint and format, then commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/diskrement-core/src
git commit -m "feat(core): port planner types (DiscStatus, DiscState, PlannedDisc, BurnPlan)"
```

---

## Task 5: The planner

Port `Planner.plan` from `core/src/main/scala/diskrement/core/Planner.scala`.
**Read the whole Scala function and its inline comments first** — the ordinal
assignment and the two-stage oversize check are both subtle, and the tests below
will not tell you *why* they are shaped that way.

**Files:**
- Modify: `crates/diskrement-core/src/planner.rs`
- Modify: `crates/diskrement-core/src/testutil.rs`

- [ ] **Step 1: Add the change-set helper**

Append to `testutil.rs` (again moving the `use` line up to the imports at the
top of the file):

```rust
use crate::diff::ChangeSet;

/// A change set holding only additions, mirroring the `changesOf` / `added`
/// helpers in `PlannerTest.scala` and `PlannerEdgeTest.scala`.
pub fn added(files: Vec<ScannedFile>) -> ChangeSet {
    ChangeSet {
        added: files,
        ..ChangeSet::empty()
    }
}
```

- [ ] **Step 2: Write the failing tests**

Add these inside `planner.rs`'s `mod tests` block. They are the ports of
`PlannerTest.scala` and `PlannerEdgeTest.scala` combined, plus one regression
test this port needs.

`ChangeSet` and `Capacity` are **not** imported here: Step 4 adds them to the
parent module, and the existing `use super::*` in this test block picks them up.
Importing them again would duplicate a name the glob already provides.

```rust
    use crate::model::DiscType;
    use crate::testutil::added;

    const CAP100: Capacity = Capacity { usable_bytes: 100 };

    fn disc_paths(d: &PlannedDisc) -> Vec<&str> {
        d.files.iter().map(|f| f.rel_path.as_str()).collect()
    }

    fn all_placed(plan: &BurnPlan) -> Vec<&str> {
        plan.discs.iter().flat_map(disc_paths).collect()
    }

    // --- PlannerTest.scala ---

    #[test]
    fn all_files_fit_on_one_fresh_disc_which_stays_open() {
        let plan = plan(&added(vec![sf("a", 30), sf("b", 30)]), &[], CAP100);
        assert_eq!(plan.discs.len(), 1);
        let d = &plan.discs[0];
        assert!(d.is_new);
        assert!(!d.will_finalize);
        assert_eq!(disc_paths(d), vec!["a", "b"]);
        assert_eq!(d.ordinal, 1);
    }

    #[test]
    fn files_are_placed_in_lexicographic_order_regardless_of_input_order() {
        let plan = plan(
            &added(vec![sf("c", 10), sf("a", 10), sf("b", 10)]),
            &[],
            CAP100,
        );
        assert_eq!(disc_paths(&plan.discs[0]), vec!["a", "b", "c"]);
    }

    #[test]
    fn a_file_that_does_not_fit_spills_wholly_to_a_new_disc() {
        // 60 + 60 cannot share a 100-byte disc.
        let plan = plan(&added(vec![sf("a", 60), sf("b", 60)]), &[], CAP100);
        assert_eq!(plan.discs.len(), 2);
        assert_eq!(disc_paths(&plan.discs[0]), vec!["a"]);
        assert!(plan.discs[0].will_finalize, "spilled disc must be finalized");
        assert_eq!(disc_paths(&plan.discs[1]), vec!["b"]);
        assert!(!plan.discs[1].will_finalize, "last disc stays open");
        assert_eq!(plan.discs[1].ordinal, 2);
    }

    #[test]
    fn an_existing_open_disc_is_topped_off_before_opening_a_new_disc() {
        // open disc 1: 100 usable, 90 used -> 10 free.
        let open = DiscState {
            ordinal: 1,
            status: DiscStatus::Open,
            usable_bytes: 100,
            used_bytes: 90,
        };
        let plan = plan(&added(vec![sf("a", 10), sf("b", 50)]), &[open], CAP100);
        assert_eq!(plan.discs.len(), 2);
        let topped = &plan.discs[0];
        assert_eq!(topped.ordinal, 1);
        assert!(!topped.is_new);
        assert_eq!(disc_paths(topped), vec!["a"]);
        assert!(topped.will_finalize, "spilled, so finalized");
        assert_eq!(topped.total_used_bytes(), 100);
        let fresh = &plan.discs[1];
        assert_eq!(fresh.ordinal, 2);
        assert!(fresh.is_new);
        assert_eq!(disc_paths(fresh), vec!["b"]);
        assert!(!fresh.will_finalize);
    }

    #[test]
    fn deletions_become_tombstones_and_cost_zero_bytes() {
        let cs = ChangeSet {
            deleted: vec![RelPath("x".to_string()), RelPath("y".to_string())],
            ..ChangeSet::empty()
        };
        let plan = plan(&cs, &[], CAP100);
        assert_eq!(
            plan.tombstones,
            vec![RelPath("x".to_string()), RelPath("y".to_string())]
        );
        assert!(all_placed(&plan).is_empty());
    }

    #[test]
    fn modified_files_are_planned_alongside_added_by_rel_path() {
        let cs = ChangeSet {
            added: vec![sf("a", 10)],
            modified: vec![sf("b", 10)],
            ..ChangeSet::empty()
        };
        let plan = plan(&cs, &[], CAP100);
        assert_eq!(disc_paths(&plan.discs[0]), vec!["a", "b"]);
    }

    #[test]
    fn a_file_larger_than_a_fresh_disc_is_reported_oversized_not_placed() {
        let plan = plan(&added(vec![sf("big", 150), sf("ok", 10)]), &[], CAP100);
        assert_eq!(
            plan.oversized
                .iter()
                .map(|f| f.rel_path.as_str())
                .collect::<Vec<_>>(),
            vec!["big"]
        );
        assert_eq!(all_placed(&plan), vec!["ok"]);
    }

    #[test]
    fn a_dead_disc_is_never_topped_off() {
        // The dead disc has plenty of free room but must be ignored entirely;
        // it keeps its ordinal, and the fresh disc takes the next one.
        let dead = DiscState {
            ordinal: 1,
            status: DiscStatus::Dead,
            usable_bytes: 100,
            used_bytes: 10,
        };
        let plan = plan(&added(vec![sf("a", 30)]), &[dead], CAP100);
        assert_eq!(plan.discs.len(), 1);
        let d = &plan.discs[0];
        assert!(d.is_new);
        assert!(!d.will_finalize);
        assert_eq!(d.ordinal, 2);
    }

    // --- PlannerEdgeTest.scala ---

    #[test]
    fn an_empty_change_set_yields_a_single_empty_open_disc() {
        let plan = plan(&ChangeSet::empty(), &[], CAP100);
        assert_eq!(plan.discs.len(), 1);
        assert!(plan.discs[0].files.is_empty());
        assert!(!plan.discs[0].will_finalize);
        assert!(plan.oversized.is_empty());
    }

    #[test]
    fn a_file_exactly_equal_to_free_space_fits() {
        let plan = plan(&added(vec![sf("a", 100)]), &[], CAP100);
        assert_eq!(plan.discs.len(), 1);
        assert_eq!(disc_paths(&plan.discs[0]), vec!["a"]);
        assert!(!plan.discs[0].will_finalize);
    }

    #[test]
    fn only_deletions_records_tombstones_and_fills_no_data_discs() {
        let cs = ChangeSet {
            deleted: vec![RelPath("x".to_string())],
            ..ChangeSet::empty()
        };
        let plan = plan(&cs, &[], CAP100);
        assert_eq!(plan.tombstones, vec![RelPath("x".to_string())]);
        assert!(plan.discs.iter().all(|d| d.files.is_empty()));
    }

    #[test]
    fn a_move_plans_the_add_and_tombstones_the_old_path() {
        let cs = ChangeSet {
            added: vec![sf("new/a", 10)],
            deleted: vec![RelPath("old/a".to_string())],
            ..ChangeSet::empty()
        };
        let plan = plan(&cs, &[], CAP100);
        assert_eq!(disc_paths(&plan.discs[0]), vec!["new/a"]);
        assert_eq!(plan.tombstones, vec![RelPath("old/a".to_string())]);
    }

    #[test]
    fn a_file_fitting_only_the_open_disc_is_placed_not_oversized() {
        // Open BD-50 (empty); new discs are BD-25. A 30 GB file fits only the
        // open disc, and the up-front oversize filter must know that.
        let open = DiscState {
            ordinal: 1,
            status: DiscStatus::Open,
            usable_bytes: 50_000_000_000,
            used_bytes: 0,
        };
        let plan = plan(
            &added(vec![sf("big", 30_000_000_000)]),
            &[open],
            Capacity::new(25_000_000_000),
        );
        assert!(plan.oversized.is_empty(), "fits the open disc");
        assert_eq!(plan.discs[0].ordinal, 1);
        assert_eq!(disc_paths(&plan.discs[0]), vec!["big"]);
    }

    #[test]
    fn a_file_fitting_neither_the_open_disc_nor_a_fresh_one_is_oversized() {
        // Open BD-50 with only 10 GB free; new discs BD-25; a 30 GB file fits
        // neither. This is the second stage of the oversize check.
        let open = DiscState {
            ordinal: 1,
            status: DiscStatus::Open,
            usable_bytes: 50_000_000_000,
            used_bytes: 40_000_000_000,
        };
        let plan = plan(
            &added(vec![sf("big", 30_000_000_000)]),
            &[open],
            Capacity::new(25_000_000_000),
        );
        assert_eq!(
            plan.oversized
                .iter()
                .map(|f| f.rel_path.as_str())
                .collect::<Vec<_>>(),
            vec!["big"]
        );
        assert!(all_placed(&plan).is_empty());
    }

    #[test]
    fn spilled_discs_get_ordinals_above_the_highest_existing_ordinal() {
        // Unusual catalog: OPEN disc 2 (full) but a FINALIZED disc 5 exists.
        // The spilled disc must clear the max ordinal (5), not just 2 + 1.
        let discs = [
            DiscState {
                ordinal: 2,
                status: DiscStatus::Open,
                usable_bytes: 100,
                used_bytes: 100,
            },
            DiscState {
                ordinal: 5,
                status: DiscStatus::Finalized,
                usable_bytes: 100,
                used_bytes: 100,
            },
        ];
        let plan = plan(&added(vec![sf("a", 50)]), &discs, CAP100);
        assert_eq!(
            plan.discs.iter().map(|d| d.ordinal).collect::<Vec<_>>(),
            vec![2, 6]
        );
        assert_eq!(disc_paths(&plan.discs[1]), vec!["a"]);
        assert!(plan.discs[0].will_finalize);
        assert!(!plan.discs[1].will_finalize);
    }

    #[test]
    fn mixed_capacity_tops_off_a_bd_r_open_disc_then_uses_bd_50_for_the_new_one() {
        let bdr_usable = Capacity::from_disc_type(DiscType::BdR, 0.0).usable_bytes;
        let bd50_usable = Capacity::from_disc_type(DiscType::BdRDl, 0.0).usable_bytes;
        // Open BD-R with only 5 bytes free.
        let open = DiscState {
            ordinal: 1,
            status: DiscStatus::Open,
            usable_bytes: bdr_usable,
            used_bytes: bdr_usable - 5,
        };
        let cs = added(vec![sf("a", 5), sf("b", bd50_usable)]); // b only fits a BD-50
        let plan = plan(&cs, &[open], Capacity::new(bd50_usable));
        assert_eq!(
            plan.discs.iter().map(|d| d.ordinal).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(disc_paths(&plan.discs[0]), vec!["a"]);
        assert_eq!(plan.discs[0].usable_bytes, bdr_usable);
        assert!(plan.discs[0].will_finalize);
        assert_eq!(disc_paths(&plan.discs[1]), vec!["b"]);
        assert_eq!(plan.discs[1].usable_bytes, bd50_usable);
        assert!(!plan.discs[1].will_finalize);
        assert!(plan.oversized.is_empty());
    }

    // --- new in the Rust port ---

    #[test]
    fn a_zero_byte_file_does_not_fit_an_overfull_disc() {
        // This pins the one place where the obvious `u64` translation changes
        // behavior. The Scala asks `f.sizeBytes <= usableBytes - usedBytes` in
        // signed arithmetic: for an overfull disc that is `0 <= -30`, false, so
        // the file spills to a fresh disc. A `saturating_sub` would make it
        // `0 <= 0`, true, and quietly place the file on a disc with no room.
        //
        // `DiscState` is a public type whose field values this crate does not
        // validate, the Scala has defined behavior at this boundary, and a
        // rewrite reproduces behavior rather than inventing it at an edge no
        // existing test covers.
        let overfull = DiscState {
            ordinal: 1,
            status: DiscStatus::Open,
            usable_bytes: 100,
            used_bytes: 130,
        };
        let plan = plan(&added(vec![sf("empty", 0)]), &[overfull], CAP100);
        assert_eq!(plan.discs.len(), 2, "the file must spill to a fresh disc");
        assert!(
            plan.discs[0].files.is_empty(),
            "nothing may be added to the overfull disc"
        );
        assert_eq!(disc_paths(&plan.discs[1]), vec!["empty"]);
        assert_eq!(plan.discs[1].ordinal, 2);
    }
```

- [ ] **Step 3: Run the tests to verify they fail**

```bash
cargo test -p diskrement-core --lib
```

Expected: compilation errors — `cannot find function plan in this scope`,
plus `cannot find type ChangeSet`/`Capacity` in this scope, because Step 4 is
what brings those into the parent module. All three are the red state.

- [ ] **Step 4: Write the implementation**

Append to `planner.rs`, above the `mod tests` block:

```rust
use crate::diff::ChangeSet;
use crate::model::Capacity;

/// A disc being filled. Local to the planner: ordinals are positional and are
/// assigned only after packing finishes.
struct Bin {
    is_new: bool,
    usable_bytes: u64,
    prior_used_bytes: u64,
    files: Vec<ScannedFile>,
}

impl Bin {
    fn fresh(cap: Capacity) -> Self {
        Bin {
            is_new: true,
            usable_bytes: cap.usable_bytes,
            prior_used_bytes: 0,
            files: Vec::new(),
        }
    }

    fn used_bytes(&self) -> u64 {
        self.prior_used_bytes + self.files.iter().map(|f| f.size_bytes).sum::<u64>()
    }

    /// Whether `f` fits in what is left of this disc.
    ///
    /// Deliberately an addition rather than `f.size_bytes <= free_bytes()`: the
    /// two disagree on an overfull disc, where a saturating `free_bytes()` of 0
    /// would accept a zero-byte file that the Scala's signed comparison
    /// rejects. `checked_add` keeps the addition form from wrapping.
    fn fits(&self, f: &ScannedFile) -> bool {
        self.used_bytes()
            .checked_add(f.size_bytes)
            .is_some_and(|total| total <= self.usable_bytes)
    }
}

/// Plan the next burn: top off the open disc, then spill onto fresh discs.
///
/// Deterministic — the result depends only on the set of placeable files, not
/// on their order, and not on how they were split between `added` and
/// `modified`.
pub fn plan(changes: &ChangeSet, discs: &[DiscState], new_disc_cap: Capacity) -> BurnPlan {
    let mut to_place: Vec<ScannedFile> = changes
        .added
        .iter()
        .chain(changes.modified.iter())
        .cloned()
        .collect();
    to_place.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));

    // The newest open disc is the one to top off. A dead or finalized disc is
    // never a candidate, however much room it appears to have.
    let existing_open = discs
        .iter()
        .filter(|d| d.status == DiscStatus::Open)
        .max_by_key(|d| d.ordinal);
    let max_ordinal = discs.iter().map(|d| d.ordinal).max().unwrap_or(0);

    // The largest disc a file could ever land on this run: a fresh blank disc,
    // or the open disc at its full capacity. A file exceeding both can never be
    // placed and is oversized regardless of fill order.
    let open_usable = existing_open.map_or(0, |d| d.usable_bytes);
    let max_usable = new_disc_cap.usable_bytes.max(open_usable);

    let mut oversized: Vec<ScannedFile> = Vec::new();
    let mut placeable: Vec<ScannedFile> = Vec::new();
    for f in to_place {
        if f.size_bytes > max_usable {
            oversized.push(f);
        } else {
            placeable.push(f);
        }
    }

    let mut current = match existing_open {
        Some(d) => Bin {
            is_new: false,
            usable_bytes: d.usable_bytes,
            prior_used_bytes: d.used_bytes,
            files: Vec::new(),
        },
        None => Bin::fresh(new_disc_cap),
    };
    let mut completed: Vec<Bin> = Vec::new();

    // Place each file in order. A file that does not fit the current disc
    // spills to a fresh one; if it does not fit a fresh disc either — it only
    // fit the open disc's full capacity, not its remaining space — it is
    // oversized. This is the second stage of the oversize check.
    for f in placeable {
        if current.fits(&f) {
            current.files.push(f);
        } else if f.size_bytes <= new_disc_cap.usable_bytes {
            completed.push(std::mem::replace(&mut current, Bin::fresh(new_disc_cap)));
            current.files.push(f);
        } else {
            oversized.push(f);
        }
    }
    completed.push(current);

    let last_idx = completed.len() - 1;
    let planned_discs: Vec<PlannedDisc> = completed
        .into_iter()
        .enumerate()
        .map(|(i, b)| {
            // The first disc keeps the existing open disc's ordinal; every
            // fresh disc takes the next ordinal above the highest existing one.
            let ordinal = if !b.is_new {
                existing_open
                    .expect("a non-new bin exists only when an open disc does")
                    .ordinal
            } else if existing_open.is_some() {
                max_ordinal + i as u32
            } else {
                max_ordinal + i as u32 + 1
            };
            PlannedDisc {
                ordinal,
                is_new: b.is_new,
                will_finalize: i != last_idx,
                files: b.files,
                prior_used_bytes: b.prior_used_bytes,
                usable_bytes: b.usable_bytes,
            }
        })
        .collect();

    oversized.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));

    BurnPlan {
        discs: planned_discs,
        tombstones: changes.deleted.clone(),
        oversized,
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p diskrement-core --lib
```

Expected: `test result: ok. 42 passed`.

Diagnosis for the three most likely failures:

- `spilled_discs_get_ordinals_above_the_highest_existing_ordinal` reporting
  `[2, 3]` instead of `[2, 6]` — the ordinal arithmetic is using the open disc's
  ordinal rather than `max_ordinal`.
- `a_file_fitting_only_the_open_disc_is_placed_not_oversized` failing — the
  up-front `max_usable` filter is comparing against `new_disc_cap` alone instead
  of the max of that and the open disc's full capacity.
- `a_zero_byte_file_does_not_fit_an_overfull_disc` failing — `fits` is using
  `free_bytes()` (or any `saturating_sub` form) instead of the checked addition.

- [ ] **Step 6: Verify lint and format**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
```

Expected: clean. If clippy objects to the `if !b.is_new { .. } else if
existing_open.is_some() { .. } else { .. }` chain, keep the shape — the
`!b.is_new` case must win over both of the others, which a `match` on
`existing_open` alone cannot express without a guard arm.

- [ ] **Step 7: Commit**

```bash
git add crates/diskrement-core/src
git commit -m "feat(core): port the ordered-fill disc planner"
```

---

## Task 6: Manifest builder

Port `core/src/main/scala/diskrement/core/Manifest.scala`.

**Files:**
- Create: `crates/diskrement-core/src/manifest.rs`
- Modify: `crates/diskrement-core/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/diskrement-core/src/manifest.rs` with only the test module:

```rust
//! The pure manifest data for one session burned onto one disc.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::ChangeSet;
    use crate::model::{ContentHash, RelPath};
    use crate::planner::PlannedDisc;
    use crate::testutil::sf;

    #[test]
    fn a_manifest_splits_a_discs_files_into_added_vs_modified_using_the_change_set() {
        let added = vec![sf("new1", 10)];
        let modified = vec![sf("edit1", 10)];
        let cs = ChangeSet {
            added: added.clone(),
            modified: modified.clone(),
            ..ChangeSet::empty()
        };
        let disc = PlannedDisc {
            ordinal: 1,
            is_new: true,
            will_finalize: false,
            files: [added, modified].concat(),
            prior_used_bytes: 0,
            usable_bytes: 100,
        };
        let m = build(
            "photos",
            1,
            &disc,
            &cs,
            vec![RelPath("gone".to_string())],
        );
        assert_eq!(m.project_name, "photos");
        assert_eq!(m.disc_ordinal, 1);
        assert_eq!(m.session_ordinal, 1);
        assert_eq!(
            m.added.iter().map(|e| e.rel_path.as_str()).collect::<Vec<_>>(),
            vec!["new1"]
        );
        assert_eq!(
            m.modified.iter().map(|e| e.rel_path.as_str()).collect::<Vec<_>>(),
            vec!["edit1"]
        );
        assert_eq!(m.deleted, vec![RelPath("gone".to_string())]);
    }

    #[test]
    fn a_manifest_entry_carries_hash_and_size() {
        let f = sf("a", 42);
        let cs = ChangeSet {
            added: vec![f.clone()],
            ..ChangeSet::empty()
        };
        let disc = PlannedDisc {
            ordinal: 1,
            is_new: true,
            will_finalize: false,
            files: vec![f],
            prior_used_bytes: 0,
            usable_bytes: 100,
        };
        let m = build("p", 1, &disc, &cs, vec![]);
        assert_eq!(
            m.added[0],
            ManifestEntry {
                rel_path: RelPath("a".to_string()),
                content_hash: ContentHash("h-a".to_string()),
                size_bytes: 42,
            }
        );
    }

    #[test]
    fn manifest_change_kind_wire_names_round_trip() {
        // Frozen by the data-compatibility contract (spec §3.1). Nothing
        // validates these strings once they reach `manifest.json`, so this
        // test is the only thing standing between a typo and an unreadable
        // disc.
        for (kind, name) in [
            (ManifestChangeKind::Added, "ADDED"),
            (ManifestChangeKind::Modified, "MODIFIED"),
            (ManifestChangeKind::Deleted, "DELETED"),
        ] {
            assert_eq!(kind.as_str(), name);
            assert_eq!(name.parse::<ManifestChangeKind>().expect("parses"), kind);
        }
        assert!("REMOVED".parse::<ManifestChangeKind>().is_err());
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p diskrement-core --lib
```

Expected: compilation errors — `cannot find function build in this scope`,
`cannot find type ManifestEntry in this scope`.

- [ ] **Step 3: Write the implementation**

Insert above the `mod tests` block in `manifest.rs`:

```rust
use std::collections::HashSet;

use crate::diff::ChangeSet;
use crate::model::{ContentHash, RelPath, ScannedFile};
use crate::planner::PlannedDisc;

/// One file's identity as recorded in a session manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestEntry {
    pub rel_path: RelPath,
    pub content_hash: ContentHash,
    pub size_bytes: u64,
}

/// How a file changed, as recorded on the disc.
///
/// Serialized as the bare uppercase name and frozen by the compatibility
/// contract (spec §3.1); `shell` renders these strings, this crate defines
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestChangeKind {
    Added,
    Modified,
    Deleted,
}

impl ManifestChangeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ManifestChangeKind::Added => "ADDED",
            ManifestChangeKind::Modified => "MODIFIED",
            ManifestChangeKind::Deleted => "DELETED",
        }
    }
}

impl std::fmt::Display for ManifestChangeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ManifestChangeKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ADDED" => Ok(ManifestChangeKind::Added),
            "MODIFIED" => Ok(ManifestChangeKind::Modified),
            "DELETED" => Ok(ManifestChangeKind::Deleted),
            other => Err(format!("unknown manifest change kind: {other}")),
        }
    }
}

/// The pure manifest data for one session burned onto one disc.
///
/// Serialization to JSON is a shell concern and is not part of this crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionManifest {
    pub project_name: String,
    pub disc_ordinal: u32,
    pub session_ordinal: u32,
    pub added: Vec<ManifestEntry>,
    pub modified: Vec<ManifestEntry>,
    pub deleted: Vec<RelPath>,
}

/// Build the manifest for `disc`, classifying its files via `changes` and
/// attaching the given `tombstones` (typically only on the last, open disc).
///
/// The partition preserves the disc's file order, which the planner already
/// sorted by rel path.
pub fn build(
    project_name: &str,
    session_ordinal: u32,
    disc: &PlannedDisc,
    changes: &ChangeSet,
    tombstones: Vec<RelPath>,
) -> SessionManifest {
    let added_paths: HashSet<&RelPath> = changes.added.iter().map(|f| &f.rel_path).collect();
    let (added_files, modified_files): (Vec<&ScannedFile>, Vec<&ScannedFile>) = disc
        .files
        .iter()
        .partition(|f| added_paths.contains(&f.rel_path));

    SessionManifest {
        project_name: project_name.to_string(),
        disc_ordinal: disc.ordinal,
        session_ordinal,
        added: added_files.into_iter().map(to_entry).collect(),
        modified: modified_files.into_iter().map(to_entry).collect(),
        deleted: tombstones,
    }
}

fn to_entry(f: &ScannedFile) -> ManifestEntry {
    ManifestEntry {
        rel_path: f.rel_path.clone(),
        content_hash: f.content_hash.clone(),
        size_bytes: f.size_bytes,
    }
}
```

- [ ] **Step 4: Wire the module into `lib.rs`**

Add `pub mod manifest;` after `pub mod diff;`, and add:

```rust
pub use manifest::{ManifestChangeKind, ManifestEntry, SessionManifest};
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p diskrement-core --lib
```

Expected: `test result: ok. 45 passed`.

- [ ] **Step 6: Verify lint and format, then commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/diskrement-core/src
git commit -m "feat(core): port the session manifest builder"
```

---

## Task 7: Proptest strategies and the diff properties

Port `Generators.scala`, `GeneratorsTest.scala`, and `DiffPropertyTest.scala`.

**Files:**
- Modify: `crates/diskrement-core/src/testutil.rs`
- Modify: `crates/diskrement-core/src/diff.rs`

- [ ] **Step 1: Write the strategies**

Append the following to `testutil.rs`, but move its four `use` lines up to join
the imports already at the top of the file — rustfmt will not reorder imports
that sit below other items, and a file with `use` statements scattered through it
is a file nobody wants to read.

Append to `testutil.rs`:

```rust
use std::collections::HashSet;

use proptest::prelude::*;

use crate::model::Capacity;
use crate::planner::{DiscState, DiscStatus};

/// A rel path of 1-4 identifier-ish segments joined by '/', mirroring
/// `Generators.genRelPath`.
pub fn rel_path_strategy() -> impl Strategy<Value = RelPath> {
    proptest::collection::vec("[a-zA-Z][a-zA-Z0-9_]{0,7}", 1..=4)
        .prop_map(|segs| RelPath(segs.join("/")))
}

/// An 8-character hex hash, mirroring `Generators.genHash`.
pub fn content_hash_strategy() -> impl Strategy<Value = ContentHash> {
    "[0-9a-f]{8}".prop_map(ContentHash)
}

/// A scanned file with a positive size, mirroring `Generators.genScannedFile`.
pub fn scanned_file_strategy() -> impl Strategy<Value = ScannedFile> {
    (
        rel_path_strategy(),
        content_hash_strategy(),
        1u64..=5_000_000_000,
    )
        .prop_map(|(rel_path, content_hash, size_bytes)| ScannedFile {
            rel_path,
            content_hash,
            size_bytes,
            mtime: epoch(),
        })
}

/// Scanned files with unique rel paths, mirroring `Generators.genScannedFiles`.
///
/// The Scala deduped with `groupBy(_.relPath).values.map(_.head)`, which loses
/// input order; keeping first-seen order costs nothing and makes shrinking
/// reproducible.
pub fn scanned_files_strategy() -> impl Strategy<Value = Vec<ScannedFile>> {
    proptest::collection::vec(scanned_file_strategy(), 0..12).prop_map(|files| {
        let mut seen: HashSet<RelPath> = HashSet::new();
        files
            .into_iter()
            .filter(|f| seen.insert(f.rel_path.clone()))
            .collect()
    })
}

/// A capacity comfortably larger than the largest generated file, so that
/// "oversized" stays a deliberate, separate concern.
pub fn new_disc_cap_strategy() -> impl Strategy<Value = Capacity> {
    (1_000_000_000u64..=10_000_000_000).prop_map(Capacity::new)
}

/// Files guaranteed to fit on a disc of the given capacity.
pub fn files_within_strategy(cap: Capacity) -> impl Strategy<Value = Vec<ScannedFile>> {
    scanned_files_strategy().prop_map(move |files| {
        files
            .into_iter()
            .map(|mut f| {
                f.size_bytes = (f.size_bytes % cap.usable_bytes).max(1);
                f
            })
            .collect()
    })
}

/// A pre-existing disc chain: zero to four discs with ascending ordinals, each
/// FINALIZED except possibly the last, which may be OPEN.
///
/// This mirrors the catalog's auto-finalize invariant — only the newest disc
/// stays open — and exercises multi-disc catalogs and open discs at ordinals
/// other than 1.
pub fn disc_states_strategy(cap: Capacity) -> impl Strategy<Value = Vec<DiscState>> {
    (
        proptest::collection::vec(0u64..=cap.usable_bytes, 0..=4),
        any::<bool>(),
    )
        .prop_map(move |(useds, last_open)| {
            let n = useds.len();
            useds
                .into_iter()
                .enumerate()
                .map(|(idx, used_bytes)| DiscState {
                    ordinal: idx as u32 + 1,
                    status: if idx + 1 == n && last_open {
                        DiscStatus::Open
                    } else {
                        DiscStatus::Finalized
                    },
                    usable_bytes: cap.usable_bytes,
                    used_bytes,
                })
                .collect()
        })
}

/// A full planner input: capacity, a pre-existing disc chain, and changes that
/// fit. Mirrors `Generators.genPlannerInput`, including the arbitrary split of
/// the same files across the `added` and `modified` buckets.
pub fn planner_input_strategy() -> impl Strategy<Value = (ChangeSet, Vec<DiscState>, Capacity)> {
    new_disc_cap_strategy().prop_flat_map(|cap| {
        (
            disc_states_strategy(cap),
            files_within_strategy(cap),
            any::<proptest::sample::Index>(),
        )
            .prop_map(move |(discs, files, idx)| {
                let split = idx.index(files.len() + 1);
                let (add, modi) = files.split_at(split);
                (
                    ChangeSet {
                        added: add.to_vec(),
                        modified: modi.to_vec(),
                        ..ChangeSet::empty()
                    },
                    discs,
                    cap,
                )
            })
    })
}
```

- [ ] **Step 2: Write the property tests**

Add to `diff.rs`'s `mod tests` block:

```rust
    use crate::testutil::{scanned_file_strategy, scanned_files_strategy};
    use proptest::prelude::*;

    // --- GeneratorsTest.scala ---

    proptest! {
        #[test]
        fn generated_scanned_file_lists_have_unique_rel_paths(
            files in scanned_files_strategy()
        ) {
            let mut paths: Vec<&RelPath> = files.iter().map(|f| &f.rel_path).collect();
            let total = paths.len();
            paths.sort();
            paths.dedup();
            prop_assert_eq!(paths.len(), total);
        }

        #[test]
        fn generated_file_sizes_are_positive(f in scanned_file_strategy()) {
            prop_assert!(f.size_bytes > 0);
        }
    }

    // --- DiffPropertyTest.scala ---

    proptest! {
        #[test]
        fn every_scanned_file_is_classified_exactly_once(
            files in scanned_files_strategy()
        ) {
            // Against an empty catalog nothing is deleted and everything is added.
            let cs = diff(&CatalogView::empty(), &files);
            let classified = cs.added.len() + cs.modified.len() + cs.unchanged.len();
            prop_assert_eq!(classified, files.len());
            prop_assert!(cs.deleted.is_empty());
        }

        #[test]
        fn diff_is_idempotent_recording_a_scan_yields_an_empty_diff_next_time(
            files in scanned_files_strategy()
        ) {
            let catalog = CatalogView::from_scanned(&files);
            let cs = diff(&catalog, &files);
            prop_assert!(cs.is_empty());
            prop_assert_eq!(cs.unchanged.len(), files.len());
        }
    }
```

- [ ] **Step 3: Run the tests to verify they pass**

```bash
cargo test -p diskrement-core --lib
```

Expected: `test result: ok. 49 passed`. Each `proptest!` body becomes one test,
so this adds 4.

If `generated_scanned_file_lists_have_unique_rel_paths` fails, the dedup in
`scanned_files_strategy` is filtering on the wrong key — the whole
`ScannedFile` rather than its `rel_path`.

- [ ] **Step 4: Verify lint and format, then commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/diskrement-core/src
git commit -m "test(core): port proptest strategies and the diff properties"
```

---

## Task 8: Planner property tests

Port `PlannerPropertyTest.scala` — all eight properties. These are the heart of
the port: they are what proves the planner behaves like the original on inputs
nobody wrote by hand.

**Files:**
- Modify: `crates/diskrement-core/src/planner.rs`
- Modify: `crates/diskrement-core/src/testutil.rs`

- [ ] **Step 1: Add the reshuffling strategy**

Property 5 needs the same input twice: once as generated, once with all
placeable files permuted and re-split across the `added`/`modified` buckets.

Append to `testutil.rs`:

```rust
/// A planner input paired with an equivalent one: the same placeable files,
/// shuffled and re-split across `added`/`modified`. Property 5 asserts both
/// produce an identical plan.
pub fn reshuffled_planner_input_strategy()
-> impl Strategy<Value = (ChangeSet, ChangeSet, Vec<DiscState>, Capacity)> {
    planner_input_strategy().prop_flat_map(|(cs, discs, cap)| {
        let combined: Vec<ScannedFile> = cs
            .added
            .iter()
            .chain(cs.modified.iter())
            .cloned()
            .collect();
        let n = combined.len();
        (
            Just(cs),
            Just(discs),
            Just(cap),
            Just(combined).prop_shuffle(),
            0..=n,
        )
            .prop_map(|(cs, discs, cap, shuffled, k)| {
                let (add, modi) = shuffled.split_at(k);
                let alt = ChangeSet {
                    added: add.to_vec(),
                    modified: modi.to_vec(),
                    ..ChangeSet::empty()
                };
                (cs, alt, discs, cap)
            })
    })
}
```

- [ ] **Step 2: Write the property tests**

Add to `planner.rs`'s `mod tests` block:

```rust
    use crate::testutil::{planner_input_strategy, reshuffled_planner_input_strategy};
    use proptest::prelude::*;

    fn placeable(cs: &ChangeSet) -> Vec<&ScannedFile> {
        cs.added.iter().chain(cs.modified.iter()).collect()
    }

    fn sorted_paths(mut v: Vec<String>) -> Vec<String> {
        v.sort();
        v
    }

    proptest! {
        /// 1. Conservation: every placeable file is placed or oversized, once.
        #[test]
        fn every_placeable_file_is_placed_or_oversized_exactly_once(
            (cs, discs, cap) in planner_input_strategy()
        ) {
            let p = plan(&cs, &discs, cap);
            let mut all: Vec<String> = p
                .discs
                .iter()
                .flat_map(|d| d.files.iter())
                .chain(p.oversized.iter())
                .map(|f| f.rel_path.0.clone())
                .collect();
            let expected = sorted_paths(
                placeable(&cs).iter().map(|f| f.rel_path.0.clone()).collect(),
            );
            let total = all.len();
            all.sort();
            prop_assert_eq!(&all, &expected);
            all.dedup();
            prop_assert_eq!(all.len(), total, "a file was placed twice");
        }

        /// 2. Capacity: no disc exceeds its usable capacity.
        #[test]
        fn no_disc_exceeds_its_usable_capacity(
            (cs, discs, cap) in planner_input_strategy()
        ) {
            let p = plan(&cs, &discs, cap);
            for d in &p.discs {
                prop_assert!(
                    d.total_used_bytes() <= d.usable_bytes,
                    "disc {} holds {} of {} usable",
                    d.ordinal,
                    d.total_used_bytes(),
                    d.usable_bytes
                );
            }
        }

        /// 3. No split: each placed file appears on exactly one disc.
        #[test]
        fn each_placed_file_appears_on_exactly_one_disc(
            (cs, discs, cap) in planner_input_strategy()
        ) {
            let p = plan(&cs, &discs, cap);
            let mut placed: Vec<String> = p
                .discs
                .iter()
                .flat_map(|d| d.files.iter())
                .map(|f| f.rel_path.0.clone())
                .collect();
            let total = placed.len();
            placed.sort();
            placed.dedup();
            prop_assert_eq!(placed.len(), total);
        }

        /// 4. Order: placed files are in lexicographic order across all discs.
        #[test]
        fn placed_files_are_in_lexicographic_order_across_discs(
            (cs, discs, cap) in planner_input_strategy()
        ) {
            let p = plan(&cs, &discs, cap);
            let placed: Vec<String> = p
                .discs
                .iter()
                .flat_map(|d| d.files.iter())
                .map(|f| f.rel_path.0.clone())
                .collect();
            prop_assert_eq!(&placed, &sorted_paths(placed.clone()));
        }

        /// 5. Determinism: any permutation or redistribution of the inputs
        ///    yields an identical plan.
        #[test]
        fn any_permutation_or_redistribution_yields_an_identical_plan(
            (cs, alt, discs, cap) in reshuffled_planner_input_strategy()
        ) {
            prop_assert_eq!(plan(&cs, &discs, cap), plan(&alt, &discs, cap));
        }

        /// 6. Open-disc-first: if an open disc exists, it is the first planned.
        #[test]
        fn an_open_disc_is_always_the_first_planned_disc(
            (cs, discs, cap) in planner_input_strategy()
        ) {
            let p = plan(&cs, &discs, cap);
            // The generator produces at most one OPEN disc, so "find the first"
            // and "take the newest" agree here — same as the Scala property.
            if let (Some(open), Some(first)) = (
                discs.iter().find(|d| d.status == DiscStatus::Open),
                p.discs.first(),
            ) {
                prop_assert_eq!(first.ordinal, open.ordinal);
                prop_assert!(!first.is_new);
            }
        }

        /// 7. Finalize: at most one planned disc stays open, and it is the last.
        #[test]
        fn at_most_one_planned_disc_stays_open_and_it_is_the_last(
            (cs, discs, cap) in planner_input_strategy()
        ) {
            let p = plan(&cs, &discs, cap);
            let open_count = p.discs.iter().filter(|d| !d.will_finalize).count();
            prop_assert!(open_count <= 1);
            if let Some(last) = p.discs.last() {
                prop_assert!(!last.will_finalize);
            }
        }

        /// 8. Tombstones cost nothing: deletions add no bytes to any disc.
        #[test]
        fn deletions_add_no_bytes_to_any_disc(
            (cs, discs, cap) in planner_input_strategy()
        ) {
            let with_deletes = ChangeSet {
                deleted: vec![RelPath("zzz/deleted".to_string())],
                ..cs.clone()
            };
            let a: u64 = plan(&cs, &discs, cap).discs.iter().map(|d| d.added_bytes()).sum();
            let b: u64 = plan(&with_deletes, &discs, cap)
                .discs
                .iter()
                .map(|d| d.added_bytes())
                .sum();
            prop_assert_eq!(a, b);
        }
    }
```

- [ ] **Step 3: Run the tests to verify they pass**

```bash
cargo test -p diskrement-core --lib
```

Expected: `test result: ok. 57 passed`.

If property 2 fails, read the shrunk counterexample carefully before touching
the planner: an `used_bytes` above `usable_bytes` in the *input* means the
generator is wrong, whereas a `total_used_bytes()` above `usable_bytes` in the
*output* means `Bin::fits` is wrong.

If property 5 fails, the planner is reading `changes.added` and
`changes.modified` in a way that distinguishes them — the only legitimate use of
that split is the manifest builder, never the planner.

- [ ] **Step 4: Verify lint and format, then commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
git add crates/diskrement-core/src
git commit -m "test(core): port the eight planner properties"
```

---

## Task 9: Retire the scaffolding, document, and verify the phase

**Files:**
- Modify: `crates/diskrement-core/src/lib.rs`
- Modify: `crates/diskrement-shell/src/lib.rs`
- Create: `crates/diskrement-core/README.md`

- [ ] **Step 1: Replace the P0 `Probe` stub with a real type**

`Probe` existed only to give the `shell -> core` edge something to link. Real
types exist now, so delete it and re-point the shell's edge proof.

In `crates/diskrement-core/src/lib.rs`, delete the `Probe` struct and its doc
comment. The final `lib.rs` reads:

```rust
//! diskrement's functional core: pure data and pure functions, no I/O.
//!
//! Nothing here touches the filesystem, SQLite, a subprocess, or the clock.
//! The shell crate owns every effect; this crate owns the change-set,
//! planning, and manifest models (spec §5.3).
//!
//! Functions are reached through their modules — `diff::diff`, `planner::plan`,
//! `manifest::build` — mirroring the Scala `Diff.diff` / `Planner.plan` /
//! `ManifestBuilder.build`. Types are re-exported here for convenience.

pub mod diff;
pub mod manifest;
pub mod model;
pub mod planner;

#[cfg(test)]
// Helpers land in the task that first needs them, which is sometimes one task
// ahead of their first consumer; the allow keeps that from failing `-D warnings`
// mid-phase.
#[allow(dead_code)]
mod testutil;

pub use diff::ChangeSet;
pub use manifest::{ManifestChangeKind, ManifestEntry, SessionManifest};
pub use model::{
    Capacity, CatalogEntry, CatalogView, ContentHash, DiscType, RelPath, ScannedFile,
};
pub use planner::{BurnPlan, DiscState, DiscStatus, PlannedDisc};

#[cfg(test)]
mod tests {
    #[test]
    fn crate_builds() {
        // Mirrors the Scala `SmokeTest`: proves the crate compiles and its test
        // harness runs.
    }
}
```

Replace `crates/diskrement-shell/src/lib.rs` with:

```rust
//! diskrement's imperative shell: every effect lives here — the SQLite
//! catalog, file scanning, CoW snapshots, staging, and the burners
//! (spec §5.3, §5.5).

use diskrement_core::RelPath;

/// Temporary scaffolding: proves the `shell -> core` edge links against a real
/// core type. P2 deletes it when the catalog store consumes core types for
/// real.
pub fn probe() -> RelPath {
    RelPath("probe".to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn core_edge_links() {
        assert_eq!(super::probe().as_str(), "probe");
    }
}
```

`crates/diskrement-cli/src/main.rs` still compiles unchanged: it only discards
the value.

- [ ] **Step 2: Confirm the purity constraint actually holds**

```bash
cargo tree --package diskrement-core --edges normal
```

Expected: `diskrement-core v0.1.0` with exactly one child, `chrono v0.4.x`, plus
chrono's own non-optional dependencies beneath it. If `proptest` appears under
`--edges normal`, it has been misplaced into `[dependencies]` — move it back to
`[dev-dependencies]`.

- [ ] **Step 3: Confirm the clock is genuinely unreachable**

The purity claim in the spec is only worth as much as the check behind it. Prove
it, then remove the proof.

Temporarily add to `lib.rs`:

```rust
#[cfg(test)]
mod purity_check {
    #[test]
    fn there_is_no_system_clock_here() {
        let _ = chrono::Utc::now();
    }
}
```

```bash
cargo test -p diskrement-core --lib 2>&1 | head -20
```

Expected: a **compilation error** — `no function or associated item named 'now'
found for struct 'Utc'`. That is the `default-features = false` guard working.

**Delete the `purity_check` module before continuing.** If it compiled instead,
the `clock` feature has been enabled somewhere — most likely another workspace
crate turning it on, since Cargo unifies features across the graph. Check
`cargo tree -p diskrement-core -f '{p} {f}'` and fix the offender before
committing.

- [ ] **Step 4: Write the crate README**

Create `crates/diskrement-core/README.md`:

```markdown
# diskrement-core

The functional core of diskrement: pure data and pure functions, no I/O.

- `model` — `RelPath`, `ContentHash`, `ScannedFile`, `CatalogEntry`,
  `CatalogView`, `DiscType`, `Capacity`
- `diff` — `ChangeSet` and `diff()`: classify a scan against the catalog's
  current view
- `planner` — `plan()`: ordered-fill disc packing, top off before spilling
- `manifest` — `build()`: the per-session manifest for one disc

This crate depends only on `chrono` with default features disabled, which
excludes the `clock` feature. `Utc::now()` does not compile here. Anything that
touches the filesystem, SQLite, or a subprocess belongs in `diskrement-shell`.

Two things in here are load-bearing beyond their size:

- **Wire spellings.** `DiscType::as_str` and `ManifestChangeKind::as_str` are
  frozen by the data-compatibility contract in
  `docs/superpowers/specs/2026-09-05-rust-rewrite-design.md` §3.1. The catalog's
  `check` constraints catch a wrong disc-type string; nothing catches a wrong
  manifest-kind string, so its round-trip test is the only guard. Do not change
  these strings.
- **`Bin::fits` in the planner** is written as an addition rather than a
  subtraction against `free_bytes()`. The two disagree on an overfull disc, and
  the addition is the one that matches the Scala. See
  `a_zero_byte_file_does_not_fit_an_overfull_disc`.
```

- [ ] **Step 5: Run the full phase verification**

```bash
cargo test --workspace \
  && cargo clippy --workspace --all-targets -- -D warnings \
  && cargo fmt --check
```

Expected: the `diskrement-core` lib target reports `test result: ok. 57 passed`,
`diskrement-shell` reports 1, `diskrement-cli`'s `binary.rs` reports 1; then
silence from clippy and rustfmt.

- [ ] **Step 6: Confirm the Scala build is untouched**

```bash
git status --porcelain
git diff --stat main -- core shell cli build.sbt project
sbt core/test
```

Expected: `git status` clean; the diff against `main` for those paths **empty**;
`sbt core/test` still reporting **51 tests passing**. P1 adds Rust; it removes
nothing.

- [ ] **Step 7: Commit**

```bash
git add crates/diskrement-core crates/diskrement-shell
git commit -m "docs(core): document diskrement-core; retire the P0 probe scaffolding"
```

---

## Phase P1 acceptance

- [ ] `cargo test -p diskrement-core --lib` reports **57 passing**, 0 failing.
- [ ] `cargo test --workspace` is green across all three crates.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` is silent.
- [ ] `cargo fmt --check` is silent.
- [ ] `cargo tree -p diskrement-core --edges normal` shows `chrono` as the only
      direct dependency, and a temporary `Utc::now()` call fails to compile.
- [ ] Every test in `core/src/test/scala/diskrement/core/` has a counterpart.
      The Rust suite is larger by six, because six ports gained a case:

      | Source | Scala | Rust | Added |
      |---|---|---|---|
      | `SmokeTest` | 1 | 1 | — (landed in P0) |
      | `ModelTest` | 4 | 5 | `CatalogView::from_scanned` |
      | `DiscTypeTest` | 5 | 7 | wire-name round-trip; unknown-name rejection |
      | `ChangeSetTest` + `DiffTest` | 8 | 8 | — |
      | `PlannerTypesTest` | 3 | 4 | `free_bytes` saturation on an overfull disc |
      | `PlannerTest` + `PlannerEdgeTest` | 16 | 17 | zero-byte file on an overfull disc |
      | `ManifestBuilderTest` | 2 | 3 | `ManifestChangeKind` wire names |
      | `GeneratorsTest` | 2 | 2 | — |
      | `DiffPropertyTest` | 2 | 2 | — |
      | `PlannerPropertyTest` | 8 | 8 | — |
      | **Total** | **51** | **57** | **+6** |

- [ ] `git diff main -- core shell cli build.sbt project` is empty and
      `sbt core/test` still reports 51 tests passing.

**Next:** phase P2 (the catalog: schema, migrations, store, and the `time`
module whose round-trip is pinned against the timestamp strings harvested in
P0), which gets its own plan document.
