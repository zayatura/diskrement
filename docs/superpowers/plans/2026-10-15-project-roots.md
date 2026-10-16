# Project Roots Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A project may archive several named directories ("roots") onto one disc
series. Each root sits under its name on every disc, and discs fill by the path *below*
each root, so Immich libraries laid out as `YYYY/YYYY-MM-DD/<file>` interleave by date.

**Architecture:** `diskrement-core` gains `RootName`, `Sources` (`Single(path)` or
`Rooted([(name, path)])`) and `FillOrder`, whose `key` sorts a rooted project's files by
*(path below the root, root name)*. `plan_set` takes a `&FillOrder`. The snapshot
reflinks each root to `<work_dir>/snapshot/<name>`, so scanning, the catalog, staging
and verify see ordinary `rel_path`s and do not change. The catalog stores roots in a new
`project_root` table (schema V4). `Project` and `ProjectInit` carry `sources: Sources`
in place of `source_root: String`. `init` takes `--root NAME=PATH` (repeated) as an
alternative to `--source`.

**Tech Stack:** Rust workspace (`diskrement-core` → `diskrement-shell` →
`diskrement-cli`), clap derive, rusqlite, proptest, insta, assert_cmd, and the CoW
scenario harness (`crates/diskrement-cli/tests/scenarios/`).

**Spec:** `docs/superpowers/specs/2026-10-15-project-roots-design.md`
**Branch:** `feat/project-roots` (already created; the spec is committed there as `7c6553a`).

---

## Ground rules for every task

- **Verified** means all three of these pass:

  ```bash
  DISKREMENT_SCENARIOS_REQUIRED=1 cargo test --workspace
  cargo clippy --workspace --all-targets -- -D warnings
  cargo fmt --check
  ```

  `DISKREMENT_SCENARIOS_REQUIRED=1` turns a skipped CoW scenario into a failure. Warnings
  are errors (`.cargo/config.toml`), so an unused import fails the build.
- **Baseline counts at `7c6553a`:**
  - `diskrement-core` 82;
  - `diskrement-shell`: lib 373, `fixture_catalog` 8, `fixture_verify` 6,
    `timestamps` 2, `xorriso_image` 5;
  - `diskrement-cli`: bin unit 107, `binary` 46, `scenarios` 20.

  Every task's report states its counts. Any change must be explained by tests the task
  added or deleted. Each task lists the counts it expects.
- **Commits:** conventional with a scope, ending with the trailer
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. One commit per task.
- **Line numbers** refer to the tree at `7c6553a`. They drift as tasks land, so find
  each site by the quoted code, not by the number.
- **Never** touch `fixtures/`. Never open a fixture catalog with `Catalog::open`: copy
  it first (`support::fixture_copy` does this).
- Run `cargo fmt` before committing; the code blocks below are close to rustfmt's
  output but not guaranteed to match it.
- If a step's code does not compile as written, fix it in the smallest way that keeps
  the step's intent, and record the deviation in the "Execution notes" section at the
  end of this file (Task 8 creates that section; until then, keep a note for it).

---

## File map

| File | Change |
|---|---|
| `crates/diskrement-core/src/roots.rs` | **New.** `RootName`, `Sources`, `RootsError`, `FillOrder` |
| `crates/diskrement-core/src/lib.rs` | `pub mod roots;` and re-exports |
| `crates/diskrement-core/src/planner.rs` | `plan_set` takes `&FillOrder`; fill-order tests and proptests |
| `crates/diskrement-core/src/testutil.rs` | `ROOTS`, `rooted_planner_input_strategy` |
| `crates/diskrement-shell/src/pipeline/planning.rs` | `plan_from` takes `&FillOrder` |
| `crates/diskrement-shell/src/fs/snapshot.rs` | `create`/`create_with` take `&Sources`; rooted snapshots |
| `crates/diskrement-shell/src/catalog/schema.rs` | V4: `project_root` |
| `crates/diskrement-shell/src/catalog/model.rs` | `ProjectInit`/`Project`: `sources: Sources` |
| `crates/diskrement-shell/src/catalog/store.rs` | `create_project` writes roots; `find_project` reads them |
| `crates/diskrement-shell/src/catalog/{mod.rs,testutil.rs}` | V4 assertions; `project_init` |
| `crates/diskrement-shell/tests/fixture_catalog.rs` | version 4; a V2 catalog reads as `Sources::Single` |
| `crates/diskrement-shell/src/error.rs` | `PipelineError::OverlappingRoots` (exit 3) |
| `crates/diskrement-shell/src/pipeline/mod.rs` | `init` checks every root; snapshot and fill order from `p.sources` |
| `crates/diskrement-shell/src/pipeline/roots_tests.rs` | **New.** Rooted `init` checks and a full cycle |
| `crates/diskrement-cli/src/args.rs` | `--root NAME=PATH`; `Cli::try_parse_checked_from`; `init_sources` |
| `crates/diskrement-cli/src/main.rs` | parse with the checks; build `Sources` |
| `crates/diskrement-cli/src/render.rs` | `status` header and `Roots:` block |
| `crates/diskrement-cli/tests/binary.rs` | `mod roots` |
| `crates/diskrement-cli/tests/scenarios/s15_roots.rs` | **New.** End-to-end scenario |
| `README.md`, `CLAUDE.md` | Docs |

Task order keeps every commit green: core types (1), the planner's order (2), the
snapshot (3), the catalog and the `Project` type (4), `init`'s checks and the pipeline
cycle (5), the CLI flag (6), the scenario (7), docs (8).

---

### Task 1: Core types — `RootName`, `Sources`, `FillOrder`

**Files:**
- Create: `crates/diskrement-core/src/roots.rs`
- Modify: `crates/diskrement-core/src/lib.rs`

- [ ] **Step 1: Write the module with its tests**

Create `crates/diskrement-core/src/roots.rs`:

```rust
//! Named source roots, and the order they imply for filling discs.
//!
//! A project archives one directory (`Sources::Single`) or several, each under
//! a name (`Sources::Rooted`). A rooted project's snapshot holds every root at
//! `snapshot/<name>`, so from the scan onwards its files are ordinary
//! `rel_path`s such as `family/anna/2024/2024-01-15/a.jpg`. Only the planner
//! needs to know roots exist: it fills discs by *(path below the root, root
//! name)*, so the roots interleave instead of following one another
//! (project-roots spec §5).

use std::path::{Path, PathBuf};

use crate::model::RelPath;

/// A root's name: the relative directory its files sit under on every disc.
///
/// One or more `/`-separated components. No component may be empty, `.` or
/// `..`, and the name may not contain NUL or `=` (`=` separates the name from
/// the path in `--root NAME=PATH`). Anything else is allowed, spaces and
/// non-ASCII included.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RootName(String);

impl RootName {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether `self` is `other` or one of its ancestor directories, compared
    /// by whole components: `family` holds `family/anna`, not `familyx`.
    fn holds(&self, other: &RootName) -> bool {
        other.0 == self.0
            || other
                .0
                .strip_prefix(self.0.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
    }
}

impl std::str::FromStr for RootName {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err("a root name cannot be empty".to_string());
        }
        if s.contains('\0') {
            return Err(format!("root name {s:?} contains NUL"));
        }
        if s.contains('=') {
            return Err(format!(
                "root name {s:?} contains '=', which separates a name from its path"
            ));
        }
        for component in s.split('/') {
            match component {
                "" => {
                    return Err(format!(
                        "root name {s:?} has an empty component \
                         (a leading, trailing or doubled '/')"
                    ))
                }
                "." | ".." => {
                    return Err(format!("root name {s:?} has a {component:?} component"))
                }
                _ => {}
            }
        }
        Ok(RootName(s.to_string()))
    }
}

impl std::fmt::Display for RootName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a project archives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sources {
    /// One directory, archived as the root of every disc.
    Single(PathBuf),
    /// Named directories, each archived under its name. Sorted by name, and no
    /// name holds another: build it with [`Sources::rooted`].
    Rooted(Vec<(RootName, PathBuf)>),
}

impl Sources {
    /// Named roots, sorted by name. Refuses an empty list, a name given twice,
    /// and a name that holds another (`family` with `family/anna`), whose files
    /// would land inside the other's directory.
    ///
    /// Only the names are checked here. Whether two *paths* overlap depends on
    /// the filesystem, so `Pipeline::init` checks that.
    pub fn rooted(mut roots: Vec<(RootName, PathBuf)>) -> Result<Sources, RootsError> {
        if roots.is_empty() {
            return Err(RootsError::Empty);
        }
        roots.sort_by(|a, b| a.0.cmp(&b.0));
        for (i, (a, _)) in roots.iter().enumerate() {
            for (b, _) in &roots[i + 1..] {
                if a == b {
                    return Err(RootsError::Duplicate(a.clone()));
                }
                // Sorted, so an ancestor always comes before its descendants
                // (a proper prefix sorts first), though not always right
                // before them: `a` < `a-b` < `a/c`.
                if a.holds(b) {
                    return Err(RootsError::Nested {
                        outer: a.clone(),
                        inner: b.clone(),
                    });
                }
            }
        }
        Ok(Sources::Rooted(roots))
    }

    /// Every directory this project archives; in name order when rooted.
    pub fn paths(&self) -> Vec<&Path> {
        match self {
            Sources::Single(path) => vec![path.as_path()],
            Sources::Rooted(roots) => roots.iter().map(|(_, path)| path.as_path()).collect(),
        }
    }
}

/// Why a set of root names was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootsError {
    Empty,
    Duplicate(RootName),
    Nested { outer: RootName, inner: RootName },
}

impl std::fmt::Display for RootsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RootsError::Empty => f.write_str("a project needs at least one root"),
            RootsError::Duplicate(name) => write!(f, "root name {name} is given twice"),
            RootsError::Nested { outer, inner } => write!(
                f,
                "root {inner} would sit inside root {outer}; root names must not nest"
            ),
        }
    }
}

impl std::error::Error for RootsError {}

/// The order the planner places files in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FillOrder {
    /// A single-source project: by `rel_path`.
    Path,
    /// A rooted project: by the path below the root, then by root name, so the
    /// roots interleave. The names are those of a [`Sources::Rooted`].
    Rooted(Vec<RootName>),
}

impl From<&Sources> for FillOrder {
    fn from(sources: &Sources) -> Self {
        match sources {
            Sources::Single(_) => FillOrder::Path,
            Sources::Rooted(roots) => {
                FillOrder::Rooted(roots.iter().map(|(name, _)| name.clone()).collect())
            }
        }
    }
}

impl FillOrder {
    /// The sort key of `path`: `(path below its root, root name)`. Under
    /// `Path`, and for a path under no root (which a rooted snapshot cannot
    /// produce), it is `(path, "")`.
    ///
    /// No two paths share a key: no name holds another, so a path has at most
    /// one root, and the key spells the path back out. The order is therefore
    /// total, and a plan still depends only on the set of files, not on the
    /// order they arrive in.
    pub fn key<'a>(&self, path: &'a RelPath) -> (&'a str, &'a str) {
        let p = path.as_str();
        match self {
            FillOrder::Path => (p, ""),
            FillOrder::Rooted(names) => names
                .iter()
                .find_map(|name| {
                    let rest = p.strip_prefix(name.as_str())?.strip_prefix('/')?;
                    Some((rest, &p[..name.as_str().len()]))
                })
                .unwrap_or((p, "")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(s: &str) -> RootName {
        s.parse().unwrap()
    }

    fn rel(s: &str) -> RelPath {
        RelPath(s.to_string())
    }

    #[test]
    fn a_root_name_may_have_several_components_spaces_and_accents() {
        for ok in ["anna", "family/anna", "Ilona Dóra", "a.b/c-d_e"] {
            assert_eq!(ok.parse::<RootName>().unwrap().as_str(), ok);
        }
    }

    #[test]
    fn a_root_name_refuses_empty_components_dots_nul_and_equals() {
        for bad in [
            "",
            "/anna",
            "anna/",
            "family//anna",
            ".",
            "..",
            "family/../x",
            "./x",
            "a\0b",
            "a=b",
        ] {
            assert!(bad.parse::<RootName>().is_err(), "{bad:?} was accepted");
        }
    }

    #[test]
    fn rooted_sorts_by_name() {
        let s = Sources::rooted(vec![
            (name("others/cecil"), "/c".into()),
            (name("family/anna"), "/a".into()),
        ])
        .unwrap();
        assert_eq!(
            s,
            Sources::Rooted(vec![
                (name("family/anna"), "/a".into()),
                (name("others/cecil"), "/c".into()),
            ])
        );
    }

    #[test]
    fn rooted_refuses_no_roots_and_a_name_given_twice() {
        assert_eq!(Sources::rooted(vec![]), Err(RootsError::Empty));
        assert_eq!(
            Sources::rooted(vec![(name("anna"), "/a".into()), (name("anna"), "/b".into())]),
            Err(RootsError::Duplicate(name("anna")))
        );
    }

    #[test]
    fn rooted_refuses_a_name_inside_another_but_not_a_shared_prefix() {
        assert_eq!(
            Sources::rooted(vec![
                (name("family/anna"), "/a".into()),
                (name("family"), "/f".into()),
            ]),
            Err(RootsError::Nested {
                outer: name("family"),
                inner: name("family/anna"),
            })
        );
        // `a` < `a-b` < `a/c`: once sorted, the ancestor is not adjacent.
        assert_eq!(
            Sources::rooted(vec![
                (name("a"), "/1".into()),
                (name("a-b"), "/2".into()),
                (name("a/c"), "/3".into()),
            ]),
            Err(RootsError::Nested {
                outer: name("a"),
                inner: name("a/c"),
            })
        );
        assert!(Sources::rooted(vec![
            (name("family"), "/f".into()),
            (name("familyx"), "/x".into()),
        ])
        .is_ok());
    }

    #[test]
    fn paths_lists_every_directory_in_name_order() {
        let s = Sources::rooted(vec![
            (name("bela"), "/lib/u2".into()),
            (name("anna"), "/lib/u1".into()),
        ])
        .unwrap();
        assert_eq!(s.paths(), vec![Path::new("/lib/u1"), Path::new("/lib/u2")]);
        assert_eq!(
            Sources::Single("/src".into()).paths(),
            vec![Path::new("/src")]
        );
    }

    #[test]
    fn a_single_source_fills_by_path() {
        assert_eq!(
            FillOrder::from(&Sources::Single("/src".into())),
            FillOrder::Path
        );
        assert_eq!(
            FillOrder::Path.key(&rel("family/anna/2021/a.jpg")),
            ("family/anna/2021/a.jpg", "")
        );
    }

    fn three_roots() -> FillOrder {
        FillOrder::from(
            &Sources::rooted(vec![
                (name("family/anna"), "/a".into()),
                (name("family/bela"), "/b".into()),
                (name("cecil"), "/c".into()),
            ])
            .unwrap(),
        )
    }

    #[test]
    fn roots_at_different_depths_interleave_by_the_path_below_them() {
        let order = three_roots();
        let mut paths = vec![
            rel("family/anna/2023/x.jpg"),
            rel("cecil/2022/y.jpg"),
            rel("family/bela/2020/z.jpg"),
            rel("family/anna/2021/w.jpg"),
        ];
        paths.sort_by(|a, b| order.key(a).cmp(&order.key(b)));
        assert_eq!(
            paths,
            vec![
                rel("family/bela/2020/z.jpg"),
                rel("family/anna/2021/w.jpg"),
                rel("cecil/2022/y.jpg"),
                rel("family/anna/2023/x.jpg"),
            ]
        );
    }

    #[test]
    fn the_same_path_below_two_roots_falls_back_to_root_name() {
        let order = three_roots();
        assert_eq!(
            order.key(&rel("family/bela/2020/a.jpg")),
            ("2020/a.jpg", "family/bela")
        );
        assert!(order.key(&rel("cecil/2020/a.jpg")) < order.key(&rel("family/anna/2020/a.jpg")));
    }

    #[test]
    fn a_name_matches_whole_components_only() {
        let order = FillOrder::from(&Sources::rooted(vec![(name("family"), "/f".into())]).unwrap());
        assert_eq!(order.key(&rel("familyx/a.jpg")), ("familyx/a.jpg", ""));
        assert_eq!(order.key(&rel("family/a.jpg")), ("a.jpg", "family"));
    }

    #[test]
    fn a_path_under_no_root_keys_on_the_whole_path() {
        assert_eq!(three_roots().key(&rel("stray.txt")), ("stray.txt", ""));
    }
}
```

