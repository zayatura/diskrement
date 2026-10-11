# Enforced Redundant Copies Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `copies = N` real. Every current version gets N burned copies on N distinct physical discs, one per copy set (1A/1B …). One cycle burns one disc per set, each disc commits as soon as it burns, and a lost copy is restored on the next cycle.

**Architecture:** Spec `docs/superpowers/specs/2026-10-11-multiple-copies-design.md`.
- **Core (`diskrement-core`):** gains a `DiscKey` (ordinal + copy set) and a `DiscArg` (what the operator types). A new pure `replication` module holds `shortfall`, `classify` and `coverage`. A per-set `planner::plan_set` sits beside the existing single-set `plan`.
- **Catalog:** migration V3 rebuilds `disc` with a `copy_set` column and a per-set unique key. `read_view` reports each version's live copy sets. A new write path records *copy placements*: a `placement` row with no `file_version` and no `change_record`.
- **Pipeline:** `plan` plans one disc per set into a v2 `plan.json`. `burn` writes the sets in order, commits each disc as soon as it burns, rewrites the staged plan after every commit, and resumes after a failure or a crash.
- **CLI:** parses `3B`, refuses `--copies` outside 1..=26, and renders copy sets. Every `copies = 1` output stays byte-identical.

**Tech Stack:** Rust 2021 workspace (`diskrement-core` → `diskrement-shell` → `diskrement-cli`); `rusqlite` (bundled SQLite), `serde`/`serde_json`, `clap` 4 derive, `proptest`, `tempfile`, `assert_cmd`, `predicates`, `insta`. **No new dependencies.**

---

## Global Constraints

- **Branch:** `feat/multiple-copies` already exists and holds the spec commit. Work there; never commit to `main`. At the end, merge with an explicit merge commit `Merge feat/multiple-copies: <summary>` and delete the branch locally and on `origin` (CLAUDE.md "Repo etiquette").
- **Commits:** conventional with a scope (`feat(core):`, `feat(catalog):`, `feat(pipeline):`, `feat(cli):`, `test(…):`, `docs(…):`, `chore:`). End every commit message with
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`, plus any further attribution line your session's instructions require.
- **After every task**, all three must pass before committing:

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

  Record the test count printed by `cargo test --workspace` in each task's commit body as `tests: <n> passed`. Only Task 1 may lower it, and it says why.
- **Scenario runs used as evidence** need a CoW `TMPDIR` and `DISKREMENT_SCENARIOS_REQUIRED=1`. Without them the scenarios print `SKIPPED`, which proves nothing. Tasks 15 and 16 must run:

```bash
DISKREMENT_SCENARIOS_REQUIRED=1 TMPDIR=<a btrfs/XFS dir> cargo test -p diskrement-cli
```

- **Invariants that must still hold** (CLAUDE.md):
  - The catalog history is append-only, and the only in-place writes are the two forward-only status transitions.
  - `DEAD` is derived, never stored.
  - No `PLANNED` rows.
  - `Pipeline` returns data and does no I/O.
  - Exit codes are unchanged.
  - `StalePlan` advice is "re-run plan", never "reset".
- **Fixture databases** are opened `?immutable=1` or copied to a `TempDir` before `Catalog::open`. Never write inside `fixtures/`.
- **Mechanical call-site updates.** When a task changes a type or signature, run `cargo build --workspace --all-targets 2>&1 | grep -E '^(error|warning)' -A6` and fix each site the compiler names, using that task's stated replacement rule. The rules are exact. Do not improvise new behaviour at a call site.

## File Structure

| File | Responsibility | Tasks |
|---|---|---|
| `crates/diskrement-core/src/model.rs` | `DiscKey`, `DiscArg`, `MAX_COPIES`, `set_letter`; `CatalogEntry.live_sets` | 2, 3 |
| `crates/diskrement-core/src/replication.rs` (new) | `shortfall`, `classify`/`DiscContents`, `coverage`/`SetCoverage` | 4 |
| `crates/diskrement-core/src/planner.rs` | `DiscState.copy_set`, `PlannedDisc.copy_set`, `plan_set` | 3, 5 |
| `crates/diskrement-core/src/manifest.rs` | `SessionManifest.copy_set`/`copied`, `build` from `DiscContents` | 6 |
| `crates/diskrement-shell/src/catalog/schema.rs` | migration V3 and the foreign-key-off rebuild hook | 7 |
| `crates/diskrement-shell/src/catalog/read.rs` | `live_sets`, `copy_set`, `DiscKey`-keyed reads | 8 |
| `crates/diskrement-shell/src/catalog/store.rs` | `DiscKey`-keyed writes, copy placements, three-way mark-dead | 9, 10 |
| `crates/diskrement-shell/src/catalog/model.rs` | `DiscWrite.copy_set`, `SessionWrite.copied`, `LostReport.degraded` | 8, 9, 10 |
| `crates/diskrement-shell/src/burn/{mod,json,media,verifier,testkit}.rs`, `burn/xorriso/{args,burner}.rs` | identity carries the copy set; JSON fields; disc-name staging dirs and volids | 6, 11 |
| `crates/diskrement-shell/src/pipeline/planning.rs` | per-set `plan_from`, `stale_detail` per disc | 12 |
| `crates/diskrement-shell/src/pipeline/staged.rs` | `plan.json` v2 and legacy detection | 12 |
| `crates/diskrement-shell/src/pipeline/reports.rs` | `SetPlan`, multi-disc `BurnReport`, coverage in `StatusReport` | 12, 13 |
| `crates/diskrement-shell/src/pipeline/mod.rs` | the per-disc burn loop; `DiscArg` resolution | 12, 13 |
| `crates/diskrement-shell/src/error.rs` | `PartialBurn`; disc errors carry names | 12, 13 |
| `crates/diskrement-cli/src/{args,main,render}.rs` | `--copies` range, `DiscArg` arguments, copy-set rendering | 12–14 |
| `crates/diskrement-cli/tests/{binary.rs,scenarios/s13_copies.rs}` | end-to-end contract | 15 |
| `CLAUDE.md`, `fixtures/README.md`, spec | docs | 1, 16 |

---

### Task 0: Baseline

**Files:** none.

- [ ] **Step 1: Record the starting state**

Run:
```bash
git switch feat/multiple-copies
cargo test --workspace 2>&1 | grep -E '^test result' | awk '{s+=$4} END {print "passed:", s}'
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check && echo CLEAN
```
Expected: a `passed: <N>` line and `CLEAN`. Write `N` down; Task 1's commit explains the drop from it.

---

### Task 1: Retire the Scala-comparison fixture tests

The user decided that the Scala build no longer holds the project back (see the spec's §3.1 amendment in this task). Seven test files compare Rust output byte for byte, or row for row, against what the Scala build wrote. V3 and the new JSON fields make every one of them fail by design. These tests stay, because they use `fixtures/` as *input* (V2 catalogs and staged discs) rather than as an oracle: `fixture_catalog.rs`, `fixture_verify.rs` and `timestamps.rs`.

**Files:**
- Delete: `crates/diskrement-shell/tests/fixture_schema.rs`, `fixture_manifest_hash.rs`, `fixture_burn.rs`, `fixture_replay.rs`, `fixture_mark_dead.rs`, `fixture_plan.rs`, `fixture_pointers.rs`
- Modify: `crates/diskrement-shell/src/testutil.rs` (drop helpers left without users)
- Modify: `fixtures/README.md`, `CLAUDE.md`, `docs/superpowers/specs/2026-10-11-multiple-copies-design.md`

- [ ] **Step 1: Delete the seven files**

```bash
cd crates/diskrement-shell/tests
git rm fixture_schema.rs fixture_manifest_hash.rs fixture_burn.rs fixture_replay.rs \
       fixture_mark_dead.rs fixture_plan.rs fixture_pointers.rs
cd -
```

- [ ] **Step 2: Remove helpers that lost their last user**

Run:
```bash
grep -rn "dump_catalog\|assert_dumps_match\|dump_tables\|CATALOG_TABLES\|SCANCACHE_TABLES" crates --include=*.rs
```
For each of the five names, if the only hits are its definition in `crates/diskrement-shell/src/testutil.rs` and the re-export line in `crates/diskrement-shell/tests/support/mod.rs`, delete the definition and drop the name from the `pub use diskrement_shell::testutil::{…}` line. If that leaves the `pub use` empty, delete the line. Keep any name that still has another user. Also delete the private `render` helper in `testutil.rs` if `dump_tables` was its only caller.

- [ ] **Step 3: Say so in `fixtures/README.md`**

Insert directly below the `# Golden fixture corpus (Rust rewrite)` heading:

```markdown
> **Status (2026-10-11):** the tests that compared Rust output against this
> corpus byte for byte or row for row (schema, manifest hash, burn replay,
> catalog replay, mark-dead replay, plan replay, pointers) were retired with
> the multiple-copies change, which deliberately changed the schema (V3) and
> the on-disc JSON. The corpus remains as **input**: V2 catalogs that must
> still migrate and read (`fixture_catalog.rs`), staged discs the verifier
> must accept (`fixture_verify.rs`), and timestamp strings (`timestamps.rs`).
```

- [ ] **Step 4: Update `CLAUDE.md`**

In the "Build & test" section, replace the paragraph that begins `The differential harness that used to compare against the Scala binary` with:

```markdown
The differential harness that used to compare against the Scala binary was deleted with the Scala tree at P10, and the byte/row comparisons against `fixtures/` were retired with multiple copies (schema V3). `fixtures/` is now a frozen corpus of V2 catalogs and staged discs, used only as input; it cannot be regenerated from `main`.
```

- [ ] **Step 5: Amend the spec**

In `docs/superpowers/specs/2026-10-11-multiple-copies-design.md` §3.1 "Invariants that change", append this bullet:

```markdown
- **The Scala-comparison fixture tests are retired.** Seven tests compared Rust
  output byte for byte or row for row against `fixtures/` (schema, manifest
  hash, burn replay, catalog replay, mark-dead replay, plan replay, pointers);
  V3 and the new JSON fields break each of them by design. The corpus stays as
  input: V2 catalogs that must migrate and read, staged discs the verifier
  must accept, and timestamp strings.
```

- [ ] **Step 6: Verify and commit**

Run the three Global Constraints commands. Expected: all pass, with a lower test count than Task 0.

```bash
git add -A
git commit -m "test(shell): retire the Scala-comparison fixture tests

Schema V3 and the copy-set JSON fields change both formats on purpose;
the corpus stays as V2 input for migration, read and verify tests.

tests: <n> passed (was <N>; the seven retired files held the difference)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: `DiscKey`, `DiscArg` and set letters in core

**Files:**
- Modify: `crates/diskrement-core/src/model.rs`
- Modify: `crates/diskrement-core/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Append inside `mod tests` in `crates/diskrement-core/src/model.rs`:

```rust
    #[test]
    fn a_disc_name_carries_its_set_letter_only_when_copies_exceed_one() {
        assert_eq!(DiscKey::new(3, 0).name(1), "3");
        assert_eq!(DiscKey::new(3, 0).name(2), "3A");
        assert_eq!(DiscKey::new(3, 1).name(2), "3B");
        assert_eq!(DiscKey::new(12, 25).name(26), "12Z");
    }

    #[test]
    fn disc_keys_order_by_ordinal_then_set() {
        let mut keys = vec![DiscKey::new(2, 0), DiscKey::new(1, 1), DiscKey::new(1, 0)];
        keys.sort();
        assert_eq!(
            keys,
            vec![DiscKey::new(1, 0), DiscKey::new(1, 1), DiscKey::new(2, 0)]
        );
    }

    #[test]
    fn a_disc_argument_parses_with_or_without_a_set_letter() {
        assert_eq!(
            "3".parse::<DiscArg>(),
            Ok(DiscArg { ordinal: 3, copy_set: None })
        );
        assert_eq!(
            "3B".parse::<DiscArg>(),
            Ok(DiscArg { ordinal: 3, copy_set: Some(1) })
        );
        assert_eq!(
            "3b".parse::<DiscArg>(),
            Ok(DiscArg { ordinal: 3, copy_set: Some(1) })
        );
        // A bare negative ordinal reaches the domain (exit 2), as today.
        assert_eq!(
            "-1".parse::<DiscArg>(),
            Ok(DiscArg { ordinal: -1, copy_set: None })
        );
    }

    #[test]
    fn a_malformed_disc_argument_is_refused() {
        for bad in ["", "B", "B3", "3BB", "-1B", "0B", "3 B", "3é"] {
            assert!(bad.parse::<DiscArg>().is_err(), "{bad:?} should not parse");
        }
    }

    #[test]
    fn a_disc_argument_displays_as_typed_but_upper_case() {
        assert_eq!("3b".parse::<DiscArg>().unwrap().to_string(), "3B");
        assert_eq!("-1".parse::<DiscArg>().unwrap().to_string(), "-1");
    }

    #[test]
    fn a_disc_argument_resolves_only_inside_the_projects_sets() {
        let arg = |s: &str| s.parse::<DiscArg>().unwrap();
        assert_eq!(arg("3").resolve(1), Some(DiscKey::new(3, 0)));
        assert_eq!(arg("3A").resolve(1), Some(DiscKey::new(3, 0)));
        assert_eq!(arg("3B").resolve(1), None);
        assert_eq!(arg("3B").resolve(2), Some(DiscKey::new(3, 1)));
        assert_eq!(arg("-1").resolve(2), None);
        assert_eq!(arg("0").resolve(2), None);
    }

    #[test]
    fn set_letters_run_from_a_to_z() {
        assert_eq!(set_letter(0), 'A');
        assert_eq!(set_letter(25), 'Z');
        assert_eq!(MAX_COPIES, 26);
    }
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p diskrement-core model::tests`
Expected: compile error, because `DiscKey`, `DiscArg`, `set_letter` and `MAX_COPIES` are not defined.

- [ ] **Step 3: Implement**

In `crates/diskrement-core/src/model.rs`, insert after the `impl std::fmt::Display for ContentHash` block:

```rust
/// The most copy sets a project may have: one per letter, `A` to `Z`.
pub const MAX_COPIES: u32 = 26;

/// The letter a copy set is shown as: 0 is `A`, 1 is `B`, and so on.
///
/// Panics past `Z`; `init` refuses `--copies` above [`MAX_COPIES`], so a
/// stored project can never reach it.
pub fn set_letter(copy_set: u32) -> char {
    assert!(copy_set < MAX_COPIES, "copy set {copy_set} has no letter");
    char::from(b'A' + copy_set as u8)
}

/// One physical disc: its ordinal within its copy set, and the set.
///
/// Field order is the sort order — ordinal first — so a sorted list reads
/// 1A, 1B, 2A, 2B: twins side by side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DiscKey {
    pub ordinal: u32,
    pub copy_set: u32,
}

impl DiscKey {
    pub fn new(ordinal: u32, copy_set: u32) -> Self {
        DiscKey { ordinal, copy_set }
    }

    /// How the operator sees this disc: `3` in a single-copy project, `3B`
    /// otherwise. Labels, staging directories, prompts and reports all use
    /// this, so a project's discs are named one way everywhere.
    pub fn name(&self, copies: u32) -> String {
        if copies <= 1 {
            self.ordinal.to_string()
        } else {
            format!("{}{}", self.ordinal, set_letter(self.copy_set))
        }
    }
}

/// A disc as the operator typed it: `3`, `3B` or `3b`.
///
/// The ordinal stays signed so `verify --disc -1` reaches the domain and is
/// answered with exit 2 (CLAUDE.md, exit-code contract) rather than being
/// refused as an argument error. A set letter is only accepted after a
/// positive ordinal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscArg {
    pub ordinal: i64,
    pub copy_set: Option<u32>,
}

impl DiscArg {
    /// The disc this names in a project with `copies` sets, or `None` when the
    /// ordinal is not positive or the letter is past the project's last set.
    /// A bare ordinal means set `A`.
    pub fn resolve(&self, copies: u32) -> Option<DiscKey> {
        let copy_set = self.copy_set.unwrap_or(0);
        let ordinal = u32::try_from(self.ordinal).ok().filter(|o| *o >= 1)?;
        (copy_set < copies).then_some(DiscKey::new(ordinal, copy_set))
    }
}

impl std::str::FromStr for DiscArg {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bad = || format!("not a disc: {s:?} (expected e.g. 3 or 3B)");
        let (digits, letter) = match s.chars().last() {
            Some(c) if c.is_ascii_alphabetic() => (&s[..s.len() - 1], Some(c.to_ascii_uppercase())),
            _ => (s, None),
        };
        let ordinal: i64 = digits.parse().map_err(|_| bad())?;
        match letter {
            Some(_) if ordinal < 1 => Err(bad()),
            Some(c) => Ok(DiscArg {
                ordinal,
                copy_set: Some(u32::from(c as u8 - b'A')),
            }),
            None => Ok(DiscArg {
                ordinal,
                copy_set: None,
            }),
        }
    }
}

impl std::fmt::Display for DiscArg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.copy_set {
            Some(set) => write!(f, "{}{}", self.ordinal, set_letter(set)),
            None => write!(f, "{}", self.ordinal),
        }
    }
}
```

`"3 B"` fails because `"3 "` is not an integer. `"3é"` fails because `é` is not ASCII, so the whole string goes to `parse`. Slicing `s.len() - 1` is safe because the removed character is ASCII.

In `crates/diskrement-core/src/lib.rs`, replace the `pub use model::{…}` line with:

```rust
pub use model::{
    set_letter, Capacity, CatalogEntry, CatalogView, ContentHash, DiscArg, DiscKey, DiscType,
    RelPath, ScannedFile, MAX_COPIES,
};
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `cargo test -p diskrement-core model::tests`
Expected: PASS.

- [ ] **Step 5: Verify and commit**

Run the three Global Constraints commands, then:
```bash
git add crates/diskrement-core/src/model.rs crates/diskrement-core/src/lib.rs
git commit -m "feat(core): name discs by ordinal and copy set

tests: <n> passed

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: `copy_set` on `DiscState`, `live_sets` on `CatalogEntry`

These are type-only changes, so they compile through the whole workspace. Real values arrive in Task 8, and until then every catalog read fills in `copy_set: 0` and an empty `live_sets`.

**Files:**
- Modify: `crates/diskrement-core/src/planner.rs` (`DiscState`)
- Modify: `crates/diskrement-core/src/model.rs` (`CatalogEntry`, `from_scanned`)
- Modify: `crates/diskrement-core/src/testutil.rs` (`entry`, `disc_states_strategy`)
- Modify: every `DiscState {` and `CatalogEntry {` literal the compiler names

- [ ] **Step 1: Change the types**

In `crates/diskrement-core/src/planner.rs`, replace the `DiscState` struct with:

```rust
/// An existing disc as known to the catalog when planning begins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscState {
    pub ordinal: u32,
    /// Which copy set the disc belongs to (0 = `A`).
    pub copy_set: u32,
    pub status: DiscStatus,
    pub usable_bytes: u64,
    pub used_bytes: u64,
}

impl DiscState {
    pub fn key(&self) -> crate::model::DiscKey {
        crate::model::DiscKey::new(self.ordinal, self.copy_set)
    }
```

Keep the existing `free_bytes` method inside that `impl` block, then close it.

In `crates/diskrement-core/src/model.rs`, change the imports to `use std::collections::{BTreeMap, BTreeSet};` and replace `CatalogEntry` with:

```rust
/// The current (live) version of one file as recorded in the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub rel_path: RelPath,
    pub content_hash: ContentHash,
    pub size_bytes: u64,
    /// The copy sets holding at least one copy of this version on a disc not
    /// marked dead. Never empty in a view the catalog produced: a version with
    /// no live copy is left out of the view entirely.
    pub live_sets: BTreeSet<u32>,
}
```

In `CatalogView::from_scanned`, add `live_sets: BTreeSet::new(),` to the `CatalogEntry` literal, and append this sentence to its doc comment: "No version is on a disc yet, so every `live_sets` is empty."

- [ ] **Step 2: Fix every literal**

Replacement rule: add `copy_set: 0,` after `ordinal: …,` in every `DiscState { … }` literal. Add `live_sets: Default::default(),` to every `CatalogEntry { … }` literal. Known sites:
- `crates/diskrement-core/src/testutil.rs`: `entry()`, `disc_states_strategy()`
- `crates/diskrement-core/src/planner.rs`: the test module, about 10 `DiscState` literals
- `crates/diskrement-shell/src/catalog/read.rs`: `read_disc_states` and `read_view`. Mark each with `// copy_set / live_sets read for real in the V3 read model.` so Task 8 can find them.
- `crates/diskrement-shell/src/pipeline/planning.rs` tests, `crates/diskrement-shell/src/pipeline/mod.rs` tests, `crates/diskrement-cli/src/render.rs` tests, `crates/diskrement-shell/tests/fixture_catalog.rs`

Run `cargo build --workspace --all-targets` until it is clean.

- [ ] **Step 3: Verify and commit**

Run the three Global Constraints commands. The count is unchanged.
```bash
git add -A
git commit -m "feat(core): carry copy sets on disc states and catalog entries

tests: <n> passed

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: The `replication` module

**Files:**
- Create: `crates/diskrement-core/src/replication.rs`
- Modify: `crates/diskrement-core/src/lib.rs`
- Modify: `crates/diskrement-core/src/testutil.rs` (`entry_in`)

- [ ] **Step 1: Add a test helper**

Append to `crates/diskrement-core/src/testutil.rs`:

```rust
/// A catalog entry whose version is live in exactly `sets`.
pub fn entry_in(path: &str, hash: &str, size: u64, sets: &[u32]) -> CatalogEntry {
    CatalogEntry {
        rel_path: RelPath(path.to_string()),
        content_hash: ContentHash(hash.to_string()),
        size_bytes: size,
        live_sets: sets.iter().copied().collect(),
    }
}
```

- [ ] **Step 2: Write the module with its failing tests**

Create `crates/diskrement-core/src/replication.rs`:

```rust
//! Copy sets: what each set is missing, how a disc's files land in the
//! catalog when it commits, and how far each set lags.
//!
//! A project with `copies = N` keeps N independent disc series. Every current
//! version should have a live copy in every series; these functions are the
//! whole definition of "should".

use crate::diff::ChangeSet;
use crate::model::{CatalogView, ScannedFile};

/// For each copy set, the unchanged files whose current version has no live
/// copy in that set — the catch-up copies its next disc must carry. Indexed by
/// copy set; each list is sorted by rel path.
///
/// Only `unchanged` files are considered: an added or modified file is a new
/// version, which every set receives anyway.
pub fn shortfall(view: &CatalogView, changes: &ChangeSet, copies: u32) -> Vec<Vec<ScannedFile>> {
    (0..copies)
        .map(|set| {
            let mut missing: Vec<ScannedFile> = changes
                .unchanged
                .iter()
                .filter(|f| {
                    view.current
                        .get(&f.rel_path)
                        .is_some_and(|e| !e.live_sets.contains(&set))
                })
                .cloned()
                .collect();
            missing.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
            missing
        })
        .collect()
}

/// How one disc's files land in the catalog at the moment the disc commits.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DiscContents {
    /// No current version: a new `file_version`, change kind `ADDED`.
    pub added: Vec<ScannedFile>,
    /// A current version with another hash: a new `file_version`, `MODIFIED`.
    pub modified: Vec<ScannedFile>,
    /// The current version itself: one more `placement` of it, nothing else.
    pub copied: Vec<ScannedFile>,
}