- [ ] **Step 2: Wire the module into the crate**

In `crates/diskrement-core/src/lib.rs`, add `pub mod roots;` after `pub mod replication;`,
and add the re-export after the `pub use replication::…;` line:

```rust
pub use roots::{FillOrder, RootName, RootsError, Sources};
```

Also update the crate doc comment's list of what the core owns. Change
`//! planning, and manifest models (spec §5.3).` to
`//! planning, manifest and source-root models (spec §5.3).`

- [ ] **Step 3: Run the core tests**

Run: `cargo test -p diskrement-core roots`
Expected: PASS, 11 tests.

- [ ] **Step 4: Verify and commit**

Run the three verification commands. Expected counts: core 93 (82 + 11); everything
else unchanged.

```bash
git add crates/diskrement-core/src/roots.rs crates/diskrement-core/src/lib.rs
git commit -m "feat(core): root names, sources and the fill order they imply

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: The planner fills in a `FillOrder`

**Files:**
- Modify: `crates/diskrement-core/src/planner.rs` (`plan`, `plan_set`, tests)
- Modify: `crates/diskrement-core/src/testutil.rs`
- Modify: `crates/diskrement-shell/src/pipeline/planning.rs` (`plan_from`, tests)
- Modify: `crates/diskrement-shell/src/pipeline/mod.rs` (`plan_scanned`, 3 test calls)

- [ ] **Step 1: Write the failing planner example tests**

In `crates/diskrement-core/src/planner.rs`, inside `mod tests`, add this block right
after the `plan_is_plan_set_for_set_a` test (before
`use crate::testutil::{planner_input_strategy, …}`):

```rust
    // --- fill order ---

    fn rooted(names: &[&str]) -> FillOrder {
        FillOrder::Rooted(names.iter().map(|n| n.parse().unwrap()).collect())
    }

    /// Four Immich-style files, two per 100-byte disc.
    fn four_dated_files() -> Vec<ScannedFile> {
        vec![
            sf("family/anna/2021/a.jpg", 40),
            sf("family/anna/2023/d.jpg", 40),
            sf("family/bela/2020/b.jpg", 40),
            sf("others/cecil/2022/c.jpg", 40),
        ]
    }

    #[test]
    fn a_rooted_fill_interleaves_roots_by_the_path_below_them() {
        let order = rooted(&["family/anna", "family/bela", "others/cecil"]);
        let p = plan_set(0, &four_dated_files(), &[], &[], CAP100, &order);
        assert_eq!(
            disc_paths(&p.discs[0]),
            vec!["family/bela/2020/b.jpg", "family/anna/2021/a.jpg"]
        );
        assert_eq!(
            disc_paths(&p.discs[1]),
            vec!["others/cecil/2022/c.jpg", "family/anna/2023/d.jpg"]
        );
    }

    #[test]
    fn the_path_order_keeps_each_root_together() {
        let p = plan_set(0, &four_dated_files(), &[], &[], CAP100, &FillOrder::Path);
        assert_eq!(
            disc_paths(&p.discs[0]),
            vec!["family/anna/2021/a.jpg", "family/anna/2023/d.jpg"]
        );
    }

    #[test]
    fn a_rooted_fill_tops_off_the_open_disc_in_the_same_order() {
        // Open disc 1 has 40 bytes free: the older file tops it off, whichever
        // root it is under, and the newer one spills.
        let order = rooted(&["family/anna", "family/bela"]);
        let open = state(1, 0, DiscStatus::Open, 60);
        let p = plan_set(
            0,
            &[sf("family/anna/2024/e.jpg", 40), sf("family/bela/2019/f.jpg", 40)],
            &[],
            &[open],
            CAP100,
            &order,
        );
        assert_eq!((p.discs[0].ordinal, p.discs[0].is_new), (1, false));
        assert_eq!(disc_paths(&p.discs[0]), vec!["family/bela/2019/f.jpg"]);
        assert_eq!(disc_paths(&p.discs[1]), vec!["family/anna/2024/e.jpg"]);
    }
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p diskrement-core planner`
Expected: compile error, because `plan_set` takes 5 arguments and `FillOrder` is not in scope.

- [ ] **Step 3: Give `plan_set` an order**

In `crates/diskrement-core/src/planner.rs`, change the import line
`use crate::model::{RelPath, ScannedFile};` to:

```rust
use crate::model::{RelPath, ScannedFile};
use crate::roots::FillOrder;
```

Replace `plan`'s last line `plan_set(0, &to_place, &changes.deleted, discs, new_disc_cap)` with:

```rust
    plan_set(
        0,
        &to_place,
        &changes.deleted,
        discs,
        new_disc_cap,
        &FillOrder::Path,
    )
```

Replace `plan_set`'s doc comment, signature and first line, which today read:

```rust
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
```

with:

```rust
/// Plan one copy set: top off that set's newest open disc, then spill onto
/// fresh discs numbered within the set, placing files in `order`.
///
/// `discs` may hold every set's discs; only those in `copy_set` are
/// considered. Deterministic — the result depends only on the set of files to
/// place, not on their order, because `order`'s key is total.
pub fn plan_set(
    copy_set: u32,
    to_place: &[ScannedFile],
    tombstones: &[RelPath],
    discs: &[DiscState],
    new_disc_cap: Capacity,
    order: &FillOrder,
) -> BurnPlan {
    let mut to_place: Vec<ScannedFile> = to_place.to_vec();
    to_place.sort_by(|a, b| order.key(&a.rel_path).cmp(&order.key(&b.rel_path)));
```

Leave `oversized.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));` alone: a report, not a fill order.

Then give the six existing test calls (in `plan_set_tops_off_only_its_own_sets_open_disc`,
`plan_set_numbers_fresh_discs_within_its_own_set`,
`plan_set_stamps_every_planned_disc_with_its_set`, `plan_is_plan_set_for_set_a`, and
two in `plan_set_ignores_other_sets`) the path order:

```bash
perl -pi -e 's/(plan_set\([^;]*?, (?:CAP100|cap))\)/$1, &FillOrder::Path)/g' crates/diskrement-core/src/planner.rs
grep -c "FillOrder::Path" crates/diskrement-core/src/planner.rs
```

Expected count: 8, made up of `plan` (1), the six calls the command just edited (6) and
`the_path_order_keeps_each_root_together` (1). If it is not 8, inspect with
`grep -n "plan_set(" crates/diskrement-core/src/planner.rs`.

- [ ] **Step 4: Run the example tests**

Run: `cargo test -p diskrement-core planner`
Expected: PASS, including the three new tests.

- [ ] **Step 5: Add the rooted proptest strategy**

In `crates/diskrement-core/src/testutil.rs`, append:

```rust
/// The roots the rooted planner properties use. Their first components
/// differ, so prefixing distinct paths with them never makes two paths equal,
/// and no name holds another.
pub const ROOTS: [&str; 3] = ["anna", "family/bela", "others/cecil"];

/// A planner input whose files each sit under one of [`ROOTS`]: the files of
/// `planner_input_strategy`, each prefixed with a root picked per file.
pub fn rooted_planner_input_strategy(
) -> impl Strategy<Value = (Vec<ScannedFile>, Vec<DiscState>, Capacity)> {
    (
        planner_input_strategy(),
        proptest::collection::vec(0..ROOTS.len(), 12),
    )
        .prop_map(|((cs, discs, cap), picks)| {
            let files = cs
                .added
                .iter()
                .chain(cs.modified.iter())
                .zip(picks)
                .map(|(f, i)| ScannedFile {
                    rel_path: RelPath(format!("{}/{}", ROOTS[i], f.rel_path)),
                    ..f.clone()
                })
                .collect();
            (files, discs, cap)
        })
}
```

(`scanned_files_strategy` yields at most 11 files, so 12 picks always cover them.)

- [ ] **Step 6: Add the rooted properties**

In `crates/diskrement-core/src/planner.rs`, change
`use crate::testutil::{planner_input_strategy, reshuffled_planner_input_strategy};` to:

```rust
    use crate::testutil::{
        planner_input_strategy, reshuffled_planner_input_strategy, rooted_planner_input_strategy,
        ROOTS,
    };
```

Inside the `proptest! { … }` block, after property 8
(`deletions_add_no_bytes_to_any_disc`), add:

```rust
        /// 9. Rooted order: placed files are in `FillOrder::key` order across
        ///    all discs.
        #[test]
        fn rooted_placed_files_are_in_key_order_across_discs(
            (files, discs, cap) in rooted_planner_input_strategy()
        ) {
            let order = rooted(&ROOTS);
            let p = plan_set(0, &files, &[], &discs, cap, &order);
            let placed: Vec<&RelPath> = p
                .discs
                .iter()
                .flat_map(|d| d.files.iter())
                .map(|f| &f.rel_path)
                .collect();
            let mut sorted = placed.clone();
            sorted.sort_by(|a, b| order.key(a).cmp(&order.key(b)));
            prop_assert_eq!(placed, sorted);
        }

        /// 10. Rooted determinism: shuffling the input changes nothing.
        #[test]
        fn a_rooted_plan_ignores_input_order(
            ((files, discs, cap), shuffled) in rooted_planner_input_strategy()
                .prop_flat_map(|input| {
                    let files = input.0.clone();
                    (Just(input), Just(files).prop_shuffle())
                })
        ) {
            let order = rooted(&ROOTS);
            prop_assert_eq!(
                plan_set(0, &files, &[], &discs, cap, &order),
                plan_set(0, &shuffled, &[], &discs, cap, &order)
            );
        }

        /// 11. One root fills exactly like the path order: a single library
        ///     under a readable name fills as it would as a plain source.
        #[test]
        fn a_single_root_fills_like_the_path_order(
            (cs, discs, cap) in planner_input_strategy()
        ) {
            let files: Vec<ScannedFile> = placeable(&cs)
                .into_iter()
                .map(|f| ScannedFile {
                    rel_path: RelPath(format!("anna/{}", f.rel_path)),
                    ..f.clone()
                })
                .collect();
            prop_assert_eq!(
                plan_set(0, &files, &[], &discs, cap, &rooted(&["anna"])),
                plan_set(0, &files, &[], &discs, cap, &FillOrder::Path)
            );
        }
```

- [ ] **Step 7: Run the core suite**

Run: `cargo test -p diskrement-core`
Expected: PASS, 99 tests (93 + 3 examples + 3 properties).

- [ ] **Step 8: Thread the order through `plan_from`**

In `crates/diskrement-shell/src/pipeline/planning.rs`:

1. Add `FillOrder` to the `use diskrement_core::{…}` list at the top.
2. Add a paragraph to `plan_from`'s doc comment, just before
   `#[allow(clippy::too_many_arguments)]`:

   ```rust
   ///
   /// `order` is the project's fill order. Every set uses the same one, so
   /// twin discs hold the same files.
   ```

3. Add the parameter between `copies: u32,` and `next_session: impl Fn(DiscKey) -> u32,`:

   ```rust
       copies: u32,
       order: &FillOrder,
       next_session: impl Fn(DiscKey) -> u32,
   ```

4. Change `let full = planner::plan_set(copy_set, &to_place, &tombstones, states, cap);` to:

   ```rust
           let full = planner::plan_set(copy_set, &to_place, &tombstones, states, cap, order);
   ```

5. Give the 17 test calls the path order. Each one's last argument is a closure line such as `|_| 1,`:

   ```bash
   perl -0pi -e 's/^( +)(\|_\| \d+,\n)/$1&FillOrder::Path,\n$1$2/mg' crates/diskrement-shell/src/pipeline/planning.rs
   grep -c "&FillOrder::Path," crates/diskrement-shell/src/pipeline/planning.rs
   ```

   Expected count: 17.

6. Add this test at the end of `mod tests`:

   ```rust
       #[test]
       fn every_set_fills_in_the_projects_order() {
           let files = vec![
               file("family/anna/2021/a.jpg", 400),
               file("family/anna/2023/d.jpg", 400),
               file("family/bela/2020/b.jpg", 400),
           ];
           let order = FillOrder::Rooted(vec![
               "family/anna".parse().unwrap(),
               "family/bela".parse().unwrap(),
           ]);
           let report = plan_from(
               &CatalogView::empty(),
               &files,
               Vec::new(),
               &[],
               DiscType::BdR,
               Capacity::new(1_000),
               false,
               2,
               &order,
               |_| 1,
           );
           assert_eq!(report.sets.len(), 2);
           for set in &report.sets {
               assert_eq!(
                   set.disc
                       .files
                       .iter()
                       .map(|f| f.rel_path.as_str())
                       .collect::<Vec<_>>(),
                   vec!["family/bela/2020/b.jpg", "family/anna/2021/a.jpg"]
               );
           }
       }
   ```

In `crates/diskrement-shell/src/pipeline/mod.rs`:

1. In `plan_scanned`, the real call: add `&FillOrder::Path,` between `p.copies,` and
   `|disc| next.get(&disc).copied().unwrap_or(1),`. Task 4 replaces it with the
   project's own order. Add `FillOrder` to the `use diskrement_core::{…}` list at the top.
2. The three test calls:

   ```bash
   perl -0pi -e 's/^( +)(\|_\| 1,\n)/$1&diskrement_core::FillOrder::Path,\n$1$2/mg' crates/diskrement-shell/src/pipeline/mod.rs
   grep -c "&diskrement_core::FillOrder::Path," crates/diskrement-shell/src/pipeline/mod.rs
   ```

   Expected count: 3.

- [ ] **Step 9: Verify and commit**

Run the three verification commands. Expected counts: core 99; shell lib 374
(373 + 1); everything else unchanged.

```bash
git add crates/diskrement-core crates/diskrement-shell/src/pipeline
git commit -m "feat(planner): fill order is a parameter; rooted projects interleave by the path below each root

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: The snapshot composes named roots

**Files:**
- Modify: `crates/diskrement-shell/src/fs/snapshot.rs`
- Modify: `crates/diskrement-shell/src/pipeline/mod.rs` (`fresh_scan`)

- [ ] **Step 1: Write the failing tests**

In `crates/diskrement-shell/src/fs/snapshot.rs`, inside `mod tests`, add after
`fn fixture()`:

```rust
    fn single(p: &Path) -> Sources {
        Sources::Single(p.to_path_buf())
    }

    fn named(name: &str, path: &Path) -> (diskrement_core::RootName, PathBuf) {
        (name.parse().unwrap(), path.to_path_buf())
    }

    /// Two libraries, `lib/anna` (one file in a subdirectory) and `lib/cecil`
    /// (one file), and a work dir.
    fn libraries() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let dir = tempfile::TempDir::new().unwrap();
        let anna = dir.path().join("lib/anna");
        let cecil = dir.path().join("lib/cecil");
        let work = dir.path().join("work");
        std::fs::create_dir_all(anna.join("2021")).unwrap();
        std::fs::create_dir_all(&cecil).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(anna.join("2021/a.jpg"), b"anna").unwrap();
        std::fs::write(cecil.join("c.jpg"), b"cecil").unwrap();
        (dir, anna, cecil, work)
    }

    #[test]
    fn a_rooted_snapshot_puts_each_root_under_its_name() {
        let (_d, anna, cecil, work) = libraries();
        let sources =
            Sources::rooted(vec![named("family/anna", &anna), named("cecil", &cecil)]).unwrap();
        let h = create_with(&sources, &work, ok_cloner).unwrap();
        assert_eq!(
            std::fs::read(h.root.join("family/anna/2021/a.jpg")).unwrap(),
            b"anna"
        );
        assert_eq!(std::fs::read(h.root.join("cecil/c.jpg")).unwrap(), b"cecil");
        let mut top: Vec<String> = std::fs::read_dir(&h.root)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        top.sort();
        assert_eq!(top, vec!["cecil", "family"]);
    }

    #[test]
    fn a_missing_root_fails_with_its_path_before_anything_is_cloned() {
        let (_d, anna, _cecil, work) = libraries();
        let gone = work.join("gone");
        let sources = Sources::rooted(vec![named("anna", &anna), named("bela", &gone)]).unwrap();
        let err = create_with(&sources, &work, ok_cloner).unwrap_err();
        assert!(
            matches!(&err, SnapshotError::SourceMissing(p) if *p == gone),
            "{err:?}"
        );
        assert!(!work.join("snapshot").exists());
    }

    /// `reflink_tree` restores each file's mtime, and it does so in every root.
    #[test]
    fn the_real_reflink_keeps_mtimes_in_every_root_or_skips_loudly() {
        let (_d, anna, cecil, work) = libraries();
        let probe = work.join("cow-probe.src");
        std::fs::write(&probe, b"0").unwrap();
        let cow = reflink_copy::reflink(&probe, work.join("cow-probe.clone")).is_ok();
        let _ = std::fs::remove_file(&probe);
        let _ = std::fs::remove_file(work.join("cow-probe.clone"));
        if !cow {
            eprintln!(
                "SKIP: {} is not on a CoW filesystem; this test needs TMPDIR on \
                 btrfs/XFS/bcachefs to execute.",
                work.display()
            );
            return;
        }
        let stamp =
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
        for f in [anna.join("2021/a.jpg"), cecil.join("c.jpg")] {
            std::fs::File::options()
                .write(true)
                .open(&f)
                .unwrap()
                .set_modified(stamp)
                .unwrap();
        }
        let sources =
            Sources::rooted(vec![named("family/anna", &anna), named("cecil", &cecil)]).unwrap();
        let h = create(&sources, &work).unwrap();
        for f in ["family/anna/2021/a.jpg", "cecil/c.jpg"] {
            let mtime = std::fs::metadata(h.root.join(f)).unwrap().modified().unwrap();
            assert_eq!(mtime, stamp, "{f}");
        }
    }
```

Then point the existing tests at `single(…)`:

```bash
perl -pi -e 's/create_with\(&src, /create_with(&single(&src), /g; s/create_with\(&work\.join\("nope"\), /create_with(&single(&work.join("nope")), /g; s/\bcreate\(&src, &work\)/create(&single(&src), &work)/g' crates/diskrement-shell/src/fs/snapshot.rs
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p diskrement-shell fs::snapshot`
Expected: compile errors, because `Sources` is not in scope and `create_with` takes a `&Path`.

- [ ] **Step 3: Take `&Sources`**

In `crates/diskrement-shell/src/fs/snapshot.rs`, add `use diskrement_core::Sources;`
below `use std::path::{Path, PathBuf};`. Replace `create_with` and `create` (from the
`/// \`create\` with the cloner injected` comment through the end of `create`) with:

```rust
/// `create` with the cloner injected; the real entry point passes
/// `reflink_tree`. Order follows `SourceSnapshotLive.create`: existence
/// check, stale-snapshot removal, work-dir creation, then the clone. A
/// clone-unsupported failure is **hard** — there is no plain-copy fallback
/// for the source freeze.
///
/// A single source is cloned to `snapshot/` itself. A rooted project's roots
/// are each cloned to `snapshot/<name>`, in name order; a name's parent
/// directories (`family/` for `family/anna`) are plain directories that exist
/// only here. Every root is checked to exist before anything is removed or
/// cloned, so a missing one fails with its own path.
pub fn create_with(
    sources: &Sources,
    work_dir: &Path,
    clone: impl Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<SnapshotHandle, SnapshotError> {
    if let Some(missing) = sources.paths().into_iter().find(|p| !p.exists()) {
        return Err(SnapshotError::SourceMissing(missing.to_path_buf()));
    }
    let dst = work_dir.join("snapshot");
    if dst.exists() {
        std::fs::remove_dir_all(&dst).map_err(SnapshotError::Io)?;
    }
    std::fs::create_dir_all(work_dir).map_err(SnapshotError::Io)?;
    // NotCowCapable's work_dir is the work dir itself (cp's dst.getParent).
    match sources {
        Sources::Single(src) => clone(src, &dst).map_err(|e| classify(work_dir, e))?,
        Sources::Rooted(roots) => {
            for (name, src) in roots {
                let target = dst.join(name.as_str());
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).map_err(SnapshotError::Io)?;
                }
                clone(src, &target).map_err(|e| classify(work_dir, e))?;
            }
        }
    }
    Ok(SnapshotHandle { root: dst })
}

pub fn create(sources: &Sources, work_dir: &Path) -> Result<SnapshotHandle, SnapshotError> {
    create_with(sources, work_dir, reflink_tree)
}
```