/// Classify `files` against `view` as it stands when their disc commits.
///
/// Called per disc, against the live catalog, so the outcome does not depend
/// on which set commits first: in a normal cycle disc 1A creates the versions
/// and 1B's files classify as copies of them. Input order is preserved within
/// each list.
pub fn classify(view: &CatalogView, files: &[ScannedFile]) -> DiscContents {
    let mut out = DiscContents::default();
    for f in files {
        match view.current.get(&f.rel_path) {
            None => out.added.push(f.clone()),
            Some(e) if e.content_hash == f.content_hash => out.copied.push(f.clone()),
            Some(_) => out.modified.push(f.clone()),
        }
    }
    out
}

/// How far one copy set lags: current versions with no live copy in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SetCoverage {
    pub missing_files: usize,
    pub missing_bytes: u64,
}

/// Coverage of every copy set, indexed by set.
pub fn coverage(view: &CatalogView, copies: u32) -> Vec<SetCoverage> {
    (0..copies)
        .map(|set| {
            let mut c = SetCoverage::default();
            for e in view.current.values() {
                if !e.live_sets.contains(&set) {
                    c.missing_files += 1;
                    c.missing_bytes += e.size_bytes;
                }
            }
            c
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::diff;
    use crate::testutil::{entry_in, sf, sf_hash, view_of};

    fn paths(files: &[ScannedFile]) -> Vec<&str> {
        files.iter().map(|f| f.rel_path.as_str()).collect()
    }

    #[test]
    fn an_unchanged_file_missing_from_a_set_is_that_sets_shortfall() {
        let view = view_of(&[entry_in("a", "h-a", 1, &[0]), entry_in("b", "h-b", 1, &[0, 1])]);
        let changes = diff(&view, &[sf_hash("a", "h-a"), sf_hash("b", "h-b")]);
        let s = shortfall(&view, &changes, 2);
        assert!(s[0].is_empty());
        assert_eq!(paths(&s[1]), vec!["a"]);
    }

    #[test]
    fn new_and_modified_files_are_never_shortfall() {
        let view = view_of(&[entry_in("m", "old", 1, &[0])]);
        let changes = diff(&view, &[sf_hash("m", "new"), sf_hash("n", "h")]);
        assert_eq!(shortfall(&view, &changes, 2), vec![vec![], vec![]]);
    }

    #[test]
    fn a_single_copy_project_has_one_set_and_no_shortfall_after_a_burn() {
        let view = view_of(&[entry_in("a", "h-a", 1, &[0])]);
        let changes = diff(&view, &[sf_hash("a", "h-a")]);
        assert_eq!(shortfall(&view, &changes, 1), vec![vec![]]);
    }

    #[test]
    fn classify_splits_added_modified_and_copied() {
        let view = view_of(&[entry_in("same", "h-same", 1, &[0]), entry_in("edit", "old", 1, &[0])]);
        let files = vec![sf("edit", 1), sf("new", 1), sf("same", 1)];
        let c = classify(&view, &files);
        assert_eq!(paths(&c.added), vec!["new"]);
        assert_eq!(paths(&c.modified), vec!["edit"]);
        assert_eq!(paths(&c.copied), vec!["same"]);
    }

    #[test]
    fn coverage_counts_files_and_bytes_missing_per_set() {
        let view = view_of(&[entry_in("a", "h", 10, &[0]), entry_in("b", "h", 5, &[1])]);
        assert_eq!(
            coverage(&view, 2),
            vec![
                SetCoverage { missing_files: 1, missing_bytes: 5 },
                SetCoverage { missing_files: 1, missing_bytes: 10 },
            ]
        );
    }

    use crate::testutil::scanned_files_strategy;
    use proptest::prelude::*;

    proptest! {
        /// For every set, the files it receives this cycle (new ∪ shortfall)
        /// and the files it already holds partition the scan exactly: nothing
        /// is skipped and nothing lands twice in one set.
        #[test]
        fn each_set_receives_exactly_what_it_does_not_already_hold(
            files in scanned_files_strategy(),
            held in proptest::collection::vec(0u8..4, 0..12),
            copies in 1u32..=3,
        ) {
            // Burned history: file i is live in the sets whose bit is set in
            // held[i]; files beyond `held`, or with no bit set, are new.
            let mut entries = Vec::new();
            for (f, bits) in files.iter().zip(held.iter()) {
                let sets: Vec<u32> = (0..copies).filter(|s| bits & (1 << s) != 0).collect();
                if !sets.is_empty() {
                    entries.push(entry_in(f.rel_path.as_str(), f.content_hash.as_str(), f.size_bytes, &sets));
                }
            }
            let view = view_of(&entries);
            let changes = diff(&view, &files);
            let short = shortfall(&view, &changes, copies);
            for set in 0..copies {
                let mut received: Vec<&str> = changes.added.iter()
                    .chain(changes.modified.iter())
                    .chain(short[set as usize].iter())
                    .map(|f| f.rel_path.as_str())
                    .collect();
                let mut held_here: Vec<&str> = files.iter()
                    .filter(|f| view.current.get(&f.rel_path)
                        .is_some_and(|e| e.live_sets.contains(&set)))
                    .map(|f| f.rel_path.as_str())
                    .collect();
                let mut all: Vec<&str> = received.clone();
                all.append(&mut held_here.clone());
                all.sort_unstable();
                let mut scan: Vec<&str> = files.iter().map(|f| f.rel_path.as_str()).collect();
                scan.sort_unstable();
                prop_assert_eq!(&all, &scan);
                received.sort_unstable();
                held_here.sort_unstable();
                prop_assert!(received.iter().all(|p| held_here.binary_search(p).is_err()));
            }
        }
    }
}
```

In `crates/diskrement-core/src/lib.rs`, add `pub mod replication;` after `pub mod planner;`, and add this line after the other `pub use` lines:

```rust
pub use replication::{DiscContents, SetCoverage};
```

Update the crate doc comment's module list: append `` `replication::shortfall` `` to the "Functions are reached through their modules" sentence.

- [ ] **Step 3: Run the tests**

Run: `cargo test -p diskrement-core replication`
Expected: PASS (six tests).

- [ ] **Step 4: Verify and commit**

```bash
git add crates/diskrement-core
git commit -m "feat(core): compute per-set shortfall, commit-time classification and coverage

tests: <n> passed

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: `plan_set`: plan one copy set

`planner::plan` stays as the single-set entry point, so its fifteen ported tests and eight properties keep their meaning. It becomes a thin wrapper over the new `plan_set`.

**Files:**
- Modify: `crates/diskrement-core/src/planner.rs`
- Modify: every `PlannedDisc {` literal the compiler names

- [ ] **Step 1: Write the failing tests**

Append to the first `mod tests` in `crates/diskrement-core/src/planner.rs`, after `burn_plan_groups_discs_tombstones_and_oversized`:

```rust
    // --- per copy set ---

    fn state(ordinal: u32, copy_set: u32, status: DiscStatus, used: u64) -> DiscState {
        DiscState { ordinal, copy_set, status, usable_bytes: 100, used_bytes: used }
    }

    #[test]
    fn plan_set_tops_off_only_its_own_sets_open_disc() {
        let discs = [
            state(1, 0, DiscStatus::Open, 10),
            state(1, 1, DiscStatus::Open, 50),
        ];
        let p = plan_set(1, &[sf("a", 10)], &[], &discs, CAP100);
        assert_eq!(p.discs.len(), 1);
        assert_eq!((p.discs[0].ordinal, p.discs[0].copy_set), (1, 1));
        assert!(!p.discs[0].is_new);
        assert_eq!(p.discs[0].prior_used_bytes, 50);
    }

    #[test]
    fn plan_set_numbers_fresh_discs_within_its_own_set() {
        // Set A already reached disc 3; set B has only disc 1, which is sealed.
        let discs = [
            state(3, 0, DiscStatus::Open, 0),
            state(1, 1, DiscStatus::Finalized, 100),
        ];
        let p = plan_set(1, &[sf("a", 10)], &[], &discs, CAP100);
        assert_eq!((p.discs[0].ordinal, p.discs[0].copy_set), (2, 1));
        assert!(p.discs[0].is_new);
    }

    #[test]
    fn plan_set_stamps_every_planned_disc_with_its_set() {
        let p = plan_set(2, &[sf("a", 60), sf("b", 60)], &[], &[], CAP100);
        assert_eq!(p.discs.len(), 2);
        assert!(p.discs.iter().all(|d| d.copy_set == 2));
        assert_eq!(p.discs[0].ordinal, 1);
        assert_eq!(p.discs[1].ordinal, 2);
    }

    #[test]
    fn plan_is_plan_set_for_set_a() {
        let cs = ChangeSet {
            added: vec![sf("a", 10)],
            modified: vec![sf("b", 10)],
            deleted: vec![RelPath("gone".to_string())],
            ..ChangeSet::empty()
        };
        assert_eq!(
            plan(&cs, &[], CAP100),
            plan_set(0, &[sf("a", 10), sf("b", 10)], &cs.deleted, &[], CAP100)
        );
    }
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p diskrement-core planner`
Expected: compile error, because `plan_set` and `PlannedDisc::copy_set` don't exist yet.

- [ ] **Step 3: Implement**

Add `pub copy_set: u32,` to `PlannedDisc`, after `ordinal`, with this doc comment: `/// The copy set this disc belongs to (0 = \`A\`).`. Add this method to `impl PlannedDisc`:

```rust
    pub fn key(&self) -> crate::model::DiscKey {
        crate::model::DiscKey::new(self.ordinal, self.copy_set)
    }
```

Replace the whole `pub fn plan(…)` function with:

```rust
/// Plan the next burn of a single-copy project: top off the open disc, then
/// spill onto fresh discs. Exactly `plan_set` for set `A`, with the files to
/// place taken from `added ∪ modified` and the tombstones from `deleted`.
pub fn plan(changes: &ChangeSet, discs: &[DiscState], new_disc_cap: Capacity) -> BurnPlan {
    let to_place: Vec<ScannedFile> = changes
        .added
        .iter()
        .chain(changes.modified.iter())
        .cloned()
        .collect();
    plan_set(0, &to_place, &changes.deleted, discs, new_disc_cap)
}

/// Plan one copy set: top off that set's newest open disc, then spill onto
/// fresh discs numbered within the set.
///
/// `discs` may hold every set's discs; only those in `copy_set` are
/// considered. Deterministic — the result depends only on the set of files to
/// place, not on their order.
pub fn plan_set(
    copy_set: u32,
    to_place: &[ScannedFile],
    tombstones: &[RelPath],
    discs: &[DiscState],
    new_disc_cap: Capacity,
) -> BurnPlan {
    let mut to_place: Vec<ScannedFile> = to_place.to_vec();
    to_place.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    let discs: Vec<&DiscState> = discs.iter().filter(|d| d.copy_set == copy_set).collect();

    // The newest open disc is the one to top off. A dead or finalized disc is
    // never a candidate, however much room it appears to have.
    let existing_open = discs
        .iter()
        .copied()
        .filter(|d| d.status == DiscStatus::Open)
        .max_by_key(|d| d.ordinal);
    let max_ordinal = discs.iter().map(|d| d.ordinal).max().unwrap_or(0);
```

The rest of the old body, from `// The largest disc a file could ever land on this run` down to the end, stays as it is, with three edits:
1. In the `PlannedDisc { … }` literal, add `copy_set,` after `ordinal,`.
2. The final `BurnPlan { … }` uses `tombstones: tombstones.to_vec(),` instead of `changes.deleted.clone()`.
3. The `use crate::diff::ChangeSet;` import stays, because `plan` still takes a `ChangeSet`.

- [ ] **Step 4: Fix every `PlannedDisc` literal**

Replacement rule: add `copy_set: 0,` after `ordinal: …,`. Known sites:
- `crates/diskrement-core/src/planner.rs` and `manifest.rs` tests
- `crates/diskrement-shell/src/pipeline/staged.rs` `read_plan`. Mark it `// copy_set read from plan.json v2 in the per-set plan.` so Task 12 can find it.
- `crates/diskrement-cli/src/render.rs` tests

- [ ] **Step 5: Add a property**

Append inside the planner's `proptest! { … }` block:

```rust
        /// Discs of other sets are invisible to `plan_set`: planning set 1
        /// against a mixed list equals planning it against its own discs
        /// renumbered as set 0, apart from the stamped set.
        #[test]
        fn plan_set_ignores_other_sets(
            (cs, discs, cap) in planner_input_strategy()
        ) {
            let to_place: Vec<ScannedFile> = placeable(&cs).into_iter().cloned().collect();
            let mut mixed: Vec<DiscState> = discs.iter().cloned()
                .map(|mut d| { d.copy_set = 1; d }).collect();
            mixed.extend(discs.iter().cloned().map(|mut d| {
                d.copy_set = 0;
                d.status = DiscStatus::Open;
                d
            }));
            let own = plan_set(0, &to_place, &[], &discs, cap);
            let other = plan_set(1, &to_place, &[], &mixed, cap);
            prop_assert_eq!(own.discs.len(), other.discs.len());
            for (a, b) in own.discs.iter().zip(other.discs.iter()) {
                prop_assert_eq!(b.copy_set, 1);
                prop_assert_eq!(
                    (a.ordinal, a.is_new, &a.files, a.prior_used_bytes),
                    (b.ordinal, b.is_new, &b.files, b.prior_used_bytes)
                );
            }
        }
```

- [ ] **Step 6: Run the tests**

Run: `cargo test -p diskrement-core planner`
Expected: PASS, including every pre-existing planner test unchanged.

- [ ] **Step 7: Verify and commit**

```bash
git add -A
git commit -m "feat(core): plan one copy set at a time

tests: <n> passed

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: The manifest records its copy set and its copies

**Files:**
- Modify: `crates/diskrement-core/src/manifest.rs`
- Modify: `crates/diskrement-shell/src/burn/json.rs` (`ManifestJson`)
- Modify: `crates/diskrement-shell/src/burn/testkit.rs`, `crates/diskrement-shell/src/pipeline/mod.rs` (`burn`'s `manifest::build` call)

- [ ] **Step 1: Write the failing tests**

In `crates/diskrement-core/src/manifest.rs` tests, replace `a_manifest_splits_a_discs_files_into_added_vs_modified_using_the_change_set` with:

```rust
    #[test]
    fn a_manifest_records_added_modified_and_copied_files_and_its_set() {
        let contents = DiscContents {
            added: vec![sf("new1", 10)],
            modified: vec![sf("edit1", 10)],
            copied: vec![sf("copy1", 10)],
        };
        let disc = PlannedDisc {
            ordinal: 1,
            copy_set: 1,
            is_new: true,
            will_finalize: false,
            files: vec![sf("copy1", 10), sf("edit1", 10), sf("new1", 10)],
            prior_used_bytes: 0,
            usable_bytes: 100,
        };
        let m = build("photos", 1, &disc, &contents, vec![RelPath("gone".to_string())]);
        assert_eq!(m.project_name, "photos");
        assert_eq!((m.disc_ordinal, m.copy_set, m.session_ordinal), (1, 1, 1));
        let names = |v: &[ManifestEntry]| v.iter().map(|e| e.rel_path.0.clone()).collect::<Vec<_>>();
        assert_eq!(names(&m.added), vec!["new1"]);
        assert_eq!(names(&m.modified), vec!["edit1"]);
        assert_eq!(names(&m.copied), vec!["copy1"]);
        assert_eq!(m.deleted, vec![RelPath("gone".to_string())]);
    }
```

In `a_manifest_entry_carries_hash_and_size`, replace the `cs` construction with `let contents = DiscContents { added: vec![f.clone()], ..DiscContents::default() };`, add `copy_set: 0,` to its `PlannedDisc`, and call `build("p", 1, &disc, &contents, vec![])`. Update the test-module imports: remove `use crate::diff::ChangeSet;` and add `use crate::replication::DiscContents;`.

- [ ] **Step 2: Implement**

Replace `SessionManifest` and `build` in `crates/diskrement-core/src/manifest.rs`:

```rust
/// The pure manifest data for one session burned onto one disc.
///
/// `copied` holds files this session placed as another copy of a version an
/// earlier session — on any disc — already recorded. Serialization to JSON is
/// a shell concern and is not part of this crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionManifest {
    pub project_name: String,
    pub disc_ordinal: u32,
    pub copy_set: u32,
    pub session_ordinal: u32,
    pub added: Vec<ManifestEntry>,
    pub modified: Vec<ManifestEntry>,
    pub copied: Vec<ManifestEntry>,
    pub deleted: Vec<RelPath>,
}

/// Build the manifest for `disc` from its commit-time classification.
///
/// `tombstones` are the cycle's deletions. Every disc of the cycle lists them,
/// so each disc describes the cycle on its own, even though the catalog
/// records each deletion once.
pub fn build(
    project_name: &str,
    session_ordinal: u32,
    disc: &PlannedDisc,
    contents: &DiscContents,
    tombstones: Vec<RelPath>,
) -> SessionManifest {
    SessionManifest {
        project_name: project_name.to_string(),
        disc_ordinal: disc.ordinal,
        copy_set: disc.copy_set,
        session_ordinal,
        added: contents.added.iter().map(to_entry).collect(),
        modified: contents.modified.iter().map(to_entry).collect(),
        copied: contents.copied.iter().map(to_entry).collect(),
        deleted: tombstones,
    }
}
```

Change the module imports to:

```rust
use crate::model::{ContentHash, RelPath, ScannedFile};
use crate::planner::PlannedDisc;
use crate::replication::DiscContents;
```

(`HashSet` and `ChangeSet` are no longer used.)

- [ ] **Step 3: Write the JSON test, failing**

In `crates/diskrement-shell/src/burn/json.rs` tests, change `manifest()` to set `copy_set: 1` and `copied: Vec::new()`. Replace `pretty_output_matches_zio_json` and `compact_output_has_scala_field_order_and_no_whitespace` with:

```rust
    /// zio-json `toJsonPretty` layout: two-space indent, `" : "`, `[]` for an
    /// empty array, no trailing newline. `copySet` follows `discOrdinal` and
    /// `copied` follows `modified`.
    #[test]
    fn pretty_output_has_the_disc_layout() {
        let expected = "{\n  \"projectName\" : \"p\",\n  \"discOrdinal\" : 3,\n  \
             \"copySet\" : 1,\n  \"sessionOrdinal\" : 1,\n  \"added\" : [],\n  \
             \"modified\" : [\n    {\n      \
             \"relPath\" : \"a.bin\",\n      \"contentHash\" : \
             \"e148cf0e8c66a7c18ea6be52a5215dd2a770d74b6c014d51d1e1e0463e882e77\",\n      \
             \"sizeBytes\" : 800\n    }\n  ],\n  \"copied\" : [],\n  \"deleted\" : [\n    \
             \"b.bin\"\n  ]\n}";
        assert_eq!(to_pretty(&ManifestJson::from(&manifest())), expected);
    }

    /// The compact encoding is what the manifest hash is taken over, so its
    /// field order and its lack of whitespace are both catalog content.
    #[test]
    fn compact_output_has_a_fixed_field_order_and_no_whitespace() {
        let compact = serde_json::to_string(&ManifestJson::from(&manifest())).unwrap();
        assert_eq!(
            compact,
            "{\"projectName\":\"p\",\"discOrdinal\":3,\"copySet\":1,\"sessionOrdinal\":1,\
             \"added\":[],\"modified\":[{\"relPath\":\"a.bin\",\"contentHash\":\
             \"e148cf0e8c66a7c18ea6be52a5215dd2a770d74b6c014d51d1e1e0463e882e77\",\
             \"sizeBytes\":800}],\"copied\":[],\"deleted\":[\"b.bin\"]}"
        );
    }

    /// A manifest written before copy sets existed has neither field; it reads
    /// as set A with no copies.
    #[test]
    fn a_manifest_without_copy_fields_reads_as_set_a() {
        let old = "{\"projectName\":\"p\",\"discOrdinal\":3,\"sessionOrdinal\":1,\
                   \"added\":[],\"modified\":[],\"deleted\":[]}";
        let m = serde_json::from_str::<ManifestJson>(old).unwrap().to_manifest().unwrap();
        assert_eq!(m.copy_set, 0);
        assert!(m.copied.is_empty());
    }
```

- [ ] **Step 4: Implement `ManifestJson`**

Replace the struct and its `impl` in `crates/diskrement-shell/src/burn/json.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestJson {
    pub project_name: String,
    pub disc_ordinal: u32,
    /// Absent on discs written before copy sets existed: those are set A.
    #[serde(default)]
    pub copy_set: u32,
    pub session_ordinal: u32,
    pub added: Vec<ManifestEntryJson>,
    pub modified: Vec<ManifestEntryJson>,
    #[serde(default)]
    pub copied: Vec<ManifestEntryJson>,
    pub deleted: Vec<String>,
}

impl ManifestJson {
    pub fn from(m: &SessionManifest) -> Self {
        ManifestJson {
            project_name: m.project_name.clone(),
            disc_ordinal: m.disc_ordinal,
            copy_set: m.copy_set,
            session_ordinal: m.session_ordinal,
            added: m.added.iter().map(ManifestEntryJson::from).collect(),
            modified: m.modified.iter().map(ManifestEntryJson::from).collect(),
            copied: m.copied.iter().map(ManifestEntryJson::from).collect(),
            deleted: m.deleted.iter().map(|p| p.0.clone()).collect(),
        }
    }

    /// The inverse, for `DiscReader` and the scenario oracle.
    pub fn to_manifest(&self) -> Result<SessionManifest, String> {
        let entries = |v: &[ManifestEntryJson]| v.iter().map(ManifestEntryJson::to_entry).collect();
        Ok(SessionManifest {
            project_name: self.project_name.clone(),
            disc_ordinal: self.disc_ordinal,
            copy_set: self.copy_set,
            session_ordinal: self.session_ordinal,
            added: entries(&self.added),
            modified: entries(&self.modified),
            copied: entries(&self.copied),
            deleted: self.deleted.iter().map(|p| RelPath(p.clone())).collect(),
        })
    }
}
```

Rewrite the module doc comment's second paragraph so it no longer claims the corpus is a byte oracle:
> Two encodings live here and they are not interchangeable. The **compact** encoding is hashed into `session.manifest_hash`, so its field order and its whitespace are catalog content; the **pretty** encoding is what lands on the disc, in zio-json's `toJsonPretty` layout, kept so discs burned before and after the Rust rewrite read the same.

- [ ] **Step 5: Fix the remaining construction sites**

- `crates/diskrement-shell/src/burn/testkit.rs` `image_of`: add `copy_set: 0,` and `copied: Vec::new(),` to the `SessionManifest`.
- `crates/diskrement-shell/src/pipeline/mod.rs` `burn`: replace the `manifest::build(…)` call with

```rust
        let contents = diskrement_core::replication::classify(&view, &report.disc.files);
        let manifest = manifest::build(
            name,
            sess_ord,
            &report.disc,
            &contents,
            report.tombstones.clone(),
        );
```

At `copies = 1` this is the same split as before: an unchanged file is never planned, so `copied` is empty, and `added`/`modified` match the change set. Leave `disc_write`'s own `cs` partition alone; Task 9 replaces it.
- Any other `SessionManifest {` literal the compiler names: add `copy_set: 0, copied: Vec::new(),`.

- [ ] **Step 6: Run the tests**

Run: `cargo test -p diskrement-core manifest && cargo test -p diskrement-shell burn::json`
Expected: PASS.

- [ ] **Step 7: Verify and commit**

```bash
git add -A
git commit -m "feat(core): record the copy set and copied files in the session manifest

tests: <n> passed

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 7: Migration V3 rebuilds `disc` with a copy set

**Files:**
- Modify: `crates/diskrement-shell/src/catalog/schema.rs`
- Modify: `crates/diskrement-shell/src/catalog/mod.rs` (tests)
- Test: `crates/diskrement-shell/tests/fixture_catalog.rs`

- [ ] **Step 1: Write the failing tests**

In `crates/diskrement-shell/src/catalog/mod.rs` tests:
- rename `a_fresh_database_migrates_to_version_2_with_every_domain_table` to `a_fresh_database_migrates_to_version_3_with_every_domain_table` and change `assert_eq!(version, 2)` to `assert_eq!(version, 3)`;
- in `re_opening_a_catalog_re_runs_migrations_idempotently`, change `vec![2]` to `vec![3]`;
- append:

```rust
    #[test]
    fn disc_ordinals_are_unique_per_copy_set_not_per_project() {
        let (_dir, cat) = temp_catalog();
        cat.conn
            .execute(
                "insert into project (name, source_root, work_dir, default_disc_type,
                                      margin_percent, copies, created_at)
                 values ('p', '/src', '/work', 'BD_R', 2.0, 2, '2026-06-01T00:00:00Z')",
                [],
            )
            .expect("project inserts");
        let insert = |set: i64| {
            cat.conn.execute(
                "insert into disc (project_id, ordinal, disc_type, usable_bytes, status, label, copy_set)
                 values (1, 1, 'BD_R', 100, 'OPEN', 'd', ?1)",
                [set],
            )
        };
        insert(0).expect("1A inserts");
        insert(1).expect("1B inserts beside 1A");
        assert!(insert(1).is_err(), "a second 1B must be refused");
        assert!(insert(-1).is_err(), "a negative copy set must be refused");
    }

    #[test]
    fn the_migration_leaves_foreign_keys_on() {
        let (_dir, cat) = temp_catalog();
        let on: i64 = cat
            .conn
            .query_row("pragma foreign_keys", [], |r| r.get(0))
            .expect("pragma readable");
        assert_eq!(on, 1);
    }
```

Append to `crates/diskrement-shell/tests/fixture_catalog.rs`:

```rust
/// A V2 catalog the Scala build wrote migrates to V3 in place: every disc keeps
/// its id, ordinal and status, lands in set A, and nothing dangles.
#[test]
fn a_v2_catalog_migrates_to_v3_keeping_every_disc_id() {
    let before: Vec<(i64, i64, String)> = {
        let conn = support::open_fixture("catalog/11-after-finalize.sqlite");
        let mut stmt = conn
            .prepare("select id, ordinal, status from disc order by id")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert!(!before.is_empty(), "the fixture has discs");

    let (_dir, cat) = open("catalog/11-after-finalize.sqlite");
    let conn = cat.raw_connection_for_tests();
    let after: Vec<(i64, i64, String, i64)> = {
        let mut stmt = conn
            .prepare("select id, ordinal, status, copy_set from disc order by id")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(
        after.iter().map(|(i, o, s, _)| (*i, *o, s.clone())).collect::<Vec<_>>(),
        before
    );
    assert!(after.iter().all(|(_, _, _, set)| *set == 0));

    let version: i64 = conn
        .query_row("select version from schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 3);
    let dangling: i64 = conn
        .query_row("select count(*) from pragma_foreign_key_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(dangling, 0);

    // A disc inserted after the migration takes an id above every old one,
    // so the AUTOINCREMENT sequence survived the rebuild.
    conn.execute(
        "insert into disc (project_id, ordinal, disc_type, usable_bytes, status, label, copy_set)
         values (1, 99, 'BD_R', 1, 'OPEN', 'x', 1)",
        [],
    )
    .unwrap();
    let max_old = before.iter().map(|(i, _, _)| *i).max().unwrap();
    assert!(conn.last_insert_rowid() > max_old);
}
```

`open` is the existing helper at the top of `fixture_catalog.rs`. It copies the fixture into a `TempDir` and calls `Catalog::open`, which runs the migrations.

- [ ] **Step 2: Run the tests to see them fail**

Run: `cargo test -p diskrement-shell catalog::tests && cargo test -p diskrement-shell --test fixture_catalog`
Expected: FAIL. The version is still 2 and there is no `copy_set` column.

- [ ] **Step 3: Implement V3 and the rebuild hook**

In `crates/diskrement-shell/src/catalog/schema.rs`, insert after `const V2`:

```rust
// Copy sets. `disc` is rebuilt rather than altered: SQLite cannot change a
// table constraint in place, and the unique key moves from
// (project_id, ordinal) to (project_id, copy_set, ordinal) so 1A and 1B can
// coexist. Ids are copied verbatim, so `session.disc_id` and
// `disc_event.disc_id` stay valid; `run_inner` switches foreign keys off around
// the transaction, as SQLite's table-rebuild procedure requires, and checks
// them before committing. `copy_set` goes last so every pre-existing column
// keeps its position.
const V3: &[&str] = &[
    "create table disc_v3 (
       id           integer primary key autoincrement,
       project_id   integer not null references project(id),
       ordinal      integer not null,
       disc_type    text    not null,
       usable_bytes integer not null,
       status       text    not null check (status in ('OPEN','FINALIZED')),
       label        text    not null,
       copy_set     integer not null default 0 check (copy_set >= 0),
       unique (project_id, copy_set, ordinal)
     )",
    "insert into disc_v3 (id, project_id, ordinal, disc_type, usable_bytes, status, label, copy_set)
     select id, project_id, ordinal, disc_type, usable_bytes, status, label, 0 from disc",
    "drop table disc",
    "alter table disc_v3 rename to disc",
];

const MIGRATIONS: &[(i64, &[&str])] = &[(1, V1), (2, V2), (3, V3)];
```

Delete the old `const MIGRATIONS` line. Replace `run_inner` with:

```rust
fn run_inner(conn: &mut Connection) -> Result<(), rusqlite::Error> {
    conn.execute(
        "create table if not exists schema_version (version integer not null)",
        [],
    )?;
    let current: i64 = conn.query_row(
        "select coalesce(max(version), 0) from schema_version",
        [],
        |row| row.get(0),
    )?;
    if MIGRATIONS.iter().all(|(v, _)| *v <= current) {
        return Ok(());
    }

    // `PRAGMA foreign_keys` is a no-op inside a transaction, so it brackets
    // the transaction rather than living in it. It is restored whether or not
    // the migration succeeds.
    conn.execute_batch("PRAGMA foreign_keys=OFF;")?;
    let migrated = migrate(conn, current);
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;
    migrated
}

fn migrate(conn: &mut Connection, current: i64) -> Result<(), rusqlite::Error> {
    let tx = conn.transaction()?;
    for (version, statements) in MIGRATIONS.iter().filter(|(v, _)| *v > current) {
        for statement in *statements {
            tx.execute(statement, [])?;
        }
        // One row, always: the Scala replaced rather than appended, and
        // `MigrationsSpec` pinned that the table holds exactly one version.
        tx.execute("delete from schema_version", [])?;
        tx.execute(
            "insert into schema_version (version) values (?1)",
            [version],
        )?;
    }
    let dangling: i64 =
        tx.query_row("select count(*) from pragma_foreign_key_check", [], |r| {
            r.get(0)
        })?;
    if dangling > 0 {
        return Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT),
            Some(format!(
                "{dangling} foreign key violations after migrating"
            )),
        ));
    }
    tx.commit()
}
```

Update the module doc comment: replace "Frozen by the data-compatibility contract (spec §3.1)." with "V1 and V2 are the schema the Scala build wrote, transcribed verbatim. V3 adds copy sets (multiple-copies spec §5.2).", and delete the sentence about `tests/fixture_schema.rs`, which was retired in Task 1.

- [ ] **Step 4: Run the tests to see them pass**

Run: `cargo test -p diskrement-shell catalog && cargo test -p diskrement-shell --test fixture_catalog`
Expected: PASS. The existing `the_disc_status_check_constraint_rejects_dead` test still passes, because its insert leaves `copy_set` to its default.

- [ ] **Step 5: Verify and commit**

```bash
git add -A
git commit -m "feat(catalog): migration V3 rebuilds disc with a per-set unique key

tests: <n> passed

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: The read model knows copy sets

Every read that names a disc takes or returns a `DiscKey`, `read_view` fills `live_sets`, and `read_disc_states` fills `copy_set`. The pipeline API doesn't change shape in this task. Its call sites use `DiscKey::new(ordinal, 0)` shims, which Task 14 removes.

**Files:**
- Modify: `crates/diskrement-shell/src/catalog/read.rs`
- Modify: `crates/diskrement-shell/src/catalog/model.rs` (`Session`, `FileHistoryEntry`, `ArchivePointer`)
- Modify: `crates/diskrement-shell/src/burn/json.rs` (`ArchivePointerJson`)
- Modify: callers in `pipeline/mod.rs`, `cli/src/render.rs`, `tests/fixture_catalog.rs`, `tests/fixture_verify.rs`

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `crates/diskrement-shell/src/catalog/read.rs`:

```rust
    /// One session on a disc in `copy_set`, its files placed as new versions.
    fn update_in(
        project: crate::catalog::model::ProjectId,
        ordinal: u32,
        copy_set: u32,
        session: u32,
        added: Vec<crate::catalog::model::FileWrite>,
    ) -> crate::catalog::model::CatalogUpdate {
        let mut u = update(project, ordinal, session, added, vec![], vec![]);
        u.discs[0].copy_set = copy_set;
        u
    }

    #[test]
    fn read_disc_states_report_each_discs_copy_set_ordered_by_ordinal_then_set() {
        let (_dir, mut cat) = temp_catalog();
        let p = cat.create_project(&project_init("p")).unwrap();
        cat.apply_update(&update_in(p, 1, 1, 1, vec![fw("b", "hb", 1)])).unwrap();
        cat.apply_update(&update_in(p, 1, 0, 1, vec![fw("a", "ha", 1)])).unwrap();
        let keys: Vec<DiscKey> = cat.read_disc_states(p).unwrap().iter().map(DiscState::key).collect();
        assert_eq!(keys, vec![DiscKey::new(1, 0), DiscKey::new(1, 1)]);
    }

    #[test]
    fn read_view_reports_the_sets_holding_a_live_copy() {
        let (_dir, mut cat) = temp_catalog();
        let p = cat.create_project(&project_init("p")).unwrap();
        cat.apply_update(&update_in(p, 1, 0, 1, vec![fw("a", "ha", 1)])).unwrap();
        let view = cat.read_view(p).unwrap();
        assert_eq!(
            view.current[&RelPath("a".into())].live_sets,
            std::collections::BTreeSet::from([0])
        );
    }

    #[test]
    fn placements_for_disc_and_recorded_contents_are_per_copy_set() {
        let (_dir, mut cat) = temp_catalog();
        let p = cat.create_project(&project_init("p")).unwrap();
        cat.apply_update(&update_in(p, 1, 0, 1, vec![fw("a", "ha", 1)])).unwrap();
        cat.apply_update(&update_in(p, 1, 1, 1, vec![fw("b", "hb", 1)])).unwrap();
        let on_1b = cat.placements_for_disc(p, DiscKey::new(1, 1)).unwrap();
        assert_eq!(on_1b.keys().cloned().collect::<Vec<_>>(), vec![RelPath("b".into())]);
        let recorded = cat.disc_recorded_contents(p, DiscKey::new(1, 0)).unwrap();
        assert_eq!(recorded.visible.keys().cloned().collect::<Vec<_>>(), vec![RelPath("a".into())]);
    }
```

Extend the test-module imports with `use diskrement_core::DiscKey;`. `update_in` sets `DiscWrite::copy_set`, which arrives in Task 9. So that Step 2 compiles, add the field to `DiscWrite` now. In `crates/diskrement-shell/src/catalog/model.rs`, add to `DiscWrite`, after `ordinal`:

```rust
    /// The copy set of the disc (0 = `A`). Persisted only when `is_new`.
    pub copy_set: u32,
```

In `write_disc` (`store.rs`), insert it: add `copy_set` to the column list of the `insert into disc` statement and `d.copy_set as i64` as the seventh parameter, so the statement reads `insert into disc (project_id, ordinal, disc_type, usable_bytes, status, label, copy_set) values (?1, ?2, ?3, ?4, ?5, ?6, ?7)`. Add `copy_set: 0,` to every other `DiscWrite {` literal the compiler names (`catalog/testutil.rs` `update`, `pipeline/mod.rs` `disc_write`, which gets `copy_set: report.disc.copy_set,`, and the store tests).

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p diskrement-shell catalog::read`
Expected: compile errors on `placements_for_disc(p, DiscKey…)` and `disc_recorded_contents`; once those are fixed, assertion failures on `copy_set` and `live_sets`.

- [ ] **Step 3: Change the model types**

In `crates/diskrement-shell/src/catalog/model.rs`:
- import `DiscKey` from `diskrement_core`;
- `Session`: replace `pub disc_ordinal: u32,` with `pub disc: DiscKey,`, and change its doc comment to "…joined to its disc's key for convenience.";
- `FileHistoryEntry`: replace `pub disc_ordinal: u32,` with `pub disc: DiscKey,`;
- `ArchivePointer`: add `pub copy_set: u32,` after `disc_ordinal`.

- [ ] **Step 4: Rewrite the queries**

In `crates/diskrement-shell/src/catalog/read.rs`:

`read_view`: replace the SQL with the one below, read a fourth column, and parse it:

```rust
                "select rel_path, content_hash, size_bytes, live_sets from (
                   select fv.rel_path, fv.content_hash, fv.size_bytes,
                          (select group_concat(distinct d.copy_set)
                             from placement p
                             join session s on s.id = p.session_id
                             join disc d    on d.id = s.disc_id
                            where p.file_version_id = fv.id
                              and not exists (select 1 from disc_event e
                                              where e.disc_id = d.id
                                                and e.kind = 'MARKED_DEAD')) as live_sets
                   from (
                     select cr.id as cr_id, cr.kind as kind, cr.file_version_id as fvid,
                            fv.rel_path as rp,
                            row_number() over (partition by fv.rel_path order by cr.id desc) as rn
                     from change_record cr
                     join file_version fv on fv.id = cr.file_version_id
                     where fv.project_id = ?1
                   ) l
                   join file_version fv on fv.id = l.fvid
                   where l.rn = 1 and l.kind <> 'DELETED'
                 ) where live_sets is not null",
```

```rust
            .query_map(params![project.0], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
```

```rust
        for row in rows {
            let (path, hash, size, sets) = row.map_err(CatalogError::from_sqlite)?;
            let live_sets = sets
                .split(',')
                .map(|s| {
                    s.parse::<u32>().map_err(|e| {
                        CatalogError::ConstraintViolation(format!("bad copy set {s:?}: {e}"))
                    })
                })
                .collect::<Result<_, _>>()?;
            let rel_path = RelPath(path);
            current.insert(
                rel_path.clone(),
                CatalogEntry {
                    rel_path,
                    content_hash: ContentHash(hash),
                    size_bytes: size as u64,
                    live_sets,
                },
            );
        }
```

The doc comment gains: "Each entry's `live_sets` names the copy sets holding such a copy; it is never empty, because a version with no live copy is left out."

`read_disc_states`: select `d.ordinal, d.copy_set, <status case>, d.usable_bytes, <used>`, `order by d.ordinal, d.copy_set`, read five columns, and set `copy_set: copy_set as u32`. Remove the Task 3 marker comment.

`sessions_by_state`: select `d.ordinal, d.copy_set, s.ordinal, s.state, s.burned_at, s.manifest_hash`, `order by d.ordinal, d.copy_set, s.ordinal`, and build `disc: DiscKey::new(disc_ordinal as u32, copy_set as u32)`.

`archive_pointers`: select `…, d.ordinal, d.copy_set, s.ordinal, p.disc_path, fvk.kind`, `order by fv.rel_path, d.ordinal, d.copy_set, s.ordinal`, and set `copy_set`.

`file_history`: select `fv.content_hash, fv.size_bytes, d.ordinal, d.copy_set, s.ordinal, …`, `order by fv.id, d.copy_set`, and build `disc: DiscKey::new(…)`. Two copies of one version are two rows of one `fv.id`, so ordering on the set keeps the output deterministic.

`placements_for_disc(&self, project: ProjectId, disc: DiscKey)` and `disc_recorded_contents(&self, project: ProjectId, disc: DiscKey)`: change `where d.project_id = ?1 and d.ordinal = ?2` to `where d.project_id = ?1 and d.ordinal = ?2 and d.copy_set = ?3`, and bind `params![project.0, disc.ordinal as i64, disc.copy_set as i64]`.

- [ ] **Step 5: Add `copySet` to `pointers.json`**

In `crates/diskrement-shell/src/burn/json.rs`, add `#[serde(default)] pub copy_set: u32,` after `disc_ordinal` in `ArchivePointerJson`, set it in `from`, and add `copy_set: 0,` to the `ArchivePointer` literal in `pointer_kinds_render_as_the_frozen_uppercase_names`. Append to that test:

```rust
        assert!(rendered.contains("\"copySet\" : 0"), "{rendered}");
```

- [ ] **Step 6: Fix the callers**

Replacement rules:
- `s.disc_ordinal == X` becomes `s.disc.ordinal == X && s.disc.copy_set == 0` in `pipeline/mod.rs` `verify_disc`. In `next_session_ordinal`, change the parameter to `disc: DiscKey` and the test to `s.disc == disc`. In `burn`, call it as `self.next_session_ordinal(p.id, report.disc.key())`.
- `placements_for_disc(p.id, ordinal)` / `disc_recorded_contents(p.id, ordinal)` become `…(p.id, DiscKey::new(ordinal, 0))` in `verify_disc`.
- `e.disc_ordinal` becomes `e.disc.ordinal` in `crates/diskrement-cli/src/render.rs` `history`. In render tests, `disc_ordinal: N` in `FileHistoryEntry` literals becomes `disc: DiscKey::new(N, 0)`.
- In `tests/fixture_catalog.rs` and `tests/fixture_verify.rs`: `placements_for_disc(id, N)` becomes `placements_for_disc(id, DiscKey::new(N, 0))`, the same for `disc_recorded_contents`; `.disc_ordinal` on a `Session` or `FileHistoryEntry` becomes `.disc.ordinal`; `ArchivePointer` literals gain `copy_set: 0`; `CatalogEntry` comparisons gain `live_sets: BTreeSet::from([0])` when the version is on a live disc.
- Any pipeline test filtering sessions by `s.disc_ordinal == 1` becomes `s.disc.ordinal == 1`.

- [ ] **Step 7: Run the tests**

Run: `cargo test --workspace`
Expected: PASS.

- [ ] **Step 8: Verify and commit**

```bash
git add -A
git commit -m "feat(catalog): read copy sets, live sets and per-set disc contents

tests: <n> passed

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: Copy placements and set-keyed writes

`FileWrite::copy_index` goes away, and a file's `copy_index` is always its disc's `copy_set`. That makes the spec's "the write path enforces equality" hold by construction rather than by a runtime check. `SessionWrite` gains `copied`. `finalize_disc_ordinals` and `mark_verified` take `DiscKey`s.

**Files:**
- Modify: `crates/diskrement-shell/src/catalog/model.rs`, `store.rs`, `testutil.rs`
- Modify: `crates/diskrement-shell/src/pipeline/mod.rs` (`disc_write`, `verify_disc`)

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `crates/diskrement-shell/src/catalog/store.rs`:

```rust
    fn copy_update(
        project: crate::catalog::model::ProjectId,
        ordinal: u32,
        copy_set: u32,
        session: u32,
        copied: Vec<crate::catalog::model::FileWrite>,
    ) -> CatalogUpdate {
        let mut u = crate::catalog::testutil::update(project, ordinal, session, vec![], vec![], vec![]);
        u.discs[0].copy_set = copy_set;
        u.discs[0].session.copied = copied;
        u
    }

    #[test]
    fn a_copy_placement_adds_one_placement_and_no_version_or_change() {
        let (_dir, mut cat) = crate::catalog::testutil::temp_catalog();
        let p = cat.create_project(&crate::catalog::testutil::project_init("p")).unwrap();
        cat.apply_update(&crate::catalog::testutil::update(
            p, 1, 1, vec![crate::catalog::testutil::fw("a", "ha", 4)], vec![], vec![],
        ))
        .unwrap();
        let count = |t| crate::catalog::testutil::count(&cat, t);
        let before = (count("file_version"), count("change_record"), count("placement"));

        cat.apply_update(&copy_update(p, 1, 1, 1, vec![crate::catalog::testutil::fw("a", "ha", 4)]))
            .unwrap();

        let count = |t| crate::catalog::testutil::count(&cat, t);
        assert_eq!(count("file_version"), before.0);
        assert_eq!(count("change_record"), before.1);
        assert_eq!(count("placement"), before.2 + 1);
        let copy_index: i64 = cat
            .raw_connection_for_tests()
            .query_row("select copy_index from placement order by id desc limit 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(copy_index, 1, "copy_index is the disc's copy set");
        let view = cat.read_view(p).unwrap();
        assert_eq!(
            view.current[&RelPath("a".into())].live_sets,
            std::collections::BTreeSet::from([0, 1])
        );
    }

    #[test]
    fn a_copy_of_a_hash_that_is_not_current_is_refused_and_rolls_back() {
        let (_dir, mut cat) = crate::catalog::testutil::temp_catalog();
        let p = cat.create_project(&crate::catalog::testutil::project_init("p")).unwrap();
        cat.apply_update(&crate::catalog::testutil::update(
            p, 1, 1, vec![crate::catalog::testutil::fw("a", "ha", 4)], vec![], vec![],
        ))
        .unwrap();
        let discs_before = crate::catalog::testutil::count(&cat, "disc");
        let err = cat
            .apply_update(&copy_update(p, 1, 1, 1, vec![crate::catalog::testutil::fw("a", "other", 4)]))
            .unwrap_err();
        assert!(matches!(err, CatalogError::ConstraintViolation(_)), "{err}");
        assert_eq!(crate::catalog::testutil::count(&cat, "disc"), discs_before);
    }

    #[test]
    fn a_second_copy_on_the_same_disc_is_refused() {
        let (_dir, mut cat) = crate::catalog::testutil::temp_catalog();
        let p = cat.create_project(&crate::catalog::testutil::project_init("p")).unwrap();
        cat.apply_update(&crate::catalog::testutil::update(
            p, 1, 1, vec![crate::catalog::testutil::fw("a", "ha", 4)], vec![], vec![],
        ))
        .unwrap();
        // Session 2 of the same disc 1A tries to place `a` again.
        let mut again = copy_update(p, 1, 0, 2, vec![crate::catalog::testutil::fw("a", "ha", 4)]);
        again.discs[0].is_new = false;
        let err = cat.apply_update(&again).unwrap_err();
        assert!(matches!(err, CatalogError::ConstraintViolation(_)), "{err}");
    }

    #[test]
    fn finalizing_and_verifying_touch_only_the_named_set() {
        let (_dir, mut cat) = crate::catalog::testutil::temp_catalog();
        let p = cat.create_project(&crate::catalog::testutil::project_init("p")).unwrap();
        cat.apply_update(&crate::catalog::testutil::update(
            p, 1, 1, vec![crate::catalog::testutil::fw("a", "ha", 4)], vec![], vec![],
        ))
        .unwrap();
        cat.apply_update(&copy_update(p, 1, 1, 1, vec![crate::catalog::testutil::fw("a", "ha", 4)]))
            .unwrap();

        cat.mark_verified(p, DiscKey::new(1, 1), 1).unwrap();
        cat.apply_update(&CatalogUpdate {
            project_id: p,
            finalize_disc_ordinals: vec![DiscKey::new(1, 1)],
            discs: vec![],
        })
        .unwrap();

        let states = cat.read_disc_states(p).unwrap();
        assert_eq!(states[0].status, DiscStatus::Open, "1A untouched");
        assert_eq!(states[1].status, DiscStatus::Finalized, "1B sealed");
        let verified = cat.sessions_by_state(p, crate::catalog::model::SessionState::Verified).unwrap();
        assert_eq!(verified.iter().map(|s| s.disc).collect::<Vec<_>>(), vec![DiscKey::new(1, 1)]);
    }
```

Add `use diskrement_core::DiscKey;` and `use crate::error::CatalogError;` to the test-module imports if they are not already there.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p diskrement-shell catalog::store`
Expected: compile errors (`copied` and the `DiscKey` signatures don't exist yet).

- [ ] **Step 3: Change the model**

In `crates/diskrement-shell/src/catalog/model.rs`:
- `FileWrite`: delete `pub copy_index: u32,` and change the doc comment to "One file written this run: its identity and where it landed. Its `copy_index` is its disc's copy set.";
- `SessionWrite`: add, after `modified`:

```rust
    /// Further copies of versions already recorded: one `placement` each, no
    /// new `file_version`, no `change_record`.
    pub copied: Vec<FileWrite>,
```

- `CatalogUpdate::finalize_disc_ordinals`: change the type to `Vec<DiscKey>` and the doc comment to "Pre-existing OPEN discs to flip to FINALIZED."

- [ ] **Step 4: Change the store**

In `crates/diskrement-shell/src/catalog/store.rs`, import `DiscKey`. Replace `disc_id_by_ordinal` and `finalize_disc` with:

```rust
fn disc_id(tx: &Transaction<'_>, project: ProjectId, disc: DiscKey) -> Result<i64, CatalogError> {
    tx.query_row(
        "select id from disc where project_id = ?1 and ordinal = ?2 and copy_set = ?3",
        params![project.0, disc.ordinal as i64, disc.copy_set as i64],
        |row| row.get::<_, i64>(0),
    )
    .optional()
    .map_err(CatalogError::from_sqlite)?
    .ok_or_else(|| {
        CatalogError::ConstraintViolation(format!(
            "no disc with ordinal {} in copy set {}",
            disc.ordinal, disc.copy_set
        ))
    })
}

/// Flip a pre-existing OPEN disc to FINALIZED. Anything else is an illegal
/// transition — including a disc that is already FINALIZED.
fn finalize_disc(tx: &Transaction<'_>, project: ProjectId, disc: DiscKey) -> Result<(), CatalogError> {
    let updated = tx
        .execute(
            "update disc set status = 'FINALIZED'
             where project_id = ?1 and ordinal = ?2 and copy_set = ?3 and status = 'OPEN'",
            params![project.0, disc.ordinal as i64, disc.copy_set as i64],
        )
        .map_err(CatalogError::from_sqlite)?;
    if updated == 0 {
        return Err(CatalogError::IllegalStatusTransition(format!(
            "disc {} (set {}) is not OPEN",
            disc.ordinal, disc.copy_set
        )));
    }
    Ok(())
}
```

`apply_update`'s loop becomes `for disc in &u.finalize_disc_ordinals { finalize_disc(tx, u.project_id, *disc)?; }`.

`mark_verified(&mut self, project: ProjectId, disc: DiscKey, session_ordinal: u32)`: its subquery becomes `(select id from disc where project_id = ?2 and ordinal = ?3 and copy_set = ?4)`, with `params![session_ordinal as i64, project.0, disc.ordinal as i64, disc.copy_set as i64]`. The error message is `"no BURNED session {session_ordinal} on disc {} (set {})"`.

In `write_disc`, use `disc_id(tx, project, DiscKey::new(d.ordinal, d.copy_set))?` for the existing-disc branch. Pass `d.copy_set` as a new final argument to each `write_file` call, and after the `modified` loop add:

```rust
    for file in &d.session.copied {
        write_copy(tx, project, session_id, disc_id, d.copy_set, file)?;
    }
```

`write_file` gains a final parameter `copy_set: u32` and binds it in place of `f.copy_index`. Add after `write_file`:

```rust
/// Place another copy of a path's current version on this disc: one
/// `placement` row and nothing else.
///
/// The version must still be current and carry the same hash — the pipeline
/// classified the file against the live catalog a moment ago, so anything else
/// is a bug — and must not already be on this disc, where a second copy would
/// protect nothing.
fn write_copy(
    tx: &Transaction<'_>,
    project: ProjectId,
    session_id: i64,
    disc_id: i64,
    copy_set: u32,
    f: &FileWrite,
) -> Result<(), CatalogError> {
    let current: Option<(i64, String, String)> = tx
        .query_row(
            "select fv.id, fv.content_hash, l.kind
             from (select cr.file_version_id as fvid, cr.kind as kind
                   from change_record cr
                   join file_version fv on fv.id = cr.file_version_id
                   where fv.project_id = ?1 and fv.rel_path = ?2
                   order by cr.id desc limit 1) l
             join file_version fv on fv.id = l.fvid",
            params![project.0, f.rel_path.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(CatalogError::from_sqlite)?;
    let file_version_id = match current {
        Some((id, hash, kind)) if kind != "DELETED" && hash == f.content_hash.as_str() => id,
        _ => {
            return Err(CatalogError::ConstraintViolation(format!(
                "cannot copy {}: no current version with hash {}",
                f.rel_path, f.content_hash
            )))
        }
    };
    let already_here: bool = tx
        .query_row(
            "select exists (select 1 from placement p
                            join session s on s.id = p.session_id
                            where p.file_version_id = ?1 and s.disc_id = ?2)",
            params![file_version_id, disc_id],
            |row| row.get(0),
        )
        .map_err(CatalogError::from_sqlite)?;
    if already_here {
        return Err(CatalogError::ConstraintViolation(format!(
            "{} already has a copy on this disc",
            f.rel_path
        )));
    }
    tx.execute(
        "insert into placement (file_version_id, session_id, disc_path, copy_index)
         values (?1, ?2, ?3, ?4)",
        params![file_version_id, session_id, f.disc_path, copy_set as i64],
    )
    .map_err(CatalogError::from_sqlite)?;
    Ok(())
}
```

`mark_dead` still calls `disc_id_by_ordinal(tx, project, disc_ordinal)`. Change that call to `disc_id(tx, project, DiscKey::new(disc_ordinal, 0))`; Task 10 rewrites `mark_dead`.

- [ ] **Step 5: Fix the callers**

Replacement rules:
- Delete `copy_index: …,` from every `FileWrite` literal (`catalog/testutil.rs` `fw_at`, `pipeline/mod.rs` `disc_write`, the store tests). Delete the doc phrase ", copy 0" from `fw`.
- Add `copied: Vec::new(),` to every `SessionWrite` literal except the one `disc_write` builds.
- `finalize_disc_ordinals: vec![N]` in tests becomes `vec![DiscKey::new(N, 0)]`.
- `mark_verified(p, N, s)` in tests becomes `mark_verified(p, DiscKey::new(N, 0), s)`. In `pipeline/mod.rs` `verify_disc`, it becomes `mark_verified(p.id, DiscKey::new(ordinal, 0), session.ordinal)`.
- The test that inserts an extra placement through `raw_connection_for_tests` (`also_place_a_on_disc_2`, store.rs around line 736) stays as it is. It still models a second live copy of the version, and Task 10 relies on that.

Replace `disc_write` in `crates/diskrement-shell/src/pipeline/mod.rs` with this commit-time version. It takes the `DiscContents` already computed for the manifest in Task 6:

```rust
/// The catalog write for one disc: its files split by commit-time
/// classification and stamped with their `/data/`-rooted disc path, plus the
/// tombstones still current in the catalog.
fn disc_write(
    label: &str,
    report: &PlanReport,
    sess_ord: u32,
    now: DateTime<Utc>,
    contents: &DiscContents,
    tombstones: Vec<RelPath>,
    manifest: &SessionManifest,
) -> DiscWrite {
    let fw = |f: &ScannedFile| FileWrite {
        rel_path: f.rel_path.clone(),
        content_hash: f.content_hash.clone(),
        size_bytes: f.size_bytes,
        mtime: f.mtime,
        disc_path: format!("/data/{}", f.rel_path.0),
    };
    DiscWrite {
        ordinal: report.disc.ordinal,
        copy_set: report.disc.copy_set,
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
            added: contents.added.iter().map(fw).collect(),
            modified: contents.modified.iter().map(fw).collect(),
            copied: contents.copied.iter().map(fw).collect(),
            deleted: tombstones,
        },
    }
}
```

In `burn`, call it as `disc_write(&label, &report, sess_ord, now, &contents, report.tombstones.clone(), &manifest)`, and drop the now-unused `change_set` binding together with its `diff::diff` call. Import `diskrement_core::DiscContents` and `diskrement_core::DiscKey`.

- [ ] **Step 6: Run the tests**

Run: `cargo test --workspace`
Expected: PASS.

- [ ] **Step 7: Verify and commit**

```bash
git add -A
git commit -m "feat(catalog): record copy placements and key disc writes by copy set

tests: <n> passed

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: Mark-dead reports degraded versions

**Files:**
- Modify: `crates/diskrement-shell/src/catalog/store.rs` (`mark_dead`, `dead_disc_versions`)
- Modify: `crates/diskrement-shell/src/catalog/model.rs` (`LostReport`)
- Modify: `crates/diskrement-shell/src/pipeline/mod.rs` (`mark_dead`), `crates/diskrement-cli/src/render.rs` (`mark_dead`)

- [ ] **Step 1: Write the failing tests**

The store tests have a `seed` helper (store.rs around line 717) and `also_place_a_on_disc_2`, which puts version `a` on disc 2 as well. Append to `mod tests`:

```rust
    #[test]
    fn a_current_version_with_another_live_copy_is_degraded_not_reburned() {
        let (_dir, mut cat) = crate::catalog::testutil::temp_catalog();
        let p = seed(&mut cat);
        also_place_a_on_disc_2(&cat);
        let report = cat.mark_dead(p, DiscKey::new(1, 0), &dead_at()).unwrap();
        assert!(report.reburn.iter().all(|r| r.rel_path.as_str() != "a"));
        assert!(report.lost.iter().all(|l| l.rel_path.as_str() != "a"));
        assert_eq!(
            report.degraded.iter().map(|d| d.rel_path.as_str()).collect::<Vec<_>>(),
            vec!["a"]
        );
        // Degraded is derived: nothing about `a` is written to lost_version.
        assert!(fates(&cat).iter().all(|(path, _)| path != "a"));
    }

    #[test]
    fn a_single_copy_project_never_reports_degraded_versions() {
        let (_dir, mut cat) = crate::catalog::testutil::temp_catalog();
        let p = seed(&mut cat);
        let report = cat.mark_dead(p, DiscKey::new(1, 0), &dead_at()).unwrap();
        assert!(report.degraded.is_empty());
        assert_eq!(report.superseded_survivors, 0);
    }
```

Before writing these tests, read `seed`, `also_place_a_on_disc_2` and `fates` (store.rs lines 707–770). If `a` is not current in `seed`'s final state, use the path `seed` leaves current instead of `"a"`, and add an assertion that the version is current. The test must exercise *current with another live copy*.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p diskrement-shell catalog::store`
Expected: compile error (the `DiscKey` argument, `degraded`, `superseded_survivors`).

- [ ] **Step 3: Change `LostReport`**

In `crates/diskrement-shell/src/catalog/model.rs`, replace `LostReport` with:

```rust
/// What marking a disc dead cost. `lost` and `reburn` are recorded in
/// `lost_version`; `degraded` and `superseded_survivors` are derived and only
/// reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LostReport {
    pub disc: DiscKey,
    pub lost: Vec<LostVersionEntry>,
    pub reburn: Vec<ReburnEntry>,
    /// Current versions that still have a live copy on another disc. The next
    /// cycle restores the missing copy.
    pub degraded: Vec<ReburnEntry>,
    /// Superseded or deleted versions that still have a live copy elsewhere.
    /// They are not lost, and not restored either: the source moved past them.
    pub superseded_survivors: usize,
}
```

Append this sentence to `ReburnEntry`'s doc comment: "Also used for `LostReport::degraded`, where the file still has a live copy."

- [ ] **Step 4: Change the store**

Replace `Casualty`, `dead_disc_versions` and the body of `mark_dead`. `mark_dead`'s signature becomes `pub fn mark_dead(&mut self, project: ProjectId, disc: DiscKey, at: &DateTime<Utc>) -> Result<LostReport, CatalogError>`.

```rust
/// One version placed on the dying disc.
struct DiscVersion {
    file_version_id: i64,
    rel_path: RelPath,
    content_hash: ContentHash,
    size_bytes: u64,
    session_ordinal: u32,
    is_current: bool,
    /// Another disc that is not dead also holds this version.
    has_other_live_copy: bool,
}

/// Every version placed on the dying disc, flagged with whether it is its
/// path's current version and whether a live copy survives elsewhere.
///
/// Grouped by version: a version with several placements on this disc (a
/// later session re-placing it) is one row, because `lost_version` is
/// append-only and one event costs a version once.
fn disc_versions(
    tx: &Transaction<'_>,
    project: ProjectId,
    disc_id: i64,
) -> Result<Vec<DiscVersion>, CatalogError> {
    let mut stmt = tx
        .prepare(
            "select fv.id, fv.rel_path, fv.content_hash, fv.size_bytes,
                    min(s.ordinal) as sess_ord,
                    max(case when cur.fvid is not null then 1 else 0 end) as is_current,
                    exists (
                      select 1 from placement p2
                      join session s2 on s2.id = p2.session_id
                      join disc d2 on d2.id = s2.disc_id
                      where p2.file_version_id = fv.id and d2.id <> ?2
                        and not exists (select 1 from disc_event e
                                        where e.disc_id = d2.id and e.kind = 'MARKED_DEAD')
                    ) as other_live
             from placement p
             join session s on s.id = p.session_id
             join file_version fv on fv.id = p.file_version_id
             left join (
               select fvid from (
                 select cr.file_version_id as fvid, cr.kind as kind,
                        row_number() over (partition by fv2.rel_path
                                           order by cr.id desc) as rn
                 from change_record cr
                 join file_version fv2 on fv2.id = cr.file_version_id
                 where fv2.project_id = ?1
               ) c where c.rn = 1 and c.kind <> 'DELETED'
             ) cur on cur.fvid = fv.id
             where s.disc_id = ?2
             group by fv.id, fv.rel_path, fv.content_hash, fv.size_bytes
             order by fv.rel_path, sess_ord",
        )
        .map_err(CatalogError::from_sqlite)?;

    let rows = stmt
        .query_map(params![project.0, disc_id], |row| {
            Ok(DiscVersion {
                file_version_id: row.get::<_, i64>(0)?,
                rel_path: RelPath(row.get::<_, String>(1)?),
                content_hash: ContentHash(row.get::<_, String>(2)?),
                size_bytes: row.get::<_, i64>(3)? as u64,
                session_ordinal: row.get::<_, i64>(4)? as u32,
                is_current: row.get::<_, i64>(5)? == 1,
                has_other_live_copy: row.get::<_, i64>(6)? == 1,
            })
        })
        .map_err(CatalogError::from_sqlite)?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(CatalogError::from_sqlite)
}
```

`mark_dead`'s body:

```rust
        self.tx(|tx| {
            let disc_id = disc_id(tx, project, disc)?;
            let already: i64 = tx
                .query_row(
                    "select count(*) from disc_event
                     where disc_id = ?1 and kind = 'MARKED_DEAD'",
                    params![disc_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from_sqlite)?;
            if already > 0 {
                return Err(CatalogError::IllegalStatusTransition(format!(
                    "disc {} (set {}) is already dead",
                    disc.ordinal, disc.copy_set
                )));
            }

            let versions = disc_versions(tx, project, disc_id)?;

            tx.execute(
                "insert into disc_event (disc_id, kind, at) values (?1, 'MARKED_DEAD', ?2)",
                params![disc_id, time::format(at)],
            )
            .map_err(CatalogError::from_sqlite)?;
            let event_id = tx.last_insert_rowid();

            for v in versions.iter().filter(|v| !v.has_other_live_copy) {
                let fate = if v.is_current { "REBURN" } else { "LOST" };
                tx.execute(
                    "insert into lost_version (event_id, file_version_id, fate)
                     values (?1, ?2, ?3)",
                    params![event_id, v.file_version_id, fate],
                )
                .map_err(CatalogError::from_sqlite)?;
            }

            let live_entry = |v: &DiscVersion| ReburnEntry {
                rel_path: v.rel_path.clone(),
                content_hash: v.content_hash.clone(),
                size_bytes: v.size_bytes,
            };
            Ok(LostReport {
                disc,
                lost: versions
                    .iter()
                    .filter(|v| !v.has_other_live_copy && !v.is_current)
                    .map(|v| LostVersionEntry {
                        rel_path: v.rel_path.clone(),
                        content_hash: v.content_hash.clone(),
                        size_bytes: v.size_bytes,
                        session_ordinal: v.session_ordinal,
                    })
                    .collect(),
                reburn: versions
                    .iter()
                    .filter(|v| !v.has_other_live_copy && v.is_current)
                    .map(live_entry)
                    .collect(),
                degraded: versions
                    .iter()
                    .filter(|v| v.has_other_live_copy && v.is_current)
                    .map(live_entry)
                    .collect(),
                superseded_survivors: versions
                    .iter()
                    .filter(|v| v.has_other_live_copy && !v.is_current)
                    .count(),
            })
        })
```

Update `mark_dead`'s doc comment, replacing its first paragraph with:
> Append a MARKED_DEAD event for `disc` and report what it cost. A version whose only live copy was on this disc becomes REBURN (current; the next cycle re-writes it from source) or LOST (superseded or deleted; gone). A version with another live copy is reported as degraded (current; the next cycle restores the copy) or counted as a superseded survivor; neither is written to `lost_version`.

- [ ] **Step 5: Fix the callers**

- The existing store tests call `cat.mark_dead(p, N, &…)`. Replace each with `cat.mark_dead(p, DiscKey::new(N, 0), &…)`. Where they compare a whole `LostReport`, add `degraded: vec![], superseded_survivors: 0` and `disc: DiscKey::new(N, 0)`. The existing test `a_version_alive_on_another_disc_is_neither_lost_nor_reburned` keeps its assertions. If it compares the whole report, it gains `degraded` holding that version (when it is current) or `superseded_survivors: 1` (when it is not).
- `pipeline/mod.rs` `mark_dead`: change the catalog call to `self.catalog.mark_dead(p.id, DiscKey::new(disc_ordinal, 0), &at)`. The pipeline API itself changes in Task 14.
- `crates/diskrement-cli/src/render.rs` `mark_dead`: `r.disc_ordinal` becomes `r.disc.ordinal`. Render tests' `LostReport` literals gain `disc: DiscKey::new(N, 0), degraded: vec![], superseded_survivors: 0` in place of `disc_ordinal: N`.
- `tests/fixture_catalog.rs`: the same `mark_dead` and `LostReport` rules, where it uses them.

- [ ] **Step 6: Run the tests**

Run: `cargo test --workspace`
Expected: PASS.

- [ ] **Step 7: Verify and commit**

```bash
git add -A
git commit -m "feat(catalog): report degraded versions when a disc dies

tests: <n> passed

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 11: Disc identity carries the copy set

A disc's identity on media (`project.json`) gains `copySet` and `copies`. Every identity comparison, prompt, volume id and staging directory uses the disc *name*, so 1A and 1B can never be confused in the drive.

**Files:**
- Modify: `crates/diskrement-shell/src/burn/mod.rs` (`DiscIdentity`)
- Modify: `crates/diskrement-shell/src/burn/json.rs` (`ProjectJson`)
- Modify: `crates/diskrement-shell/src/burn/xorriso/burner.rs`, `burn/xorriso/args.rs` (`volid`)
- Modify: `crates/diskrement-shell/src/burn/verifier.rs` (`DiscExpectation`, both verifiers)
- Modify: `crates/diskrement-shell/src/burn/testkit.rs`, `crates/diskrement-shell/src/pipeline/mod.rs`

- [ ] **Step 1: Write the failing tests**

Append to `crates/diskrement-shell/src/burn/json.rs` tests:

```rust
    #[test]
    fn project_json_records_the_copy_set_and_copies() {
        let id = DiscIdentity {
            project_name: "p".to_string(),
            disc_ordinal: 2,
            copy_set: 1,
            copies: 2,
            session_ordinal: 1,
            created_at: Utc.timestamp_opt(1_757_100_066, 0).unwrap(),
        };
        let rendered = to_pretty(&ProjectJson::from(&id));
        assert!(rendered.contains("\"copySet\" : 1"), "{rendered}");
        assert!(rendered.contains("\"copies\" : 2"), "{rendered}");
        let parsed: ProjectJson = serde_json::from_str(&rendered).unwrap();
        assert_eq!(parsed.to_identity().unwrap(), id);
    }

    #[test]
    fn a_project_json_without_copy_fields_is_set_a_of_one() {
        let old = "{\"projectName\":\"p\",\"discOrdinal\":1,\"sessionOrdinal\":1,\
                   \"createdAt\":\"2026-08-30T10:00:00Z\"}";
        let id = serde_json::from_str::<ProjectJson>(old).unwrap().to_identity().unwrap();
        assert_eq!((id.copy_set, id.copies), (0, 1));
    }
```

Append to `crates/diskrement-shell/src/burn/mod.rs` (add a `#[cfg(test)] mod tests` block if there is none):

```rust
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

    #[test]
    fn the_identity_displays_with_the_disc_name() {
        assert_eq!(id(1, 0, 1).to_string(), "DiscIdentity(photos,1,2,2026-08-30T10:00:00Z)");
        assert_eq!(id(1, 1, 2).to_string(), "DiscIdentity(photos,1B,2,2026-08-30T10:00:00Z)");
    }
}
```

Append to the `volid` tests in `crates/diskrement-shell/src/burn/xorriso/args.rs`:

```rust
    #[test]
    fn the_volume_id_carries_the_set_letter() {
        assert_eq!(volid("photos", "3B"), "PHOTOS_DISC_3B");
    }
```

and change the existing `volid(…, N)` calls in that file's tests to `volid(…, "N")`.

In `crates/diskrement-shell/src/burn/xorriso/burner.rs` tests, add a test that a wrong-set twin is re-prompted. Model it on the existing test that inserts the wrong disc (search for `re-prompt` or `wrong` in that test module), with the found identity's `copy_set` set to 1 while the image's is 0. Assert that `operator.said("Not the expected disc")`, or whatever line the existing wrong-disc test asserts, appears, and that the burn eventually returns `Aborted` when the script runs out.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p diskrement-shell burn`
Expected: compile errors.

- [ ] **Step 3: Implement the identity**

In `crates/diskrement-shell/src/burn/mod.rs`, replace `DiscIdentity` and its `Display`:

```rust
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
    pub fn disc_name(&self) -> String {
        self.disc().name(self.copies)
    }

    /// The same physical disc series member: project, ordinal and copy set.
    /// The session is deliberately not compared.
    pub fn same_disc(&self, other: &DiscIdentity) -> bool {
        self.project_name == other.project_name && self.disc() == other.disc()
    }
}

/// Interpolated into `PostBurnCheckFailed`'s detail and the verifier's
/// re-prompt line. At `copies = 1` it reads as it always has.
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
```

Import `DiscKey` from `diskrement_core`.

In `crates/diskrement-shell/src/burn/json.rs`, replace `ProjectJson` and its `impl`:

```rust
fn one() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectJson {
    pub project_name: String,
    pub disc_ordinal: u32,
    /// Absent on discs written before copy sets existed: those are set A of a
    /// single-copy project.
    #[serde(default)]
    pub copy_set: u32,
    #[serde(default = "one")]
    pub copies: u32,
    pub session_ordinal: u32,
    pub created_at: String,
}

impl ProjectJson {
    pub fn from(id: &DiscIdentity) -> Self {
        ProjectJson {
            project_name: id.project_name.clone(),
            disc_ordinal: id.disc_ordinal,
            copy_set: id.copy_set,
            copies: id.copies,
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
            copy_set: self.copy_set,
            copies: self.copies,
            session_ordinal: self.session_ordinal,
            created_at,
        })
    }
}
```

- [ ] **Step 4: Use names and keys in the burner and the verifiers**

`crates/diskrement-shell/src/burn/xorriso/args.rs`:

```rust
pub fn volid(project_name: &str, disc_name: &str) -> String {
    format!("{project_name}_DISC_{disc_name}")
        .to_uppercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .take(32)
        .collect()
}
```

`crates/diskrement-shell/src/burn/xorriso/burner.rs`:
- `await_correct_media`: in both prompt strings, replace `id.disc_ordinal` with `id.disc_name()`. In the match guard, replace `found.project_name == id.project_name && found.disc_ordinal == id.disc_ordinal` with `found.same_disc(id)`. In the mismatch message, `found.disc_ordinal` becomes `found.disc_name()`.
- `write`: `args::volid(&id.project_name, &id.disc_name())`.
- `post_burn_check`: the predicate becomes `f.same_disc(id) && f.session_ordinal == id.session_ordinal`, and the message's `id.disc_ordinal` becomes `id.disc_name()`.
- Tests: every `DiscIdentity` literal gains `copy_set: 0, copies: 1,`. Expected strings are unchanged, because at `copies = 1` the name is the bare ordinal.

`crates/diskrement-shell/src/burn/verifier.rs`:
- `DiscExpectation`: replace `pub disc_ordinal: u32,` with

```rust
    pub disc: DiscKey,
    /// `DiscKey::name` for this project: the staging directory and the prompt.
    pub disc_name: String,
```

- `StagingVerifier::verify`: `.join(format!("disc-{}", exp.disc_name))`.
- `DiscVerifier::await_disc`: the prompt uses `exp.disc_name`, and the matcher becomes `id.project_name == exp.project_name && id.disc() == exp.disc`.
- Any other `exp.disc_ordinal` becomes `exp.disc_name` in text and `exp.disc` in comparisons. Test literals gain `disc: DiscKey::new(N, 0), disc_name: "N".to_string(),` in place of `disc_ordinal: N`.

`crates/diskrement-shell/src/burn/testkit.rs` `identity`: add `copy_set: 0, copies: 1,`.

`crates/diskrement-shell/src/burn/media.rs` tests: identity literals gain `copy_set: 0, copies: 1,`.

`crates/diskrement-shell/src/pipeline/mod.rs`:
- `burn`: the `DiscIdentity` literal gains `copy_set: report.disc.copy_set, copies: p.copies,`. The staging dir becomes `.join(format!("disc-{}", report.disc.key().name(p.copies)))`, and the label becomes `format!("{name} disc {}", report.disc.key().name(p.copies))`.
- `verify_disc`: the `DiscExpectation` gains `disc: DiscKey::new(ordinal, 0), disc_name: DiscKey::new(ordinal, 0).name(p.copies),` in place of `disc_ordinal: ordinal`.

`crates/diskrement-shell/tests/xorriso_image.rs`: identity and expectation literals follow the same rules.

- [ ] **Step 5: Run the tests**

Run: `cargo test --workspace`
Expected: PASS. If `xorriso` is installed, also confirm `cargo test -p diskrement-shell --test xorriso_image` reports its tests as run, not skipped.

- [ ] **Step 6: Verify and commit**

```bash
git add -A
git commit -m "feat(burn): identify discs by ordinal and copy set on media

tests: <n> passed

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 12: Plan and burn one disc per copy set

This is the heart of the feature, and it changes `PlanReport` and `BurnReport` together. The plan therefore moves to v2, `burn` becomes a per-disc loop, and the two renderers move in the same task so the workspace keeps compiling.

**Files:**
- Modify: `crates/diskrement-shell/src/pipeline/reports.rs`
- Modify: `crates/diskrement-shell/src/pipeline/planning.rs`
- Modify: `crates/diskrement-shell/src/pipeline/staged.rs`
- Modify: `crates/diskrement-shell/src/pipeline/mod.rs` (`plan`, `burn`, `burn_disc`, `disc_write`, `next_session_ordinal`, tests)
- Modify: `crates/diskrement-shell/src/error.rs` (`PartialBurn`)
- Modify: `crates/diskrement-cli/src/render.rs` (`plan_report`, `burn_report`, `error_line`), `crates/diskrement-cli/src/main.rs`

- [ ] **Step 1: New report types**

In `crates/diskrement-shell/src/pipeline/reports.rs`, replace `PlanReport`, `DiscBurn` and `BurnReport` with:

```rust
/// One copy set's share of the cycle: the disc it writes, plus what spills to
/// later cycles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetPlan {
    pub disc: PlannedDisc,
    /// How many of `disc.files` are catch-up copies rather than new files.
    pub catch_up: usize,
    /// The session ordinal this burn writes onto the disc (1 for a new disc).
    /// A catalog already one past it means the disc was committed before the
    /// staged plan could be rewritten, and `burn` skips it.
    pub session_ordinal: u32,
    pub remaining: RemainingEstimate,
}

/// The discs this cycle writes — one per copy set with something to do — in
/// set order.
///
/// `tombstones` are the whole scan's deletions. Every disc's manifest lists
/// them; the catalog records each one once, on the first disc to commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanReport {
    pub copies: u32,
    pub sets: Vec<SetPlan>,
    pub disc_type: DiscType,
    pub tombstones: Vec<RelPath>,
    pub oversized: Vec<ScannedFile>,
}

/// What one disc's burn produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscBurn {
    pub disc: DiscKey,
    pub label: String,
    pub finalized: bool,
    pub staging_dir: PathBuf,
    pub image_bytes: u64,
}

/// The result of one burn cycle: every disc written this run, the largest
/// remaining estimate among their sets, and each set's coverage afterwards.
///
/// `discs` can be empty when every planned disc had already been committed by
/// an earlier, interrupted run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BurnReport {
    pub discs: Vec<DiscBurn>,
    pub remaining: RemainingEstimate,
    pub coverage: Vec<SetCoverage>,
}
```

Change the imports to `use diskrement_core::{DiscKey, DiscState, DiscType, PlannedDisc, RelPath, ScannedFile, SetCoverage};`. Remove the second `use diskrement_core::{DiscType, …}` line.

- [ ] **Step 2: Write the failing planning tests**

In `crates/diskrement-shell/src/pipeline/planning.rs` tests, add these helpers and tests:

```rust
    fn plan1(view: &CatalogView, files: &[ScannedFile], states: &[DiscState], finalize: bool) -> PlanReport {
        plan_from(view, files, states, DiscType::BdR, Capacity::new(1_000), finalize, 1, |_| 1)
    }

    fn plan2(view: &CatalogView, files: &[ScannedFile], states: &[DiscState], finalize: bool) -> PlanReport {
        plan_from(view, files, states, DiscType::BdR, Capacity::new(1_000), finalize, 2, |_| 7)
    }

    fn live_in(files: &[ScannedFile], sets: &[u32]) -> CatalogView {
        let mut view = CatalogView::from_scanned(files);
        for e in view.current.values_mut() {
            e.live_sets = sets.iter().copied().collect();
        }
        view
    }

    fn open(ordinal: u32, copy_set: u32, used: u64) -> DiscState {
        DiscState { ordinal, copy_set, status: DiscStatus::Open, usable_bytes: 1_000, used_bytes: used }
    }

    #[test]
    fn a_fresh_two_copy_project_plans_twin_new_discs() {
        let files = vec![file("a.bin", 300), file("b.bin", 300)];
        let report = plan2(&CatalogView::empty(), &files, &[], false);
        assert_eq!(report.copies, 2);
        assert_eq!(report.sets.len(), 2);
        for (set, sp) in report.sets.iter().enumerate() {
            assert_eq!(sp.disc.key(), diskrement_core::DiscKey::new(1, set as u32));
            assert!(sp.disc.is_new);
            assert_eq!(sp.disc.files, files);
            assert_eq!(sp.catch_up, 0);
            assert_eq!(sp.session_ordinal, 1, "a new disc always starts at session 1");
        }
    }

    #[test]
    fn a_set_missing_a_copy_gets_it_as_catch_up_and_the_full_set_writes_nothing() {
        let files = vec![file("a.bin", 300)];
        let view = live_in(&files, &[0]);
        let report = plan2(&view, &files, &[open(1, 0, 300)], false);
        assert_eq!(report.sets.len(), 1, "set A has nothing to do");
        let sp = &report.sets[0];
        assert_eq!(sp.disc.key(), diskrement_core::DiscKey::new(1, 1));
        assert_eq!(sp.catch_up, 1);
        assert_eq!(sp.disc.files, files);
    }

    #[test]
    fn an_existing_open_disc_records_the_session_it_will_receive() {
        let files = vec![file("a.bin", 300), file("n.bin", 10)];
        let view = live_in(&files[..1], &[0, 1]);
        let report = plan2(&view, &files, &[open(1, 0, 300), open(1, 1, 300)], false);
        assert!(report.sets.iter().all(|sp| sp.session_ordinal == 7));
    }

    #[test]
    fn finalize_with_nothing_to_place_seals_every_sets_open_disc() {
        let files = vec![file("a.bin", 300)];
        let view = live_in(&files, &[0, 1]);
        let report = plan2(&view, &files, &[open(1, 0, 300), open(1, 1, 300)], true);
        assert_eq!(report.sets.len(), 2);
        assert!(report.sets.iter().all(|sp| sp.disc.will_finalize && sp.disc.files.is_empty()));
    }

    #[test]
    fn a_deletion_only_cycle_still_plans_one_set_a_disc_for_its_tombstones() {
        let kept = vec![file("a.bin", 300)];
        let view = live_in(&[file("a.bin", 300), file("gone.bin", 10)], &[0, 1]);
        let report = plan2(&view, &kept, &[open(1, 0, 310), open(1, 1, 310)], false);
        assert_eq!(report.sets.len(), 1);
        assert_eq!(report.sets[0].disc.key(), diskrement_core::DiscKey::new(1, 0));
        assert_eq!(report.tombstones, vec![RelPath("gone.bin".into())]);
    }

    #[test]
    fn a_single_copy_plan_has_exactly_one_set() {
        let report = plan1(&CatalogView::empty(), &[file("a.bin", 300)], &[], false);
        assert_eq!(report.copies, 1);
        assert_eq!(report.sets.len(), 1);
    }
```

Update the existing tests in this file mechanically: replace each `plan_from(view, files, states, disc_type, cap, finalize)` call with `plan_from(view, files, states, disc_type, cap, finalize, 1, |_| 1)`. Replace `report.disc` with `report.sets[0].disc` and `report.remaining` with `report.sets[0].remaining`. Replace `stale_detail(&report, &scanned)` with `stale_detail(&report.sets[0].disc, &report.tombstones, &scanned)`.

- [ ] **Step 3: Run them to see them fail**

Run: `cargo test -p diskrement-shell pipeline::planning`
Expected: compile errors.

- [ ] **Step 4: Implement `plan_from` and `stale_detail`**

Replace `plan_from` and `stale_detail` in `crates/diskrement-shell/src/pipeline/planning.rs`:

```rust
/// Diff once, plan every copy set, and reduce each set's full plan to the
/// **one disc that set writes this cycle**.
///
/// Per set: burn the first disc that actually carries files. The planner
/// keeps a set's open disc first by contract; when a new file cannot top it
/// off, that open disc is an empty finalizing placeholder — skip it so this
/// cycle writes the new disc instead of wasting a cycle on an empty session.
/// The skipped disc stays open.
///
/// A set with nothing to place is left out, unless `finalize` and it has an
/// open disc to seal. If that leaves no set at all (a no-op scan, or a
/// deletion-only one), the plan holds set A's head as-is, so the tombstones
/// still have a session to ride on — exactly the single-copy behaviour.
///
/// `next_session` returns the session ordinal an existing disc gets next; it
/// is recorded per set so `burn` can tell a disc it already committed from one
/// it has not.
#[allow(clippy::too_many_arguments)]
pub fn plan_from(
    view: &CatalogView,
    scanned: &[ScannedFile],
    states: &[DiscState],
    disc_type: DiscType,
    cap: Capacity,
    finalize: bool,
    copies: u32,
    next_session: impl Fn(DiscKey) -> u32,
) -> PlanReport {
    let changes = diff::diff(view, scanned);
    let new_files: Vec<ScannedFile> = changes
        .added
        .iter()
        .chain(changes.modified.iter())
        .cloned()
        .collect();
    let catch_up = replication::shortfall(view, &changes, copies);

    let mut sets = Vec::new();
    let mut oversized: Vec<ScannedFile> = Vec::new();
    let mut set_a_discs: Vec<PlannedDisc> = Vec::new();
    for (copy_set, extra) in (0..copies).zip(catch_up.iter()) {
        let to_place: Vec<ScannedFile> = new_files.iter().chain(extra.iter()).cloned().collect();
        let full = planner::plan_set(copy_set, &to_place, &changes.deleted, states, cap);
        for f in full.oversized {
            if !oversized.iter().any(|o| o.rel_path == f.rel_path) {
                oversized.push(f);
            }
        }
        // `plan_set` always returns at least one disc.
        let chosen = match full.discs.iter().position(|d| !d.files.is_empty()) {
            Some(i) => Some(i),
            None if finalize && !full.discs[0].is_new => Some(0),
            None => None,
        };
        if let Some(i) = chosen {
            sets.push(set_plan(&full.discs, i, finalize, extra, &next_session));
        }
        if copy_set == 0 {
            set_a_discs = full.discs;
        }
    }
    if sets.is_empty() {
        sets.push(set_plan(&set_a_discs, 0, finalize, &catch_up[0], &next_session));
    }
    oversized.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));

    PlanReport {
        copies,
        sets,
        disc_type,
        tombstones: changes.deleted,
        oversized,
    }
}

/// One set's chosen disc, with what spills behind it.
fn set_plan(
    discs: &[PlannedDisc],
    idx: usize,
    finalize: bool,
    catch_up: &[ScannedFile],
    next_session: &impl Fn(DiscKey) -> u32,
) -> SetPlan {
    let mut disc = discs[idx].clone();
    // The planner already seals this disc iff the plan spilled; `--finalize`
    // forces a seal even when everything fit.
    disc.will_finalize = disc.will_finalize || finalize;
    let tail = &discs[idx + 1..];
    let catch_up_paths: HashSet<&RelPath> = catch_up.iter().map(|f| &f.rel_path).collect();
    SetPlan {
        catch_up: disc
            .files
            .iter()
            .filter(|f| catch_up_paths.contains(&f.rel_path))
            .count(),
        session_ordinal: if disc.is_new {
            1
        } else {
            next_session(disc.key())
        },
        remaining: RemainingEstimate {
            more_discs: tail.len(),
            more_files: tail.iter().map(|d| d.files.len()).sum(),
            more_bytes: tail
                .iter()
                .flat_map(|d| d.files.iter())
                .map(|f| f.size_bytes)
                .sum(),
        },
        disc,
    }
}

/// Why `disc` no longer describes `scanned`, or `None` if it still does.
///
/// `scan --force` re-stages the scan without touching the plan, so the two
/// can drift apart. The disc is stale when a file it places is missing from
/// the scan or has a different hash or size there, or when a path the cycle
/// tombstones is present again. A file the scan added that the plan does not
/// place is not staleness: the next cycle picks it up.
pub fn stale_detail(
    disc: &PlannedDisc,
    tombstones: &[RelPath],
    scanned: &[ScannedFile],
) -> Option<String> {
    let by_path: HashMap<&RelPath, &ScannedFile> =
        scanned.iter().map(|f| (&f.rel_path, f)).collect();
    for planned in &disc.files {
        let what = match by_path.get(&planned.rel_path) {
            None => "is gone from",
            Some(f) if f.content_hash != planned.content_hash => "changed in",
            Some(f) if f.size_bytes != planned.size_bytes => "changed in",
            Some(_) => continue,
        };
        return Some(format!(
            "{} {what} the staged scan after this plan was staged",
            planned.rel_path
        ));
    }
    tombstones
        .iter()
        .find(|t| by_path.contains_key(t))
        .map(|t| format!("{t} is back in the staged scan after this plan tombstoned it"))
}
```

Imports at the top of `planning.rs`:

```rust
use std::collections::{HashMap, HashSet};

use diskrement_core::{
    diff, planner, replication, Capacity, CatalogView, DiscKey, DiscState, DiscType, PlannedDisc,
    RelPath, ScannedFile,
};

use crate::pipeline::reports::{PlanReport, RemainingEstimate, SetPlan};
```

Rewrite the module doc so it no longer mentions the fixture oracle:
> The pure half of `plan`: everything between reading the staged scan and writing `staged/plan.json`. No filesystem, no catalog, no clock — `Pipeline::plan` is the thin edge around this.

- [ ] **Step 5: Stage `plan.json` v2**

In `crates/diskrement-shell/src/pipeline/staged.rs`, replace `PlannedDiscJson`, `PlanReportJson`, `write_plan` and `read_plan`, and add `is_legacy_plan`:

```rust
#[derive(Serialize, Deserialize)]
struct PlannedDiscJson {
    ordinal: u32,
    #[serde(rename = "copySet")]
    copy_set: u32,
    #[serde(rename = "isNew")]
    is_new: bool,
    #[serde(rename = "willFinalize")]
    will_finalize: bool,
    files: Vec<ScannedFileJson>,
    #[serde(rename = "priorUsedBytes")]
    prior_used_bytes: u64,
    #[serde(rename = "usableBytes")]
    usable_bytes: u64,
}

#[derive(Serialize, Deserialize)]
struct SetPlanJson {
    disc: PlannedDiscJson,
    #[serde(rename = "catchUp")]
    catch_up: usize,
    #[serde(rename = "sessionOrdinal")]
    session_ordinal: u32,
    remaining: RemainingJson,
}

/// The v2 staged plan: one entry per copy set this cycle writes. A v1 plan
/// had a single top-level `disc` instead of `sets`; see `is_legacy_plan`.
#[derive(Serialize, Deserialize)]
struct PlanReportJson {
    copies: u32,
    sets: Vec<SetPlanJson>,
    #[serde(rename = "discType")]
    disc_type: String,
    tombstones: Vec<String>,
    oversized: Vec<ScannedFileJson>,
}

pub fn write_plan(work_dir: &Path, report: &PlanReport) -> std::io::Result<()> {
    let json = PlanReportJson {
        copies: report.copies,
        sets: report
            .sets
            .iter()
            .map(|sp| SetPlanJson {
                disc: PlannedDiscJson {
                    ordinal: sp.disc.ordinal,
                    copy_set: sp.disc.copy_set,
                    is_new: sp.disc.is_new,
                    will_finalize: sp.disc.will_finalize,
                    files: sp.disc.files.iter().map(ScannedFileJson::of).collect(),
                    prior_used_bytes: sp.disc.prior_used_bytes,
                    usable_bytes: sp.disc.usable_bytes,
                },
                catch_up: sp.catch_up,
                session_ordinal: sp.session_ordinal,
                remaining: RemainingJson {
                    more_discs: sp.remaining.more_discs,
                    more_files: sp.remaining.more_files,
                    more_bytes: sp.remaining.more_bytes,
                },
            })
            .collect(),
        disc_type: report.disc_type.as_str().to_string(),
        tombstones: report
            .tombstones
            .iter()
            .map(|p| p.as_str().to_string())
            .collect(),
        oversized: report.oversized.iter().map(ScannedFileJson::of).collect(),
    };
    write_json(
        &plan_path(work_dir),
        &serde_json::to_string(&json).expect("serializable"),
    )
}

/// `Ok(None)` when nothing is staged; a decode failure — including an enum
/// spelling this build does not know — is a corrupt-file defect. Check
/// `is_legacy_plan` first: a v1 plan is not corrupt, just old.
pub fn read_plan(work_dir: &Path) -> Result<Option<PlanReport>, serde_json::Error> {
    let Some(raw) = read_opt(&plan_path(work_dir))? else {
        return Ok(None);
    };
    let j: PlanReportJson = serde_json::from_str(&raw)?;
    let files = |v: Vec<ScannedFileJson>| -> Result<Vec<ScannedFile>, serde_json::Error> {
        v.into_iter().map(ScannedFileJson::into_scanned).collect()
    };
    let mut sets = Vec::with_capacity(j.sets.len());
    for s in j.sets {
        sets.push(SetPlan {
            disc: PlannedDisc {
                ordinal: s.disc.ordinal,
                copy_set: s.disc.copy_set,
                is_new: s.disc.is_new,
                will_finalize: s.disc.will_finalize,
                files: files(s.disc.files)?,
                prior_used_bytes: s.disc.prior_used_bytes,
                usable_bytes: s.disc.usable_bytes,
            },
            catch_up: s.catch_up,
            session_ordinal: s.session_ordinal,
            remaining: RemainingEstimate {
                more_discs: s.remaining.more_discs,
                more_files: s.remaining.more_files,
                more_bytes: s.remaining.more_bytes,
            },
        });
    }
    Ok(Some(PlanReport {
        copies: j.copies,
        sets,
        disc_type: DiscType::from_str(&j.disc_type).map_err(decode_error)?,
        tombstones: j.tombstones.into_iter().map(RelPath).collect(),
        oversized: files(j.oversized)?,
    }))
}

/// True when the staged plan is a v1 plan — a single top-level `disc` — from a
/// build that predates copy sets. Anything unreadable is not "legacy";
/// `read_plan` reports it.
pub fn is_legacy_plan(work_dir: &Path) -> bool {
    std::fs::read_to_string(plan_path(work_dir))
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .is_some_and(|v| v.get("disc").is_some() && v.get("sets").is_none())
}
```

Import `SetPlan` from `crate::pipeline::reports`. Remove the Task 5 marker comment. Rewrite the module doc's last sentence to: "Field spellings follow the Scala zio-json codec's camelCase. The plan is v2 since copy sets (one entry per set); a v1 plan is detected by `is_legacy_plan` and answered with a stale-plan error."

Update the existing staged tests so they build `PlanReport` in the new shape (`copies: 1, sets: vec![SetPlan { disc, catch_up: 0, session_ordinal: 1, remaining }]`), and add:

```rust
    #[test]
    fn a_two_set_plan_round_trips() {
        let dir = tempfile::TempDir::new().unwrap();
        let disc = |set| PlannedDisc {
            ordinal: 1,
            copy_set: set,
            is_new: true,
            will_finalize: false,
            files: vec![],
            prior_used_bytes: 0,
            usable_bytes: 100,
        };
        let remaining = RemainingEstimate { more_discs: 0, more_files: 0, more_bytes: 0 };
        let report = PlanReport {
            copies: 2,
            sets: vec![
                SetPlan { disc: disc(0), catch_up: 0, session_ordinal: 1, remaining: remaining.clone() },
                SetPlan { disc: disc(1), catch_up: 3, session_ordinal: 1, remaining },
            ],
            disc_type: DiscType::BdR,
            tombstones: vec![RelPath("gone".into())],
            oversized: vec![],
        };
        write_plan(dir.path(), &report).unwrap();
        assert_eq!(read_plan(dir.path()).unwrap(), Some(report));
        assert!(!is_legacy_plan(dir.path()));
    }

    #[test]
    fn a_v1_plan_is_recognised_as_legacy() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("staged")).unwrap();
        std::fs::write(
            plan_path(dir.path()),
            r#"{"disc":{"ordinal":1,"isNew":true,"willFinalize":false,"files":[],
                "priorUsedBytes":0,"usableBytes":100},"discType":"BD_R","tombstones":[],
                "remaining":{"moreDiscs":0,"moreFiles":0,"moreBytes":0},"oversized":[]}"#,
        )
        .unwrap();
        assert!(is_legacy_plan(dir.path()));
    }
```

`PlanReport` derives `PartialEq`, so the `assert_eq!` compiles.

- [ ] **Step 6: Add `PartialBurn`**

In `crates/diskrement-shell/src/error.rs`, add a variant after `Burn`:

```rust
    /// A multi-disc burn that committed some discs and then failed. The cause
    /// decides the exit code — an aborted prompt still exits 0 — and the
    /// message says what to do next.
    #[error(
        "{} — {} committed; {} not written (re-run burn)",
        .cause, .committed.join(", "), .pending.join(", ")
    )]
    PartialBurn {
        committed: Vec<String>,
        pending: Vec<String>,
        cause: Box<PipelineError>,
    },
```

In `exit_code`, add the arm `PipelineError::PartialBurn { cause, .. } => cause.exit_code(),`. Add to the error tests:

```rust
    #[test]
    fn a_partial_burn_carries_its_causes_exit_code_and_names_both_halves() {
        let partial = |cause| PipelineError::PartialBurn {
            committed: vec!["1A".into()],
            pending: vec!["1B".into()],
            cause: Box::new(cause),
        };
        let media = partial(PipelineError::Burn(crate::burn::BurnError::NoMedia("/dev/sr0".into())));
        assert_eq!(media.exit_code(), 6);
        assert_eq!(
            media.to_string(),
            "no disc in /dev/sr0 — 1A committed; 1B not written (re-run burn)"
        );
        assert_eq!(partial(PipelineError::Burn(crate::burn::BurnError::Aborted)).exit_code(), 0);
    }
```

- [ ] **Step 7: Rewrite `plan` and `burn` in the pipeline**

In `crates/diskrement-shell/src/pipeline/mod.rs`:

Replace `next_session_ordinal` with a map and a lookup:

```rust
    /// One past the highest session ordinal committed on each disc that has
    /// any (`PipelineLive.existingSessions`). Both BURNED and VERIFIED count.
    fn next_sessions(&self, project: ProjectId) -> Result<BTreeMap<DiscKey, u32>, PipelineError> {
        let mut next: BTreeMap<DiscKey, u32> = BTreeMap::new();
        for state in [SessionState::Burned, SessionState::Verified] {
            for s in self
                .catalog
                .sessions_by_state(project, state)
                .map_err(Self::failed)?
            {
                let n = next.entry(s.disc).or_insert(1);
                *n = (*n).max(s.ordinal + 1);
            }
        }
        Ok(next)
    }

    /// The session ordinal `disc` gets next: 1 for a disc with none yet.
    fn next_session_ordinal(&self, project: ProjectId, disc: DiscKey) -> Result<u32, PipelineError> {
        Ok(self.next_sessions(project)?.get(&disc).copied().unwrap_or(1))
    }
```

In `plan`, replace the `planning::plan_from(…)` call with:

```rust
        let next = self.next_sessions(p.id)?;
        let report = planning::plan_from(
            &view,
            &scanned,
            &states,
            disc_type,
            cap,
            finalize,
            p.copies,
            |disc| next.get(&disc).copied().unwrap_or(1),
        );
```

Update `plan`'s doc comment: "…decide the disc each copy set writes this cycle…".

Replace `burn` (and its doc comment) with the loop below, and add `burn_disc` after it:

```rust
    /// `burn`: write the staged plan's discs in set order, committing each one
    /// as soon as it is written.
    ///
    /// Per disc the ordering is the design (`PipelineLive.burn`): the disc tree
    /// is staged **before** anything reaches the live catalog, and the snapshot
    /// embedded on the disc is the same update applied to a work-dir copy, so
    /// the disc knows about its own session — and about every disc this cycle
    /// already committed. After each commit the staged plan is rewritten to the
    /// discs still to write, so it stays the recovery unit and no `PLANNED` row
    /// is ever persisted. A failure after at least one commit is a
    /// `PartialBurn`; re-running `burn` resumes at the first uncommitted disc.
    ///
    /// `now` is supplied by the caller rather than read from the clock: it
    /// keeps the pipeline deterministic.
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
        if staged::is_legacy_plan(&self.paths.work_dir) {
            return Err(PipelineError::StalePlan(
                "it was staged by an older version of diskrement".to_string(),
            ));
        }
        let mut report = staged::read_plan(&self.paths.work_dir)
            .map_err(Self::failed)?
            .ok_or_else(|| PipelineError::NoStagedPlan(name.to_string()))?;
        if !report.oversized.is_empty() {
            return Err(PipelineError::OversizedFiles(
                report
                    .oversized
                    .iter()
                    .map(|f| f.rel_path.clone())
                    .collect(),
            ));
        }

        let mut done: Vec<DiscKey> = Vec::new();
        let mut written: Vec<DiscBurn> = Vec::new();
        let mut remaining = RemainingEstimate {
            more_discs: 0,
            more_files: 0,
            more_bytes: 0,
        };
        while let Some(sp) = report.sets.first().cloned() {
            match self.burn_disc(&p, &scanned, &report, &sp, now, burner) {
                Ok(burned) => {
                    done.push(sp.disc.key());
                    written.extend(burned);
                    remaining = RemainingEstimate {
                        more_discs: remaining.more_discs.max(sp.remaining.more_discs),
                        more_files: remaining.more_files.max(sp.remaining.more_files),
                        more_bytes: remaining.more_bytes.max(sp.remaining.more_bytes),
                    };
                    report.sets.remove(0);
                    if !report.sets.is_empty() {
                        staged::write_plan(&self.paths.work_dir, &report).map_err(Self::failed)?;
                    }
                }
                Err(e) if done.is_empty() => return Err(e),
                Err(e) => {
                    return Err(PipelineError::PartialBurn {
                        committed: done.iter().map(|k| k.name(p.copies)).collect(),
                        pending: report
                            .sets
                            .iter()
                            .map(|s| s.disc.key().name(p.copies))
                            .collect(),
                        cause: Box::new(e),
                    })
                }
            }
        }

        snapshot::release(&SnapshotHandle {
            root: self.paths.work_dir.join("snapshot"),
        });
        staged::clear(&self.paths.work_dir).map_err(Self::failed)?;
        let view = self.catalog.read_view(p.id).map_err(Self::failed)?;
        Ok(BurnReport {
            discs: written,
            remaining,
            coverage: replication::coverage(&view, p.copies),
        })
    }

    /// Write one planned disc and commit its session. `Ok(None)` means the
    /// catalog already holds the session this plan expected: an earlier run
    /// committed the disc and stopped before rewriting the staged plan.
    fn burn_disc(
        &mut self,
        p: &Project,
        scanned: &[ScannedFile],
        report: &PlanReport,
        sp: &SetPlan,
        now: DateTime<Utc>,
        burner: &Burner<'_>,
    ) -> Result<Option<DiscBurn>, PipelineError> {
        let disc = &sp.disc;
        let key = disc.key();
        let disc_name = key.name(p.copies);

        // The catalog cannot catch this: deadness is derived from `disc_event`,
        // so the disc row itself still reads OPEN. Without this guard the
        // burner would commit a session onto a disc that no longer physically
        // exists.
        let states = self.catalog.read_disc_states(p.id).map_err(Self::failed)?;
        if states
            .iter()
            .any(|s| s.key() == key && s.status == DiscStatus::Dead)
        {
            return Err(PipelineError::StalePlan(format!(
                "disc {disc_name} was marked dead after this plan was staged"
            )));
        }

        let sess_ord = self.next_session_ordinal(p.id, key)?;
        if sess_ord == sp.session_ordinal + 1 {
            return Ok(None);
        }
        if sess_ord != sp.session_ordinal {
            return Err(PipelineError::StalePlan(format!(
                "disc {disc_name} would get session {sess_ord}, but this plan was staged for session {}",
                sp.session_ordinal
            )));
        }

        if let Some(detail) = planning::stale_detail(disc, &report.tombstones, scanned) {
            return Err(PipelineError::StalePlan(detail));
        }

        let snap_root = self.paths.work_dir.join("snapshot");
        if !snap_root.exists() {
            return Err(PipelineError::SnapshotMissing(p.name.clone()));
        }

        // Classified against the catalog as it stands now — after any disc
        // this cycle already committed — so a twin's files become copies of
        // the versions its sibling just recorded. A tombstone is written by
        // the first disc to commit; later discs find the path already gone.
        let view = self.catalog.read_view(p.id).map_err(Self::failed)?;
        let contents = replication::classify(&view, &disc.files);
        let tombstones: Vec<RelPath> = report
            .tombstones
            .iter()
            .filter(|t| view.current.contains_key(*t))
            .cloned()
            .collect();
        let manifest = manifest::build(&p.name, sess_ord, disc, &contents, report.tombstones.clone());
        let label = format!("{} disc {disc_name}", p.name);
        let update = CatalogUpdate {
            project_id: p.id,
            finalize_disc_ordinals: Vec::new(),
            discs: vec![disc_write(
                &label,
                disc,
                report.disc_type,
                sess_ord,
                now,
                &contents,
                tombstones,
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
                .join(format!("disc-{disc_name}")),
            snapshot_root: snap_root,
            files: disc.files.clone(),
            metadata: DiscMetadata {
                identity: DiscIdentity {
                    project_name: p.name.clone(),
                    disc_ordinal: disc.ordinal,
                    copy_set: disc.copy_set,
                    copies: p.copies,
                    session_ordinal: sess_ord,
                    created_at: now,
                },
                manifest,
                catalog_snapshot: tmp_catalog.clone(),
                pointers,
            },
            close_disc: disc.will_finalize,
        };
        let burned = burner.burn(&image)?;

        // The crash-safe commit point: the disc exists, so the catalog may now
        // learn about it. The committed state equals the embedded snapshot.
        self.catalog.apply_update(&update).map_err(Self::failed)?;
        let _ = std::fs::remove_file(&tmp_catalog);

        Ok(Some(DiscBurn {
            disc: key,
            label,
            finalized: disc.will_finalize,
            staging_dir: burned.staging_dir,
            image_bytes: burned.image_bytes,
        }))
    }
```

Change `disc_write` to take the disc and disc type directly, since `PlanReport` no longer has a single disc:

```rust
#[allow(clippy::too_many_arguments)]
fn disc_write(
    label: &str,
    disc: &PlannedDisc,
    disc_type: DiscType,
    sess_ord: u32,
    now: DateTime<Utc>,
    contents: &DiscContents,
    tombstones: Vec<RelPath>,
    manifest: &SessionManifest,
) -> DiscWrite {
```

In its body, replace `report.disc.` with `disc.` and `report.disc_type` with `disc_type`.

Imports: add `PlannedDisc`, `replication` and `DiscKey` from `diskrement_core`, and `SetPlan` and `RemainingEstimate` from `reports`. Remove any import the compiler then reports as unused (`diff`, `ChangeSet`).

- [ ] **Step 8: Fix and extend the pipeline tests**

Mechanical rules for the existing tests in `pipeline/mod.rs`:
- `plan_from(…)` calls gain `, 1, |_| 1`.
- `report.disc` on a `PlanReport` becomes `report.sets[0].disc`; `report.remaining` becomes `report.sets[0].remaining`.
- On a `BurnReport`, `report.disc.ordinal` becomes `report.discs[0].disc.ordinal`, and `report.disc.label`/`.finalized` become `report.discs[0].label`/`.finalized`.
- `a_second_session_on_the_same_disc_gets_ordinal_two`: replace the hand-built `plan_from` call with the pipeline's own `plan` (`Pipeline::new(…).plan("p", None, false).unwrap()`), so the staged plan records session 2.
- `a_failed_burn_leaves_the_catalog_and_the_staged_plan_alone`: unchanged. A failure on the first disc is still the raw error, not `PartialBurn`.

Give `staged_env` a copies parameter. Rename it to `staged_env_with(files, finalize, copies)`. Inside, pass `copies` to `create_project` (`copies,`) and to `plan_from` (`…, finalize, copies, |_| 1)`). Then add:

```rust
    fn staged_env(files: &[(&str, &[u8])], finalize: bool) -> Env {
        staged_env_with(files, finalize, 1)
    }
```

Append the new tests after `burn_reads_the_snapshot_not_the_source_root`:

```rust
    #[test]
    fn a_two_copy_burn_writes_twins_and_the_second_holds_copies() {
        let mut env = staged_env_with(&[("a.bin", b"aaaa"), ("b.bin", b"bbbb")], false, 2);
        let report = run_burn(&mut env).unwrap();
        assert_eq!(
            report.discs.iter().map(|d| d.label.as_str()).collect::<Vec<_>>(),
            vec!["p disc 1A", "p disc 1B"]
        );
        assert!(env.work.join("staging/disc-1A/data/a.bin").is_file());
        assert!(env.work.join("staging/disc-1B/data/a.bin").is_file());
        let count = |t: &str| -> i64 {
            env.catalog
                .raw_connection_for_tests()
                .query_row(&format!("select count(*) from {t}"), [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(count("file_version"), 2, "1B recorded copies, not new versions");
        assert_eq!(count("placement"), 4);
        assert_eq!(report.coverage, vec![SetCoverage::default(), SetCoverage::default()]);
        assert!(staged::read_plan(&env.work).unwrap().is_none(), "staged state cleared");
    }

    /// A regular file where 1B's staging directory goes makes the stager fail
    /// on 1B only.
    fn block_disc(env: &Env, name: &str) -> std::path::PathBuf {
        let blocker = env.work.join(format!("staging/disc-{name}"));
        std::fs::create_dir_all(blocker.parent().unwrap()).unwrap();
        std::fs::write(&blocker, b"not a directory").unwrap();
        blocker
    }

    #[test]
    fn a_failure_on_the_second_set_keeps_the_first_and_resumes_at_the_second() {
        let mut env = staged_env_with(&[("a.bin", b"aaaa")], false, 2);
        let blocker = block_disc(&env, "1B");

        let err = run_burn(&mut env).unwrap_err();
        assert!(matches!(err, PipelineError::PartialBurn { .. }), "{err}");
        assert_eq!(err.exit_code(), 6);
        assert!(err.to_string().ends_with("1A committed; 1B not written (re-run burn)"), "{err}");

        let staged = staged::read_plan(&env.work).unwrap().expect("plan survives");
        assert_eq!(staged.sets.len(), 1);
        assert_eq!(staged.sets[0].disc.copy_set, 1);
        assert!(env.work.join("snapshot").exists(), "the snapshot survives too");

        std::fs::remove_file(&blocker).unwrap();
        let report = run_burn(&mut env).unwrap();
        assert_eq!(
            report.discs.iter().map(|d| d.label.as_str()).collect::<Vec<_>>(),
            vec!["p disc 1B"]
        );
        let sessions = env.catalog.sessions_by_state(ProjectId(1), SessionState::Burned).unwrap();
        assert_eq!(
            sessions.iter().map(|s| s.disc).collect::<Vec<_>>(),
            vec![DiscKey::new(1, 0), DiscKey::new(1, 1)]
        );
    }

    #[test]
    fn a_disc_committed_before_the_plan_was_rewritten_is_skipped() {
        let mut env = staged_env_with(&[("a.bin", b"aaaa")], false, 2);
        let original = std::fs::read(staged::plan_path(&env.work)).unwrap();
        let blocker = block_disc(&env, "1B");
        run_burn(&mut env).unwrap_err();

        // Simulate the crash window: 1A is committed, but the plan on disk
        // still lists both sets.
        std::fs::write(staged::plan_path(&env.work), original).unwrap();
        std::fs::remove_file(&blocker).unwrap();

        let report = run_burn(&mut env).unwrap();
        assert_eq!(report.discs.len(), 1, "1A skipped, 1B written");
        let on_1a = env
            .catalog
            .sessions_by_state(ProjectId(1), SessionState::Burned)
            .unwrap()
            .into_iter()
            .filter(|s| s.disc == DiscKey::new(1, 0))
            .count();
        assert_eq!(on_1a, 1, "1A was not burned twice");
    }

    #[test]
    fn a_plan_staged_by_an_older_build_is_stale_not_corrupt() {
        let mut env = staged_env(&[("a.bin", b"aaaa")], false);
        std::fs::write(
            staged::plan_path(&env.work),
            r#"{"disc":{"ordinal":1,"isNew":true,"willFinalize":false,"files":[],
                "priorUsedBytes":0,"usableBytes":100},"discType":"BD_R","tombstones":[],
                "remaining":{"moreDiscs":0,"moreFiles":0,"moreBytes":0},"oversized":[]}"#,
        )
        .unwrap();
        let err = run_burn(&mut env).unwrap_err();
        assert!(matches!(err, PipelineError::StalePlan(_)), "{err}");
        assert!(!err.to_string().contains("reset"), "{err}");
    }
```

Import `DiscKey` and `SetCoverage` from `diskrement_core` in the test module.

- [ ] **Step 9: Render the plan and the burn**

In `crates/diskrement-cli/src/render.rs`, replace `plan_report` and `burn_report`:

```rust
/// `Render.plan`. Three shapes: nothing to do, bare seals, and one summary
/// pair per copy set with the optional deletion and oversize lines. At
/// `copies = 1` there is one set and the output is the single-disc text.
pub fn plan_report(p: &PlanReport) -> String {
    let copies = p.copies;
    let nothing = p.sets.iter().all(|s| s.disc.files.is_empty())
        && p.tombstones.is_empty()
        && p.oversized.is_empty();
    if nothing {
        let seals: Vec<String> = p
            .sets
            .iter()
            .filter(|s| s.disc.will_finalize)
            .map(|s| {
                format!(
                    "Plan: finalize disc {} (seal; no new files).",
                    s.disc.key().name(copies)
                )
            })
            .collect();
        return if seals.is_empty() {
            "Nothing to burn: no changes to plan.".to_string()
        } else {
            seals.join("\n")
        };
    }
    let mut lines = Vec::new();
    for s in &p.sets {
        let d = &s.disc;
        let name = d.key().name(copies);
        let kind = if d.is_new {
            "new disc".to_string()
        } else {
            format!("appending to open disc {name}")
        };
        let fin = if d.will_finalize {
            "will FINALIZE"
        } else {
            "left OPEN"
        };
        let count = if copies > 1 {
            format!(
                "{} files ({} new, {} catch-up)",
                d.files.len(),
                d.files.len() - s.catch_up,
                s.catch_up
            )
        } else {
            format!("{} files", d.files.len())
        };
        lines.push(format!(
            "Plan: disc {name} ({kind}, {}) — {count}, {}, {fin}",
            p.disc_type,
            gb(d.total_used_bytes())
        ));
        lines.push(if s.remaining.more_discs == 0 {
            "This disc completes the set.".to_string()
        } else {
            format!(
                "~{} more files (~{} more discs) remain after this one.",
                s.remaining.more_files, s.remaining.more_discs
            )
        });
    }
    if !p.tombstones.is_empty() {
        lines.push(format!("Deletions: {}", p.tombstones.len()));
    }
    if !p.oversized.is_empty() {
        lines.push("Oversized (cannot be placed):".to_string());
        for f in &p.oversized {
            lines.push(format!("  {}  {}", f.rel_path.as_str(), gb(f.size_bytes)));
        }
    }
    lines.join("\n")
}

/// `Render.burn`: one block per disc written, the copy coverage when the
/// project keeps more than one copy, then what to do next.
pub fn burn_report(r: &BurnReport, copies: u32) -> String {
    let mut lines = Vec::new();
    if r.discs.is_empty() {
        lines.push("No disc written: every planned disc was already committed.".to_string());
    }
    for d in &r.discs {
        lines.push(format!(
            "✔ Wrote disc {}  —  label: \"{}\"",
            d.disc.name(copies),
            d.label
        ));
        lines.push(
            if d.finalized {
                "Status: FINALIZED — set this disc aside, it is sealed."
            } else {
                "Status: left OPEN — may receive more sessions later."
            }
            .to_string(),
        );
    }
    if copies > 1 {
        lines.extend(coverage_lines(&r.coverage));
    }
    lines.push(if r.remaining.more_discs == 0 {
        "No more changes — the set is up to date.".to_string()
    } else {
        format!(
            "~{} more discs of changes remain. Run: scan → plan → burn",
            r.remaining.more_discs
        )
    });
    lines.join("\n")
}

/// One line when every set is complete, otherwise one line per lagging set.
fn coverage_lines(coverage: &[SetCoverage]) -> Vec<String> {
    if coverage.iter().all(|c| c.missing_files == 0) {
        return vec![format!(
            "Copies: every current file has {} copies.",
            coverage.len()
        )];
    }
    coverage
        .iter()
        .enumerate()
        .filter(|(_, c)| c.missing_files > 0)
        .map(|(set, c)| {
            format!(
                "Copies: set {} is missing {} files ({}).",
                set_letter(set as u32),
                c.missing_files,
                gb(c.missing_bytes)
            )
        })
        .collect()
}
```

In `error_line`, add an arm before `other =>`:

```rust
        PipelineError::PartialBurn {
            committed,
            pending,
            cause,
        } if matches!(
            **cause,
            PipelineError::Burn(diskrement_shell::burn::BurnError::Aborted)
        ) =>
        {
            format!(
                "Aborted. {} committed; {} not written (re-run burn).",
                committed.join(", "),
                pending.join(", ")
            )
        }
```

Import `diskrement_core::{set_letter, SetCoverage}`.

In `crates/diskrement-cli/src/main.rs`, change `render::burn_report(&report)` to `render::burn_report(&report, p.copies)`.

Render tests: build `PlanReport` values as `PlanReport { copies: 1, sets: vec![SetPlan { disc, catch_up: 0, session_ordinal: 1, remaining }], … }`, and build `BurnReport` values as `BurnReport { discs: vec![DiscBurn { disc: DiscKey::new(N, 0), … }], remaining, coverage: vec![SetCoverage::default()] }`. Call `burn_report(&r, 1)`. The existing expected strings and the `burn.snap`/`plan.snap` snapshots must not change. Then add:

```rust
    #[test]
    fn a_two_copy_plan_names_each_set_and_splits_new_from_catch_up() {
        let disc = |set, files| PlannedDisc {
            ordinal: 2,
            copy_set: set,
            is_new: true,
            will_finalize: false,
            files,
            prior_used_bytes: 0,
            usable_bytes: 25_000_000_000,
        };
        let none = RemainingEstimate { more_discs: 0, more_files: 0, more_bytes: 0 };
        let p = PlanReport {
            copies: 2,
            sets: vec![
                SetPlan { disc: disc(0, vec![scanned("n", 1)]), catch_up: 0, session_ordinal: 1, remaining: none.clone() },
                SetPlan { disc: disc(1, vec![scanned("c", 1), scanned("n", 1)]), catch_up: 1, session_ordinal: 1, remaining: none },
            ],
            disc_type: DiscType::BdR,
            tombstones: vec![],
            oversized: vec![],
        };
        assert_eq!(
            plan_report(&p),
            "Plan: disc 2A (new disc, BD_R) — 1 files (1 new, 0 catch-up), 0.0 GB, left OPEN\n\
             This disc completes the set.\n\
             Plan: disc 2B (new disc, BD_R) — 2 files (1 new, 1 catch-up), 0.0 GB, left OPEN\n\
             This disc completes the set."
        );
    }

    #[test]
    fn a_two_copy_burn_lists_both_discs_and_the_coverage() {
        let d = |set| DiscBurn {
            disc: DiscKey::new(1, set),
            label: format!("p disc 1{}", if set == 0 { 'A' } else { 'B' }),
            finalized: false,
            staging_dir: "/w/staging".into(),
            image_bytes: 1,
        };
        let r = BurnReport {
            discs: vec![d(0), d(1)],
            remaining: RemainingEstimate { more_discs: 0, more_files: 0, more_bytes: 0 },
            coverage: vec![SetCoverage::default(), SetCoverage { missing_files: 3, missing_bytes: 2_000_000_000 }],
        };
        assert_eq!(
            burn_report(&r, 2),
            "✔ Wrote disc 1A  —  label: \"p disc 1A\"\n\
             Status: left OPEN — may receive more sessions later.\n\
             ✔ Wrote disc 1B  —  label: \"p disc 1B\"\n\
             Status: left OPEN — may receive more sessions later.\n\
             Copies: set B is missing 3 files (2.0 GB).\n\
             No more changes — the set is up to date."
        );
    }

    #[test]
    fn an_aborted_second_disc_names_what_was_committed() {
        let err = PipelineError::PartialBurn {
            committed: vec!["1A".into()],
            pending: vec!["1B".into()],
            cause: Box::new(PipelineError::Burn(diskrement_shell::burn::BurnError::Aborted)),
        };
        assert_eq!(error_line(&err), "Aborted. 1A committed; 1B not written (re-run burn).");
        assert_eq!(err.exit_code(), 0);
    }
```

- [ ] **Step 10: Run the tests**

Run: `cargo test --workspace`
Expected: PASS, and the insta snapshots are unchanged (`git status` shows no `.snap.new` files).

- [ ] **Step 11: Verify and commit**

```bash
git add -A
git commit -m "feat(pipeline): plan and burn one disc per copy set, committing each disc

tests: <n> passed

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 13: Status, verify, mark-dead and history speak copy sets

**Files:**
- Modify: `crates/diskrement-shell/src/pipeline/mod.rs` (`status`, `verify`, `verify_disc`, `mark_dead`, `resolve_disc`)
- Modify: `crates/diskrement-shell/src/pipeline/reports.rs` (`StatusReport`, `VerifyReport`)
- Modify: `crates/diskrement-shell/src/error.rs` (disc errors carry names)
- Modify: `crates/diskrement-cli/src/render.rs` (`status_report`, `verify_report`, `history`, `mark_dead`), `crates/diskrement-cli/src/main.rs`
- Modify: `crates/diskrement-shell/tests/fixture_verify.rs`

- [ ] **Step 1: Change the reports and errors**

In `crates/diskrement-shell/src/pipeline/reports.rs`:

```rust
/// `StatusReport`. `open_discs` holds each copy set's open disc, if it has
/// one; `coverage` is indexed by copy set.
#[derive(Debug)]
pub struct StatusReport {
    pub discs: Vec<DiscState>,
    pub open_discs: Vec<DiscKey>,
    pub total_current_files: usize,
    pub last_burn_at: Option<DateTime<Utc>>,
    pub coverage: Vec<SetCoverage>,
}
```

In `VerifyReport`, replace `pub disc_ordinal: u32,` with `pub disc: DiscKey,`.

In `crates/diskrement-shell/src/error.rs`, change three variants to carry the disc's name as the operator sees it:

```rust
    /// A `--disc` or `mark-dead` argument that names no disc in this project.
    /// Carries the argument as typed (normalised to an upper-case letter), so
    /// `-1` and `9C` are echoed back.
    #[error("no disc with ordinal {0}")]
    UnknownDisc(String),
    /// A `--disc` naming a disc marked dead. A *sweep* skips dead discs
    /// quietly; naming one is an assertion of intent, and answering it with a
    /// reassuring exit 0 would be a lie.
    #[error("disc {0} is marked dead")]
    DiscIsDead(String),
    /// A `disc mark-dead` naming a disc that already carries a `MARKED_DEAD`
    /// event.
    #[error("disc {0} is already marked dead")]
    DiscAlreadyDead(String),
```

The messages are unchanged for a single-copy project, where a name is a bare ordinal.

- [ ] **Step 2: Write the failing pipeline tests**

The mark-dead test module in `pipeline/mod.rs` (around line 1800) has helpers `env()`, `cycle()`, `run_mark_dead(env, ordinal)` and `run_verify(env, disc)`. Change `run_mark_dead` to take `disc: &str` and call `pipeline.mark_dead("p", disc.parse().unwrap(), at(…))`. Change `run_verify` to take `disc: Option<&str>` and pass `disc.map(|d| d.parse().unwrap())`. Update their callers: `run_mark_dead(&mut env, 1)` becomes `run_mark_dead(&mut env, "1")`, and `run_verify(&mut env, Some(1))` becomes `run_verify(&mut env, Some("1"))`. Replace the error-variant assertions: `PipelineError::UnknownDisc(9)` becomes `PipelineError::UnknownDisc(ref s) if s == "9"`, and likewise for `DiscAlreadyDead(1)` and `DiscIsDead(1)` with `"1"`.

Then add a two-copy test module after the mark-dead module:

```rust
#[cfg(test)]
mod copies_tests {
    use super::*;
    use crate::burn::simulated::SimulatedBurner;
    use crate::progress::NoopSink;
    use chrono::TimeZone;
    use diskrement_core::{DiscArg, DiscKey, DiscType};

    struct Env {
        _dir: tempfile::TempDir,
        catalog: Catalog,
        work: PathBuf,
        source: PathBuf,
    }

    fn at(offset: i64) -> chrono::DateTime<Utc> {
        Utc.timestamp_opt(1_757_000_000 + offset, 0).unwrap()
    }

    fn paths(env: &Env) -> PipelinePaths {
        PipelinePaths {
            work_dir: env.work.clone(),
            scan_cache: env.work.join("scancache.sqlite"),
        }
    }

    /// A two-copy project with `a.bin` burned to 1A and 1B.
    fn burned_twice() -> Env {
        let dir = tempfile::TempDir::new().unwrap();
        let (source, work) = (dir.path().join("src"), dir.path().join("work"));
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(source.join("a.bin"), b"aaaa").unwrap();
        let mut env = Env {
            catalog: Catalog::open(&dir.path().join("cat.sqlite")).unwrap(),
            _dir: dir,
            work,
            source,
        };
        let pi = ProjectInit {
            name: "p".into(),
            source_root: env.source.display().to_string(),
            work_dir: env.work.display().to_string(),
            default_disc_type: DiscType::BdR,
            margin_percent: 2.0,
            copies: 2,
            created_at: at(0),
        };
        let p = paths(&env);
        let mut pipeline = Pipeline::new(&mut env.catalog, p, &NoopSink);
        pipeline.init(&pi).unwrap();
        pipeline.scan("p", false, false).unwrap();
        pipeline.plan("p", None, false).unwrap();
        let burner = Burner::Simulated(SimulatedBurner::new(&NoopSink, 2));
        pipeline.burn("p", at(1), &burner).unwrap();
        env
    }

    fn pipeline(env: &mut Env) -> Pipeline<'_> {
        let p = paths(env);
        Pipeline::new(&mut env.catalog, p, &NoopSink)
    }

    #[test]
    fn status_reports_each_sets_open_disc_and_full_coverage() {
        let mut env = burned_twice();
        let s = pipeline(&mut env).status("p").unwrap();
        assert_eq!(s.open_discs, vec![DiscKey::new(1, 0), DiscKey::new(1, 1)]);
        assert!(s.coverage.iter().all(|c| c.missing_files == 0));
    }

    #[test]
    fn marking_one_twin_dead_degrades_and_status_shows_the_gap() {
        let mut env = burned_twice();
        let report = pipeline(&mut env)
            .mark_dead("p", "1B".parse::<DiscArg>().unwrap(), at(2))
            .unwrap();
        assert_eq!(report.disc, DiscKey::new(1, 1));
        assert!(report.lost.is_empty() && report.reburn.is_empty());
        assert_eq!(report.degraded.len(), 1);
        let s = pipeline(&mut env).status("p").unwrap();
        assert_eq!(s.coverage[1].missing_files, 1);
    }

    #[test]
    fn verify_names_a_disc_by_set_and_refuses_a_letter_past_the_last_set() {
        let mut env = burned_twice();
        let verifier = Verifier::Staging(crate::burn::verifier::StagingVerifier::new(
            env.work.join("staging"),
        ));
        let one = pipeline(&mut env)
            .verify("p", Some("1B".parse().unwrap()), &verifier)
            .unwrap();
        assert_eq!(one.iter().map(|r| r.disc).collect::<Vec<_>>(), vec![DiscKey::new(1, 1)]);
        assert!(one[0].mismatches.is_empty() && !one[0].skipped);

        let err = pipeline(&mut env)
            .verify("p", Some("1C".parse().unwrap()), &verifier)
            .unwrap_err();
        assert!(matches!(err, PipelineError::UnknownDisc(ref s) if s == "1C"), "{err}");
    }
}
```

`scan` needs a CoW work dir. If the existing pipeline tests that call `scan` skip or fail on a non-CoW `TMPDIR`, follow whatever they do: look at how `scan_on_a_fresh_project_reports_all_added_and_stages_them` sets itself up and mirror it. If those tests simply require CoW, these do too, and the Global Constraints' CoW `TMPDIR` covers them.

- [ ] **Step 3: Run them to see them fail**

Run: `cargo test -p diskrement-shell pipeline`
Expected: compile errors (`mark_dead`'s and `verify`'s argument types, `open_discs`, `coverage`, `VerifyReport::disc`).

- [ ] **Step 4: Implement**

In `crates/diskrement-shell/src/pipeline/mod.rs`, add a free function near `same_filesystem`:

```rust
/// The disc `arg` names in this project, or `UnknownDisc` echoing the
/// argument. A bare ordinal means set A; a letter past the project's last set,
/// or an ordinal no disc has, is unknown.
fn resolve_disc(
    p: &Project,
    arg: DiscArg,
    states: &[DiscState],
) -> Result<DiscKey, PipelineError> {
    arg.resolve(p.copies)
        .filter(|k| states.iter().any(|s| s.key() == *k))
        .ok_or_else(|| PipelineError::UnknownDisc(arg.to_string()))
}
```

`status`: replace the `open_disc_ordinal` computation with

```rust
        // Each set's first open disc, as the single-copy report always showed
        // the first one it found.
        let mut open_discs: Vec<DiscKey> = Vec::new();
        for d in discs.iter().filter(|d| d.status == DiscStatus::Open) {
            if !open_discs.iter().any(|k| k.copy_set == d.copy_set) {
                open_discs.push(d.key());
            }
        }
```

and build the report with `open_discs, coverage: replication::coverage(&view, p.copies), total_current_files: view.current.len(), discs, last_burn_at`.

`verify(&mut self, name: &str, disc: Option<DiscArg>, verifier: &Verifier<'_>)`:

```rust
        let p = self.project(name)?;
        let states = self.catalog.read_disc_states(p.id).map_err(Self::failed)?;
        let sessions = self
            .catalog
            .sessions_by_state(p.id, SessionState::Burned)
            .map_err(Self::failed)?;

        let wanted = match disc {
            Some(arg) => {
                let key = resolve_disc(&p, arg, &states)?;
                if states.iter().any(|s| s.key() == key && s.status == DiscStatus::Dead) {
                    return Err(PipelineError::DiscIsDead(key.name(p.copies)));
                }
                Some(key)
            }
            None => None,
        };

        let targets: Vec<DiscKey> = states
            .iter()
            .filter(|s| s.status != DiscStatus::Dead)
            .map(DiscState::key)
            .filter(|k| wanted.is_none_or(|w| w == *k))
            .collect();

        let mut reports = Vec::with_capacity(targets.len());
        for disc in targets {
            reports.push(self.verify_disc(&p, disc, &sessions, verifier)?);
        }
        Ok(reports)
```

Replace the doc paragraph about `disc_ordinal` being `i64` with: "`disc` is the operator's `DiscArg`, whose ordinal stays signed so `--disc -1` is answered here, with exit 2, rather than refused by the parser."

`verify_disc(&mut self, p: &Project, disc: DiscKey, sessions: &[Session], verifier: &Verifier<'_>)`: use `disc` for `placements_for_disc`, `disc_recorded_contents` and `mark_verified`; the session filter is `s.disc == disc`; the expectation is `disc, disc_name: disc.name(p.copies)`; the report is `VerifyReport { disc, … }`. Remove every `DiscKey::new(ordinal, 0)` shim left by Tasks 8, 9 and 11.

`mark_dead(&mut self, name: &str, disc: DiscArg, at: DateTime<Utc>)`:

```rust
        let p = self.project(name)?;
        let states = self.catalog.read_disc_states(p.id).map_err(Self::failed)?;
        let key = resolve_disc(&p, disc, &states)?;
        if states.iter().any(|s| s.key() == key && s.status == DiscStatus::Dead) {
            return Err(PipelineError::DiscAlreadyDead(key.name(p.copies)));
        }
        self.catalog
            .mark_dead(p.id, key, &at)
            .map_err(Self::failed)
```

Replace the doc paragraph about `disc_ordinal` being `u32` with: "`disc` was validated by the parser as a positive ordinal with an optional set letter: mark-dead is irreversible, so a malformed argument never reaches the catalog."

Import `DiscArg`, `DiscKey` and `DiscState` from `diskrement_core`.

- [ ] **Step 5: Render the four reports**

In `crates/diskrement-cli/src/render.rs`:

`status_report(p: &Project, r: &StatusReport)`:

```rust
pub fn status_report(p: &Project, r: &StatusReport) -> String {
    // Two spaces after the project name; margin is %.1f plus a literal %.
    let copies = if p.copies > 1 {
        format!(", {} copies", p.copies)
    } else {
        String::new()
    };
    let mut out = format!(
        "Project {}  (source {}, {}, margin {:.1}%{copies})\n",
        p.name, p.source_root, p.default_disc_type, p.margin_percent
    );
    out.push_str("Discs:\n");
    for d in &r.discs {
        let open = if r.open_discs.contains(&d.key()) {
            "  <- open"
        } else {
            ""
        };
        out.push_str(&format!(
            "  #{:<3} {:<10} {} / {}{}\n",
            d.key().name(p.copies),
            d.status.as_str(),
            gb(d.used_bytes),
            gb(d.usable_bytes),
            open
        ));
    }
    if p.copies > 1 {
        out.push_str("Copies:\n");
        for (set, c) in r.coverage.iter().enumerate() {
            let letter = set_letter(set as u32);
            if c.missing_files == 0 {
                out.push_str(&format!("  set {letter}: complete\n"));
            } else {
                out.push_str(&format!(
                    "  set {letter}: {} files ({}) missing\n",
                    c.missing_files,
                    gb(c.missing_bytes)
                ));
            }
        }
    }
    let last = r
        .last_burn_at
        .as_ref()
        .map(time::format)
        .unwrap_or_else(|| "never".to_string());
    // Six spaces between the count and "Last burn:"; no trailing newline —
    // `main` prints with println!.
    out.push_str(&format!(
        "Current files: {}      Last burn: {}",
        r.total_current_files, last
    ));
    out
}
```

`verify_report(reports: &[VerifyReport], copies: u32)`: replace each `v.disc_ordinal` with `v.disc.name(copies)`.

`history(rel_path: &RelPath, entries: &[FileHistoryEntry], copies: u32)`: replace `e.disc.ordinal` with `e.disc.name(copies)`.

`mark_dead(r: &LostReport, copies: u32)`: the header becomes `format!("Disc {} marked DEAD.", r.disc.name(copies))`. After the re-burn block, before `lines.join`, append:

```rust
    if !r.degraded.is_empty() {
        lines.push(format!(
            "Short of a copy — restored on next scan/plan/burn ({}):",
            r.degraded.len()
        ));
        lines.extend(
            r.degraded
                .iter()
                .map(|e| format!("  {}", e.rel_path.as_str())),
        );
    }
    if r.superseded_survivors > 0 {
        lines.push(format!(
            "Superseded versions still held on another disc: {}",
            r.superseded_survivors
        ));
    }
```

Both additions print nothing in a single-copy project, so `mark_dead.snap` stays as it is.

`crates/diskrement-cli/src/main.rs`:
- `Command::Status`: unchanged (`status_report(&p, &report)`).
- `Command::Verify`: `pipeline.verify(&name, disc.map(|o| diskrement_core::DiscArg { ordinal: o, copy_set: None }), &verifier)?` and `render::verify_report(&reports, p.copies)`. Task 14 turns the argument itself into a `DiscArg`.
- `Command::History`: `render::history(&rel, &entries, p.copies)`.
- `Command::Disc`: `pipeline.mark_dead(&name, diskrement_core::DiscArg { ordinal: i64::from(ordinal), copy_set: None }, chrono::Utc::now())?` and `render::mark_dead(&report, p.copies)`.

Render tests and snapshot tests: pass `1` as the new `copies` argument. `StatusReport` literals use `open_discs: vec![DiscKey::new(2, 0)]` (or `vec![]`) in place of `open_disc_ordinal`, plus `coverage: vec![SetCoverage::default()]`. `VerifyReport` literals use `disc: DiscKey::new(N, 0)`. The expected strings and `.snap` files do not change. Add:

```rust
    #[test]
    fn a_two_copy_status_names_discs_by_set_and_shows_coverage() {
        let mut p = project();
        p.copies = 2;
        let d = |ordinal, copy_set, status| DiscState {
            ordinal,
            copy_set,
            status,
            usable_bytes: 25_000_000_000,
            used_bytes: 1_000_000_000,
        };
        let r = StatusReport {
            discs: vec![d(1, 0, DiscStatus::Open), d(1, 1, DiscStatus::Dead)],
            open_discs: vec![DiscKey::new(1, 0)],
            total_current_files: 2,
            last_burn_at: None,
            coverage: vec![
                SetCoverage::default(),
                SetCoverage { missing_files: 2, missing_bytes: 300_000_000 },
            ],
        };
        assert_eq!(
            status_report(&p, &r),
            "Project photos  (source /srv/photos, BD_R, margin 2.0%, 2 copies)\n\
             Discs:\n  \
             #1A  OPEN       1.0 GB / 25.0 GB  <- open\n  \
             #1B  DEAD       1.0 GB / 25.0 GB\n\
             Copies:\n  \
             set A: complete\n  \
             set B: 2 files (0.3 GB) missing\n\
             Current files: 2      Last burn: never"
        );
    }

    #[test]
    fn mark_dead_reports_degraded_files_and_surviving_superseded_versions() {
        let r = LostReport {
            disc: DiscKey::new(1, 1),
            lost: vec![],
            reburn: vec![],
            degraded: vec![ReburnEntry {
                rel_path: RelPath("a.bin".into()),
                content_hash: ContentHash("aa".into()),
                size_bytes: 4,
            }],
            superseded_survivors: 1,
        };
        assert_eq!(
            mark_dead(&r, 2),
            "Disc 1B marked DEAD.\n\
             No versions permanently lost.\n\
             No live files to re-burn.\n\
             Short of a copy — restored on next scan/plan/burn (1):\n  \
             a.bin\n\
             Superseded versions still held on another disc: 1"
        );
    }
```

Import `ReburnEntry` and `DiscKey` in the render test module where needed.

`crates/diskrement-shell/tests/fixture_verify.rs`: `sweep(env, disc: Option<i64>)` passes `disc.map(|o| DiscArg { ordinal: o, copy_set: None })`. The assertions become `PipelineError::DiscIsDead(ref s) if s == "3"` and `PipelineError::UnknownDisc(ref s) if s == "99"`, and `.disc_ordinal` on a report becomes `.disc.ordinal`.

- [ ] **Step 6: Run the tests**

Run: `cargo test --workspace`
Expected: PASS, with no `.snap.new` files.

- [ ] **Step 7: Verify and commit**

```bash
git add -A
git commit -m "feat(pipeline): resolve discs by set and report copy coverage

tests: <n> passed

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 14: CLI arguments: `--copies` range and `3B`

**Files:**
- Modify: `crates/diskrement-cli/src/args.rs`
- Modify: `crates/diskrement-cli/src/main.rs`

- [ ] **Step 1: Write the failing tests**

In `crates/diskrement-cli/src/args.rs` tests, append:

```rust
    fn init_with_copies(n: &str) -> Result<Cli, clap::Error> {
        Cli::try_parse_from([
            "diskrement", "init", "p", "--source", "/s", "--work-dir", "/w", "--copies", n,
        ])
    }

    #[test]
    fn copies_must_be_between_one_and_the_letter_count() {
        assert!(init_with_copies("1").is_ok());
        assert!(init_with_copies("26").is_ok());
        assert!(init_with_copies("0").is_err());
        assert!(init_with_copies("27").is_err());
        assert_eq!(diskrement_core::MAX_COPIES, 26, "the parser's range is spelled out");
    }

    #[test]
    fn mark_dead_takes_a_disc_name() {
        let cli = Cli::try_parse_from(["diskrement", "disc", "mark-dead", "p", "2b"]).unwrap();
        let Command::Disc { command: DiscCommand::MarkDead { disc, .. } } = cli.command else {
            panic!("not mark-dead")
        };
        assert_eq!(disc.to_string(), "2B");
    }

    #[test]
    fn mark_dead_refuses_a_non_positive_or_malformed_disc() {
        for bad in ["0", "-1", "2BB", "B2", "x"] {
            let err = Cli::try_parse_from(["diskrement", "disc", "mark-dead", "p", bad]).unwrap_err();
            assert!(
                err.to_string().contains("disc must be a positive ordinal with an optional set letter"),
                "{bad}: {err}"
            );
        }
    }

    #[test]
    fn verify_takes_a_disc_name_and_lets_a_negative_ordinal_through() {
        let cli = Cli::try_parse_from(["diskrement", "verify", "p", "--disc", "3B"]).unwrap();
        let Command::Verify { disc, .. } = cli.command else { panic!("not verify") };
        assert_eq!(disc.map(|d| d.to_string()), Some("3B".to_string()));
        let cli = Cli::try_parse_from(["diskrement", "verify", "p", "--disc", "-1"]).unwrap();
        let Command::Verify { disc, .. } = cli.command else { panic!("not verify") };
        assert_eq!(disc.map(|d| d.ordinal), Some(-1));
    }
```

Update the existing args tests that destructure `MarkDead { ordinal, .. }` or compare `disc: Option<i64>`. `ordinal` becomes `disc`, and comparisons go through `.to_string()` or `.ordinal`. A test asserting the old "disc ordinal must be a positive whole number" message switches to the new sentence.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p diskrement-cli args`
Expected: FAIL / compile errors.

- [ ] **Step 3: Implement**

In `crates/diskrement-cli/src/args.rs`:
- `Init.copies`: `#[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..=26))]`, with the doc comment `/// How many copies of every file to keep, each on its own disc series (1–26, one letter per set).`
- `Verify.disc`: `disc: Option<DiscArg>`. Keep `allow_negative_numbers = true`. Change the doc comment on `Verify` to: "`allow_negative_numbers` is deliberate: `--disc -1` must reach the domain, which answers with exit 2. Rejecting it here would be exit 1, and the exit ladder is contract (spec §3.2)."
- `MarkDead`: rename `ordinal: u32` to `disc: DiscArg` with `#[arg(value_parser = disc_name)]` and the doc comment `/// The disc to mark dead: its ordinal, plus its set letter in a multi-copy project (e.g. 3B).`
- Replace `disc_ordinal` with:

```rust
/// The `disc mark-dead` argument, validated at parse time: a positive ordinal
/// with an optional set letter. Mark-dead is irreversible, so a bad argument
/// is refused before anything opens the catalog — unlike `verify --disc`,
/// which hands a negative ordinal to the domain (exit 2).
fn disc_name(raw: &str) -> Result<DiscArg, String> {
    match raw.parse::<DiscArg>() {
        Ok(a) if a.ordinal > 0 && a.ordinal <= i64::from(i32::MAX) => Ok(a),
        _ => Err(format!(
            "disc must be a positive ordinal with an optional set letter (e.g. 3 or 3B), got {raw}"
        )),
    }
}
```

- Import `diskrement_core::DiscArg`.

In `crates/diskrement-cli/src/main.rs`:
- `Command::Verify { name, disc }`: `pipeline.verify(&name, disc, &verifier)?`.
- `Command::Disc { command: args::DiscCommand::MarkDead { name, disc, yes } }`: the prompt is `format!("Mark disc {disc} of {name} DEAD? Superseded versions on it are lost forever; its live files will re-plan.")`, and the call is `pipeline.mark_dead(&name, disc, chrono::Utc::now())?`.

- [ ] **Step 4: Run the tests**

Run: `cargo test --workspace`
Expected: PASS. `binary.rs`'s existing `--disc 99`, `--disc -1` and mark-dead tests keep their exit codes and messages.

- [ ] **Step 5: Verify and commit**

```bash
git add -A
git commit -m "feat(cli): bound --copies to 1-26 and accept disc names like 3B

tests: <n> passed

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 15: End-to-end: binary contract and a two-copy scenario

**Files:**
- Modify: `crates/diskrement-cli/tests/binary.rs`
- Modify: `crates/diskrement-cli/tests/scenarios/support.rs` (`disc_dir_named`)
- Create: `crates/diskrement-cli/tests/scenarios/s13_copies.rs`
- Modify: `crates/diskrement-cli/tests/scenarios/main.rs`

- [ ] **Step 1: Binary contract tests**

Append to `crates/diskrement-cli/tests/binary.rs`, in the module that holds the other `init` tests (search for `fn init_`), using its existing `project()`/`base()` helpers:

```rust
    #[test]
    fn copies_outside_one_to_twenty_six_is_a_usage_error() {
        for n in ["0", "27"] {
            let p = project();
            base(&p)
                .args(["init", "p", "--source"])
                .arg(&p.source)
                .arg("--work-dir")
                .arg(&p.work)
                .args(["--copies", n])
                .assert()
                .failure()
                .code(1);
        }
    }

    #[test]
    fn a_set_letter_past_the_projects_copies_is_an_unknown_disc() {
        let f = burned_project();
        run(&f, &["verify", "p", "--disc", "1B"])
            .failure()
            .code(2)
            .stderr(contains("no disc with ordinal 1B"));
    }
```

If `burned_project`/`run` live in a different module from `project`/`base`, put the second test in the verify module next to `verifying_a_named_disc_that_does_not_exist_exits_2`.

- [ ] **Step 2: Scenario support**

Add to `impl Scratch` in `crates/diskrement-cli/tests/scenarios/support.rs`:

```rust
    /// `<work>/staging/disc-<name>`, for a multi-copy project's `1A`, `1B`, …
    pub fn disc_dir_named(&self, name: &str) -> PathBuf {
        self.work.join(format!("staging/disc-{name}"))
    }
```

- [ ] **Step 3: The scenario**

Create `crates/diskrement-cli/tests/scenarios/s13_copies.rs`:

```rust
//! Scenario 13: two copy sets. Every cycle writes one disc per set; a burn
//! that fails on the second set keeps the first and resumes; a dead twin is
//! restored onto a fresh disc of its own set while the other set carries on.

use crate::{disc, support};

fn init_two_copies(cli: &support::Cli, s: &support::Scratch) {
    let r = cli.run(&[
        "init",
        "p",
        "--source",
        &s.source.display().to_string(),
        "--work-dir",
        &s.work.display().to_string(),
        "--copies",
        "2",
    ]);
    assert_eq!(r.code, 0, "init failed: {}{}", r.stdout, r.stderr);
}

fn scan_plan_burn(cli: &support::Cli) -> support::Output {
    let sc = cli.run(&["scan", "p"]);
    assert_eq!(sc.code, 0, "scan: {}{}", sc.stdout, sc.stderr);
    let pl = cli.run(&["plan", "p"]);
    assert_eq!(pl.code, 0, "plan: {}{}", pl.stdout, pl.stderr);
    cli.run(&["burn", "p", "--yes"])
}

fn names(m: &[diskrement_shell::burn::json::ManifestEntryJson]) -> Vec<&str> {
    m.iter().map(|e| e.rel_path.as_str()).collect()
}

#[test]
fn each_cycle_writes_one_disc_per_set_and_a_dead_twin_is_restored() {
    let Some(s) = support::scratch_or_skip("s13 two copies") else {
        return;
    };
    let cli = s.cli();
    support::write(&s.source, "a.txt", 128);
    support::write(&s.source, "b.txt", 128);
    init_two_copies(&cli, &s);

    // Cycle 1: both sets receive a and b. 1A records the versions; 1B
    // records copies of them.
    let b1 = scan_plan_burn(&cli);
    assert_eq!(b1.code, 0, "{}{}", b1.stdout, b1.stderr);
    assert!(b1.out_has("Wrote disc 1A"), "{}", b1.stdout);
    assert!(b1.out_has("Wrote disc 1B"), "{}", b1.stdout);
    assert!(b1.out_has("Copies: every current file has 2 copies."), "{}", b1.stdout);
    for name in ["1A", "1B"] {
        assert_eq!(
            disc::data_files(&s.disc_dir_named(name)),
            support::set(&["a.txt", "b.txt"]),
            "disc {name}"
        );
    }
    let m1b = disc::manifest(&s.disc_dir_named("1B"));
    assert_eq!(m1b.copy_set, 1);
    assert!(m1b.added.is_empty(), "1B adds nothing; 1A did");
    assert_eq!(names(&m1b.copied), vec!["a.txt", "b.txt"]);
    let id1b = disc::project_json(&s.disc_dir_named("1B"));
    assert_eq!((id1b.copy_set, id1b.copies), (1, 2));

    let st = cli.run(&["status", "p"]);
    assert!(st.out_has("#1A"), "{}", st.stdout);
    assert!(st.out_has("#1B"), "{}", st.stdout);
    assert!(st.out_has("set B: complete"), "{}", st.stdout);

    // Cycle 2: b changes, and 1B's burn fails — a regular file sits where its
    // staging directory goes. 1A is committed anyway.
    support::modify(&s.source, "b.txt", 128);
    let blocker = s.disc_dir_named("1B");
    std::fs::remove_dir_all(&blocker).unwrap();
    std::fs::write(&blocker, b"not a directory").unwrap();
    let b2 = scan_plan_burn(&cli);
    assert_eq!(b2.code, 6, "{}{}", b2.stdout, b2.stderr);
    assert!(
        b2.err_has("1A committed; 1B not written (re-run burn)"),
        "{}",
        b2.stderr
    );
    let st = cli.run(&["status", "p"]);
    assert!(st.out_has("set B: 1 files"), "{}", st.stdout);

    // Re-running burn alone writes only what is left.
    std::fs::remove_file(&blocker).unwrap();
    let b3 = cli.run(&["burn", "p", "--yes"]);
    assert_eq!(b3.code, 0, "{}{}", b3.stdout, b3.stderr);
    assert!(b3.out_has("Wrote disc 1B"), "{}", b3.stdout);
    assert!(!b3.out_has("Wrote disc 1A"), "{}", b3.stdout);
    assert!(cli.run(&["status", "p"]).out_has("set B: complete"));

    // 1B dies. Every version on it is still on 1A: nothing is lost or
    // re-burned; the current ones are short a copy.
    let dead = cli.run(&["disc", "mark-dead", "p", "1B", "--yes"]);
    assert_eq!(dead.code, 0, "{}{}", dead.stdout, dead.stderr);
    assert!(dead.out_has("Disc 1B marked DEAD."), "{}", dead.stdout);
    assert!(dead.out_has("No versions permanently lost."), "{}", dead.stdout);
    assert!(dead.out_has("No live files to re-burn."), "{}", dead.stdout);
    assert!(
        dead.out_has("Short of a copy — restored on next scan/plan/burn (2):"),
        "{}",
        dead.stdout
    );
    assert!(
        dead.out_has("Superseded versions still held on another disc: 1"),
        "{}",
        dead.stdout
    );

    // Recovery: set B's next disc, 2B, carries the two catch-up copies; set A
    // has nothing to do.
    let sc = cli.run(&["scan", "p"]);
    assert!(
        sc.out_has("Scanned: 0 added, 0 modified, 0 deleted, 2 unchanged."),
        "{}",
        sc.stdout
    );
    let pl = cli.run(&["plan", "p"]);
    assert!(
        pl.out_has("Plan: disc 2B (new disc, BD_R) — 2 files (0 new, 2 catch-up)"),
        "{}",
        pl.stdout
    );
    assert!(!pl.out_has("disc 1A"), "{}", pl.stdout);
    let b4 = cli.run(&["burn", "p", "--yes"]);
    assert_eq!(b4.code, 0, "{}{}", b4.stdout, b4.stderr);
    assert!(b4.out_has("Wrote disc 2B"), "{}", b4.stdout);
    assert_eq!(
        disc::data_files(&s.disc_dir_named("2B")),
        support::set(&["a.txt", "b.txt"])
    );

    let v = cli.run(&["verify", "p"]);
    assert_eq!(v.code, 0, "{}{}", v.stdout, v.stderr);
    assert!(v.line_starts_with("disc 1A:"), "{}", v.stdout);
    assert!(v.line_starts_with("disc 2B:"), "{}", v.stdout);
    assert!(!v.line_starts_with("disc 1B:"), "{}", v.stdout);
    let named = cli.run(&["verify", "p", "--disc", "1B"]);
    assert_eq!(named.code, 2);
    assert!(named.err_has("disc 1B is marked dead"), "{}", named.stderr);
}
```

Add `mod s13_copies;` to `crates/diskrement-cli/tests/scenarios/main.rs`, after `mod s12_realistic_tree;`. If `ManifestEntryJson` is not reachable as `diskrement_shell::burn::json::ManifestEntryJson`, check how `disc.rs` imports `ManifestJson` and use the same path.

- [ ] **Step 4: Run with scenarios required**

Run:
```bash
DISKREMENT_SCENARIOS_REQUIRED=1 TMPDIR=<a btrfs/XFS dir> cargo test -p diskrement-cli
```
Expected: PASS, with no `SKIPPED` line in the output. If a scenario fails, use superpowers:systematic-debugging. Do not loosen an assertion to make it pass.

- [ ] **Step 5: Verify and commit**

Run the three Global Constraints commands as well, then:
```bash
git add -A
git commit -m "test(cli): pin the copies contract and a two-copy recovery scenario

tests: <n> passed (scenarios run with DISKREMENT_SCENARIOS_REQUIRED=1)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 16: Docs, deviations, final verification and merge

**Files:**
- Modify: `CLAUDE.md`
- Modify: `crates/diskrement-shell/src/catalog/mod.rs` (`raw_connection_for_tests` doc)
- Modify: `docs/superpowers/specs/2026-10-11-multiple-copies-design.md` (deviations)
- Modify: `docs/superpowers/plans/2026-10-11-multiple-copies.md` (execution deviations)

- [ ] **Step 1: CLAUDE.md**

Under "Invariants — do not "fix" these":
- Replace the **One disc per cycle** bullet with:

```markdown
- **One disc per copy set per cycle.** Each scan→plan→burn writes at most one disc/session per copy set (`copies` sets, shown as letters: 1A/1B). `burn` writes the sets in order and commits each disc as soon as it is burned; the staged plan is rewritten to what is left after every commit, so a failure after the first commit is a `PartialBurn` that a re-run of `burn` resumes. `plan --finalize` seals every set's open disc.
```

- Add a bullet after **`LOST` is per physical copy…**:

```markdown
- **A copy placement has no version of its own.** A disc that holds another copy of a current version records one `placement` (its `copy_index` is its disc's `copy_set`) and no `file_version` or `change_record`. Classification is done per disc at commit time against the live catalog, so 1B's files become copies of what 1A just recorded. A version with no live copy anywhere is still re-added as a new version (REBURN).
```

- In the "Known deferred debt" paragraph, delete "`copies>1` redundancy degrades silently". Keep the other items.
- In "Gotchas", add:

```markdown
- **Disc names depend on `copies`.** `DiscKey::name(copies)` is `3` in a single-copy project and `3B` otherwise; labels, staging dirs (`staging/disc-3B`), volids, prompts and reports all go through it. Never format a disc ordinal by hand.
```

- [ ] **Step 2: Stale comments**

In `crates/diskrement-shell/src/catalog/mod.rs`, replace the `raw_connection_for_tests` doc with:
> Direct access to the underlying connection, for tests that need raw SQL: schema assertions, row counts, and the occasional placement a test inserts by hand. Not part of the operational API — every caller in `src/` goes through a named method.

Run `grep -rn "copies > 1\|copies>1\|until .copies" crates --include=*.rs`. Rewrite or delete any comment that still describes copies as unimplemented.

- [ ] **Step 3: Record deviations**

Append to the spec, as a new last section:

```markdown
## 10. Deviations recorded during planning

- **`placement.copy_index` is enforced by construction, not checked.**
  `FileWrite` has no `copy_index`; the store writes the disc's `copy_set`, so a
  mismatch cannot be expressed (§5.3).
- **The staged plan carries counts, not per-file tags.** Each set entry holds
  `catchUp` (how many of its files are catch-up copies) and `sessionOrdinal`
  (the session the burn writes, used to skip a disc committed before the plan
  was rewritten). Files are classified at commit time anyway (§7.2), so a tag
  would never be read (§6.1).
- **`pointers.json` gains `copySet`.** A pointer naming only an ordinal is
  ambiguous between twins; this narrows D5 without touching the known
  dead-flag debt.
- **`planner::plan` remains** as the single-set wrapper over `plan_set`, so the
  ported planner tests and properties keep their meaning.
- **`-1B` is refused by `DiscArg` parsing everywhere**, and a bare `-1` still
  reaches the domain for `verify --disc` (exit 2).
```

Append an `## Execution deviations` section at the end of this plan. List anything the implementation did differently from these tasks, one bullet each, saying what and why. Write "None." if there were none.

- [ ] **Step 4: Final verification**

Use superpowers:verification-before-completion. Run:

```bash
cargo test --workspace 2>&1 | grep -E '^test result' | awk '{s+=$4; f+=$6} END {print "passed:", s, "failed:", f}'
DISKREMENT_SCENARIOS_REQUIRED=1 TMPDIR=<a btrfs/XFS dir> cargo test -p diskrement-cli 2>&1 | grep -E 'SKIPPED|test result'
cargo test -p diskrement-shell --test xorriso_image 2>&1 | grep -E 'skip|test result'
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
cargo build --release
```

Expected: 0 failed; no `SKIPPED` scenario line; xorriso tests reported as run, or their skip reason stated; clippy and fmt clean; release build succeeds. Compare the passed count against Task 1's. It must be higher, by at least the tests these tasks added.

- [ ] **Step 5: Commit the docs**

```bash
git add -A
git commit -m "docs: record copy-set invariants and planning deviations

Closes #1.

tests: <n> passed

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 6: Merge (ask the user first)**

Use superpowers:finishing-a-development-branch. Following CLAUDE.md's etiquette, once the user agrees:

```bash
git switch main
git merge --no-ff feat/multiple-copies -m "Merge feat/multiple-copies: enforced redundant copies, one disc per copy set per cycle

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git branch -d feat/multiple-copies
git push origin main
git push origin --delete feat/multiple-copies   # only if the branch was pushed
```

## Execution deviations

- **Task 8 → 10:** `file_history` orders by `fv.id, d.copy_set, d.ordinal, s.ordinal` (the plan's `fv.id, d.copy_set` is not a total order once one version has several placements), and a two-set `live_sets` test (including a dead set dropping out) was added with Task 10, once copy placements could be written.
- **Task 10:** the plan's degraded test called `also_place_a_on_disc_2` straight after `seed`, but `seed` writes only disc 1, so the helper's session subquery is NULL and the insert fails. The test first burns disc 2 (`update(p, 2, 1, [c])`), as the existing `a_version_alive_on_another_disc_is_neither_lost_nor_reburned` does, and asserts `a@a1` is current before the disc dies. A companion test pins `superseded_survivors` (a superseded `b@b1` with a live copy on disc 2 is neither lost nor recorded). The pipeline test asserting `report.disc_ordinal == 1` (no stated rule) now asserts `report.disc == DiscKey::new(1, 0)`.
- **Task 11:** the existing wrong-disc burner test asserts the session-1 "written media" line, which a twin cannot trigger. The wrong-twin test is therefore a top-off (session 2, set A expected) offered the set-B twin at session 1; it asserts `Not the expected disc: found disc 'photos' #1B at session 1. Swap discs and confirm again.`, `Aborted` once the operator declines, and that nothing was committed. The burner's mismatch description uses `found.disc_name()`, computed from the *found* disc's own `copies`. `DiscIdentity`'s Display keeps its Scala-`toString` doc note alongside the plan's wording. `verify_disc` still uses the `DiscKey::new(ordinal, 0)` shim until Task 13.
- **Task 10 (review fix):** `degraded` counts a current version only when no live copy remains in the dying disc's copy set (`disc_versions` gains an `other_live_same_set` column). The plan's "another live copy anywhere" promised a restore the planner never makes when the surviving copy is in the same set, which could happen even at copies=1. A current version still covered within its own set goes in no bucket. The `lost_version` LOST/REBURN split and `superseded_survivors` still key on any other live copy. Raw test inserts now use a `copy_index` equal to their disc's copy set, and the degraded test writes its 1B copy through the `copied` path. Spec §8.1 is clarified to match. Separately, from the Task 9 review, `write_disc`'s "is not OPEN" error now names the set.
- **Task 11 hardening:** `DiscIdentity::disc_name()` no longer panics on identities read off media: when `copies > 1` and `copy_set >= MAX_COPIES` (a corrupt `project.json`) it renders `<ordinal>?<copy set>` (e.g. `3?26`), so the burner describes and re-prompts instead of exiting 5; `DiscKey::name`/`set_letter` are unchanged. `volid` now truncates only the project-name part, so the `_DISC_<name>` suffix always survives the 32-character cap and twins of a long-named project keep distinct volume ids; short-name volids are unchanged.
- **Task 12:** three existing planning tests (`finalize_of_an_unchanged_tree_seals_the_open_disc_with_no_files`, `a_file_that_cannot_top_off_the_open_disc_opens_a_fresh_one_this_cycle`, `a_no_op_scan_plans_an_empty_disc_rather_than_panicking`) built their "already burned" view with `CatalogView::from_scanned`, whose `live_sets` are empty, so `shortfall` now (correctly) re-placed every unchanged file as a set-A catch-up copy. They build the view with the task's own `live_in(files, &[0])` helper instead, which is what they always meant: the files are live on a set-A disc. Their assertions are unchanged. The staged plan's field-spelling test now asserts the v2 paths (`copies`, `sets[0].disc.copySet`, `sets[0].catchUp`, `sets[0].sessionOrdinal`, `sets[0].remaining.*`) rather than the v1 top-level `disc`/`remaining`. `disc_write`'s body uses the `disc_type` shorthand (clippy `redundant_field_names`).
- **Task 12 (review fixes):**
  - **Atomic staged files.** `staged::write_json` (shared by `scan.json` and `plan.json`) writes `<file>.tmp`, calls `sync_all`, renames it over the target and fsyncs `staged/`. `burn` rewrites `plan.json` right after each commit, exactly in the crash window, and a truncated plan would have exited 5 on re-run. `clear` also removes a leftover `*.tmp`, so a crash before the rename cannot make `clear` trip over a non-empty `staged/`.
  - **`PartialBurn` gains `unrecorded: Option<String>`.** This is a disc that was physically written but whose `apply_update` failed (`burn_disc` now returns a private `DiscFailure::{Unwritten, Unrecorded}`). It is reported as "1B written but not recorded", never as "not written".
  - **Advice follows the cause** (`error::partial_burn_advice`):
    - `StalePlan` → "(re-run plan, then burn)".
    - `SnapshotMissing` → "(re-run scan --force, then plan and burn)". The coordinator suggested plain `plan` here, but `plan` cannot recreate the snapshot; only a fresh scan does.
    - Anything else → "(re-run burn)".
    - If every disc committed and only the cleanup failed → "(nothing is left to write)", and the "not written" part is left out because nothing is pending.
    - No advice ever says "reset".
  - **`burn` wraps late failures in `PartialBurn`.** A failed plan rewrite after a commit, and a failed `clear`/`read_view` after the last commit, now carry the committed discs instead of a bare `Failed`. A failure before any commit is still the raw error.
  - **The CLI shares the summary.** The Aborted arm in `error_line` now uses `partial_burn_summary`.
  - **The tombstone filter uses `Catalog::fingerprints`** ("latest change is not DELETED", not filtered by dead discs) instead of `read_view`. A tombstone whose version lost its last live copy between `plan` and `burn` is now recorded, as the manifest says it is. At copies=1 this is the pre-Task-12 behaviour.
  - **`coverage_lines` on an empty coverage prints nothing**, instead of claiming "0 copies".
  - **New tests:**
    - Atomic replace: a new inode and no `.tmp` left behind.
    - `clear` removes `.tmp` files.
    - Three new error tests for the cause-based advice, `unrecorded`, and the empty pending list.
    - A copies=2 deletion records exactly one DELETED, on 1A's session 2, and both manifests list it. A `|_| true` filter now fails this test.
    - A tombstone survives its version's last copy dying.
    - A plan-rewrite failure is a `PartialBurn` that resumes without re-burning 1A.
    - A cleanup failure names both discs and says nothing about "not written".
    - A commit failure, forced with a temp trigger on the live connection, reports 1B as unrecorded.
    - Render: an empty coverage prints no "Copies:" line.
    - The crash-window test now also asserts that the disc written is 1B.