- [ ] **Step 4: Keep `fresh_scan` compiling**

In `crates/diskrement-shell/src/pipeline/mod.rs`, `fresh_scan` begins:

```rust
        let src = PathBuf::from(&p.source_root);
        // Snapshot failures (missing source, non-CoW, I/O) are Failed here —
        // exit 5 — because only `init` produces the typed 3s (`PipelineLive`
        // maps SnapshotError to Failed in freshScan).
        let handle = snapshot::create(&src, &self.paths.work_dir).map_err(Self::failed)?;
```

Replace that with:

```rust
        // Until `Project` carries its `Sources` (the next task), a project is
        // its one `source_root`.
        let sources = Sources::Single(PathBuf::from(&p.source_root));
        // Snapshot failures (missing source, non-CoW, I/O) are Failed here —
        // exit 5 — because only `init` produces the typed 3s (`PipelineLive`
        // maps SnapshotError to Failed in freshScan).
        let handle = snapshot::create(&sources, &self.paths.work_dir).map_err(Self::failed)?;
```

and add `Sources` to the `use diskrement_core::{…}` list at the top of the file.

- [ ] **Step 5: Verify and commit**

Run: `cargo test -p diskrement-shell fs::snapshot`. Expected: PASS.

Run the three verification commands. Expected counts: shell lib 377 (374 + 3).

```bash
git add crates/diskrement-shell/src/fs/snapshot.rs crates/diskrement-shell/src/pipeline/mod.rs
git commit -m "feat(fs): the snapshot composes named roots under their names

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: The catalog stores roots; `Project` carries `Sources`

This task changes a struct field that four files construct, so it is larger than the
others. It is one commit because the workspace does not compile partway through.

**Files:**
- Modify: `crates/diskrement-shell/src/catalog/schema.rs`, `catalog/mod.rs`,
  `catalog/model.rs`, `catalog/store.rs`, `catalog/testutil.rs`
- Modify: `crates/diskrement-shell/tests/fixture_catalog.rs`
- Modify: `crates/diskrement-shell/src/pipeline/mod.rs`
- Modify: `crates/diskrement-cli/src/main.rs`, `crates/diskrement-cli/src/render.rs`

- [ ] **Step 1: Write the failing catalog tests**

In `crates/diskrement-shell/src/catalog/mod.rs` tests:
- rename `a_fresh_database_migrates_to_version_3_with_every_domain_table` to
  `a_fresh_database_migrates_to_version_4_with_every_domain_table`, change
  `assert_eq!(version, 3);` to `assert_eq!(version, 4);`, and add `"project_root",`
  to the `for expected in [ … ]` list after `"project",`;
- in `re_opening_a_catalog_re_runs_migrations_idempotently`, change
  `assert_eq!(versions, vec![3], …)` to `assert_eq!(versions, vec![4], …)`;
- add a test that a V3 catalog (the live `immich-dsz` one is V3) migrates and keeps its
  project. It turns a fresh catalog back into V3 by hand, which is exactly V3's shape
  because V4 only adds a table:

  ```rust
      #[test]
      fn a_v3_catalog_gains_project_root_and_keeps_its_projects() {
          let (dir, mut cat) = temp_catalog();
          cat.create_project(&super::testutil::project_init("p"))
              .expect("project");
          cat.conn
              .execute_batch("drop table project_root; update schema_version set version = 3;")
              .expect("back to V3");
          drop(cat);

          let cat = super::Catalog::open(&dir.path().join("catalog.sqlite")).expect("V3 → V4");
          let version: i64 = cat
              .conn
              .query_row("select version from schema_version", [], |r| r.get(0))
              .unwrap();
          assert_eq!(version, 4);
          let roots: i64 = cat
              .conn
              .query_row("select count(*) from project_root", [], |r| r.get(0))
              .unwrap();
          assert_eq!(roots, 0);
          assert!(cat.find_project("p").unwrap().is_some());
      }
  ```

In `crates/diskrement-shell/src/catalog/store.rs` tests, add to the imports at the top
of `mod tests`:

```rust
    use std::path::PathBuf;

    use diskrement_core::Sources;

    use crate::catalog::model::ProjectInit;
```

change the `source_root` assertion in `create_project_then_find_project_round_trips`
(`assert_eq!(project.source_root, "/src");`) to:

```rust
        assert_eq!(project.sources, Sources::Single(PathBuf::from("/src")));
```

and add after `find_project_returns_none_for_an_unknown_name`:

```rust
    #[test]
    fn a_rooted_project_round_trips_its_roots_and_leaves_source_root_empty() {
        let (_dir, mut cat) = temp_catalog();
        let sources = Sources::rooted(vec![
            ("others/cecil".parse().unwrap(), PathBuf::from("/lib/u3")),
            ("family/anna".parse().unwrap(), PathBuf::from("/lib/u1")),
        ])
        .unwrap();
        cat.create_project(&ProjectInit {
            sources: sources.clone(),
            ..project_init("fam")
        })
        .expect("project is created");
        // A second rooted project: its roots must not leak into the first's.
        cat.create_project(&ProjectInit {
            sources: Sources::rooted(vec![("zed".parse().unwrap(), PathBuf::from("/lib/z"))])
                .unwrap(),
            ..project_init("other")
        })
        .expect("second project is created");

        let project = cat
            .find_project("fam")
            .expect("query succeeds")
            .expect("the project exists");
        assert_eq!(project.sources, sources);
        let source_root: String = cat
            .raw_connection_for_tests()
            .query_row(
                "select source_root from project where name = 'fam'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(source_root, "");
        assert_eq!(count(&cat, "project_root"), 3);
    }

    #[test]
    fn a_refused_project_writes_none_of_its_roots() {
        let (_dir, mut cat) = temp_catalog();
        cat.create_project(&project_init("fam"))
            .expect("first create");
        let rooted = ProjectInit {
            sources: Sources::rooted(vec![("anna".parse().unwrap(), PathBuf::from("/lib/u1"))])
                .unwrap(),
            ..project_init("fam")
        };
        assert!(matches!(
            cat.create_project(&rooted),
            Err(CatalogError::DuplicateProject(_))
        ));
        assert_eq!(count(&cat, "project_root"), 0);
        assert_eq!(
            cat.find_project("fam").unwrap().unwrap().sources,
            Sources::Single(PathBuf::from("/src"))
        );
    }
```

In `crates/diskrement-shell/tests/fixture_catalog.rs`:
- in `a_v2_catalog_migrates_to_v3_keeping_every_disc_id`, change
  `assert_eq!(version, 3);` to `assert_eq!(version, 4, "V3, then V4 on top");`;
- add `use diskrement_core::Sources;` to the imports and this test after
  `the_initial_catalog_holds_a_project_and_no_discs`:

  ```rust
  /// A catalog written before roots existed reads back as a single source:
  /// V4 adds an empty `project_root` and changes no project row.
  #[test]
  fn a_v2_catalog_reads_back_as_a_single_source() {
      let (_dir, cat) = open("catalog/01-after-init.sqlite");
      let project = cat
          .find_project("p")
          .expect("query")
          .expect("project exists");
      assert_eq!(
          project.sources,
          Sources::Single("/tmp/diskrement-harvest.RENbbg/source".into())
      );
      let roots: i64 = cat
          .raw_connection_for_tests()
          .query_row("select count(*) from project_root", [], |r| r.get(0))
          .unwrap();
      assert_eq!(roots, 0);
  }
  ```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p diskrement-shell catalog`
Expected: compile errors, because `Project` has no field `sources` and `ProjectInit` has no field `sources`.

- [ ] **Step 3: Schema V4**

In `crates/diskrement-shell/src/catalog/schema.rs`, after the `V3` constant, add:

```rust
// Named source roots (project-roots spec §3.4). Written once, by `init`, in
// the project row's transaction, and never updated or deleted. A project with
// no rows here has a single source in `project.source_root`; a rooted project
// stores '' there, because relaxing that column's `not null` would mean
// rebuilding `project`, which every other table references.
const V4: &[&str] = &["create table if not exists project_root (
       project_id integer not null references project(id),
       name       text    not null,
       path       text    not null,
       unique (project_id, name)
     )"];
```

change `MIGRATIONS` to `&[(1, V1), (2, V2), (3, V3), (4, V4)]`, and in the module doc
comment change `//! adds copy sets (multiple-copies spec §5.2). The two \`check\`` to
`//! adds copy sets (multiple-copies spec §5.2), V4 named source roots. The two \`check\``.

- [ ] **Step 4: The model**

In `crates/diskrement-shell/src/catalog/model.rs`, add `Sources` to the
`use diskrement_core::{…}` list, and in both `ProjectInit` and `Project` replace
`pub source_root: String,` with:

```rust
    /// What the project archives. A rooted project's roots live in
    /// `project_root`, and its `project.source_root` column holds `''`.
    pub sources: Sources,
```

In `crates/diskrement-shell/src/catalog/testutil.rs`, add `use std::path::PathBuf;`, add
`Sources` to the `diskrement_core` import, and in `project_init` replace
`source_root: "/src".to_string(),` with `sources: Sources::Single(PathBuf::from("/src")),`.

- [ ] **Step 5: The store**

In `crates/diskrement-shell/src/catalog/store.rs`:
- change `use std::path::Path;` to `use std::path::{Path, PathBuf};`;
- add `RootName, Sources` to the `use diskrement_core::{…}` list;
- replace `create_project` and `find_project` (from `/// Insert a project row, returning its new id.`
  through the end of `find_project`) with:

```rust
    /// Insert a project row and, for a rooted project, its roots, in one
    /// transaction, returning the new id. A rooted project stores `''` in
    /// `source_root`.
    pub fn create_project(&mut self, p: &ProjectInit) -> Result<ProjectId, CatalogError> {
        let name = p.name.clone();
        let source_root = match &p.sources {
            Sources::Single(path) => path.to_string_lossy().into_owned(),
            Sources::Rooted(_) => String::new(),
        };
        self.tx(|tx| {
            let inserted = tx.execute(
                "insert into project
                   (name, source_root, work_dir, default_disc_type, margin_percent,
                    copies, created_at)
                 values (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    p.name,
                    source_root,
                    p.work_dir,
                    p.default_disc_type.as_str(),
                    p.margin_percent,
                    p.copies as i64,
                    time::format(&p.created_at),
                ],
            );
            let id = match inserted {
                Ok(_) => ProjectId(tx.last_insert_rowid()),
                Err(e) if is_unique_violation(&e) => {
                    return Err(CatalogError::DuplicateProject(name))
                }
                Err(e) => return Err(CatalogError::from_sqlite(e)),
            };
            if let Sources::Rooted(roots) = &p.sources {
                for (root, path) in roots {
                    tx.execute(
                        "insert into project_root (project_id, name, path)
                         values (?1, ?2, ?3)",
                        params![id.0, root.as_str(), path.to_string_lossy().into_owned()],
                    )
                    .map_err(CatalogError::from_sqlite)?;
                }
            }
            Ok(id)
        })
    }

    /// Look a project up by its unique name, with its roots if it has any.
    pub fn find_project(&self, name: &str) -> Result<Option<Project>, CatalogError> {
        let row = self
            .conn
            .query_row(
                "select id, name, source_root, work_dir, default_disc_type,
                        margin_percent, copies, created_at
                 from project where name = ?1",
                params![name],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, f64>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, String>(7)?,
                    ))
                },
            )
            .optional()
            .map_err(CatalogError::from_sqlite)?;
        let Some((id, name, source_root, work_dir, disc_type, margin, copies, created_at)) = row
        else {
            return Ok(None);
        };

        let roots = self.project_roots(id)?;
        let sources = if roots.is_empty() {
            Sources::Single(PathBuf::from(source_root))
        } else {
            Sources::rooted(roots)
                .map_err(|e| CatalogError::ConstraintViolation(e.to_string()))?
        };
        Ok(Some(Project {
            id: ProjectId(id),
            name,
            sources,
            work_dir,
            default_disc_type: DiscType::from_str(&disc_type)
                .map_err(CatalogError::ConstraintViolation)?,
            margin_percent: margin,
            copies: copies as u32,
            created_at: time::parse(&created_at)?,
        }))
    }

    /// A project's `project_root` rows; empty for a single-source project.
    fn project_roots(&self, project: i64) -> Result<Vec<(RootName, PathBuf)>, CatalogError> {
        let mut stmt = self
            .conn
            .prepare("select name, path from project_root where project_id = ?1")
            .map_err(CatalogError::from_sqlite)?;
        let rows = stmt
            .query_map(params![project], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(CatalogError::from_sqlite)?;
        rows.map(|row| {
            let (name, path) = row.map_err(CatalogError::from_sqlite)?;
            let name = name
                .parse::<RootName>()
                .map_err(CatalogError::ConstraintViolation)?;
            Ok((name, PathBuf::from(path)))
        })
        .collect()
    }
```

- [ ] **Step 6: The pipeline uses `p.sources`**

In `crates/diskrement-shell/src/pipeline/mod.rs`:

1. `init` begins:

   ```rust
           let src = PathBuf::from(&pi.source_root);
           let work = PathBuf::from(&pi.work_dir);
           // `getFileStore` equality via st_dev; a missing source surfaces here
           // as Failed (exit 5), exactly like the Scala getFileStore throw.
           let (sdev, wdev) = same_filesystem(&src, &work).map_err(Self::failed)?;
           if sdev != wdev {
               return Err(PipelineError::NotSameFilesystem { src, work });
           }
   ```

   Replace that with:

   ```rust
           let work = PathBuf::from(&pi.work_dir);
           for src in pi.sources.paths() {
               // `getFileStore` equality via st_dev; a missing source surfaces
               // here as Failed (exit 5), exactly like the Scala getFileStore
               // throw.
               let (sdev, wdev) = same_filesystem(src, &work).map_err(Self::failed)?;
               if sdev != wdev {
                   return Err(PipelineError::NotSameFilesystem {
                       src: src.to_path_buf(),
                       work,
                   });
               }
           }
   ```

   Also update the doc line `/// \`init\`: same-filesystem check, CoW probe, then the one project row.`
   to `/// \`init\`: same-filesystem check for every root, CoW probe, then the project row and its roots.`

2. In `fresh_scan`, replace the Task 3 shim (the two comment lines starting
   `// Until \`Project\` carries its \`Sources\``, plus
   `let sources = Sources::Single(PathBuf::from(&p.source_root));`) and change the
   `snapshot::create(&sources, …)` call to:

   ```rust
           let handle = snapshot::create(&p.sources, &self.paths.work_dir).map_err(Self::failed)?;
   ```

3. In `plan_scanned`, change the `&FillOrder::Path,` that Task 2 added to
   `&FillOrder::from(&p.sources),`.

4. The six test `ProjectInit`s:

   ```bash
   perl -pi -e 's/source_root: (.*)\.display\(\)\.to_string\(\),/sources: diskrement_core::Sources::Single($1.clone()),/' crates/diskrement-shell/src/pipeline/mod.rs
   grep -c "sources: diskrement_core::Sources::Single" crates/diskrement-shell/src/pipeline/mod.rs
   grep -n "source_root" crates/diskrement-shell/src/pipeline/mod.rs
   ```

   Expected count: 6. The second grep should list only the test name
   `burn_reads_the_snapshot_not_the_source_root`, and no field.

   If `Sources` is now unused at the top of `mod.rs` (the shim was its only user),
   remove it from the `use diskrement_core::{…}` list there. Task 5 adds it back.

- [ ] **Step 7: The CLI**

In `crates/diskrement-cli/src/main.rs`, add `use diskrement_core::Sources;` and in the
`Command::Init` arm change `source_root: source.display().to_string(),` to
`sources: Sources::Single(source),`. (The `--root` flag comes in Task 6.)

In `crates/diskrement-cli/src/render.rs`:
- change `use diskrement_core::{set_letter, RelPath, SetCoverage};` to
  `use diskrement_core::{set_letter, RelPath, SetCoverage, Sources};`;
- in `status_report`, replace

  ```rust
      let mut out = format!(
          "Project {}  (source {}, {}, margin {:.1}%{copies})\n",
          p.name, p.source_root, p.default_disc_type, p.margin_percent
      );
      out.push_str("Discs:\n");
  ```

  with

  ```rust
      let source = match &p.sources {
          Sources::Single(path) => format!("source {}", path.display()),
          Sources::Rooted(roots) if roots.len() == 1 => "1 root".to_string(),
          Sources::Rooted(roots) => format!("{} roots", roots.len()),
      };
      let mut out = format!(
          "Project {}  ({source}, {}, margin {:.1}%{copies})\n",
          p.name, p.default_disc_type, p.margin_percent
      );
      if let Sources::Rooted(roots) = &p.sources {
          out.push_str("Roots:\n");
          for (name, path) in roots {
              out.push_str(&format!("  {name} → {}\n", path.display()));
          }
      }
      out.push_str("Discs:\n");
  ```

- in `mod tests`'s `fn project()`, change `source_root: "/srv/photos".into(),` to
  `sources: Sources::Single("/srv/photos".into()),`;
- in `mod snapshots`'s `fn project(name: &str, source: &str)`, change
  `source_root: source.to_string(),` to `sources: Sources::Single(source.into()),`;
- add these tests to `mod tests`, after
  `a_single_copy_status_names_the_files_with_no_live_copy`:

  ```rust
      fn nothing_burned() -> StatusReport {
          StatusReport {
              discs: vec![],
              open_discs: vec![],
              coverage: diskrement_core::Coverage::default(),
              total_current_files: 0,
              last_burn_at: None,
          }
      }

      #[test]
      fn a_rooted_status_names_its_roots_before_the_discs() {
          let mut p = project();
          p.sources = Sources::rooted(vec![
              ("others/cecil".parse().unwrap(), "/lib/u3".into()),
              ("family/anna".parse().unwrap(), "/lib/u1".into()),
          ])
          .unwrap();
          assert_eq!(
              status_report(&p, &nothing_burned()),
              "Project photos  (2 roots, BD_R, margin 2.0%)\n\
               Roots:\n  \
               family/anna → /lib/u1\n  \
               others/cecil → /lib/u3\n\
               Discs:\n\
               Current files: 0      Last burn: never"
          );
      }

      #[test]
      fn a_status_with_one_root_says_one_root() {
          let mut p = project();
          p.sources = Sources::rooted(vec![("anna".parse().unwrap(), "/lib/u1".into())]).unwrap();
          assert!(status_report(&p, &nothing_burned()).starts_with(
              "Project photos  (1 root, BD_R, margin 2.0%)\nRoots:\n  anna → /lib/u1\nDiscs:\n"
          ));
      }
  ```

The insta snapshots (`status.snap`) must not change: a plain project renders exactly as
before. If `cargo test` reports a snapshot diff, the header code is wrong; fix the code,
not the `.snap`.

- [ ] **Step 8: Verify and commit**

Run the three verification commands. Expected counts: shell lib 380 (377 + 2 store
tests + 1 migration test); `fixture_catalog` 9; cli bin unit 109 (107 + 2); everything else unchanged.

```bash
git add crates/diskrement-shell crates/diskrement-cli/src
git commit -m "feat(catalog): projects carry their sources; schema V4 stores named roots

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: `init` checks every root; a rooted cycle end to end

**Files:**
- Modify: `crates/diskrement-shell/src/error.rs`
- Modify: `crates/diskrement-shell/src/pipeline/mod.rs` (`init`, `same_filesystem`, new `overlapping_roots`)
- Create: `crates/diskrement-shell/src/pipeline/roots_tests.rs`

- [ ] **Step 1: Write the failing error tests**

In `crates/diskrement-shell/src/error.rs`, in
`exit_codes_are_the_contract_for_the_project_and_filesystem_variants`, add before
`assert_eq!(PipelineError::Failed("x".into()).exit_code(), 5);`:

```rust
        assert_eq!(
            PipelineError::OverlappingRoots {
                a: "/lib".into(),
                b: "/lib/u1".into()
            }
            .exit_code(),
            3
        );
```

and in `messages_match_the_scala_render_verbatim`, before the `Failed` assertion:

```rust
        assert_eq!(
            PipelineError::OverlappingRoots {
                a: "/lib".into(),
                b: "/lib/u1".into()
            }
            .to_string(),
            "roots /lib and /lib/u1 overlap: each file must belong to exactly one root"
        );
```

- [ ] **Step 2: Write the failing pipeline tests**

Create `crates/diskrement-shell/src/pipeline/roots_tests.rs`:

```rust
//! Rooted projects through the pipeline: `init`'s checks on the roots, and a
//! full plan → burn → verify cycle whose discs fill by date across roots
//! (project-roots spec §3.3, §4, §5).

use super::*;
use crate::burn::simulated::SimulatedBurner;
use crate::burn::verifier::StagingVerifier;
use crate::progress::NoopSink;
use chrono::TimeZone;
use diskrement_core::RootName;

struct Env {
    _dir: tempfile::TempDir,
    catalog: Catalog,
    lib: PathBuf,
    work: PathBuf,
}

fn at(offset: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(1_757_000_000 + offset, 0).unwrap()
}

fn name(s: &str) -> RootName {
    s.parse().unwrap()
}

/// Three Immich-style libraries under `lib/` (`<user>/YYYY/YYYY-MM-DD/<file>`).
/// Every file is 100_000 bytes, so two fit the 250_000 usable bytes that a
/// 99.999% margin leaves (see `tiny_project_init` in `tests`).
fn env() -> Env {
    let dir = tempfile::TempDir::new().unwrap();
    let lib = dir.path().join("lib");
    let work = dir.path().join("work");
    for rel in [
        "u1/2021/2021-03-01/a.jpg",
        "u1/2023/2023-07-01/d.jpg",
        "u2/2020/2020-01-01/b.jpg",
        "u3/2022/2022-05-05/c.jpg",
    ] {
        let p = lib.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let body: Vec<u8> = rel.bytes().cycle().take(100_000).collect();
        std::fs::write(&p, body).unwrap();
    }
    std::fs::create_dir_all(&work).unwrap();
    Env {
        catalog: Catalog::open(&dir.path().join("cat.sqlite")).unwrap(),
        _dir: dir,
        lib,
        work,
    }
}

fn three_roots(e: &Env) -> Sources {
    Sources::rooted(vec![
        (name("family/anna"), e.lib.join("u1")),
        (name("family/bela"), e.lib.join("u2")),
        (name("others/cecil"), e.lib.join("u3")),
    ])
    .unwrap()
}

fn init_of(e: &Env, sources: Sources, copies: u32) -> ProjectInit {
    ProjectInit {
        name: "p".into(),
        sources,
        work_dir: e.work.display().to_string(),
        default_disc_type: DiscType::BdR,
        margin_percent: 99.999,
        copies,
        created_at: at(0),
    }
}

fn pipeline(e: &mut Env) -> Pipeline<'_> {
    let paths = PipelinePaths {
        work_dir: e.work.clone(),
        scan_cache: e.work.join("scancache.sqlite"),
    };
    Pipeline::new(&mut e.catalog, paths, &NoopSink)
}

/// Every file staged on disc `name`, relative to its `data/`, sorted.
fn staged(e: &Env, name: &str) -> Vec<String> {
    let data = e.work.join(format!("staging/disc-{name}/data"));
    let mut files: Vec<String> = walkdir::WalkDir::new(&data)
        .into_iter()
        .map(Result::unwrap)
        .filter(|x| x.file_type().is_file())
        .map(|x| {
            x.path()
                .strip_prefix(&data)
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    files.sort();
    files
}

#[test]
fn a_rooted_cycle_fills_each_disc_with_the_oldest_files_across_roots() {
    let mut e = env();
    let pi = init_of(&e, three_roots(&e), 2);
    pipeline(&mut e).init(&pi).unwrap();
    let burner = Burner::Simulated(SimulatedBurner::new(&NoopSink, 2));

    // Cycle 1: the two oldest files. By rel_path alone this disc would hold
    // both of family/anna's files instead.
    let out = pipeline(&mut e).plan("p", None, false, false).unwrap();
    assert_eq!(out.scan.added, 4);
    pipeline(&mut e).burn("p", at(1), &burner).unwrap();
    let oldest = vec![
        "family/anna/2021/2021-03-01/a.jpg",
        "family/bela/2020/2020-01-01/b.jpg",
    ];
    assert_eq!(staged(&e, "1A"), oldest);
    assert_eq!(staged(&e, "1B"), oldest);

    // The disc's embedded catalog names the roots: it describes its own layout.
    // Opened `immutable=1`, so reading it leaves no `-wal`/`-shm` in the staged
    // tree.
    let embedded = rusqlite::Connection::open_with_flags(
        format!(
            "file:{}?immutable=1",
            e.work
                .join("staging/disc-1B/.diskrement/catalog.sqlite")
                .display()
        ),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .unwrap();
    let names: Vec<String> = embedded
        .prepare("select name from project_root order by name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(names, ["family/anna", "family/bela", "others/cecil"]);

    // Cycle 2: the rest.
    pipeline(&mut e).plan("p", None, false, false).unwrap();
    pipeline(&mut e).burn("p", at(2), &burner).unwrap();
    let newest = vec![
        "family/anna/2023/2023-07-01/d.jpg",
        "others/cecil/2022/2022-05-05/c.jpg",
    ];
    assert_eq!(staged(&e, "2A"), newest);
    assert_eq!(staged(&e, "2B"), newest);

    let verifier = Verifier::Staging(StagingVerifier::new(e.work.join("staging"), &NoopSink));
    let reports = pipeline(&mut e).verify("p", None, &verifier).unwrap();
    assert_eq!(reports.len(), 4);
    assert!(
        reports.iter().all(|r| r.mismatches.is_empty() && !r.skipped),
        "{reports:?}"
    );
}

/// `init` refuses roots whose paths overlap, writes no project, and exits 3.
fn refused_overlap(e: &mut Env, roots: Vec<(RootName, PathBuf)>) -> PipelineError {
    let pi = init_of(e, Sources::rooted(roots).unwrap(), 1);
    let err = pipeline(e).init(&pi).unwrap_err();
    assert_eq!(err.exit_code(), 3, "{err}");
    assert!(matches!(
        pipeline(e).status("p"),
        Err(PipelineError::ProjectNotFound(_))
    ));
    err
}

#[test]
fn init_refuses_two_roots_at_the_same_directory() {
    let mut e = env();
    let u1 = e.lib.join("u1");
    let err = refused_overlap(&mut e, vec![(name("anna"), u1.clone()), (name("bela"), u1.clone())]);
    assert!(
        matches!(&err, PipelineError::OverlappingRoots { a, b } if *a == u1 && *b == u1),
        "{err:?}"
    );
}

#[test]
fn init_refuses_a_root_inside_another() {
    let mut e = env();
    let (lib, u1) = (e.lib.clone(), e.lib.join("u1"));
    let err = refused_overlap(&mut e, vec![(name("all"), lib), (name("anna"), u1)]);
    assert!(matches!(err, PipelineError::OverlappingRoots { .. }), "{err:?}");
}

#[test]
fn init_resolves_symlinks_before_comparing_roots() {
    let mut e = env();
    let (u1, alias) = (e.lib.join("u1"), e.lib.join("alias"));
    std::os::unix::fs::symlink(&u1, &alias).unwrap();
    let err = refused_overlap(&mut e, vec![(name("anna"), u1), (name("alias"), alias)]);
    assert!(matches!(err, PipelineError::OverlappingRoots { .. }), "{err:?}");
}

#[test]
fn sibling_roots_whose_paths_share_a_prefix_do_not_overlap() {
    let mut e = env();
    std::fs::create_dir_all(e.lib.join("u10")).unwrap();
    let sources = Sources::rooted(vec![
        (name("a"), e.lib.join("u1")),
        (name("b"), e.lib.join("u10")),
    ])
    .unwrap();
    let pi = init_of(&e, sources, 1);
    pipeline(&mut e).init(&pi).unwrap();
}

#[test]
fn a_missing_root_fails_init_naming_it() {
    let mut e = env();
    let gone = e.lib.join("gone");
    let sources = Sources::rooted(vec![
        (name("anna"), e.lib.join("u1")),
        (name("gone"), gone.clone()),
    ])
    .unwrap();
    let pi = init_of(&e, sources, 1);
    let err = pipeline(&mut e).init(&pi).unwrap_err();
    assert_eq!(err.exit_code(), 5);
    assert!(err.to_string().contains(&gone.display().to_string()), "{err}");
}

#[test]
fn a_root_gone_after_init_fails_plan_naming_it() {
    let mut e = env();
    let pi = init_of(&e, three_roots(&e), 1);
    pipeline(&mut e).init(&pi).unwrap();
    std::fs::remove_dir_all(e.lib.join("u3")).unwrap();
    let err = pipeline(&mut e).plan("p", None, false, false).unwrap_err();
    assert_eq!(err.exit_code(), 5);
    let msg = err.to_string();
    assert!(msg.contains("source root does not exist"), "{msg}");
    assert!(msg.contains(&e.lib.join("u3").display().to_string()), "{msg}");
}

#[test]
fn init_refuses_a_root_on_another_filesystem_naming_it() {
    let Ok(other) = tempfile::TempDir::new_in("/dev/shm") else {
        eprintln!("SKIP: /dev/shm is not available as a second filesystem");
        return;
    };
    let mut e = env();
    let dev = |p: &Path| std::fs::metadata(p).unwrap().dev();
    if dev(other.path()) == dev(&e.work) {
        eprintln!("SKIP: /dev/shm is on the work dir's filesystem");
        return;
    }
    let sources = Sources::rooted(vec![
        (name("anna"), e.lib.join("u1")),
        (name("elsewhere"), other.path().to_path_buf()),
    ])
    .unwrap();
    let pi = init_of(&e, sources, 1);
    let err = pipeline(&mut e).init(&pi).unwrap_err();
    assert_eq!(err.exit_code(), 3);
    assert!(
        matches!(&err, PipelineError::NotSameFilesystem { src, .. } if src == other.path()),
        "{err:?}"
    );
}
```

Register it in `crates/diskrement-shell/src/pipeline/mod.rs`, below `pub mod staged;`:

```rust

#[cfg(test)]
mod roots_tests;
```

- [ ] **Step 3: Run them to see them fail**

Run: `cargo test -p diskrement-shell roots_tests`
Expected: compile error, because `PipelineError::OverlappingRoots` does not exist.

- [ ] **Step 4: The error variant**

In `crates/diskrement-shell/src/error.rs`, after the `NotCowCapable { … },` variant, add:

```rust
    /// Two roots that would archive the same files: the same directory, or
    /// one inside the other, after resolving symlinks. The paths are as the
    /// operator gave them. An environment problem, so exit 3.
    #[error("roots {a} and {b} overlap: each file must belong to exactly one root")]
    OverlappingRoots {
        a: std::path::PathBuf,
        b: std::path::PathBuf,
    },
```

and add `| PipelineError::OverlappingRoots { .. }` to the `=> 3` arm of `exit_code`.
Also extend CLAUDE.md's exit-code sentence in Task 8, not here.

- [ ] **Step 5: `init` checks the paths**

In `crates/diskrement-shell/src/pipeline/mod.rs`:

1. Add `Sources` back to the `use diskrement_core::{…}` list if Task 4 removed it.
2. In `init`, after the `for src in pi.sources.paths() { … }` loop and before
   `snapshot::check_cow_capable(…)`, add:

   ```rust
           if let Some((a, b)) = overlapping_roots(&pi.sources).map_err(Self::failed)? {
               return Err(PipelineError::OverlappingRoots { a, b });
           }
   ```

3. Replace `same_filesystem` with a version that names the path it could not read:

   ```rust
   /// `getFileStore` equality via `st_dev` (`PipelineLive.sameFilesystem`): both
   /// paths must exist and carry the same device id. An error names the path,
   /// so a missing root says which one.
   fn same_filesystem(source: &Path, work: &Path) -> std::io::Result<(u64, u64)> {
       let dev = |p: &Path| {
           std::fs::metadata(p)
               .map(|m| m.dev())
               .map_err(|e| std::io::Error::new(e.kind(), format!("{}: {e}", p.display())))
       };
       Ok((dev(source)?, dev(work)?))
   }

   /// Two roots that would archive the same files: the same directory, or one
   /// inside the other. Compared after resolving symlinks, component by
   /// component (`/lib/u1` does not hold `/lib/u10`), and returned as given. A
   /// single source has nothing to overlap.
   fn overlapping_roots(sources: &Sources) -> std::io::Result<Option<(PathBuf, PathBuf)>> {
       let Sources::Rooted(roots) = sources else {
           return Ok(None);
       };
       let canonical = roots
           .iter()
           .map(|(_, given)| given.canonicalize().map(|c| (c, given)))
           .collect::<std::io::Result<Vec<(PathBuf, &PathBuf)>>>()?;
       for (i, (a, a_given)) in canonical.iter().enumerate() {
           for (b, b_given) in &canonical[i + 1..] {
               if a.starts_with(b) || b.starts_with(a) {
                   return Ok(Some(((*a_given).clone(), (*b_given).clone())));
               }
           }
       }
       Ok(None)
   }
   ```

- [ ] **Step 6: Run the new tests**

Run: `cargo test -p diskrement-shell roots_tests error::tests`
Expected: PASS. The cross-filesystem test prints `SKIP:` on a machine without a
separate `/dev/shm`; that is fine.

- [ ] **Step 7: Verify and commit**

Run the three verification commands. Expected counts: shell lib 388 (380 + 8 in
`roots_tests`); the two `error.rs` tests were extended, not added.

```bash
git add crates/diskrement-shell/src
git commit -m "feat(pipeline): init checks every root; a rooted cycle fills by date across roots

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: `init --root NAME=PATH`

**Files:**
- Modify: `crates/diskrement-cli/src/args.rs`
- Modify: `crates/diskrement-cli/src/main.rs`
- Modify: `crates/diskrement-cli/tests/binary.rs`

- [ ] **Step 1: Write the failing argument tests**

In `crates/diskrement-cli/src/args.rs` tests, update the two `init_*` tests:
- `init_with_defaults`: the destructuring becomes
  `let Command::Init { name, source, root, work_dir, disc_type, margin, copies } = cli.command`,
  `assert_eq!(source, PathBuf::from("/srv/p"));` becomes
  `assert_eq!(source, Some(PathBuf::from("/srv/p")));`, and add `assert!(root.is_empty());`.
- `init_with_explicit_disc_type_margin_copies`: the destructuring becomes
  `let Command::Init { disc_type, margin, copies, .. } = cli.command`, and delete
  `let _ = (name, source, work_dir);`.

Then add at the end of `mod tests`:

```rust
    fn init_args(extra: &[&str]) -> Vec<String> {
        ["diskrement", "init", "fam", "--work-dir", "/w"]
            .iter()
            .chain(extra)
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn init_takes_repeated_named_roots() {
        let cli = Cli::try_parse_checked_from(init_args(&[
            "--root",
            "family/anna=/lib/u1",
            "--root",
            "cecil=/lib/u3",
        ]))
        .unwrap();
        let Command::Init { source, root, .. } = cli.command else {
            panic!("not init")
        };
        assert_eq!(source, None);
        assert_eq!(
            root,
            vec![
                ("family/anna".parse().unwrap(), PathBuf::from("/lib/u1")),
                ("cecil".parse().unwrap(), PathBuf::from("/lib/u3")),
            ]
        );
    }

    #[test]
    fn a_root_path_may_contain_equals() {
        let cli = Cli::try_parse_checked_from(init_args(&["--root", "anna=/lib/a=b"])).unwrap();
        let Command::Init { root, .. } = cli.command else {
            panic!("not init")
        };
        assert_eq!(root[0].1, PathBuf::from("/lib/a=b"));
    }

    #[test]
    fn init_needs_exactly_one_of_source_and_root() {
        let neither = Cli::try_parse_checked_from(init_args(&[])).unwrap_err();
        assert_eq!(
            neither.kind(),
            clap::error::ErrorKind::MissingRequiredArgument
        );
        let both = Cli::try_parse_checked_from(init_args(&["--source", "/s", "--root", "a=/lib/a"]))
            .unwrap_err();
        assert_eq!(both.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn a_malformed_root_is_refused() {
        for bad in ["anna", "anna=", "=/lib/a", "family//anna=/lib/a", "../x=/lib/a"] {
            let err = Cli::try_parse_checked_from(init_args(&["--root", bad])).unwrap_err();
            assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation, "{bad}");
        }
    }

    #[test]
    fn root_names_given_twice_or_nested_are_refused() {
        for [first, second] in [
            ["anna=/lib/a", "anna=/lib/b"],
            ["family=/lib/f", "family/anna=/lib/a"],
        ] {
            let err = Cli::try_parse_checked_from(init_args(&["--root", first, "--root", second]))
                .unwrap_err();
            assert_eq!(
                err.kind(),
                clap::error::ErrorKind::ValueValidation,
                "{first} {second}"
            );
        }
        assert!(Cli::try_parse_checked_from(init_args(&[
            "--root",
            "family=/lib/f",
            "--root",
            "familyx=/lib/x",
        ]))
        .is_ok());
    }
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p diskrement-cli --bin diskrement args`
Expected: compile error, because `Command::Init` has no `root` field and `Cli::try_parse_checked_from` does not exist.

- [ ] **Step 3: The flag**

In `crates/diskrement-cli/src/args.rs`:

1. Imports become:

   ```rust
   use std::ffi::OsString;
   use std::path::PathBuf;

   use clap::{CommandFactory as _, Parser, Subcommand};
   use diskrement_core::{DiscArg, DiscType, RootName, RootsError, Sources};
   ```

2. Replace the `Init` variant's attribute line and its `source` field. Today they read:

   ```rust
       /// Create a project and run filesystem checks.
       Init {
           name: String,
           #[arg(long)]
           source: PathBuf,
   ```

   Change them to:

   ```rust
       /// Create a project and run filesystem checks.
       #[command(group(
           clap::ArgGroup::new("sources")
               .required(true)
               .args(["source", "root"])
       ))]
       Init {
           name: String,
           /// The directory to archive. Exclusive with --root.
           #[arg(long)]
           source: Option<PathBuf>,
           /// A named directory to archive, as NAME=PATH; repeat for each.
           /// Its files go under NAME/ on every disc, and NAME may contain '/'.
           /// Exclusive with --source.
           #[arg(long = "root", value_name = "NAME=PATH", value_parser = root_arg)]
           root: Vec<(RootName, PathBuf)>,
   ```

3. Add after the `Cli` struct:

   ```rust
   impl Cli {
       /// `try_parse_from`, plus the check clap cannot express on its own: the
       /// `--root` names as a set. A name given twice, or one inside another,
       /// is a clap `ValueValidation` error, so it exits 1 like any usage
       /// error, before anything opens the catalog.
       pub fn try_parse_checked_from<I, T>(args: I) -> Result<Cli, clap::Error>
       where
           I: IntoIterator<Item = T>,
           T: Into<OsString> + Clone,
       {
           let cli = Cli::try_parse_from(args)?;
           if let Command::Init { source, root, .. } = &cli.command {
               init_sources(source.clone(), root.clone()).map_err(|e| {
                   Cli::command().error(clap::error::ErrorKind::ValueValidation, e)
               })?;
           }
           Ok(cli)
       }
   }

   /// A project's sources from `init`'s flags. The `sources` group guarantees
   /// exactly one of `--source` and `--root` is present.
   pub fn init_sources(
       source: Option<PathBuf>,
       root: Vec<(RootName, PathBuf)>,
   ) -> Result<Sources, RootsError> {
       match source {
           Some(path) => Ok(Sources::Single(path)),
           None => Sources::rooted(root),
       }
   }

   /// One `--root NAME=PATH`, split at the first `=`: a path may contain `=`,
   /// a name may not. The name is checked here; the names as a set are checked
   /// by `Cli::try_parse_checked_from`.
   fn root_arg(raw: &str) -> Result<(RootName, PathBuf), String> {
       let (name, path) = raw
           .split_once('=')
           .ok_or_else(|| format!("expected NAME=PATH, got {raw}"))?;
       if path.is_empty() {
           return Err(format!("root {name} has no path (expected NAME=PATH)"));
       }
       Ok((name.parse()?, PathBuf::from(path)))
   }
   ```

- [ ] **Step 4: `main` parses with the check and builds `Sources`**

In `crates/diskrement-cli/src/main.rs`:

1. Delete `use clap::Parser as _;`. `main` no longer calls a `Parser` method itself.
2. Change `match args::Cli::try_parse() {` to
   `match args::Cli::try_parse_checked_from(std::env::args_os()) {`. Keep the comment
   above it and add one line to it: `// \`try_parse_checked_from\` also refuses bad \`--root\` sets the same way.`
3. In the `Command::Init { … }` pattern, add `root,` after `source,`. In the arm, before
   `let pi = ProjectInit {`, add:

   ```rust
               // Cannot fail: `try_parse_checked_from` already accepted the roots.
               let sources = args::init_sources(source, root)
                   .map_err(|e| PipelineError::Failed(e.to_string()))?;
   ```

   and change `sources: Sources::Single(source),` to `sources,`. Remove
   `use diskrement_core::Sources;` if it is now unused.

- [ ] **Step 5: Run the argument tests**

Run: `cargo test -p diskrement-cli --bin diskrement args`
Expected: PASS.

- [ ] **Step 6: Binary tests**

Append to `crates/diskrement-cli/tests/binary.rs`:

```rust
mod roots {
    use super::*;
    use std::path::{Path, PathBuf};

    fn cmd() -> Command {
        Command::cargo_bin("diskrement").expect("binary builds")
    }

    /// Two Immich-style libraries under `lib/`; the catalog's directory does
    /// not exist yet, so a test can tell whether a command opened it.
    struct Fixture {
        _dir: tempfile::TempDir,
        lib: PathBuf,
        catalog: PathBuf,
        work: PathBuf,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let lib = root.join("lib");
        for rel in ["u1/2021/2021-03-01/a.jpg", "u2/2020/2020-01-01/b.jpg"] {
            let p = lib.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, rel.as_bytes()).unwrap();
        }
        Fixture {
            lib,
            catalog: root.join("catalog/cat.sqlite"),
            work: root.join("work"),
            _dir: dir,
        }
    }

    fn root(name: &str, path: &Path) -> String {
        format!("{name}={}", path.display())
    }

    fn init(f: &Fixture, roots: &[String]) -> assert_cmd::assert::Assert {
        let mut c = cmd();
        c.arg("--catalog")
            .arg(&f.catalog)
            .args(["init", "fam", "--work-dir"])
            .arg(&f.work);
        for r in roots {
            c.arg("--root").arg(r);
        }
        c.assert()
    }

    #[test]
    fn status_lists_the_roots_and_plan_scans_under_their_names() {
        let f = fixture();
        std::fs::create_dir_all(f.catalog.parent().unwrap()).unwrap();
        init(
            &f,
            &[
                root("family/anna", &f.lib.join("u1")),
                root("bela", &f.lib.join("u2")),
            ],
        )
        .success()
        .stdout(contains("Created project fam (id 1)."));

        cmd()
            .arg("--catalog")
            .arg(&f.catalog)
            .args(["status", "fam"])
            .assert()
            .success()
            .stdout(
                contains("Project fam  (2 roots, BD_R, margin 2.0%)\nRoots:\n")
                    .and(contains(format!("  bela → {}\n", f.lib.join("u2").display())))
                    .and(contains(format!(
                        "  family/anna → {}\n",
                        f.lib.join("u1").display()
                    ))),
            );

        cmd()
            .arg("--catalog")
            .arg(&f.catalog)
            .args(["plan", "fam"])
            .assert()
            .success()
            .stdout(contains(
                "Scanned: 2 added, 0 modified, 0 deleted, 0 unchanged.",
            ));
        let scan = std::fs::read_to_string(f.work.join("staged/scan.json")).unwrap();
        assert!(scan.contains("family/anna/2021/2021-03-01/a.jpg"), "{scan}");
        assert!(scan.contains("bela/2020/2020-01-01/b.jpg"), "{scan}");
    }

    #[test]
    fn source_and_root_together_or_neither_are_usage_errors() {
        let f = fixture();
        init(&f, &[]).failure().code(1).stderr(contains("error:"));
        cmd()
            .arg("--catalog")
            .arg(&f.catalog)
            .args(["init", "fam", "--work-dir"])
            .arg(&f.work)
            .arg("--source")
            .arg(&f.lib)
            .arg("--root")
            .arg(root("anna", &f.lib.join("u1")))
            .assert()
            .failure()
            .code(1)
            .stderr(contains("error:"));
        assert!(!f.catalog.exists(), "a usage error opens no catalog");
    }

    #[test]
    fn bad_root_names_are_usage_errors_before_any_catalog_opens() {
        let f = fixture();
        let (u1, u2) = (f.lib.join("u1"), f.lib.join("u2"));
        for (roots, needle) in [
            (vec![root("family//anna", &u1)], "empty component"),
            (vec![root("anna", &u1), root("anna", &u2)], "given twice"),
            (
                vec![root("family", &u1), root("family/anna", &u2)],
                "must not nest",
            ),
        ] {
            init(&f, &roots).failure().code(1).stderr(contains(needle));
        }
        assert!(!f.catalog.exists(), "a usage error opens no catalog");
    }

    #[test]
    fn overlapping_root_paths_exit_3() {
        let f = fixture();
        std::fs::create_dir_all(f.catalog.parent().unwrap()).unwrap();
        init(&f, &[root("all", &f.lib), root("anna", &f.lib.join("u1"))])
            .failure()
            .code(3)
            .stderr(contains("overlap"));
    }

    #[test]
    fn a_missing_root_exits_5_naming_it() {
        let f = fixture();
        std::fs::create_dir_all(f.catalog.parent().unwrap()).unwrap();
        let gone = f.lib.join("gone");
        init(&f, &[root("anna", &f.lib.join("u1")), root("gone", &gone)])
            .failure()
            .code(5)
            .stderr(contains(gone.display().to_string()));
    }
}
```

Spec §6's "root on a different filesystem" row has no binary test: a temp dir offers no
second filesystem. `roots_tests::init_refuses_a_root_on_another_filesystem_naming_it`
(Task 5) covers it where `/dev/shm` is one.

The `std::fs::create_dir_all(f.catalog.parent()…)` lines make sure the commands that get
as far as the catalog do not fail for an unrelated reason. Check this: `main`'s
`create_dir_all(&resolved.data_dir)` may already create it. If so, drop those lines and
note it.

- [ ] **Step 7: Verify and commit**

Run the three verification commands. Expected counts: cli bin unit 114 (109 + 5);
`binary` 51 (46 + 5).

Also look at the help text by hand: `cargo run -q -p diskrement-cli -- init --help`.
It should list `--source <SOURCE>` and `--root <NAME=PATH>` with the doc comments.

```bash
git add crates/diskrement-cli
git commit -m "feat(cli): init --root NAME=PATH archives several named directories

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Scenario 15 — a rooted project end to end

**Files:**
- Create: `crates/diskrement-cli/tests/scenarios/s15_roots.rs`
- Modify: `crates/diskrement-cli/tests/scenarios/main.rs`

- [ ] **Step 1: Write the scenario**

Create `crates/diskrement-cli/tests/scenarios/s15_roots.rs`:

```rust
//! Scenario 15: three Immich-style libraries as named roots of one project.
//! Discs fill by date across the libraries rather than one library after
//! another. Each disc carries the roots as readable top-level directories,
//! and its embedded catalog records them. Unlike s05, which file lands on
//! which disc *is* the contract here (project-roots spec §5).

use crate::{disc, support};

#[test]
fn a_rooted_project_fills_discs_by_date_across_its_roots() {
    let Some(s) = support::scratch_or_skip("s15 named roots") else {
        return;
    };
    // 400-byte files under a 1000-byte cap: two per disc.
    let cli = s.cli_with_disc_bytes(1000);
    for f in [
        "u1/2021/2021-03-01/a.jpg",
        "u1/2023/2023-07-01/d.jpg",
        "u2/2020/2020-01-01/b.jpg",
        "u3/2022/2022-05-05/c.jpg",
    ] {
        support::write(&s.source, f, 400);
    }
    let root = |name: &str, dir: &str| format!("{name}={}", s.source.join(dir).display());
    let work = s.work.display().to_string();
    let (anna, bela, cecil) = (
        root("family/anna", "u1"),
        root("family/bela", "u2"),
        root("others/cecil", "u3"),
    );
    let r = cli.run(&[
        "init", "fam", "--work-dir", &work, "--root", &anna, "--root", &bela, "--root", &cecil,
    ]);
    assert_eq!(r.code, 0, "init failed: {}{}", r.stdout, r.stderr);

    // Cycle 1: the two oldest files, from two different libraries.
    let pl1 = cli.run(&["plan", "fam"]);
    assert_eq!(pl1.code, 0, "{}{}", pl1.stdout, pl1.stderr);
    assert!(pl1.out_has("Scanned: 4 added"), "{}", pl1.stdout);
    let b1 = cli.run(&["burn", "fam", "--yes"]);
    assert_eq!(b1.code, 0, "{}{}", b1.stdout, b1.stderr);
    assert_eq!(
        disc::data_files(&s.disc_dir(1)),
        support::set(&[
            "family/anna/2021/2021-03-01/a.jpg",
            "family/bela/2020/2020-01-01/b.jpg",
        ])
    );

    // Cycle 2: the rest.
    let pl2 = cli.run(&["plan", "fam"]);
    assert_eq!(pl2.code, 0, "{}{}", pl2.stdout, pl2.stderr);
    let b2 = cli.run(&["burn", "fam", "--yes"]);
    assert_eq!(b2.code, 0, "{}{}", b2.stdout, b2.stderr);
    assert_eq!(
        disc::data_files(&s.disc_dir(2)),
        support::set(&[
            "family/anna/2023/2023-07-01/d.jpg",
            "others/cecil/2022/2022-05-05/c.jpg",
        ])
    );

    // The disc describes its own layout: its embedded catalog has the roots.
    // `immutable=1`, so reading it writes nothing beside it.
    let embedded = rusqlite::Connection::open_with_flags(
        format!(
            "file:{}?immutable=1",
            s.disc_dir(2).join(".diskrement/catalog.sqlite").display()
        ),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .unwrap();
    let names: Vec<String> = embedded
        .prepare("select name from project_root order by name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(names, ["family/anna", "family/bela", "others/cecil"]);

    let st = cli.run(&["status", "fam"]);
    assert!(st.out_has("(3 roots, BD_R"), "{}", st.stdout);
    assert!(st.out_has("  family/bela → "), "{}", st.stdout);
    assert_eq!(cli.run(&["verify", "fam"]).code, 0);
    let h = cli.run(&["history", "fam", "family/bela/2020/2020-01-01/b.jpg"]);
    assert_eq!(h.code, 0, "{}{}", h.stdout, h.stderr);
    assert!(h.out_has("  v1  disc 1 / session 1"), "{}", h.stdout);
}
```

Add `mod s15_roots;` after `mod s14_deferred_deletions;` in
`crates/diskrement-cli/tests/scenarios/main.rs`.

- [ ] **Step 2: Run it**

Run: `DISKREMENT_SCENARIOS_REQUIRED=1 cargo test -p diskrement-cli --test scenarios s15`
Expected: PASS, and not `SKIPPED`.

- [ ] **Step 3: Verify and commit**

Run the three verification commands. Expected counts: `scenarios` 21.

```bash
git add crates/diskrement-cli/tests/scenarios
git commit -m "test(cli): scenario 15, a rooted project fills discs by date across roots

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: Docs

**Files:**
- Modify: `README.md`, `CLAUDE.md`
- Modify: this plan (execution notes)

- [ ] **Step 1: README**

In `README.md`, after the paragraph that begins
`The per-project catalog defaults to` and before `### Real burning`, add:

````markdown
### Several directories in one project

Instead of one `--source`, a project can archive several directories onto one disc
series, each under a name you choose:

```
diskrement init family --work-dir /srv/work \
  --root family/anna=/srv/immich/library/65966390-… \
  --root family/bela=/srv/immich/library/4efdf184-… \
  --root others/cecil=/srv/immich/library/48199862-…
```

Each root's files appear on the discs under its name (`family/anna/2024/…`). Discs fill
in order of the path *below* each root, then the root name, so libraries laid out by date
(Immich's `YYYY/YYYY-MM-DD/`) interleave: the first disc holds the oldest files from
every root. A name may contain `/`, but one name may not sit inside another (`family`
with `family/anna`), and two roots may not share a directory. The roots are fixed at
`init`. `status` lists them.
````

- [ ] **Step 2: CLAUDE.md**

Add to the end of "Gotchas that will bite":

```markdown
- **Fill order is `FillOrder::key`, not `rel_path`.** A rooted project (`init --root
  NAME=PATH`) fills discs by *(path below the root, root name)*, so its libraries
  interleave by date (Immich's `YYYY/YYYY-MM-DD/` layout). Anything that orders files
  for placement goes through the project's `FillOrder::from(&sources)`, the same one
  for every copy set. Reports (`oversized`, `unplaceable_catch_up`) stay in `rel_path`
  order.
```

Add to "Invariants — do not "fix" these", after the embedded-catalog bullet:

```markdown
- **A rooted project is one tree from the snapshot on.** `plan` reflinks each root to `<work_dir>/snapshot/<name>`; scanning, the catalog, staging and both verifiers see ordinary `rel_path`s under the root names, and only the planner's fill order knows roots exist. Roots are written once, by `init` (`project_root`, schema V4; `project.source_root` holds `''`), and never changed: renaming one would read as deleting and re-adding every file under it.
```

In the exit-code bullet, change
`3 = environment/oversize (including source names that are not UTF-8)` to
`3 = environment/oversize (including source names that are not UTF-8 and overlapping roots)`.

- [ ] **Step 3: Execution notes**

Append to this plan:

```markdown
---

## Execution notes

<one bullet per deviation from the steps above, with the reason; "None." if there were none>
```

Fill it in with every deviation recorded during Tasks 1–7.

- [ ] **Step 4: Verify and commit**

Run the three verification commands one last time. Expected final counts: core 99;
shell lib 388, `fixture_catalog` 9, `fixture_verify` 6, `timestamps` 2,
`xorriso_image` 5; cli bin unit 114, `binary` 51, `scenarios` 21.

```bash
git add README.md CLAUDE.md docs/superpowers/plans/2026-10-15-project-roots.md
git commit -m "docs: several named roots per project; fill order is FillOrder::key

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

Then merge per the repo etiquette (`superpowers:finishing-a-development-branch`):
`git switch main && git merge --no-ff feat/project-roots -m "Merge feat/project-roots: several named roots share one disc series"`,
delete the branch locally and on `origin`, and push `main`.

---

## Execution notes

- Task 1 (review fix, 21c3155): `RootName` also refuses control characters (replacing the NUL-only check) and components over 255 bytes (`NAME_MAX`, Linux and Rock Ridge), because names are permanent once stored in a catalog or burned; spec §3.2/§8 updated. `RootsError` Display quotes names (`{:?}`). `FillOrder::key`'s doc comment reworded: the key is unique because it spells the path back; non-nesting only makes a path's root well-defined.
- Task 2: the plan's perl for the 17 `plan_from` test calls also matched the new test's `|_| 1,` line when run after appending it; the duplicate `&FillOrder::Path,` was removed by hand.
- Task 2 (review fix, 67fa79b): `rooted_planner_input_strategy` sometimes puts the same base file under a second root, so the root-name tie-break in `FillOrder::key` is exercised (a mutated key fails property 10). Spec §5 notes that a rooted disc's manifest lists and catalog rows follow fill order.
- Task 3 (review fix, 65f3a57): a rooted clone's `NotCowCapable` error names the root and its path (`classify_root`), since with several roots the usual EXDEV cause is a root that moved filesystems after `init`. New test `a_rooted_clone_that_cannot_reflink_names_its_root` (+1 shell lib test: every later expected shell lib count is one higher than the plan says); the missing-root test also pins that a stale snapshot survives.
- Task 4 (review fix, 46b87bd): `create_project` re-checks a `Sources::Rooted` through `Sources::rooted` (and refuses an unsorted list and an empty `Single`) before writing anything, because the variant is public; `find_project` refuses a project with `source_root = ''` and no roots instead of reading it as `Single("")`, and prefixes stored-root errors with `project_root of project {name}:`. +2 shell lib tests (`malformed_sources_are_refused_before_anything_is_written`, `a_project_with_neither_a_source_nor_roots_does_not_read_back`); the fixture test was renamed `a_v2_catalog_migrates_to_current_keeping_every_disc_id`. Shell lib now runs 3 above the plan's numbers.
- Task 5 (review fix, ed24f93): the overlap tests pin that `OverlappingRoots` reports the paths as given (the symlink-alias test fails if canonical paths are returned); a `canonicalize` failure in `overlapping_roots` names its path; the `init` doc comment lists the overlap check.
- Task 6: `mod roots` in `tests/binary.rs` drops the plan's three `create_dir_all(f.catalog.parent())` lines (the plan allowed it): with `--catalog`, `paths::resolve` makes the catalog's parent the data dir and `dispatch` creates it before `init` runs.
- Task 6 (review fix, dda04a6): a duplicate or nested `--root` error is built against the `init` subcommand so it prints init's usage line; `root_arg`'s doc says the `&str` parser is deliberate (a non-UTF-8 root would be stored lossily and never plan); `dispatch` validates the roots before creating the work dir or opening the catalog.
- Task 7 (review fix, 6a7e77a): the end-to-end data in `s15_roots.rs` and `pipeline/roots_tests.rs` moves anna's older file to 2022 and cecil's to 2021, so disc 1 = {bela 2020, cecil 2021} and disc 2 = {anna 2022, anna 2023}. The plan's data split the same way under group-first, round-robin and basename order; now only the date key gives that disc 1, and it crosses the `family/`/`others/` boundary (a rel_path-order mutation of `FillOrder::key` fails both tests). s15 also runs with `--copies 2` (twins compared, embedded catalog read from 2B, history names 1A and 1B), asserts every `status` root line and checks that `verify` covers all four discs.
- Task 8: the README and CLAUDE.md text goes beyond the plan's. The README section also documents the name rules (no empty, `.` or `..` component; no component over 255 bytes; no control character; no `=`), that `--root` splits at its first `=`, that a non-UTF-8 path is a usage error (exit 1), and that overlapping roots exit 3 and report the paths as given. The CLAUDE.md invariant bullet adds the `RootName` limits, the parse-time non-UTF-8 refusal, the as-given overlap paths and the root-naming `NotCowCapable`. The spec needed no change: §8 does not describe the end-to-end data, only that disc 1A holds the oldest files across roots, which still holds.
- Final counts against the plan's expected ones: core 99 (as planned); shell lib 391 (plan 388: +3 from the review-fix tests of Tasks 3 and 4); `fixture_catalog` 9, `fixture_verify` 6, `timestamps` 2, `xorriso_image` 5 (as planned); cli bin unit 114, `binary` 51, `scenarios` 21 (as planned). Run with `DISKREMENT_SCENARIOS_REQUIRED=1`; clippy and `cargo fmt --check` clean.
