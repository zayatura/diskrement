# Rust Rewrite — Phase P2: The Catalog

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port `diskrement.shell.catalog` and `diskrement.shell.model` into `diskrement-shell` — the schema and its two migrations transcribed verbatim, the `time` module that owns the `java.time.Instant.toString()` wire format, and all fifteen store operations — proving compatibility against the P0 fixture corpus on every run.

**Architecture:** One `rusqlite::Connection` owned by a `Catalog` struct; no pool, no trait, no injected service. Readers take `&self`, writers take `&mut self` and go through `Catalog::tx`, whose `Transaction` guard rolls back on drop unless committed — which is what retires the `setAutoCommit(false)` gotcha structurally rather than by convention. Every SQL statement is transcribed from the Scala, because the queries *are* the specification of the derived-`DEAD`, `LOST`-per-copy, and current-version semantics.

**Tech Stack:** Rust stable (pinned by `rust-toolchain.toml`, edition 2021), `rusqlite` 0.32 with the `bundled` feature, `chrono` 0.4, `thiserror` 2, `tempfile` 3.

**Spec:** `docs/superpowers/specs/2026-09-05-rust-rewrite-design.md` — this plan implements phase P2 (§9). The compatibility contract it defends is §3.1; the subsystem shape is §5.5; the fixture comparison rules are §4.

**Branch:** `feat/rewrite-p2`, forked from `feat/rust-rewrite` (spec §10). Create it before Task 1; Task 14 merges it back with an explicit merge commit and deletes it.

```bash
git switch feat/rust-rewrite && git switch -c feat/rewrite-p2
```

---

## Context for the implementer

You are porting a working Scala 3 program. The sources for this phase are:

| Scala | ported to |
|---|---|
| `shell/src/main/scala/diskrement/shell/catalog/Migrations.scala` | `catalog/schema.rs` |
| `shell/src/main/scala/diskrement/shell/catalog/Db.scala` | `catalog/mod.rs` (mostly deleted — see below) |
| `shell/src/main/scala/diskrement/shell/catalog/CatalogStore.scala` | `catalog/mod.rs` + `catalog/store.rs` |
| `shell/src/main/scala/diskrement/shell/catalog/CatalogStoreLive.scala` | `catalog/store.rs` + `catalog/read.rs` |
| `shell/src/main/scala/diskrement/shell/model/*.scala` | `catalog/model.rs` |

and their tests in `shell/src/test/scala/diskrement/shell/catalog/`. **Read the Scala
file before porting it.** It is the specification, and its comments explain
decisions the code alone does not. That tree still builds and is not deleted
until P9.

Seven things about this port that are easy to get wrong:

1. **Transcribe the SQL, don't rewrite it.** The DDL is frozen by the
   data-compatibility contract, `check` constraints included. The queries are
   frozen by something subtler: `readView`, `fileHistory`, `archivePointers` and
   `deadDiscVersions` each encode a domain rule in a window function, and a
   "cleaner" rewrite is how a porting defect gets in. Change whitespace and
   parameter syntax; change nothing else.

2. **`DEAD` is never written.** `disc.status` keeps its
   `check (status in ('OPEN','FINALIZED'))` constraint forever; dead-ness is
   derived from `disc_event` inside the read queries. Task 9's second test
   asserts the disc *row* is unchanged, not merely that the derived value reads
   `DEAD` — a test asserting only the derived value passes against an illegal
   update-in-place.

3. **Types cross a boundary here.** The core uses `u64` for byte counts and
   `u32` for ordinals; SQLite has only `i64`. Cast explicitly at the boundary
   (`ord as i64` on the way in, `row.get::<_, i64>(0)? as u32` on the way out)
   rather than leaning on whichever `FromSql` impls happen to exist. Every value
   in this domain is far below `i64::MAX`.

4. **The fixture corpus must be opened immutably.** A plain read-only open of a
   WAL-mode database creates `-wal` and `-shm` files beside it — which would
   dirty `fixtures/` and invalidate `SHA256SUMS`. Open every fixture through the
   `file:...?immutable=1` URI (Task 1 writes the helper; use it everywhere).

5. **`Db.scala` mostly evaporates.** No pool, no `wrapConn` proxy, no
   `Clock.ClockLive` dance — three of the six `CLAUDE.md` gotchas die with it.
   What survives is one line of pragmas: `journal_mode=WAL`, `foreign_keys=ON`,
   `synchronous=FULL`.

6. **Reads need no transaction.** The Scala wrapped every read in
   `transaction(...)` because that was how zio-jdbc handed you a connection. Here
   a read is `self.conn.prepare(...)`. Only writers open a transaction.

7. **Deferred debt is ported as-is (spec §1).** `fingerprints` is deliberately
   *not* filtered by dead disc, and `archivePointers` has no dead flag. Task 12
   pins the first of those against fixture `08-after-mark-dead.sqlite`, where
   `read_view` is empty but `fingerprints` still returns `a.bin`. That asymmetry
   is intended; do not "fix" it.

### What "done" means for this phase

```
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

All three clean. Run all three before every commit. Run plain `cargo fmt` first —
the code in this plan is written to be readable in a document, not to be
byte-identical to rustfmt's output, and `--check` is a gate rather than a
formatter.

A fixture test that silently asserts nothing is worse than no test. Every sweep
over the corpus in this plan asserts a nonzero count of what it examined; keep
that habit if you add more.

### Test counts

Baseline from P1: `diskrement-core` lib **57**, `diskrement-shell` lib **1**
(the `probe` smoke test, deleted in Task 1), `diskrement-cli` `tests/binary.rs`
**1**. Each task states the number `cargo test -p diskrement-shell --lib` should
report. The phase ends at:

| binary | end of P2 |
|---|---|
| `diskrement-core` lib | 58 |
| `diskrement-shell` lib | 43 |
| `diskrement-shell` `tests/timestamps.rs` | 2 |
| `diskrement-shell` `tests/fixture_schema.rs` | 1 |
| `diskrement-shell` `tests/fixture_catalog.rs` | 7 |
| `diskrement-shell` `tests/fixture_replay.rs` | 1 |
| `diskrement-cli` `tests/binary.rs` | 1 |

---

## File structure

| File | Responsibility |
|------|----------------|
| `crates/diskrement-shell/src/lib.rs` | Module declarations |
| `crates/diskrement-shell/src/error.rs` | `CatalogError` and the rusqlite classification helpers |
| `crates/diskrement-shell/src/catalog/mod.rs` | The `Catalog` struct, `open`, the `tx` guard, re-exports |
| `crates/diskrement-shell/src/catalog/time.rs` | `format` / `parse` for the frozen timestamp format |
| `crates/diskrement-shell/src/catalog/schema.rs` | v1 + v2 DDL, verbatim; the forward migration runner |
| `crates/diskrement-shell/src/catalog/model.rs` | Row and update types (`ProjectInit`, `CatalogUpdate`, `Session`, …) |
| `crates/diskrement-shell/src/catalog/store.rs` | Write path: projects, `apply_update`, `mark_verified`, `mark_dead`, `snapshot_to` |
| `crates/diskrement-shell/src/catalog/read.rs` | Read path: views, fingerprints, disc states, pointers, history |
| `crates/diskrement-shell/src/catalog/testutil.rs` | `#[cfg(test)]` only: temp catalogs and update builders |
| `crates/diskrement-shell/tests/support/mod.rs` | Fixture access, logical dumps, schema normalisation (reused P5+) |
| `crates/diskrement-shell/tests/timestamps.rs` | The corpus-wide timestamp round-trip sweep |
| `crates/diskrement-shell/tests/fixture_schema.rs` | Rust-migrated DDL vs the fixture's DDL |
| `crates/diskrement-shell/tests/fixture_catalog.rs` | Read methods against Scala-written catalogs |
| `crates/diskrement-shell/tests/fixture_replay.rs` | Replay burn cycle 1, diff the logical dump |
| `crates/diskrement-shell/README.md` | What the crate is, and the two P2 gotchas |
| `crates/diskrement-core/src/planner.rs` | *(modified)* `DiscStatus` wire spellings |

Unit tests live in `#[cfg(test)] mod tests` at the bottom of the file they test.
Cross-implementation fixture tests live in `tests/` because they must consume
the public API exactly as a later phase will.

Two deliberate differences from the spec's §5.3 file list. `store.rs` is split
into `store.rs` (writes) and `read.rs` (queries) — one file carrying all fifteen
operations plus their tests would run past a thousand lines. And `model.rs`
holds the types the Scala kept in a sibling `shell.model` package; they are
consumed only by the catalog, so they live with it.

---

## Task 1: The `time` module and the corpus timestamp sweep

This is the highest-risk item in the whole rewrite (spec §11), so it goes first.
Every time-shaped value in the catalog, the sidecar cache, and the staged JSON is
`java.time.Instant.toString()`: ISO-8601, `Z`-suffixed, with **variable**
fractional precision — no digits when the instant is whole-second, otherwise 3,
6, or 9. `chrono`'s `to_rfc3339_opts(SecondsFormat::AutoSi, true)` is claimed to
have exactly that rule. The sweep proves it against strings the Scala build
actually wrote.

**Files:**
- Create: `crates/diskrement-shell/src/catalog/mod.rs`
- Create: `crates/diskrement-shell/src/catalog/time.rs`
- Create: `crates/diskrement-shell/tests/support/mod.rs`
- Create: `crates/diskrement-shell/tests/timestamps.rs`
- Modify: `crates/diskrement-shell/src/lib.rs`

- [x] **Step 1: Replace the P0 scaffolding in `lib.rs`**

The `probe()` function was P0 scaffolding whose own comment says P2 deletes it.
Replace the whole file with:

```rust
//! diskrement's imperative shell: every effect lives here — the SQLite
//! catalog, file scanning, CoW snapshots, staging, and the burners
//! (spec §5.3, §5.5).

pub mod catalog;
pub mod error;
```

`error` is created in Task 2; add its `mod` line then. For this task the file
declares `pub mod catalog;` only.

- [x] **Step 2: Create the catalog module skeleton**

`crates/diskrement-shell/src/catalog/mod.rs`:

```rust
//! The SQLite catalog: schema, migrations, and every persistence operation.
//!
//! One `rusqlite::Connection`, no pool. diskrement opens the catalog, does its
//! work, and exits; there is no concurrency to pool for (spec §5.5).

pub mod time;
```

- [x] **Step 3: Write the failing tests**

`crates/diskrement-shell/src/catalog/time.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    /// The four fraction widths `java.time.Instant.toString()` can emit, plus
    /// two shapes that look like edge cases and are not: a trailing-zero
    /// millisecond value and a single nanosecond.
    const SHAPES: &[&str] = &[
        "2026-06-01T00:00:00Z",
        "2026-09-05T20:01:03.260Z",
        "2026-09-05T20:01:06.100Z",
        "2026-09-05T20:01:06.123456Z",
        "2026-09-05T20:01:06.623959846Z",
        "2026-09-05T20:01:06.000000001Z",
    ];

    #[test]
    fn every_fraction_width_round_trips_byte_for_byte() {
        for s in SHAPES {
            let parsed = parse(s).expect("fixture-shaped timestamp parses");
            assert_eq!(&format(&parsed), s, "round trip changed {s}");
        }
    }

    #[test]
    fn a_whole_second_instant_has_no_fraction() {
        let at = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        assert_eq!(format(&at), "2026-06-01T00:00:00Z");
    }

    #[test]
    fn a_non_utc_offset_parses_and_renders_as_utc() {
        // Nothing diskrement writes carries an offset, but a hand-edited
        // catalog might, and silently mis-parsing it would corrupt an ordering.
        let at = parse("2026-06-01T02:00:00+02:00").expect("offset form parses");
        assert_eq!(format(&at), "2026-06-01T00:00:00Z");
    }

    #[test]
    fn a_non_timestamp_is_rejected() {
        assert!(parse("").is_err());
        assert!(parse("yesterday").is_err());
        assert!(parse("2026-06-01").is_err());
    }
}
```

- [x] **Step 4: Run the tests to verify they fail**

```bash
cargo test -p diskrement-shell --lib
```

Expected: compilation errors — `cannot find function parse in this scope`.

- [x] **Step 5: Write the implementation**

Prepend to `crates/diskrement-shell/src/catalog/time.rs`, above `mod tests`:

```rust
//! The one place that converts between `chrono::DateTime<Utc>` and the
//! timestamp format frozen by the data-compatibility contract (spec §3.1).
//!
//! That format is whatever `java.time.Instant.toString()` produced: ISO-8601,
//! `Z`-suffixed, with zero fractional digits when the instant is whole-second
//! and otherwise 3, 6, or 9 — the shortest width that loses nothing.
//! `SecondsFormat::AutoSi` has the same rule. `tests/timestamps.rs` holds it to
//! that against every timestamp string in the P0 fixture corpus.
//!
//! Both directions live here so a later phase's JSON and sidecar-cache writers
//! cannot quietly grow a second, differently-behaved formatter.

use chrono::{DateTime, SecondsFormat, Utc};

/// Render an instant the way the Scala build wrote it.
pub fn format(at: &DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

/// Parse a timestamp written by either implementation.
pub fn parse(s: &str) -> Result<DateTime<Utc>, TimeError> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|source| TimeError {
            value: s.to_string(),
            source,
        })
}

/// A stored string that is not a timestamp. Reachable only from a corrupt or
/// hand-edited catalog, but it is data from outside the process, so it is an
/// error rather than a panic.
#[derive(Debug, thiserror::Error)]
#[error("not an ISO-8601 instant: {value:?}")]
pub struct TimeError {
    pub value: String,
    #[source]
    pub source: chrono::ParseError,
}
```

- [x] **Step 6: Run the tests to verify they pass**

```bash
cargo test -p diskrement-shell --lib
```

Expected: `test result: ok. 4 passed`.

- [x] **Step 7: Write the fixture support module**

`crates/diskrement-shell/tests/support/mod.rs`. This module is shared by four
integration test files; each of them uses part of it, so the `dead_code` allow
is required — without it `-D warnings` fails on the parts a given file does not
call.

```rust
//! Shared helpers for the fixture-compatibility tests.
//!
//! The corpus under `fixtures/` was harvested from the Scala build in P0 and is
//! the reference for every compatibility test from P2 onward (spec §4). It is
//! read-only in the strongest sense available: see `open_fixture`.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

/// Absolute path of the committed fixture corpus.
pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .canonicalize()
        .expect("fixtures/ is committed at the repository root")
}

/// Open a fixture database without touching it.
///
/// The corpus databases were left in WAL mode by the Scala build, and a plain
/// read-only open of a WAL database creates `-wal` and `-shm` files beside it —
/// which would dirty `fixtures/` and invalidate `SHA256SUMS`. The
/// `immutable=1` URI parameter tells SQLite the file cannot change, so it skips
/// the WAL machinery entirely and writes nothing.
pub fn open_fixture(rel: &str) -> Connection {
    let path = fixtures_dir().join(rel);
    assert!(path.is_file(), "missing fixture: {}", path.display());
    let uri = format!("file:{}?immutable=1", path.display());
    Connection::open_with_flags(
        &uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .unwrap_or_else(|e| panic!("opening fixture {rel}: {e}"))
}

/// Every `*.sqlite` under one fixture subdirectory, in filename order (the
/// filenames are numbered, so that is scenario order).
pub fn fixture_dbs(subdir: &str) -> Vec<String> {
    let dir = fixtures_dir().join(subdir);
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .map(|entry| entry.expect("readable dir entry").file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".sqlite"))
        .collect();
    names.sort();
    assert!(!names.is_empty(), "no fixture databases under {subdir}");
    names.into_iter().map(|n| format!("{subdir}/{n}")).collect()
}
```

- [x] **Step 8: Write the corpus sweep**

`crates/diskrement-shell/tests/timestamps.rs`:

```rust
//! Holds `catalog::time` to the strings the Scala build actually wrote.
//!
//! This is the test the spec's §11 risk exists for. It reads every timestamp
//! column of every fixture database and asserts `format(parse(s)) == s`, so a
//! drift in fractional-precision rules fails here rather than on burned media.

mod support;

use diskrement_shell::catalog::time;
use support::{fixture_dbs, open_fixture};

/// (table, column) pairs holding `Instant.toString()` output in the catalog.
const CATALOG_COLUMNS: &[(&str, &str)] = &[
    ("project", "created_at"),
    ("session", "burned_at"),
    ("file_version", "mtime"),
    ("file_version", "first_seen_at"),
    ("disc_event", "at"),
];

/// The same, for the fingerprint sidecar written by `scan`.
const SCANCACHE_COLUMNS: &[(&str, &str)] = &[
    ("fingerprint", "mtime"),
    ("fingerprint", "updated_at"),
];

fn sweep(dbs: &[String], columns: &[(&str, &str)]) -> (usize, Vec<usize>) {
    let mut checked = 0usize;
    let mut widths = Vec::new();
    for db in dbs {
        let conn = open_fixture(db);
        for (table, column) in columns {
            let sql = format!("select {column} from {table} where {column} is not null");
            let mut stmt = conn.prepare(&sql).expect("fixture column exists");
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .expect("column is text");
            for row in rows {
                let stored = row.expect("readable row");
                let parsed = time::parse(&stored)
                    .unwrap_or_else(|e| panic!("{db} {table}.{column}: {e}"));
                assert_eq!(
                    time::format(&parsed),
                    stored,
                    "{db} {table}.{column} did not round-trip"
                );
                let width = stored
                    .split_once('.')
                    .map(|(_, frac)| frac.trim_end_matches('Z').len())
                    .unwrap_or(0);
                widths.push(width);
                checked += 1;
            }
        }
    }
    (checked, widths)
}

#[test]
fn every_catalog_timestamp_round_trips() {
    let (checked, widths) = sweep(&fixture_dbs("catalog"), CATALOG_COLUMNS);
    // A sweep that examined nothing would pass silently; say what it saw.
    assert!(checked > 0, "the sweep read no timestamps at all");
    println!("checked {checked} catalog timestamps");
    // The corpus is documented (fixtures/README.md) to contain both a
    // millisecond-precision shape (file mtimes) and a nanosecond one
    // (createdAt, first_seen_at). If a re-harvest ever loses one of those, the
    // sweep stops covering the case it exists for.
    assert!(widths.contains(&3), "no 3-digit fraction in the corpus");
    assert!(widths.contains(&9), "no 9-digit fraction in the corpus");
}

#[test]
fn every_scancache_timestamp_round_trips() {
    let (checked, _) = sweep(&fixture_dbs("scancache"), SCANCACHE_COLUMNS);
    assert!(checked > 0, "the sweep read no timestamps at all");
    println!("checked {checked} scancache timestamps");
}
```

- [x] **Step 9: Run the sweep**

```bash
cargo test -p diskrement-shell --test timestamps -- --nocapture
```

Expected: `test result: ok. 2 passed`, with the two `checked N …` lines showing
nonzero counts.

If `every_catalog_timestamp_round_trips` fails on a nine-digit value, the
formatter is not using `SecondsFormat::AutoSi`. If it fails on
`...T20:01:06.100Z` by producing `.1`, the same. Do not "fix" it by padding to a
fixed width — Java's rule is variable, and a fixed width breaks the other cases.

- [x] **Step 10: Confirm the corpus is untouched**

```bash
git status --porcelain fixtures/
```

Expected: no output. If `-wal`/`-shm` files appear, something opened a fixture
without the `immutable=1` URI; delete them and fix the caller.

- [x] **Step 11: Verify lint and format, then commit**

```bash
cargo test -p diskrement-shell \
  && cargo clippy --workspace --all-targets -- -D warnings \
  && cargo fmt --check
git add crates/diskrement-shell
git commit -m "feat(catalog): port the Instant wire format, pinned to the fixture corpus"
```

Expected: lib 4 passed, timestamps 2 passed, clippy and rustfmt silent.

---

## Task 2: Schema, migrations, and opening a catalog

Port `Migrations.scala` and the surviving half of `Db.scala`. Read both first.

**Files:**
- Create: `crates/diskrement-shell/src/error.rs`
- Create: `crates/diskrement-shell/src/catalog/schema.rs`
- Create: `crates/diskrement-shell/src/catalog/testutil.rs`
- Create: `crates/diskrement-shell/tests/fixture_schema.rs`
- Modify: `crates/diskrement-shell/src/lib.rs`
- Modify: `crates/diskrement-shell/src/catalog/mod.rs`

- [x] **Step 1: Write the failing tests**

Append to `crates/diskrement-shell/src/catalog/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::testutil::temp_catalog;

    #[test]
    fn a_fresh_database_migrates_to_version_2_with_every_domain_table() {
        let (_dir, cat) = temp_catalog();
        let version: i64 = cat
            .conn
            .query_row("select max(version) from schema_version", [], |r| r.get(0))
            .expect("schema_version is populated");
        assert_eq!(version, 2);

        let mut stmt = cat
            .conn
            .prepare("select name from sqlite_master where type='table' order by name")
            .expect("sqlite_master is queryable");
        let tables: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .expect("names are text")
            .map(|r| r.expect("readable row"))
            .collect();

        for expected in [
            "change_record",
            "disc",
            "disc_event",
            "file_version",
            "lost_version",
            "placement",
            "project",
            "session",
        ] {
            assert!(tables.contains(&expected.to_string()), "missing {expected}");
        }
    }

    #[test]
    fn re_opening_a_catalog_re_runs_migrations_idempotently() {
        let (dir, cat) = temp_catalog();
        let path = dir.path().join("catalog.sqlite");
        drop(cat);

        let cat = super::Catalog::open(&path).expect("re-open migrates cleanly");
        let mut stmt = cat
            .conn
            .prepare("select version from schema_version")
            .expect("schema_version is queryable");
        let versions: Vec<i64> = stmt
            .query_map([], |r| r.get::<_, i64>(0))
            .expect("versions are integers")
            .map(|r| r.expect("readable row"))
            .collect();
        assert_eq!(versions, vec![2], "the version table must hold one row");
    }

    #[test]
    fn opening_applies_the_wal_foreign_key_and_synchronous_pragmas() {
        // Note the catalog is a real file: an in-memory database always reports
        // journal_mode=memory, which would make this test vacuous.
        let (_dir, cat) = temp_catalog();
        let journal: String = cat
            .conn
            .query_row("pragma journal_mode", [], |r| r.get(0))
            .expect("pragma readable");
        let foreign_keys: i64 = cat
            .conn
            .query_row("pragma foreign_keys", [], |r| r.get(0))
            .expect("pragma readable");
        let synchronous: i64 = cat
            .conn
            .query_row("pragma synchronous", [], |r| r.get(0))
            .expect("pragma readable");
        assert_eq!(journal, "wal");
        assert_eq!(foreign_keys, 1);
        assert_eq!(synchronous, 2);
    }

    #[test]
    fn the_disc_status_check_constraint_rejects_dead() {
        // The derived-DEAD invariant depends on this constraint existing, so it
        // is pinned here rather than assumed from the DDL text.
        let (_dir, cat) = temp_catalog();
        cat.conn
            .execute(
                "insert into project (name, source_root, work_dir, default_disc_type,
                                      margin_percent, copies, created_at)
                 values ('p', '/src', '/work', 'BD_R', 2.0, 1, '2026-06-01T00:00:00Z')",
                [],
            )
            .expect("project inserts");
        let err = cat.conn.execute(
            "insert into disc (project_id, ordinal, disc_type, usable_bytes, status, label)
             values (1, 1, 'BD_R', 100, 'DEAD', 'd1')",
            [],
        );
        assert!(err.is_err(), "the disc table must refuse a DEAD status");
    }
}
```

- [x] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p diskrement-shell --lib
```

Expected: compilation errors — `unresolved module testutil`, `cannot find type Catalog`.

- [x] **Step 3: Write the error type**

`crates/diskrement-shell/src/error.rs`:

```rust
//! Expected, recoverable failures. One `thiserror` enum per subsystem
//! (spec §5.5); `BurnError`, `PipelineError` and the shared `exit_code()`
//! arrive in later phases.

/// Persistence failures the caller is expected to handle.
///
/// The Scala equivalent was a sealed `CatalogError` reached through
/// `refineOrDie`: expected faults became typed errors and anything else killed
/// the fiber. `Unexpected` is that second case — it stays a value here, and P6
/// maps it to the same exit code an unexpected failure had (5).
#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("project already exists: {0}")]
    DuplicateProject(String),
    #[error("project not found: {0}")]
    ProjectNotFound(String),
    #[error("{0}")]
    IllegalStatusTransition(String),
    #[error("migration failed: {0}")]
    MigrationFailed(#[source] rusqlite::Error),
    #[error("constraint violation: {0}")]
    ConstraintViolation(String),
    #[error("unreadable timestamp in the catalog: {0}")]
    BadTimestamp(#[from] crate::catalog::time::TimeError),
    #[error(transparent)]
    Unexpected(#[from] rusqlite::Error),
}

impl CatalogError {
    /// Classify a SQLite failure the way `CatalogStoreLive.refineOrDie` did.
    ///
    /// The Scala matched the English text of the driver's message for
    /// `"constraint"`; SQLite hands us a result code instead, so this cannot
    /// drift with a driver's wording or a locale.
    pub(crate) fn from_sqlite(e: rusqlite::Error) -> Self {
        match &e {
            rusqlite::Error::SqliteFailure(f, message)
                if f.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                CatalogError::ConstraintViolation(
                    message.clone().unwrap_or_else(|| f.to_string()),
                )
            }
            _ => CatalogError::Unexpected(e),
        }
    }
}

/// Whether a SQLite failure is specifically a UNIQUE violation.
///
/// Distinguishing this is what turns a duplicate `project.name` into
/// `DuplicateProject` rather than a generic constraint error.
pub(crate) fn is_unique_violation(e: &rusqlite::Error) -> bool {
    matches!(
        e,
        rusqlite::Error::SqliteFailure(f, _)
            if f.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
    )
}
```

Add `pub mod error;` to `crates/diskrement-shell/src/lib.rs`.

- [x] **Step 4: Transcribe the schema**

`crates/diskrement-shell/src/catalog/schema.rs`. Every statement below is copied
from `Migrations.scala`; the only changes are Rust string syntax and
indentation.

```rust
//! The catalog schema, transcribed from the Scala `Migrations` object.
//!
//! Frozen by the data-compatibility contract (spec §3.1). The two `check`
//! constraints are load-bearing, not decoration: `status in ('OPEN','FINALIZED')`
//! is what makes the derived-`DEAD` invariant enforceable by the database, and
//! `kind in ('MARKED_DEAD')` is what keeps `disc_event` a closed vocabulary.
//! `tests/fixture_schema.rs` compares what this produces against a catalog the
//! Scala build wrote.
//!
//! Migrations run forward only, all pending versions in one transaction.

use rusqlite::Connection;

use crate::error::CatalogError;

const V1: &[&str] = &[
    "create table if not exists project (
       id                integer primary key autoincrement,
       name              text    not null unique,
       source_root       text    not null,
       work_dir          text    not null,
       default_disc_type text    not null,
       margin_percent    real    not null,
       copies            integer not null default 1,
       created_at        text    not null
     )",
    "create table if not exists disc (
       id          integer primary key autoincrement,
       project_id  integer not null references project(id),
       ordinal     integer not null,
       disc_type   text    not null,
       usable_bytes integer not null,
       status      text    not null check (status in ('OPEN','FINALIZED')),
       label       text    not null,
       unique (project_id, ordinal)
     )",
    "create table if not exists session (
       id            integer primary key autoincrement,
       disc_id       integer not null references disc(id),
       ordinal       integer not null,
       state         text    not null check (state in ('PLANNED','BURNED','VERIFIED')),
       burned_at     text,
       manifest_hash text,
       unique (disc_id, ordinal)
     )",
    "create table if not exists file_version (
       id            integer primary key autoincrement,
       project_id    integer not null references project(id),
       rel_path      text    not null,
       content_hash  text    not null,
       size_bytes    integer not null,
       mtime         text    not null,
       first_seen_at text    not null
     )",
    "create table if not exists placement (
       id              integer primary key autoincrement,
       file_version_id integer not null references file_version(id),
       session_id      integer not null references session(id),
       disc_path       text    not null,
       copy_index      integer not null
     )",
    "create table if not exists change_record (
       id              integer primary key autoincrement,
       session_id      integer not null references session(id),
       file_version_id integer not null references file_version(id),
       kind            text    not null check (kind in ('ADDED','MODIFIED','DELETED'))
     )",
    "create index if not exists idx_file_version_path on file_version(project_id, rel_path)",
    "create index if not exists idx_placement_session on placement(session_id)",
    "create index if not exists idx_change_record_fv on change_record(file_version_id)",
];

const V2: &[&str] = &[
    "create table if not exists disc_event (
       id      integer primary key autoincrement,
       disc_id integer not null references disc(id),
       kind    text    not null check (kind in ('MARKED_DEAD')),
       at      text    not null
     )",
    // The immutable record of each version's fate at mark-dead time.
    // `file_history` re-derives LOST from disc_event rather than reading this
    // table, and cannot reproduce it: a REBURN-fated version that is later
    // superseded reads LOST from then on. Keep it as the audit trail of what a
    // given event actually cost.
    "create table if not exists lost_version (
       id              integer primary key autoincrement,
       event_id        integer not null references disc_event(id),
       file_version_id integer not null references file_version(id),
       fate            text    not null check (fate in ('LOST','REBURN'))
     )",
    "create index if not exists idx_disc_event_disc on disc_event(disc_id)",
    // `mark_dead`'s "is this version also on another live disc?" probe is a
    // correlated subquery keyed on placement.file_version_id; without this it
    // full-scans placement once per version on the dying disc.
    "create index if not exists idx_placement_fv on placement(file_version_id)",
];

const MIGRATIONS: &[(i64, &[&str])] = &[(1, V1), (2, V2)];

/// Apply every migration whose version exceeds the stored one, in order.
pub(crate) fn run(conn: &mut Connection) -> Result<(), CatalogError> {
    run_inner(conn).map_err(CatalogError::MigrationFailed)
}

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

    let tx = conn.transaction()?;
    for (version, statements) in MIGRATIONS.iter().filter(|(v, _)| *v > current) {
        for statement in *statements {
            tx.execute(statement, [])?;
        }
        // One row, always: the Scala replaced rather than appended, and
        // `MigrationsSpec` pins that the table holds exactly one version.
        tx.execute("delete from schema_version", [])?;
        tx.execute("insert into schema_version (version) values (?1)", [version])?;
    }
    tx.commit()
}
```

- [x] **Step 5: Write `Catalog::open` and the transaction guard**

Replace `crates/diskrement-shell/src/catalog/mod.rs` with:

```rust
//! The SQLite catalog: schema, migrations, and every persistence operation.
//!
//! One `rusqlite::Connection`, no pool. diskrement opens the catalog, does its
//! work, and exits; there is no concurrency to pool for (spec §5.5). That one
//! decision retires four of the six gotchas the Scala build accumulated around
//! zio-jdbc: no `DriverManager` registry to serialise tests against, no pool
//! TTL fiber needing a live clock, no `getClientInfo()` proxy, and — because a
//! `Transaction` rolls back on drop unless committed — no way to write a
//! transaction that silently auto-commits.

pub mod model;
mod read;
mod schema;
mod store;
pub mod time;

#[cfg(test)]
mod testutil;

use std::path::Path;

use rusqlite::{Connection, Transaction};

use crate::error::CatalogError;

/// A project catalog: one open SQLite database, migrated to the current schema.
pub struct Catalog {
    conn: Connection,
}

impl Catalog {
    /// Open the catalog at `path`, creating the file if absent, and run every
    /// pending migration.
    ///
    /// `foreign_keys` is a per-connection pragma in SQLite, so all three are
    /// applied on every open, exactly as `Db.acquireConn` did.
    pub fn open(path: &Path) -> Result<Self, CatalogError> {
        let mut conn = Connection::open(path).map_err(CatalogError::MigrationFailed)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;",
        )
        .map_err(CatalogError::MigrationFailed)?;
        schema::run(&mut conn)?;
        Ok(Catalog { conn })
    }

    /// Run `f` inside one transaction, committing only if it returns `Ok`.
    ///
    /// This is the structural replacement for the Scala `tx` helper, which had
    /// to prepend `setAutoCommit(false)` to every transaction body because
    /// xerial's driver opened connections in auto-commit mode and zio-jdbc
    /// would then never roll back. Here an early return drops the guard, and
    /// dropping an uncommitted `Transaction` rolls it back.
    pub(crate) fn tx<T>(
        &mut self,
        f: impl FnOnce(&Transaction<'_>) -> Result<T, CatalogError>,
    ) -> Result<T, CatalogError> {
        let tx = self.conn.transaction().map_err(CatalogError::from_sqlite)?;
        let out = f(&tx)?;
        tx.commit().map_err(CatalogError::from_sqlite)?;
        Ok(out)
    }
}
```

`read.rs`, `store.rs` and `model.rs` do not exist yet. Create all three as empty
files for now so the module tree compiles:

```bash
: > crates/diskrement-shell/src/catalog/read.rs
: > crates/diskrement-shell/src/catalog/store.rs
: > crates/diskrement-shell/src/catalog/model.rs
```

- [x] **Step 6: Write the test helper**

`crates/diskrement-shell/src/catalog/testutil.rs`:

```rust
//! Test-only helpers shared by the catalog's unit tests.
//!
//! Every test gets a real SQLite file in its own `TempDir` rather than an
//! in-memory database: the production code sets `journal_mode=WAL`, which an
//! in-memory database silently ignores, and `snapshot_to` needs a real file.
//! Hold the returned `TempDir` for as long as the catalog — dropping it deletes
//! the directory.

use tempfile::TempDir;

use super::Catalog;

pub(crate) fn temp_catalog() -> (TempDir, Catalog) {
    let dir = TempDir::new().expect("a writable temp directory");
    let catalog = Catalog::open(&dir.path().join("catalog.sqlite")).expect("catalog opens");
    (dir, catalog)
}
```

Add `tempfile` to the shell crate's dev-dependencies if it is not already there
(`crates/diskrement-shell/Cargo.toml` — P0 added it; confirm before editing).

- [x] **Step 7: Run the tests to verify they pass**

```bash
cargo test -p diskrement-shell --lib
```

Expected: `test result: ok. 8 passed`.

- [x] **Step 8: Write the schema-compatibility test**

`crates/diskrement-shell/tests/fixture_schema.rs`:

```rust
//! Proves the transcribed DDL produces the same schema the Scala build did.
//!
//! Comparison is on normalised text, not bytes: SQLite stores each object's
//! `CREATE` statement as it was typed (minus `IF NOT EXISTS`), so the two
//! implementations differ only in indentation. Collapsing whitespace keeps the
//! comparison strict about everything that matters — column order, types,
//! defaults, and every `check` and `unique` clause.

mod support;

use rusqlite::Connection;
use support::open_fixture;

fn schema_of(conn: &Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare(
            "select type, name, tbl_name, coalesce(sql, '')
             from sqlite_master order by type, name",
        )
        .expect("sqlite_master is queryable");
    let mut objects: Vec<String> = stmt
        .query_map([], |row| {
            let kind: String = row.get(0)?;
            let name: String = row.get(1)?;
            let table: String = row.get(2)?;
            let sql: String = row.get(3)?;
            let normalised = sql.split_whitespace().collect::<Vec<_>>().join(" ");
            Ok(format!("{kind} {name} on {table}: {normalised}"))
        })
        .expect("rows are text")
        .map(|r| r.expect("readable row"))
        .collect();
    objects.sort();
    objects
}

#[test]
fn a_freshly_migrated_catalog_has_the_schema_the_scala_build_wrote() {
    let dir = tempfile::TempDir::new().expect("a writable temp directory");
    let ours = diskrement_shell::catalog::Catalog::open(&dir.path().join("catalog.sqlite"))
        .expect("catalog opens");
    let theirs = open_fixture("catalog/01-after-init.sqlite");

    let expected = schema_of(&theirs);
    let actual = schema_of(&ours.raw_connection_for_tests());

    assert!(
        expected.len() >= 17,
        "the fixture schema looks truncated: {expected:#?}"
    );
    for (want, got) in expected.iter().zip(actual.iter()) {
        assert_eq!(want, got);
    }
    assert_eq!(expected.len(), actual.len(), "object count differs");
}
```

This needs read access to the connection from outside the crate. Add to
`crates/diskrement-shell/src/catalog/mod.rs`, inside `impl Catalog`:

```rust
    /// Direct access to the underlying connection, for tests that need raw
    /// SQL: schema comparison, logical row dumps, and the extra placements a
    /// copies>1 scenario needs but no public method can yet express. Not part
    /// of the operational API — every caller in `src/` goes through a named
    /// method.
    #[doc(hidden)]
    pub fn raw_connection_for_tests(&self) -> &Connection {
        &self.conn
    }
```

- [x] **Step 9: Run the schema test**

```bash
cargo test -p diskrement-shell --test fixture_schema
```

Expected: `test result: ok. 1 passed`.

If it fails on a missing `sqlite_sequence`, that means no table with
`autoincrement` was created — check the DDL transcription. If it fails on a
`check` clause, the transcription dropped a constraint; fix the DDL, never the
test.

- [x] **Step 10: Verify lint and format, then commit**

```bash
cargo test -p diskrement-shell \
  && cargo clippy --workspace --all-targets -- -D warnings \
  && cargo fmt --check
git add crates/diskrement-shell
git commit -m "feat(catalog): transcribe the schema, migrations, and catalog open"
```

Expected: lib 8 passed, three integration binaries green.

---

## Task 3: Row and update types

Port the nine files under `shell/src/main/scala/diskrement/shell/model/`. Read
them first — the scaladoc on `DiscWrite`, `SupersededVersion` and `LostReport`
explains semantics the field names do not.

**Files:**
- Modify: `crates/diskrement-shell/src/catalog/model.rs`
- Modify: `crates/diskrement-core/src/planner.rs`

- [x] **Step 1: Write the failing tests**

Append to `crates/diskrement-core/src/planner.rs`, inside its existing
`mod tests` block:

```rust
    #[test]
    fn disc_status_wire_names_round_trip() {
        // Frozen by the data-compatibility contract (spec §3.1): OPEN and
        // FINALIZED are the only values `disc.status` may hold — its check
        // constraint enforces that. DEAD parses because `read_disc_states`
        // derives it in the query's case expression, and is never written.
        for (status, name) in [
            (DiscStatus::Open, "OPEN"),
            (DiscStatus::Finalized, "FINALIZED"),
            (DiscStatus::Dead, "DEAD"),
        ] {
            assert_eq!(status.as_str(), name);
            assert_eq!(name.parse::<DiscStatus>().expect("parses"), status);
        }
        assert!("OPENED".parse::<DiscStatus>().is_err());
    }
```

Append to `crates/diskrement-shell/src/catalog/model.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_state_wire_names_round_trip() {
        // `session.state` carries a check constraint over exactly these three.
        for (state, name) in [
            (SessionState::Planned, "PLANNED"),
            (SessionState::Burned, "BURNED"),
            (SessionState::Verified, "VERIFIED"),
        ] {
            assert_eq!(state.as_str(), name);
            assert_eq!(name.parse::<SessionState>().expect("parses"), state);
        }
        assert!("DONE".parse::<SessionState>().is_err());
    }

    #[test]
    fn version_status_renders_the_names_the_cli_prints() {
        // Not a stored value — `file_history` derives it — but `history`
        // prints it, so the spellings are part of the render oracle.
        assert_eq!(VersionStatus::Current.as_str(), "CURRENT");
        assert_eq!(VersionStatus::Superseded.as_str(), "SUPERSEDED");
        assert_eq!(VersionStatus::Deleted.as_str(), "DELETED");
        assert_eq!(VersionStatus::Lost.as_str(), "LOST");
    }
}
```

- [x] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p diskrement-core --lib && cargo test -p diskrement-shell --lib
```

Expected: compilation errors — `no method named as_str found for enum DiscStatus`,
`cannot find type SessionState`.

- [x] **Step 3: Give `DiscStatus` its wire spellings**

In `crates/diskrement-core/src/planner.rs`, below the `DiscStatus` enum:

```rust
impl DiscStatus {
    /// The catalog spelling. `Open` and `Finalized` are the only values the
    /// `disc.status` column may hold; `Dead` exists only as the result of the
    /// derivation in `read_disc_states` and is never written. Frozen by the
    /// data-compatibility contract; see the spec, §3.1.
    pub fn as_str(self) -> &'static str {
        match self {
            DiscStatus::Open => "OPEN",
            DiscStatus::Finalized => "FINALIZED",
            DiscStatus::Dead => "DEAD",
        }
    }
}

impl std::fmt::Display for DiscStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for DiscStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "OPEN" => Ok(DiscStatus::Open),
            "FINALIZED" => Ok(DiscStatus::Finalized),
            "DEAD" => Ok(DiscStatus::Dead),
            other => Err(format!("unknown disc status: {other}")),
        }
    }
}
```

- [x] **Step 4: Write the model types**

Prepend to `crates/diskrement-shell/src/catalog/model.rs`, above `mod tests`:

```rust
//! The row and update types the catalog reads and writes.
//!
//! These are the Scala `diskrement.shell.model` package: values that exist only
//! because something is persisted. Domain types the planner and manifest
//! builder share — `RelPath`, `ContentHash`, `DiscType`, `DiscStatus` — stay in
//! `diskrement-core`.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use diskrement_core::{ContentHash, DiscStatus, DiscType, ManifestChangeKind, RelPath};

/// Primary key of a project row (a SQLite rowid).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectId(pub i64);

/// Fields needed to create a project (no id yet).
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectInit {
    pub name: String,
    pub source_root: String,
    pub work_dir: String,
    pub default_disc_type: DiscType,
    pub margin_percent: f64,
    pub copies: u32,
    pub created_at: DateTime<Utc>,
}

/// A persisted project row.
#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub source_root: String,
    pub work_dir: String,
    pub default_disc_type: DiscType,
    pub margin_percent: f64,
    pub copies: u32,
    pub created_at: DateTime<Utc>,
}

/// One file written this run: its identity, where it landed, and which copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileWrite {
    pub rel_path: RelPath,
    pub content_hash: ContentHash,
    pub size_bytes: u64,
    pub mtime: DateTime<Utc>,
    pub disc_path: String,
    pub copy_index: u32,
}

/// One session burned onto one disc this run.
///
/// `deleted` holds tombstones, which reference the *current* version of each
/// path rather than carrying a version of their own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionWrite {
    pub ordinal: u32,
    pub burned_at: DateTime<Utc>,
    pub manifest_hash: String,
    pub added: Vec<FileWrite>,
    pub modified: Vec<FileWrite>,
    pub deleted: Vec<RelPath>,
}

/// One disc touched or created this run, with its single new session.
///
/// `disc_type`, `usable_bytes` and `label` are persisted only when `is_new` is
/// true; for an existing disc they are ignored and the stored row is kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscWrite {
    pub ordinal: u32,
    pub is_new: bool,
    pub disc_type: DiscType,
    pub usable_bytes: u64,
    /// The resulting status: `Open` for the last disc, `Finalized` otherwise.
    pub status: DiscStatus,
    pub label: String,
    pub session: SessionWrite,
}

/// The full set of catalog changes from one successful burn run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogUpdate {
    pub project_id: ProjectId,
    /// Pre-existing OPEN discs to flip to FINALIZED.
    pub finalize_disc_ordinals: Vec<u32>,
    pub discs: Vec<DiscWrite>,
}

/// Lifecycle state of a burn session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Planned,
    Burned,
    Verified,
}

impl SessionState {
    /// The catalog spelling, constrained by `check (state in (...))`.
    pub fn as_str(self) -> &'static str {
        match self {
            SessionState::Planned => "PLANNED",
            SessionState::Burned => "BURNED",
            SessionState::Verified => "VERIFIED",
        }
    }
}

impl std::fmt::Display for SessionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for SessionState {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "PLANNED" => Ok(SessionState::Planned),
            "BURNED" => Ok(SessionState::Burned),
            "VERIFIED" => Ok(SessionState::Verified),
            other => Err(format!("unknown session state: {other}")),
        }
    }
}

/// A persisted session row, joined to its disc's ordinal for convenience.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub disc_ordinal: u32,
    pub ordinal: u32,
    pub state: SessionState,
    pub burned_at: Option<DateTime<Utc>>,
    pub manifest_hash: Option<String>,
}

/// The prior-version identity the scanner needs to decide whether a file is
/// unchanged (size + mtime match -> reuse the stored content hash).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileFingerprint {
    pub size_bytes: u64,
    pub mtime: DateTime<Utc>,
    pub content_hash: ContentHash,
}

/// One non-current (superseded or deleted) file version and where its bytes
/// physically live. `kind` is the latest change-record kind for that version:
/// `Added`/`Modified` for a superseded older version, `Deleted` for a
/// tombstoned one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivePointer {
    pub rel_path: RelPath,
    pub content_hash: ContentHash,
    pub size_bytes: u64,
    pub disc_ordinal: u32,
    pub session_ordinal: u32,
    pub disc_path: String,
    pub kind: ManifestChangeKind,
}

/// Lifecycle status of one file version relative to its path's current state.
///
/// `Lost` = a non-current version whose disc was marked dead: the bytes are
/// gone. It is per *physical copy*, not per content hash — after a recovery
/// burn two versions sharing a hash can legitimately read `Lost` and `Current`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionStatus {
    Current,
    Superseded,
    Deleted,
    Lost,
}

impl VersionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            VersionStatus::Current => "CURRENT",
            VersionStatus::Superseded => "SUPERSEDED",
            VersionStatus::Deleted => "DELETED",
            VersionStatus::Lost => "LOST",
        }
    }
}

impl std::fmt::Display for VersionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One version of a path in its full history, with where its bytes live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileHistoryEntry {
    pub content_hash: ContentHash,
    pub size_bytes: u64,
    pub disc_ordinal: u32,
    pub session_ordinal: u32,
    pub status: VersionStatus,
}

/// A file version on a disc hidden by a later session of the SAME disc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupersededVersion {
    pub session_ordinal: u32,
    pub rel_path: RelPath,
    pub content_hash: ContentHash,
}

/// Everything the catalog records as physically on one disc: the merged view a
/// mount shows (`visible`), and versions hidden within the disc
/// (`superseded`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscRecordedContents {
    pub visible: BTreeMap<RelPath, ContentHash>,
    pub superseded: Vec<SupersededVersion>,
}

/// A version whose only copy was on the now-dead disc and whose path has moved
/// past it — those bytes are permanently gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LostVersionEntry {
    pub rel_path: RelPath,
    pub content_hash: ContentHash,
    pub size_bytes: u64,
    pub session_ordinal: u32,
}

/// A live file whose only burned copy was on the dead disc; the next
/// scan/plan/burn writes it again from the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReburnEntry {
    pub rel_path: RelPath,
    pub content_hash: ContentHash,
    pub size_bytes: u64,
}

/// What marking a disc dead cost. Recorded in `lost_version`, rendered by the
/// CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LostReport {
    pub disc_ordinal: u32,
    pub lost: Vec<LostVersionEntry>,
    pub reburn: Vec<ReburnEntry>,
}
```

- [x] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p diskrement-core --lib && cargo test -p diskrement-shell --lib
```

Expected: core `58 passed`, shell `10 passed`.

- [x] **Step 6: Verify lint and format, then commit**

```bash
cargo test --workspace \
  && cargo clippy --workspace --all-targets -- -D warnings \
  && cargo fmt --check
git add crates/diskrement-core/src crates/diskrement-shell/src
git commit -m "feat(catalog): port the persistence row and update types"
```

---

## Task 4: Projects

Port `CatalogStoreLive.createProject` and `.findProject`, and the assertions in
`ProjectSpec.scala`.

**Files:**
- Modify: `crates/diskrement-shell/src/catalog/store.rs`
- Modify: `crates/diskrement-shell/src/catalog/testutil.rs`

- [x] **Step 1: Extend the test helper**

Append to `crates/diskrement-shell/src/catalog/testutil.rs`:

```rust
use chrono::{DateTime, TimeZone, Utc};
use diskrement_core::DiscType;

use super::model::ProjectInit;

/// The fixed "burned at" instant every unit test uses, matching the Scala
/// specs' `ts`.
pub(crate) fn ts() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 6, 1, 10, 0, 0).unwrap()
}

pub(crate) fn project_init(name: &str) -> ProjectInit {
    ProjectInit {
        name: name.to_string(),
        source_root: "/src".to_string(),
        work_dir: "/work".to_string(),
        default_disc_type: DiscType::BdR,
        margin_percent: 2.0,
        copies: 1,
        created_at: Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap(),
    }
}
```

- [x] **Step 2: Write the failing tests**

Append to `crates/diskrement-shell/src/catalog/store.rs`:

```rust
#[cfg(test)]
mod tests {
    use diskrement_core::DiscType;

    use crate::catalog::testutil::{project_init, temp_catalog};
    use crate::error::CatalogError;

    #[test]
    fn create_project_then_find_project_round_trips() {
        let (_dir, mut cat) = temp_catalog();
        let init = project_init("photos");
        let id = cat.create_project(&init).expect("project is created");

        let found = cat.find_project("photos").expect("query succeeds");
        let project = found.expect("the project exists");
        assert_eq!(project.id, id);
        assert_eq!(project.name, "photos");
        assert_eq!(project.default_disc_type, DiscType::BdR);
        assert_eq!(project.source_root, "/src");
        assert_eq!(project.work_dir, "/work");
        assert_eq!(project.margin_percent, 2.0);
        assert_eq!(project.copies, 1);
        // The timestamp survives the trip through TEXT storage unchanged.
        assert_eq!(project.created_at, init.created_at);
    }

    #[test]
    fn creating_a_duplicate_name_fails_with_duplicate_project() {
        let (_dir, mut cat) = temp_catalog();
        cat.create_project(&project_init("photos")).expect("first create");
        let err = cat
            .create_project(&project_init("photos"))
            .expect_err("the unique index rejects the second");
        assert!(
            matches!(&err, CatalogError::DuplicateProject(name) if name == "photos"),
            "expected DuplicateProject, got {err:?}"
        );
    }

    #[test]
    fn find_project_returns_none_for_an_unknown_name() {
        let (_dir, mut cat) = temp_catalog();
        assert!(cat.find_project("nope").expect("query succeeds").is_none());
    }
}
```

- [x] **Step 3: Run the tests to verify they fail**

```bash
cargo test -p diskrement-shell --lib
```

Expected: compilation errors — `no method named create_project found for struct Catalog`.

- [x] **Step 4: Write the implementation**

Prepend to `crates/diskrement-shell/src/catalog/store.rs`, above `mod tests`:

```rust
//! The catalog's write path: projects, burn updates, verification, and the
//! mark-dead event.
//!
//! Every method here takes `&mut self` because `Connection::transaction`
//! does — which makes "this operation mutates the catalog" a fact the compiler
//! knows rather than a convention.

use std::str::FromStr;

use rusqlite::{params, OptionalExtension};

use super::model::{Project, ProjectId, ProjectInit};
use super::time;
use super::Catalog;
use crate::error::{is_unique_violation, CatalogError};
use diskrement_core::DiscType;

impl Catalog {
    /// Insert a project row, returning its new id.
    pub fn create_project(&mut self, p: &ProjectInit) -> Result<ProjectId, CatalogError> {
        let name = p.name.clone();
        self.tx(|tx| {
            let inserted = tx.execute(
                "insert into project
                   (name, source_root, work_dir, default_disc_type, margin_percent,
                    copies, created_at)
                 values (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    p.name,
                    p.source_root,
                    p.work_dir,
                    p.default_disc_type.as_str(),
                    p.margin_percent,
                    p.copies as i64,
                    time::format(&p.created_at),
                ],
            );
            match inserted {
                Ok(_) => Ok(ProjectId(tx.last_insert_rowid())),
                Err(e) if is_unique_violation(&e) => Err(CatalogError::DuplicateProject(name)),
                Err(e) => Err(CatalogError::from_sqlite(e)),
            }
        })
    }

    /// Look a project up by its unique name.
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

        row.map(
            |(id, name, source_root, work_dir, disc_type, margin, copies, created_at)| {
                Ok(Project {
                    id: ProjectId(id),
                    name,
                    source_root,
                    work_dir,
                    default_disc_type: DiscType::from_str(&disc_type)
                        .map_err(CatalogError::ConstraintViolation)?,
                    margin_percent: margin,
                    copies: copies as u32,
                    created_at: time::parse(&created_at)?,
                })
            },
        )
        .transpose()
    }
}
```

Note `find_project` takes `&self`: it reads. Import only what this task uses —
warnings are errors here, so `Transaction` arrives with the free functions in
Task 5.

- [x] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p diskrement-shell --lib
```

Expected: `test result: ok. 13 passed`.

- [x] **Step 6: Verify lint and format, then commit**

```bash
cargo test --workspace \
  && cargo clippy --workspace --all-targets -- -D warnings \
  && cargo fmt --check
git add crates/diskrement-shell/src
git commit -m "feat(catalog): port project creation and lookup"
```

---

## Task 5: `apply_update` — the burn write path

Port `CatalogStoreLive.applyUpdate` and its four private helpers
(`finalizeDisc`, `writeDisc`, `writeFile`, `writeTombstone`), plus the
assertions in `ApplyUpdateSpec.scala`. **Read the Scala first** — `writeDisc`'s
existing-disc branch and `writeTombstone`'s "latest version for that path" rule
are both load-bearing.

This is the one place that persists a burn, and the invariant it carries is
*commit after success*: no `PLANNED` rows are ever written, and the whole update
is one transaction, so a crash before commit leaves the catalog untouched.

**Files:**
- Modify: `crates/diskrement-shell/src/catalog/store.rs`
- Modify: `crates/diskrement-shell/src/catalog/testutil.rs`

- [x] **Step 1: Extend the test helper**

Append to `crates/diskrement-shell/src/catalog/testutil.rs`:

```rust
use diskrement_core::{ContentHash, DiscStatus, RelPath};

use super::model::{CatalogUpdate, DiscWrite, FileWrite, ProjectId, SessionWrite};

/// A file write with the conventional test values: mtime `ts()`, disc path
/// `/data/<path>`, copy 0.
pub(crate) fn fw(path: &str, hash: &str, size: u64) -> FileWrite {
    fw_at(path, hash, size, ts())
}

pub(crate) fn fw_at(
    path: &str,
    hash: &str,
    size: u64,
    mtime: chrono::DateTime<chrono::Utc>,
) -> FileWrite {
    FileWrite {
        rel_path: RelPath(path.to_string()),
        content_hash: ContentHash(hash.to_string()),
        size_bytes: size,
        mtime,
        disc_path: format!("/data/{path}"),
        copy_index: 0,
    }
}

/// One session on one disc, the shape every ported spec's `session` helper
/// built: the disc is new exactly when this is its first session.
pub(crate) fn update(
    project: ProjectId,
    disc_ordinal: u32,
    session_ordinal: u32,
    added: Vec<FileWrite>,
    modified: Vec<FileWrite>,
    deleted: Vec<RelPath>,
) -> CatalogUpdate {
    CatalogUpdate {
        project_id: project,
        finalize_disc_ordinals: Vec::new(),
        discs: vec![DiscWrite {
            ordinal: disc_ordinal,
            is_new: session_ordinal == 1,
            disc_type: diskrement_core::DiscType::BdR,
            usable_bytes: 1_000_000,
            status: DiscStatus::Open,
            label: format!("d{disc_ordinal}"),
            session: SessionWrite {
                ordinal: session_ordinal,
                burned_at: ts(),
                manifest_hash: format!("m{disc_ordinal}-{session_ordinal}"),
                added,
                modified,
                deleted,
            },
        }],
    }
}

/// Row count of one table, for the atomicity assertions.
pub(crate) fn count(cat: &super::Catalog, table: &str) -> i64 {
    cat.raw_connection_for_tests()
        .query_row(&format!("select count(*) from {table}"), [], |r| r.get(0))
        .expect("count query succeeds")
}
```

- [x] **Step 2: Write the failing tests**

Append inside the existing `mod tests` block in
`crates/diskrement-shell/src/catalog/store.rs`:

```rust
    use crate::catalog::model::{CatalogUpdate, DiscWrite, SessionWrite};
    use crate::catalog::testutil::{count, fw, ts, update};
    use diskrement_core::DiscStatus;

    #[test]
    fn apply_update_writes_discs_sessions_versions_placements_and_changes() {
        let (_dir, mut cat) = temp_catalog();
        let pid = cat.create_project(&project_init("p")).expect("project");
        cat.apply_update(&update(
            pid,
            1,
            1,
            vec![fw("a", "h-a", 10), fw("b", "h-b", 20)],
            vec![],
            vec![],
        ))
        .expect("update applies");

        assert_eq!(count(&cat, "disc"), 1);
        assert_eq!(count(&cat, "session"), 1);
        assert_eq!(count(&cat, "file_version"), 2);
        assert_eq!(count(&cat, "placement"), 2);
        assert_eq!(count(&cat, "change_record"), 2);
    }

    #[test]
    fn a_failing_apply_update_rolls_back_every_insert() {
        let (_dir, mut cat) = temp_catalog();
        let pid = cat.create_project(&project_init("p")).expect("project");

        // A second disc marked `is_new = false` that does not exist: the id
        // lookup fails partway through, after the first disc's rows are in.
        let mut bad = update(pid, 1, 1, vec![fw("a", "h-a", 10)], vec![], vec![]);
        bad.discs.push(DiscWrite {
            ordinal: 99,
            is_new: false,
            disc_type: diskrement_core::DiscType::BdR,
            usable_bytes: 100,
            status: DiscStatus::Finalized,
            label: "ghost".to_string(),
            session: SessionWrite {
                ordinal: 2,
                burned_at: ts(),
                manifest_hash: "m2".to_string(),
                added: vec![],
                modified: vec![],
                deleted: vec![],
            },
        });

        let err = cat.apply_update(&bad).expect_err("the ghost disc fails");
        assert!(
            matches!(err, CatalogError::ConstraintViolation(_)),
            "expected ConstraintViolation, got {err:?}"
        );
        assert_eq!(count(&cat, "disc"), 0);
        assert_eq!(count(&cat, "session"), 0);
        assert_eq!(count(&cat, "file_version"), 0);
        assert_eq!(count(&cat, "placement"), 0);
        assert_eq!(count(&cat, "change_record"), 0);
    }

    #[test]
    fn finalize_disc_ordinals_seal_an_open_disc_and_refuse_a_sealed_one() {
        let (_dir, mut cat) = temp_catalog();
        let pid = cat.create_project(&project_init("p")).expect("project");
        cat.apply_update(&update(pid, 1, 1, vec![fw("a", "h-a", 10)], vec![], vec![]))
            .expect("first burn");

        let seal = CatalogUpdate {
            project_id: pid,
            finalize_disc_ordinals: vec![1],
            discs: vec![],
        };
        cat.apply_update(&seal).expect("sealing an OPEN disc works");

        let status: String = cat
            .raw_connection_for_tests()
            .query_row("select status from disc where ordinal = 1", [], |r| r.get(0))
            .expect("disc row exists");
        assert_eq!(status, "FINALIZED");

        let err = cat.apply_update(&seal).expect_err("sealing twice fails");
        assert!(
            matches!(err, CatalogError::IllegalStatusTransition(_)),
            "expected IllegalStatusTransition, got {err:?}"
        );
    }

    #[test]
    fn a_tombstone_for_an_unknown_path_is_refused() {
        let (_dir, mut cat) = temp_catalog();
        let pid = cat.create_project(&project_init("p")).expect("project");
        let err = cat
            .apply_update(&update(
                pid,
                1,
                1,
                vec![],
                vec![],
                vec![diskrement_core::RelPath("ghost".to_string())],
            ))
            .expect_err("there is no version to tombstone");
        assert!(
            matches!(err, CatalogError::ConstraintViolation(_)),
            "expected ConstraintViolation, got {err:?}"
        );
    }
```

- [x] **Step 3: Run the tests to verify they fail**

```bash
cargo test -p diskrement-shell --lib
```

Expected: compilation errors — `no method named apply_update found for struct Catalog`.

- [x] **Step 4: Write the implementation**

Add to the `impl Catalog` block in `crates/diskrement-shell/src/catalog/store.rs`:

```rust
    /// Persist one successful burn: every disc touched, its single new session,
    /// and the file versions, placements and change records that session wrote.
    ///
    /// One transaction, so this is the crash-safe commit point — the invariant
    /// the whole burn flow is built around. Nothing is written before the discs
    /// are staged successfully, and a failure here leaves the catalog exactly
    /// as it was.
    pub fn apply_update(&mut self, u: &CatalogUpdate) -> Result<(), CatalogError> {
        self.tx(|tx| {
            for ordinal in &u.finalize_disc_ordinals {
                finalize_disc(tx, u.project_id, *ordinal)?;
            }
            for disc in &u.discs {
                write_disc(tx, u.project_id, disc)?;
            }
            Ok(())
        })
    }
```

and, below the `impl` block, the free functions:

```rust
/// Flip a pre-existing OPEN disc to FINALIZED. Anything else is an illegal
/// transition — including a disc that is already FINALIZED.
fn finalize_disc(
    tx: &Transaction<'_>,
    project: ProjectId,
    ordinal: u32,
) -> Result<(), CatalogError> {
    let updated = tx
        .execute(
            "update disc set status = 'FINALIZED'
             where project_id = ?1 and ordinal = ?2 and status = 'OPEN'",
            params![project.0, ordinal as i64],
        )
        .map_err(CatalogError::from_sqlite)?;
    if updated == 0 {
        return Err(CatalogError::IllegalStatusTransition(format!(
            "disc {ordinal} is not OPEN"
        )));
    }
    Ok(())
}

fn disc_id_by_ordinal(
    tx: &Transaction<'_>,
    project: ProjectId,
    ordinal: u32,
) -> Result<i64, CatalogError> {
    tx.query_row(
        "select id from disc where project_id = ?1 and ordinal = ?2",
        params![project.0, ordinal as i64],
        |row| row.get::<_, i64>(0),
    )
    .optional()
    .map_err(CatalogError::from_sqlite)?
    .ok_or_else(|| CatalogError::ConstraintViolation(format!("no disc with ordinal {ordinal}")))
}

/// Insert or find the disc row, insert its session, then write the session's
/// files and tombstones.
///
/// Sessions are always recorded as BURNED: no PLANNED row is ever persisted.
fn write_disc(
    tx: &Transaction<'_>,
    project: ProjectId,
    d: &DiscWrite,
) -> Result<(), CatalogError> {
    let disc_id = if d.is_new {
        tx.execute(
            "insert into disc (project_id, ordinal, disc_type, usable_bytes, status, label)
             values (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                project.0,
                d.ordinal as i64,
                d.disc_type.as_str(),
                d.usable_bytes as i64,
                d.status.as_str(),
                d.label,
            ],
        )
        .map_err(CatalogError::from_sqlite)?;
        tx.last_insert_rowid()
    } else {
        let id = disc_id_by_ordinal(tx, project, d.ordinal)?;
        if d.status == DiscStatus::Finalized {
            let updated = tx
                .execute(
                    "update disc set status = 'FINALIZED' where id = ?1 and status = 'OPEN'",
                    params![id],
                )
                .map_err(CatalogError::from_sqlite)?;
            if updated == 0 {
                return Err(CatalogError::IllegalStatusTransition(format!(
                    "disc {} is not OPEN",
                    d.ordinal
                )));
            }
        }
        id
    };

    tx.execute(
        "insert into session (disc_id, ordinal, state, burned_at, manifest_hash)
         values (?1, ?2, 'BURNED', ?3, ?4)",
        params![
            disc_id,
            d.session.ordinal as i64,
            time::format(&d.session.burned_at),
            d.session.manifest_hash,
        ],
    )
    .map_err(CatalogError::from_sqlite)?;
    let session_id = tx.last_insert_rowid();

    for file in &d.session.added {
        write_file(
            tx,
            project,
            session_id,
            file,
            ManifestChangeKind::Added,
            &d.session.burned_at,
        )?;
    }
    for file in &d.session.modified {
        write_file(
            tx,
            project,
            session_id,
            file,
            ManifestChangeKind::Modified,
            &d.session.burned_at,
        )?;
    }
    for path in &d.session.deleted {
        write_tombstone(tx, project, session_id, path)?;
    }
    Ok(())
}

fn write_file(
    tx: &Transaction<'_>,
    project: ProjectId,
    session_id: i64,
    f: &FileWrite,
    kind: ManifestChangeKind,
    first_seen_at: &DateTime<Utc>,
) -> Result<(), CatalogError> {
    tx.execute(
        "insert into file_version
           (project_id, rel_path, content_hash, size_bytes, mtime, first_seen_at)
         values (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            project.0,
            f.rel_path.as_str(),
            f.content_hash.as_str(),
            f.size_bytes as i64,
            time::format(&f.mtime),
            time::format(first_seen_at),
        ],
    )
    .map_err(CatalogError::from_sqlite)?;
    let file_version_id = tx.last_insert_rowid();

    tx.execute(
        "insert into placement (file_version_id, session_id, disc_path, copy_index)
         values (?1, ?2, ?3, ?4)",
        params![file_version_id, session_id, f.disc_path, f.copy_index as i64],
    )
    .map_err(CatalogError::from_sqlite)?;

    tx.execute(
        "insert into change_record (session_id, file_version_id, kind) values (?1, ?2, ?3)",
        params![session_id, file_version_id, kind.as_str()],
    )
    .map_err(CatalogError::from_sqlite)?;
    Ok(())
}

/// Tombstone the path's current version — the latest `file_version` row for
/// that path. A tombstone writes no new version: it is a change record against
/// the version that was live.
fn write_tombstone(
    tx: &Transaction<'_>,
    project: ProjectId,
    session_id: i64,
    rel_path: &RelPath,
) -> Result<(), CatalogError> {
    let file_version_id: Option<i64> = tx
        .query_row(
            "select id from file_version
             where project_id = ?1 and rel_path = ?2
             order by id desc limit 1",
            params![project.0, rel_path.as_str()],
            |row| row.get(0),
        )
        .optional()
        .map_err(CatalogError::from_sqlite)?;

    match file_version_id {
        Some(id) => {
            tx.execute(
                "insert into change_record (session_id, file_version_id, kind)
                 values (?1, ?2, 'DELETED')",
                params![session_id, id],
            )
            .map_err(CatalogError::from_sqlite)?;
            Ok(())
        }
        None => Err(CatalogError::ConstraintViolation(format!(
            "cannot tombstone unknown path {}",
            rel_path.as_str()
        ))),
    }
}
```

Extend the imports at the top of `store.rs`:

```rust
use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension, Transaction};
use diskrement_core::{DiscStatus, DiscType, ManifestChangeKind, RelPath};

use super::model::{CatalogUpdate, DiscWrite, FileWrite, Project, ProjectId, ProjectInit};
```

- [x] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p diskrement-shell --lib
```

Expected: `test result: ok. 17 passed`.

- [x] **Step 6: Verify lint and format, then commit**

```bash
cargo test --workspace \
  && cargo clippy --workspace --all-targets -- -D warnings \
  && cargo fmt --check
git add crates/diskrement-shell/src
git commit -m "feat(catalog): port applyUpdate, the crash-safe burn commit point"
```

---

## Task 6: The current-view queries

Port `readView`, `fingerprints`, `sessionsByState` and `readDiscStates`, and the
assertions in `ReadViewSpec`, `FingerprintsSpec`, `SessionsByStateSpec` and
`DiscStatesSpec`.

Two of these queries already carry the dead-disc derivation, even though nothing
writes a `disc_event` until Task 9. Port them whole; Task 12 exercises them
against a fixture catalog that does have one.

**Files:**
- Modify: `crates/diskrement-shell/src/catalog/read.rs`

- [x] **Step 1: Write the failing tests**

`crates/diskrement-shell/src/catalog/read.rs`:

```rust
#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use diskrement_core::{ContentHash, DiscState, DiscStatus, RelPath};

    use crate::catalog::model::{FileFingerprint, SessionState};
    use crate::catalog::testutil::{count, fw, fw_at, project_init, temp_catalog, update};

    #[test]
    fn read_view_adds_supersedes_and_forgets_deleted_paths() {
        let (_dir, mut cat) = temp_catalog();
        let pid = cat.create_project(&project_init("p")).expect("project");
        cat.apply_update(&update(
            pid,
            1,
            1,
            vec![fw("a", "a1", 10), fw("b", "b1", 20), fw("c", "c1", 30)],
            vec![],
            vec![],
        ))
        .expect("first session");
        cat.apply_update(&update(
            pid,
            1,
            2,
            vec![],
            vec![fw("b", "b2", 25)],
            vec![RelPath("c".to_string())],
        ))
        .expect("second session");

        let view = cat.read_view(pid).expect("view reads");
        let paths: Vec<&str> = view.current.keys().map(RelPath::as_str).collect();
        assert_eq!(paths, vec!["a", "b"]);
        assert_eq!(
            view.current[&RelPath("a".to_string())].content_hash,
            ContentHash("a1".to_string())
        );
        assert_eq!(
            view.current[&RelPath("b".to_string())].content_hash,
            ContentHash("b2".to_string())
        );
        // The deleted path's version row is still there: the catalog is
        // append-only, and the tombstone is a change record, not a deletion.
        assert!(count(&cat, "file_version") >= 4);
    }

    #[test]
    fn fingerprints_return_the_current_version_with_its_mtime() {
        let (_dir, mut cat) = temp_catalog();
        let pid = cat.create_project(&project_init("p")).expect("project");
        let mt_a = Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
        let mt_b1 = Utc.with_ymd_and_hms(2026, 5, 2, 0, 0, 0).unwrap();
        let mt_b2 = Utc.with_ymd_and_hms(2026, 5, 3, 0, 0, 0).unwrap();
        let mt_c = Utc.with_ymd_and_hms(2026, 5, 4, 0, 0, 0).unwrap();

        cat.apply_update(&update(
            pid,
            1,
            1,
            vec![
                fw_at("a", "a1", 10, mt_a),
                fw_at("b", "b1", 20, mt_b1),
                fw_at("c", "c1", 30, mt_c),
            ],
            vec![],
            vec![],
        ))
        .expect("first session");
        cat.apply_update(&update(
            pid,
            1,
            2,
            vec![],
            vec![fw_at("b", "b2", 25, mt_b2)],
            vec![RelPath("c".to_string())],
        ))
        .expect("second session");

        let fps = cat.fingerprints(pid).expect("fingerprints read");
        let paths: Vec<&str> = fps.keys().map(RelPath::as_str).collect();
        assert_eq!(paths, vec!["a", "b"]);
        assert_eq!(
            fps[&RelPath("a".to_string())],
            FileFingerprint {
                size_bytes: 10,
                mtime: mt_a,
                content_hash: ContentHash("a1".to_string()),
            }
        );
        assert_eq!(
            fps[&RelPath("b".to_string())],
            FileFingerprint {
                size_bytes: 25,
                mtime: mt_b2,
                content_hash: ContentHash("b2".to_string()),
            }
        );
    }

    #[test]
    fn apply_update_records_sessions_as_burned_and_never_planned() {
        let (_dir, mut cat) = temp_catalog();
        let pid = cat.create_project(&project_init("p")).expect("project");
        cat.apply_update(&update(pid, 1, 1, vec![fw("a", "h", 10)], vec![], vec![]))
            .expect("burn");

        let burned = cat
            .sessions_by_state(pid, SessionState::Burned)
            .expect("query succeeds");
        assert_eq!(burned.len(), 1);
        assert_eq!(burned[0].disc_ordinal, 1);
        assert_eq!(burned[0].ordinal, 1);
        assert_eq!(burned[0].state, SessionState::Burned);
        assert_eq!(burned[0].manifest_hash.as_deref(), Some("m1-1"));

        assert!(cat
            .sessions_by_state(pid, SessionState::Planned)
            .expect("query succeeds")
            .is_empty());
    }

    #[test]
    fn read_disc_states_derives_used_bytes_from_placements() {
        let (_dir, mut cat) = temp_catalog();
        let pid = cat.create_project(&project_init("p")).expect("project");
        cat.apply_update(&update(
            pid,
            1,
            1,
            vec![fw("a", "h-a", 10), fw("b", "h-b", 20)],
            vec![],
            vec![],
        ))
        .expect("disc 1");
        cat.apply_update(&update(pid, 2, 1, vec![fw("c", "h-c", 10)], vec![], vec![]))
            .expect("disc 2");

        let states = cat.read_disc_states(pid).expect("states read");
        assert_eq!(
            states,
            vec![
                DiscState {
                    ordinal: 1,
                    status: DiscStatus::Open,
                    usable_bytes: 1_000_000,
                    used_bytes: 30,
                },
                DiscState {
                    ordinal: 2,
                    status: DiscStatus::Open,
                    usable_bytes: 1_000_000,
                    used_bytes: 10,
                },
            ]
        );
    }
}
```

- [x] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p diskrement-shell --lib
```

Expected: compilation errors — `no method named read_view found for struct Catalog`.

- [x] **Step 3: Write the implementation**

Prepend to `crates/diskrement-shell/src/catalog/read.rs`, above `mod tests`:

```rust
//! The catalog's read path.
//!
//! Every query here is transcribed from `CatalogStoreLive`. They look
//! elaborate because each one encodes a domain rule in SQL rather than in Rust:
//! "the current version of a path" is the newest `change_record` for it, and
//! "the bytes still exist" is the absence of a `MARKED_DEAD` event on the disc
//! holding them. Rewriting one of these into something more readable is how a
//! porting defect gets in.
//!
//! Reads take `&self` and open no transaction: a single connection sees a
//! consistent database, and nothing here writes.

use std::collections::BTreeMap;
use std::str::FromStr;

use rusqlite::params;

use super::model::{FileFingerprint, ProjectId, Session, SessionState};
use super::time;
use super::Catalog;
use crate::error::CatalogError;
use diskrement_core::{CatalogEntry, CatalogView, ContentHash, DiscState, DiscStatus, RelPath};

impl Catalog {
    /// The project's live files: for each path, its newest version, provided
    /// that version's newest change is not a tombstone and at least one copy of
    /// it sits on a disc that has not been marked dead.
    pub fn read_view(&self, project: ProjectId) -> Result<CatalogView, CatalogError> {
        let mut stmt = self
            .conn
            .prepare(
                "select fv.rel_path, fv.content_hash, fv.size_bytes
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
                   and exists (
                     select 1 from placement p
                     join session s on s.id = p.session_id
                     where p.file_version_id = fv.id
                       and not exists (select 1 from disc_event e
                                       where e.disc_id = s.disc_id and e.kind = 'MARKED_DEAD'))",
            )
            .map_err(CatalogError::from_sqlite)?;

        let rows = stmt
            .query_map(params![project.0], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(CatalogError::from_sqlite)?;

        let mut current = BTreeMap::new();
        for row in rows {
            let (path, hash, size) = row.map_err(CatalogError::from_sqlite)?;
            let rel_path = RelPath(path);
            current.insert(
                rel_path.clone(),
                CatalogEntry {
                    rel_path,
                    content_hash: ContentHash(hash),
                    size_bytes: size as u64,
                },
            );
        }
        Ok(CatalogView { current })
    }

    /// Size, mtime and hash of every live version, for the scanner's
    /// "unchanged?" check.
    ///
    /// Deliberately *not* filtered by dead disc, unlike `read_view`: a dead
    /// disc does not make the source file change, and re-hashing it would be
    /// wasted work. Ported as-is with the rest of the deferred debt (spec §1).
    pub fn fingerprints(
        &self,
        project: ProjectId,
    ) -> Result<BTreeMap<RelPath, FileFingerprint>, CatalogError> {
        let mut stmt = self
            .conn
            .prepare(
                "select fv.rel_path, fv.size_bytes, fv.mtime, fv.content_hash
                 from (
                   select cr.kind as kind, cr.file_version_id as fvid,
                          row_number() over (partition by fv.rel_path order by cr.id desc) as rn
                   from change_record cr
                   join file_version fv on fv.id = cr.file_version_id
                   where fv.project_id = ?1
                 ) l
                 join file_version fv on fv.id = l.fvid
                 where l.rn = 1 and l.kind <> 'DELETED'",
            )
            .map_err(CatalogError::from_sqlite)?;

        let rows = stmt
            .query_map(params![project.0], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(CatalogError::from_sqlite)?;

        let mut out = BTreeMap::new();
        for row in rows {
            let (path, size, mtime, hash) = row.map_err(CatalogError::from_sqlite)?;
            out.insert(
                RelPath(path),
                FileFingerprint {
                    size_bytes: size as u64,
                    mtime: time::parse(&mtime)?,
                    content_hash: ContentHash(hash),
                },
            );
        }
        Ok(out)
    }

    /// Sessions in one lifecycle state, ordered by disc then session ordinal.
    pub fn sessions_by_state(
        &self,
        project: ProjectId,
        state: SessionState,
    ) -> Result<Vec<Session>, CatalogError> {
        let mut stmt = self
            .conn
            .prepare(
                "select d.ordinal, s.ordinal, s.state, s.burned_at, s.manifest_hash
                 from session s
                 join disc d on d.id = s.disc_id
                 where d.project_id = ?1 and s.state = ?2
                 order by d.ordinal, s.ordinal",
            )
            .map_err(CatalogError::from_sqlite)?;

        let rows = stmt
            .query_map(params![project.0, state.as_str()], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            })
            .map_err(CatalogError::from_sqlite)?;

        let mut out = Vec::new();
        for row in rows {
            let (disc_ordinal, ordinal, state, burned_at, manifest_hash) =
                row.map_err(CatalogError::from_sqlite)?;
            out.push(Session {
                disc_ordinal: disc_ordinal as u32,
                ordinal: ordinal as u32,
                state: SessionState::from_str(&state).map_err(CatalogError::ConstraintViolation)?,
                burned_at: burned_at.as_deref().map(time::parse).transpose()?,
                manifest_hash,
            });
        }
        Ok(out)
    }

    /// Every disc with its derived status and used bytes.
    ///
    /// `DEAD` is computed here, in the case expression, from `disc_event` —
    /// the `disc.status` column itself only ever holds OPEN or FINALIZED.
    pub fn read_disc_states(&self, project: ProjectId) -> Result<Vec<DiscState>, CatalogError> {
        let mut stmt = self
            .conn
            .prepare(
                "select d.ordinal,
                        case when exists (select 1 from disc_event e
                                          where e.disc_id = d.id and e.kind = 'MARKED_DEAD')
                             then 'DEAD' else d.status end as status,
                        d.usable_bytes,
                        coalesce((select sum(fv.size_bytes)
                                  from placement p
                                  join session s on s.id = p.session_id
                                  join file_version fv on fv.id = p.file_version_id
                                  where s.disc_id = d.id), 0) as used
                 from disc d
                 where d.project_id = ?1
                 order by d.ordinal",
            )
            .map_err(CatalogError::from_sqlite)?;

        let rows = stmt
            .query_map(params![project.0], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .map_err(CatalogError::from_sqlite)?;

        let mut out = Vec::new();
        for row in rows {
            let (ordinal, status, usable, used) = row.map_err(CatalogError::from_sqlite)?;
            out.push(DiscState {
                ordinal: ordinal as u32,
                status: DiscStatus::from_str(&status).map_err(CatalogError::ConstraintViolation)?,
                usable_bytes: usable as u64,
                used_bytes: used as u64,
            });
        }
        Ok(out)
    }
}
```

- [x] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p diskrement-shell --lib
```

Expected: `test result: ok. 21 passed`.

- [x] **Step 5: Verify lint and format, then commit**

```bash
cargo test --workspace \
  && cargo clippy --workspace --all-targets -- -D warnings \
  && cargo fmt --check
git add crates/diskrement-shell/src
git commit -m "feat(catalog): port the current-view, fingerprint and disc-state queries"
```

---

## Task 7: Archive pointers and file history

Port `archivePointers` and `fileHistory`, and the assertions in
`ArchivePointersSpec` and `FileHistorySpec`.

Both queries answer "what happened to this version", and both hinge on the same
two window functions: the newest change record *for a version* (which gives its
kind) and the newest change record *for a path* (which gives the current
version). Keep them distinct — swapping them is a silent, plausible-looking bug.

**Files:**
- Modify: `crates/diskrement-shell/src/catalog/read.rs`

- [x] **Step 1: Write the failing tests**

Append inside `mod tests` in `crates/diskrement-shell/src/catalog/read.rs`:

```rust
    use crate::catalog::model::{ArchivePointer, FileHistoryEntry, VersionStatus};
    use diskrement_core::ManifestChangeKind;

    #[test]
    fn archive_pointers_locate_superseded_and_deleted_versions_only() {
        let (_dir, mut cat) = temp_catalog();
        let pid = cat.create_project(&project_init("p")).expect("project");
        cat.apply_update(&update(
            pid,
            1,
            1,
            vec![fw("a", "a1", 10), fw("b", "b1", 20), fw("c", "c1", 30)],
            vec![],
            vec![],
        ))
        .expect("first session");
        cat.apply_update(&update(
            pid,
            1,
            2,
            vec![],
            vec![fw("b", "b2", 25)],
            vec![RelPath("c".to_string())],
        ))
        .expect("second session");

        let pointers = cat.archive_pointers(pid).expect("pointers read");
        assert_eq!(
            pointers,
            vec![
                ArchivePointer {
                    rel_path: RelPath("b".to_string()),
                    content_hash: ContentHash("b1".to_string()),
                    size_bytes: 20,
                    disc_ordinal: 1,
                    session_ordinal: 1,
                    disc_path: "/data/b".to_string(),
                    kind: ManifestChangeKind::Added,
                },
                ArchivePointer {
                    rel_path: RelPath("c".to_string()),
                    content_hash: ContentHash("c1".to_string()),
                    size_bytes: 30,
                    disc_ordinal: 1,
                    session_ordinal: 1,
                    disc_path: "/data/c".to_string(),
                    kind: ManifestChangeKind::Deleted,
                },
            ]
        );
    }

    #[test]
    fn a_re_added_path_leaves_exactly_one_deleted_pointer_behind() {
        let (_dir, mut cat) = temp_catalog();
        let pid = cat.create_project(&project_init("p")).expect("project");
        cat.apply_update(&update(pid, 1, 1, vec![fw("x", "x1", 10)], vec![], vec![]))
            .expect("add");
        cat.apply_update(&update(
            pid,
            1,
            2,
            vec![],
            vec![],
            vec![RelPath("x".to_string())],
        ))
        .expect("delete");
        cat.apply_update(&update(pid, 1, 3, vec![fw("x", "x2", 15)], vec![], vec![]))
            .expect("re-add");

        let pointers = cat.archive_pointers(pid).expect("pointers read");
        assert_eq!(pointers.len(), 1);
        assert_eq!(pointers[0].content_hash, ContentHash("x1".to_string()));
        assert_eq!(pointers[0].kind, ManifestChangeKind::Deleted);
    }

    #[test]
    fn file_history_orders_versions_oldest_first_with_their_status() {
        let (_dir, mut cat) = temp_catalog();
        let pid = cat.create_project(&project_init("p")).expect("project");
        cat.apply_update(&update(
            pid,
            1,
            1,
            vec![fw("a", "a1", 10), fw("b", "b1", 20)],
            vec![],
            vec![],
        ))
        .expect("first session");
        cat.apply_update(&update(pid, 1, 2, vec![], vec![fw("a", "a2", 15)], vec![]))
            .expect("modify a");
        cat.apply_update(&update(
            pid,
            1,
            3,
            vec![],
            vec![],
            vec![RelPath("b".to_string())],
        ))
        .expect("delete b");

        assert_eq!(
            cat.file_history(pid, &RelPath("a".to_string()))
                .expect("history reads"),
            vec![
                FileHistoryEntry {
                    content_hash: ContentHash("a1".to_string()),
                    size_bytes: 10,
                    disc_ordinal: 1,
                    session_ordinal: 1,
                    status: VersionStatus::Superseded,
                },
                FileHistoryEntry {
                    content_hash: ContentHash("a2".to_string()),
                    size_bytes: 15,
                    disc_ordinal: 1,
                    session_ordinal: 2,
                    status: VersionStatus::Current,
                },
            ]
        );
        assert_eq!(
            cat.file_history(pid, &RelPath("b".to_string()))
                .expect("history reads"),
            vec![FileHistoryEntry {
                content_hash: ContentHash("b1".to_string()),
                size_bytes: 20,
                disc_ordinal: 1,
                session_ordinal: 1,
                status: VersionStatus::Deleted,
            }]
        );
    }
```

- [x] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p diskrement-shell --lib
```

Expected: compilation errors — `no method named archive_pointers found`.

- [x] **Step 3: Write the implementation**

Add to the `impl Catalog` block in `crates/diskrement-shell/src/catalog/read.rs`:

```rust
    /// Where the bytes of every non-current version physically live.
    ///
    /// This is what gets baked into each burned disc's `pointers.json`, which
    /// is why the known gap matters: there is no dead flag here, so a pointer
    /// to a dead disc is durable misinformation on already-written media.
    /// Ported as-is (spec §1) — the fix is a `lost` column, not a filter.
    pub fn archive_pointers(
        &self,
        project: ProjectId,
    ) -> Result<Vec<ArchivePointer>, CatalogError> {
        let mut stmt = self
            .conn
            .prepare(
                "select fv.rel_path, fv.content_hash, fv.size_bytes,
                        d.ordinal, s.ordinal, p.disc_path, fvk.kind
                 from file_version fv
                 join placement p on p.file_version_id = fv.id
                 join session s   on s.id = p.session_id
                 join disc d      on d.id = s.disc_id
                 join (
                   select cr.file_version_id as fvid, cr.kind as kind,
                          row_number() over (partition by cr.file_version_id
                                             order by cr.id desc) as rn
                   from change_record cr
                 ) fvk on fvk.fvid = fv.id and fvk.rn = 1
                 where fv.project_id = ?1
                   and fv.id not in (
                     select fvid from (
                       select cr.file_version_id as fvid, cr.kind as kind,
                              row_number() over (partition by fv2.rel_path
                                                 order by cr.id desc) as rn
                       from change_record cr
                       join file_version fv2 on fv2.id = cr.file_version_id
                       where fv2.project_id = ?1
                     ) cur
                     where cur.rn = 1 and cur.kind <> 'DELETED'
                   )
                 order by fv.rel_path, d.ordinal, s.ordinal",
            )
            .map_err(CatalogError::from_sqlite)?;

        let rows = stmt
            .query_map(params![project.0], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })
            .map_err(CatalogError::from_sqlite)?;

        let mut out = Vec::new();
        for row in rows {
            let (path, hash, size, disc_ordinal, session_ordinal, disc_path, kind) =
                row.map_err(CatalogError::from_sqlite)?;
            out.push(ArchivePointer {
                rel_path: RelPath(path),
                content_hash: ContentHash(hash),
                size_bytes: size as u64,
                disc_ordinal: disc_ordinal as u32,
                session_ordinal: session_ordinal as u32,
                disc_path,
                kind: ManifestChangeKind::from_str(&kind)
                    .map_err(CatalogError::ConstraintViolation)?,
            });
        }
        Ok(out)
    }

    /// Every version of one path, oldest first, with where it lives and how it
    /// stands now.
    ///
    /// The status precedence is the rule to preserve: current wins over
    /// dead-disc. A path's current version whose only copy sits on a dead disc
    /// still reads `Current`, because the source file still exists and the next
    /// burn re-writes it. Only a *non-current* version on a dead disc is
    /// `Lost` — which is why `Lost` is per physical copy, not per content hash.
    pub fn file_history(
        &self,
        project: ProjectId,
        rel_path: &RelPath,
    ) -> Result<Vec<FileHistoryEntry>, CatalogError> {
        let mut stmt = self
            .conn
            .prepare(
                "select fv.content_hash, fv.size_bytes, d.ordinal, s.ordinal, fvk.kind,
                        case when fv.id = cur.fvid then 1 else 0 end as is_current,
                        case when exists (select 1 from disc_event e
                                          where e.disc_id = d.id and e.kind = 'MARKED_DEAD')
                             then 1 else 0 end as disc_dead
                 from file_version fv
                 join placement p on p.file_version_id = fv.id
                 join session s   on s.id = p.session_id
                 join disc d      on d.id = s.disc_id
                 join (
                   select cr.file_version_id as fvid, cr.kind as kind,
                          row_number() over (partition by cr.file_version_id
                                             order by cr.id desc) as rn
                   from change_record cr
                 ) fvk on fvk.fvid = fv.id and fvk.rn = 1
                 left join (
                   select fvid from (
                     select cr.file_version_id as fvid, cr.kind as kind,
                            row_number() over (partition by fv2.rel_path
                                               order by cr.id desc) as rn
                     from change_record cr
                     join file_version fv2 on fv2.id = cr.file_version_id
                     where fv2.project_id = ?1 and fv2.rel_path = ?2
                   ) c where c.rn = 1 and c.kind <> 'DELETED'
                 ) cur on cur.fvid = fv.id
                 where fv.project_id = ?1 and fv.rel_path = ?2
                 order by fv.id",
            )
            .map_err(CatalogError::from_sqlite)?;

        let rows = stmt
            .query_map(params![project.0, rel_path.as_str()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            })
            .map_err(CatalogError::from_sqlite)?;

        let mut out = Vec::new();
        for row in rows {
            let (hash, size, disc_ordinal, session_ordinal, kind, is_current, disc_dead) =
                row.map_err(CatalogError::from_sqlite)?;
            let status = if is_current == 1 {
                VersionStatus::Current
            } else if disc_dead == 1 {
                VersionStatus::Lost
            } else if kind == "DELETED" {
                VersionStatus::Deleted
            } else {
                VersionStatus::Superseded
            };
            out.push(FileHistoryEntry {
                content_hash: ContentHash(hash),
                size_bytes: size as u64,
                disc_ordinal: disc_ordinal as u32,
                session_ordinal: session_ordinal as u32,
                status,
            });
        }
        Ok(out)
    }
```

Extend `read.rs`'s imports:

```rust
use super::model::{ArchivePointer, FileHistoryEntry, VersionStatus};
use diskrement_core::ManifestChangeKind;
```

- [x] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p diskrement-shell --lib
```

Expected: `test result: ok. 24 passed`.

- [x] **Step 5: Verify lint and format, then commit**

```bash
cargo test --workspace \
  && cargo clippy --workspace --all-targets -- -D warnings \
  && cargo fmt --check
git add crates/diskrement-shell/src
git commit -m "feat(catalog): port archivePointers and fileHistory"
```

---

## Task 8: Per-disc contents and `mark_verified`

Port `placementsForDisc`, `discRecordedContents` and `markVerified`, and the
assertions in `DiscRecordedContentsSpec` and `MarkVerifiedSpec`.

The two per-disc queries answer different questions and a later phase depends on
the difference: `placements_for_disc` is what the *last* session staged (verify
checks a disc against its own recorded contents), while
`disc_recorded_contents` is everything physically on the disc, including
versions a later session of that same disc hid.

**Files:**
- Modify: `crates/diskrement-shell/src/catalog/read.rs`
- Modify: `crates/diskrement-shell/src/catalog/store.rs`

- [x] **Step 1: Write the failing tests**

Append inside `mod tests` in `crates/diskrement-shell/src/catalog/read.rs`:

```rust
    use crate::catalog::model::{DiscRecordedContents, SupersededVersion};
    use std::collections::BTreeMap;

    #[test]
    fn placements_for_disc_report_the_latest_sessions_files_only() {
        let (_dir, mut cat) = temp_catalog();
        let pid = cat.create_project(&project_init("p")).expect("project");
        cat.apply_update(&update(pid, 1, 1, vec![fw("a", "a1", 10)], vec![], vec![]))
            .expect("session 1");
        cat.apply_update(&update(pid, 1, 2, vec![fw("b", "b1", 20)], vec![], vec![]))
            .expect("session 2");

        let placements = cat.placements_for_disc(pid, 1).expect("query succeeds");
        assert_eq!(
            placements,
            BTreeMap::from([(RelPath("b".to_string()), ContentHash("b1".to_string()))])
        );
    }

    #[test]
    fn disc_recorded_contents_hide_earlier_versions_but_still_list_them() {
        let (_dir, mut cat) = temp_catalog();
        let pid = cat.create_project(&project_init("p")).expect("project");
        cat.apply_update(&update(
            pid,
            1,
            1,
            vec![fw("a.txt", "aa", 10), fw("b.txt", "bb", 20)],
            vec![],
            vec![],
        ))
        .expect("session 1");
        cat.apply_update(&update(pid, 1, 2, vec![], vec![fw("a.txt", "a2", 12)], vec![]))
            .expect("session 2");

        let recorded = cat.disc_recorded_contents(pid, 1).expect("query succeeds");
        assert_eq!(
            recorded.visible,
            BTreeMap::from([
                (RelPath("a.txt".to_string()), ContentHash("a2".to_string())),
                (RelPath("b.txt".to_string()), ContentHash("bb".to_string())),
            ])
        );
        assert_eq!(
            recorded.superseded,
            vec![SupersededVersion {
                session_ordinal: 1,
                rel_path: RelPath("a.txt".to_string()),
                content_hash: ContentHash("aa".to_string()),
            }]
        );
    }

    #[test]
    fn another_discs_versions_do_not_leak_into_recorded_contents() {
        let (_dir, mut cat) = temp_catalog();
        let pid = cat.create_project(&project_init("p")).expect("project");
        cat.apply_update(&update(pid, 1, 1, vec![fw("a.txt", "aa", 10)], vec![], vec![]))
            .expect("disc 1");
        cat.apply_update(&update(pid, 2, 1, vec![fw("z.txt", "zz", 30)], vec![], vec![]))
            .expect("disc 2");

        assert_eq!(
            cat.disc_recorded_contents(pid, 1).expect("query succeeds"),
            DiscRecordedContents {
                visible: BTreeMap::from([(
                    RelPath("a.txt".to_string()),
                    ContentHash("aa".to_string())
                )]),
                superseded: vec![],
            }
        );
        assert_eq!(
            cat.disc_recorded_contents(pid, 2).expect("query succeeds"),
            DiscRecordedContents {
                visible: BTreeMap::from([(
                    RelPath("z.txt".to_string()),
                    ContentHash("zz".to_string())
                )]),
                superseded: vec![],
            }
        );
    }
```

Append inside `mod tests` in `crates/diskrement-shell/src/catalog/store.rs`:

```rust
    use crate::catalog::model::SessionState;

    #[test]
    fn mark_verified_flips_a_burned_session() {
        let (_dir, mut cat) = temp_catalog();
        let pid = cat.create_project(&project_init("p")).expect("project");
        cat.apply_update(&update(pid, 1, 1, vec![fw("a", "h", 10)], vec![], vec![]))
            .expect("burn");

        cat.mark_verified(pid, 1, 1).expect("verification recorded");

        let verified = cat
            .sessions_by_state(pid, SessionState::Verified)
            .expect("query succeeds");
        assert_eq!(verified.len(), 1);
        assert_eq!((verified[0].disc_ordinal, verified[0].ordinal), (1, 1));
        assert!(cat
            .sessions_by_state(pid, SessionState::Burned)
            .expect("query succeeds")
            .is_empty());
    }

    #[test]
    fn marking_a_session_that_is_not_burned_is_an_illegal_transition() {
        let (_dir, mut cat) = temp_catalog();
        let pid = cat.create_project(&project_init("p")).expect("project");
        let err = cat
            .mark_verified(pid, 1, 1)
            .expect_err("there is no such session");
        assert!(
            matches!(err, CatalogError::IllegalStatusTransition(_)),
            "expected IllegalStatusTransition, got {err:?}"
        );
    }
```

- [x] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p diskrement-shell --lib
```

Expected: compilation errors — `no method named placements_for_disc found`,
`no method named mark_verified found`.

- [x] **Step 3: Write the read implementations**

Add to the `impl Catalog` block in `crates/diskrement-shell/src/catalog/read.rs`:

```rust
    /// What the burner staged on `disc_ordinal`'s LATEST session — used by
    /// verify to check a disc against its own recorded contents rather than
    /// against the current view.
    ///
    /// A seal session (`plan --finalize`) places no files, so this is
    /// legitimately empty for a disc whose last session was a seal.
    pub fn placements_for_disc(
        &self,
        project: ProjectId,
        disc_ordinal: u32,
    ) -> Result<BTreeMap<RelPath, ContentHash>, CatalogError> {
        let mut stmt = self
            .conn
            .prepare(
                "select fv.rel_path, fv.content_hash
                 from placement p
                 join session s   on s.id = p.session_id
                 join disc d      on d.id = s.disc_id
                 join file_version fv on fv.id = p.file_version_id
                 where d.project_id = ?1 and d.ordinal = ?2
                   and s.ordinal = (select max(s2.ordinal) from session s2
                                    where s2.disc_id = d.id)",
            )
            .map_err(CatalogError::from_sqlite)?;

        let rows = stmt
            .query_map(params![project.0, disc_ordinal as i64], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(CatalogError::from_sqlite)?;

        let mut out = BTreeMap::new();
        for row in rows {
            let (path, hash) = row.map_err(CatalogError::from_sqlite)?;
            out.insert(RelPath(path), ContentHash(hash));
        }
        Ok(out)
    }

    /// Everything recorded as physically on one disc: the merged view a mount
    /// shows, plus the versions later sessions of that same disc hid.
    ///
    /// Powers the two-tier read-back verify — the visible set is checked by
    /// mounting, the superseded set by extracting the session that wrote it.
    pub fn disc_recorded_contents(
        &self,
        project: ProjectId,
        disc_ordinal: u32,
    ) -> Result<DiscRecordedContents, CatalogError> {
        let mut stmt = self
            .conn
            .prepare(
                "select rp, ch, sess_ord, rn from (
                   select fv.rel_path as rp, fv.content_hash as ch, s.ordinal as sess_ord,
                          row_number() over (partition by fv.rel_path
                                             order by s.ordinal desc, p.id desc) as rn
                   from placement p
                   join session s on s.id = p.session_id
                   join disc d on d.id = s.disc_id
                   join file_version fv on fv.id = p.file_version_id
                   where d.project_id = ?1 and d.ordinal = ?2
                 ) order by rp, rn",
            )
            .map_err(CatalogError::from_sqlite)?;

        let rows = stmt
            .query_map(params![project.0, disc_ordinal as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .map_err(CatalogError::from_sqlite)?;

        let mut visible = BTreeMap::new();
        let mut superseded = Vec::new();
        for row in rows {
            let (path, hash, session_ordinal, rank) = row.map_err(CatalogError::from_sqlite)?;
            if rank == 1 {
                visible.insert(RelPath(path), ContentHash(hash));
            } else {
                superseded.push(SupersededVersion {
                    session_ordinal: session_ordinal as u32,
                    rel_path: RelPath(path),
                    content_hash: ContentHash(hash),
                });
            }
        }
        Ok(DiscRecordedContents {
            visible,
            superseded,
        })
    }
```

Extend `read.rs`'s imports with
`use super::model::{DiscRecordedContents, SupersededVersion};`.

- [x] **Step 4: Write `mark_verified`**

Add to the `impl Catalog` block in `crates/diskrement-shell/src/catalog/store.rs`:

```rust
    /// Record that a burned session was read back and matched.
    ///
    /// Only a BURNED session can become VERIFIED; anything else — a missing
    /// session, or one already verified — is an illegal transition.
    pub fn mark_verified(
        &mut self,
        project: ProjectId,
        disc_ordinal: u32,
        session_ordinal: u32,
    ) -> Result<(), CatalogError> {
        self.tx(|tx| {
            let updated = tx
                .execute(
                    "update session set state = 'VERIFIED'
                     where state = 'BURNED' and ordinal = ?1
                       and disc_id = (select id from disc
                                      where project_id = ?2 and ordinal = ?3)",
                    params![session_ordinal as i64, project.0, disc_ordinal as i64],
                )
                .map_err(CatalogError::from_sqlite)?;
            if updated == 0 {
                return Err(CatalogError::IllegalStatusTransition(format!(
                    "no BURNED session {session_ordinal} on disc {disc_ordinal}"
                )));
            }
            Ok(())
        })
    }
```

- [x] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p diskrement-shell --lib
```

Expected: `test result: ok. 29 passed`.

- [x] **Step 6: Verify lint and format, then commit**

```bash
cargo test --workspace \
  && cargo clippy --workspace --all-targets -- -D warnings \
  && cargo fmt --check
git add crates/diskrement-shell/src
git commit -m "feat(catalog): port per-disc contents and markVerified"
```

---

## Task 9: `mark_dead` — the coaster event

Port `markDead` and its private `deadDiscVersions`, and all eleven assertions in
`MarkDeadSpec.scala`. **Read both the Scala method and the scaladoc on
`deadDiscVersions` before starting** — the "no copy on another live disc" filter
and the group-by-version rule are each pinned by a separate test, and neither is
obvious from the SQL alone.

This is where the derived-`DEAD` invariant is established: marking a disc dead
appends one `disc_event` row and one `lost_version` row per casualty, and
touches the `disc` row not at all.

**Files:**
- Modify: `crates/diskrement-shell/src/catalog/store.rs`

- [x] **Step 1: Write the failing tests**

Append inside `mod tests` in `crates/diskrement-shell/src/catalog/store.rs`:

```rust
    use chrono::{DateTime, TimeZone, Utc};
    use diskrement_core::{ContentHash, RelPath};

    fn dead_at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 2, 9, 0, 0).unwrap()
    }

    fn dead_at2() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 3, 9, 0, 0).unwrap()
    }

    /// Disc 1, session 1 adds a@a1 and b@b1; session 2 modifies b to b2 — so
    /// b@b1 is superseded in place, on the same disc.
    fn seed(cat: &mut crate::catalog::Catalog) -> crate::catalog::model::ProjectId {
        let pid = cat.create_project(&project_init("p")).expect("project");
        cat.apply_update(&update(
            pid,
            1,
            1,
            vec![fw("a", "a1", 10), fw("b", "b1", 20)],
            vec![],
            vec![],
        ))
        .expect("session 1");
        cat.apply_update(&update(pid, 1, 2, vec![], vec![fw("b", "b2", 25)], vec![]))
            .expect("session 2");
        pid
    }

    /// Put a's version on disc 2 as well, with a raw extra placement — exactly
    /// what copies>1 will eventually do for real. No public method can express
    /// this yet, so the test writes the row itself.
    fn also_place_a_on_disc_2(cat: &crate::catalog::Catalog) {
        cat.raw_connection_for_tests()
            .execute(
                "insert into placement (file_version_id, session_id, disc_path, copy_index)
                 select (select id from file_version where rel_path = 'a'),
                        (select s.id from session s join disc d on d.id = s.disc_id
                         where d.ordinal = 2),
                        '/data/a', 1",
                [],
            )
            .expect("the extra placement inserts");
    }

    /// Every recorded casualty, as (relPath, fate), sorted.
    fn fates(cat: &crate::catalog::Catalog) -> Vec<(String, String)> {
        let conn = cat.raw_connection_for_tests();
        let mut stmt = conn
            .prepare(
                "select fv.rel_path, lv.fate from lost_version lv
                 join file_version fv on fv.id = lv.file_version_id",
            )
            .expect("query prepares");
        let mut rows: Vec<(String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .expect("rows are text")
            .map(|r| r.expect("readable row"))
            .collect();
        rows.sort();
        rows
    }

    #[test]
    fn mark_dead_splits_versions_into_lost_and_reburn_and_records_both() {
        let (_dir, mut cat) = temp_catalog();
        let pid = seed(&mut cat);
        let report = cat.mark_dead(pid, 1, &dead_at()).expect("disc 1 dies");

        assert_eq!(report.disc_ordinal, 1);
        assert_eq!(
            report
                .lost
                .iter()
                .map(|e| (e.rel_path.as_str(), e.content_hash.as_str(), e.session_ordinal))
                .collect::<Vec<_>>(),
            vec![("b", "b1", 1)]
        );
        let mut reburn: Vec<(&str, &str)> = report
            .reburn
            .iter()
            .map(|e| (e.rel_path.as_str(), e.content_hash.as_str()))
            .collect();
        reburn.sort();
        assert_eq!(reburn, vec![("a", "a1"), ("b", "b2")]);

        assert_eq!(
            cat.read_disc_states(pid).expect("states read"),
            vec![diskrement_core::DiscState {
                ordinal: 1,
                status: diskrement_core::DiscStatus::Dead,
                usable_bytes: 1_000_000,
                used_bytes: 55,
            }]
        );
        assert_eq!(
            fates(&cat),
            vec![
                ("a".to_string(), "REBURN".to_string()),
                ("b".to_string(), "LOST".to_string()),
                ("b".to_string(), "REBURN".to_string()),
            ]
        );
    }

    #[test]
    fn deadness_is_derived_so_the_disc_row_is_untouched() {
        // The status column keeps its OPEN/FINALIZED check forever; DEAD exists
        // only in the derived DiscState. Without this assertion, an
        // update-in-place implementation would still satisfy the read_disc_states
        // assertion above.
        let (_dir, mut cat) = temp_catalog();
        let pid = seed(&mut cat);
        cat.mark_dead(pid, 1, &dead_at()).expect("disc 1 dies");

        let conn = cat.raw_connection_for_tests();
        let status: String = conn
            .query_row("select status from disc where ordinal = 1", [], |r| r.get(0))
            .expect("the disc row is still there");
        assert_eq!(status, "OPEN");

        let mut stmt = conn.prepare("select kind from disc_event").expect("prepares");
        let events: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .expect("rows are text")
            .map(|r| r.expect("readable row"))
            .collect();
        assert_eq!(events, vec!["MARKED_DEAD".to_string()]);
    }

    #[test]
    fn a_version_alive_on_another_disc_is_neither_lost_nor_reburned() {
        let (_dir, mut cat) = temp_catalog();
        let pid = seed(&mut cat);
        cat.apply_update(&update(pid, 2, 1, vec![fw("c", "c1", 5)], vec![], vec![]))
            .expect("disc 2 exists");
        also_place_a_on_disc_2(&cat);

        let report = cat.mark_dead(pid, 1, &dead_at()).expect("disc 1 dies");
        assert_eq!(
            report.reburn.iter().map(|e| e.rel_path.as_str()).collect::<Vec<_>>(),
            vec!["b"]
        );
        assert_eq!(
            report.lost.iter().map(|e| e.rel_path.as_str()).collect::<Vec<_>>(),
            vec!["b"]
        );
        // Nothing was recorded against a: its bytes still exist on live disc 2.
        assert_eq!(
            fates(&cat),
            vec![
                ("b".to_string(), "LOST".to_string()),
                ("b".to_string(), "REBURN".to_string()),
            ]
        );
    }

    #[test]
    fn a_copy_on_an_already_dead_disc_no_longer_shields_a_version() {
        let (_dir, mut cat) = temp_catalog();
        let pid = seed(&mut cat);
        cat.apply_update(&update(pid, 2, 1, vec![fw("c", "c1", 5)], vec![], vec![]))
            .expect("disc 2 exists");
        also_place_a_on_disc_2(&cat);

        let second = cat.mark_dead(pid, 2, &dead_at()).expect("disc 2 dies");
        // While disc 1 lived, a's copy there kept it off disc 2's casualty list.
        assert_eq!(
            second.reburn.iter().map(|e| e.rel_path.as_str()).collect::<Vec<_>>(),
            vec!["c"]
        );
        assert!(second.lost.is_empty());

        let first = cat.mark_dead(pid, 1, &dead_at2()).expect("disc 1 dies");
        // Now that disc 2 is dead too, a's only surviving copy is gone.
        assert_eq!(
            first
                .reburn
                .iter()
                .map(|e| (e.rel_path.as_str(), e.content_hash.as_str()))
                .collect::<Vec<_>>(),
            vec![("a", "a1"), ("b", "b2")]
        );
        assert_eq!(
            first.lost.iter().map(|e| e.rel_path.as_str()).collect::<Vec<_>>(),
            vec!["b"]
        );
    }

    #[test]
    fn a_tombstoned_path_is_lost_never_reburned() {
        let (_dir, mut cat) = temp_catalog();
        let pid = seed(&mut cat);
        cat.apply_update(&update(
            pid,
            1,
            3,
            vec![],
            vec![],
            vec![RelPath("a".to_string())],
        ))
        .expect("delete a");

        let report = cat.mark_dead(pid, 1, &dead_at()).expect("disc 1 dies");
        assert_eq!(
            report
                .lost
                .iter()
                .map(|e| (e.rel_path.as_str(), e.content_hash.as_str(), e.session_ordinal))
                .collect::<Vec<_>>(),
            vec![("a", "a1", 1), ("b", "b1", 1)]
        );
        assert_eq!(
            report
                .reburn
                .iter()
                .map(|e| (e.rel_path.as_str(), e.content_hash.as_str()))
                .collect::<Vec<_>>(),
            vec![("b", "b2")]
        );
        assert_eq!(
            fates(&cat),
            vec![
                ("a".to_string(), "LOST".to_string()),
                ("b".to_string(), "LOST".to_string()),
                ("b".to_string(), "REBURN".to_string()),
            ]
        );
    }

    #[test]
    fn several_placements_of_one_version_make_one_casualty() {
        let (_dir, mut cat) = temp_catalog();
        let pid = seed(&mut cat);
        // b@b2 was burned in session 2; pretend a second copy also landed in
        // session 1. `lost_version` is append-only, so one version must still
        // produce exactly one row.
        cat.raw_connection_for_tests()
            .execute(
                "insert into placement (file_version_id, session_id, disc_path, copy_index)
                 select (select max(id) from file_version where rel_path = 'b'),
                        (select s.id from session s join disc d on d.id = s.disc_id
                         where d.ordinal = 1 and s.ordinal = 1),
                        '/data/b', 1",
                [],
            )
            .expect("the extra placement inserts");

        let report = cat.mark_dead(pid, 1, &dead_at()).expect("disc 1 dies");
        assert_eq!(
            report
                .reburn
                .iter()
                .map(|e| (e.rel_path.as_str(), e.content_hash.as_str()))
                .collect::<Vec<_>>(),
            vec![("a", "a1"), ("b", "b2")]
        );
        assert_eq!(
            report.lost.iter().map(|e| e.rel_path.as_str()).collect::<Vec<_>>(),
            vec!["b"]
        );
        assert_eq!(
            fates(&cat),
            vec![
                ("a".to_string(), "REBURN".to_string()),
                ("b".to_string(), "LOST".to_string()),
                ("b".to_string(), "REBURN".to_string()),
            ]
        );
    }

    #[test]
    fn marking_a_dead_disc_again_fails_and_so_does_an_unknown_ordinal() {
        let (_dir, mut cat) = temp_catalog();
        let pid = seed(&mut cat);
        cat.mark_dead(pid, 1, &dead_at()).expect("disc 1 dies");

        let duplicate = cat.mark_dead(pid, 1, &dead_at()).expect_err("already dead");
        assert!(
            matches!(duplicate, CatalogError::IllegalStatusTransition(_)),
            "expected IllegalStatusTransition, got {duplicate:?}"
        );

        let unknown = cat.mark_dead(pid, 9, &dead_at()).expect_err("no such disc");
        assert!(
            matches!(unknown, CatalogError::ConstraintViolation(_)),
            "expected ConstraintViolation, got {unknown:?}"
        );
    }

    #[test]
    fn read_view_drops_current_versions_whose_only_copy_is_dead() {
        let (_dir, mut cat) = temp_catalog();
        let pid = seed(&mut cat);
        let before = cat.read_view(pid).expect("view reads");
        assert_eq!(
            before.current.keys().map(RelPath::as_str).collect::<Vec<_>>(),
            vec!["a", "b"]
        );

        cat.mark_dead(pid, 1, &dead_at()).expect("disc 1 dies");
        assert!(cat.read_view(pid).expect("view reads").current.is_empty());
    }

    #[test]
    fn a_current_version_with_a_live_copy_stays_in_the_view() {
        let (_dir, mut cat) = temp_catalog();
        let pid = seed(&mut cat);
        cat.apply_update(&update(pid, 2, 1, vec![fw("c", "c1", 5)], vec![], vec![]))
            .expect("disc 2 exists");
        also_place_a_on_disc_2(&cat);
        cat.mark_dead(pid, 1, &dead_at()).expect("disc 1 dies");

        // a survives on live disc 2; b@b2 is disc-1-only and drops; c is native
        // to disc 2.
        assert_eq!(
            cat.read_view(pid)
                .expect("view reads")
                .current
                .keys()
                .map(RelPath::as_str)
                .collect::<Vec<_>>(),
            vec!["a", "c"]
        );
    }

    #[test]
    fn file_history_marks_dead_disc_versions_lost() {
        let (_dir, mut cat) = temp_catalog();
        let pid = seed(&mut cat);
        cat.mark_dead(pid, 1, &dead_at()).expect("disc 1 dies");

        let history = cat
            .file_history(pid, &RelPath("b".to_string()))
            .expect("history reads");
        assert_eq!(
            history.iter().map(|e| e.status).collect::<Vec<_>>(),
            vec![
                crate::catalog::model::VersionStatus::Lost,
                crate::catalog::model::VersionStatus::Current,
            ]
        );
    }

    #[test]
    fn a_current_version_on_a_dead_disc_still_renders_current() {
        // a@a1 is both a's only version and its current one, and disc 1 — its
        // only copy — is dead. is_current wins over disc_dead: the source still
        // has the file and it is about to be re-burned.
        let (_dir, mut cat) = temp_catalog();
        let pid = seed(&mut cat);
        cat.mark_dead(pid, 1, &dead_at()).expect("disc 1 dies");

        let history = cat
            .file_history(pid, &RelPath("a".to_string()))
            .expect("history reads");
        assert_eq!(
            history.iter().map(|e| e.status).collect::<Vec<_>>(),
            vec![crate::catalog::model::VersionStatus::Current]
        );
    }
```

Note the unused-import risk: `ContentHash` is imported above for readability but
may not be referenced by these tests. If clippy flags it, delete the import
rather than adding an allow.

- [x] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p diskrement-shell --lib
```

Expected: compilation errors — `no method named mark_dead found for struct Catalog`.

- [x] **Step 3: Write the implementation**

Add to the `impl Catalog` block in `crates/diskrement-shell/src/catalog/store.rs`:

```rust
    /// Append a MARKED_DEAD event for `disc_ordinal` and record its casualties.
    ///
    /// Every version whose only copy was on this disc becomes either REBURN
    /// (the path's current version — the next scan/plan/burn re-writes it from
    /// source) or LOST (a superseded or deleted version — permanently gone).
    /// The `disc` row itself is never touched: dead-ness is derived from
    /// `disc_event` at read time.
    pub fn mark_dead(
        &mut self,
        project: ProjectId,
        disc_ordinal: u32,
        at: &DateTime<Utc>,
    ) -> Result<LostReport, CatalogError> {
        self.tx(|tx| {
            let disc_id = disc_id_by_ordinal(tx, project, disc_ordinal)?;
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
                    "disc {disc_ordinal} is already dead"
                )));
            }

            let casualties = dead_disc_versions(tx, project, disc_id)?;

            tx.execute(
                "insert into disc_event (disc_id, kind, at) values (?1, 'MARKED_DEAD', ?2)",
                params![disc_id, time::format(at)],
            )
            .map_err(CatalogError::from_sqlite)?;
            let event_id = tx.last_insert_rowid();

            for casualty in &casualties {
                let fate = if casualty.is_current { "REBURN" } else { "LOST" };
                tx.execute(
                    "insert into lost_version (event_id, file_version_id, fate)
                     values (?1, ?2, ?3)",
                    params![event_id, casualty.file_version_id, fate],
                )
                .map_err(CatalogError::from_sqlite)?;
            }

            Ok(LostReport {
                disc_ordinal,
                lost: casualties
                    .iter()
                    .filter(|c| !c.is_current)
                    .map(|c| LostVersionEntry {
                        rel_path: c.rel_path.clone(),
                        content_hash: c.content_hash.clone(),
                        size_bytes: c.size_bytes,
                        session_ordinal: c.session_ordinal,
                    })
                    .collect(),
                reburn: casualties
                    .iter()
                    .filter(|c| c.is_current)
                    .map(|c| ReburnEntry {
                        rel_path: c.rel_path.clone(),
                        content_hash: c.content_hash.clone(),
                        size_bytes: c.size_bytes,
                    })
                    .collect(),
            })
        })
    }
```

and, below the other free functions:

```rust
/// One version that dies with the disc.
struct Casualty {
    file_version_id: i64,
    rel_path: RelPath,
    content_hash: ContentHash,
    size_bytes: u64,
    session_ordinal: u32,
    is_current: bool,
}

/// Versions placed on the dying disc that have no copy on any other live disc,
/// flagged with whether each is its path's current version.
///
/// Grouped by version: a version with several placements on this disc (a later
/// session, or copies>1) is one casualty, not one per placement —
/// `lost_version` is append-only.
///
/// The "no copy on another live disc" filter is correct for the lost/reburn
/// split under copies>1. Restoring a lost *copy* is a separate concern that
/// lands with copies>1: at copies=2, a version whose disc-1 copy dies while
/// disc 3 lives is correctly not a casualty, and nothing re-plans a
/// replacement — `read_view` is a path->entry map with no copy count, so
/// redundancy silently degrades 2 -> 1. Ported as-is (spec §1).
fn dead_disc_versions(
    tx: &Transaction<'_>,
    project: ProjectId,
    disc_id: i64,
) -> Result<Vec<Casualty>, CatalogError> {
    let mut stmt = tx
        .prepare(
            "select fv.id, fv.rel_path, fv.content_hash, fv.size_bytes,
                    min(s.ordinal) as sess_ord,
                    max(case when cur.fvid is not null then 1 else 0 end) as is_current
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
               and not exists (
                 select 1 from placement p2
                 join session s2 on s2.id = p2.session_id
                 join disc d2 on d2.id = s2.disc_id
                 where p2.file_version_id = fv.id and d2.id <> ?2
                   and not exists (select 1 from disc_event e
                                   where e.disc_id = d2.id and e.kind = 'MARKED_DEAD'))
             group by fv.id, fv.rel_path, fv.content_hash, fv.size_bytes
             order by fv.rel_path, sess_ord",
        )
        .map_err(CatalogError::from_sqlite)?;

    let rows = stmt
        .query_map(params![project.0, disc_id], |row| {
            Ok(Casualty {
                file_version_id: row.get::<_, i64>(0)?,
                rel_path: RelPath(row.get::<_, String>(1)?),
                content_hash: ContentHash(row.get::<_, String>(2)?),
                size_bytes: row.get::<_, i64>(3)? as u64,
                session_ordinal: row.get::<_, i64>(4)? as u32,
                is_current: row.get::<_, i64>(5)? == 1,
            })
        })
        .map_err(CatalogError::from_sqlite)?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(CatalogError::from_sqlite)
}
```

Extend `store.rs`'s imports with:

```rust
use super::model::{LostReport, LostVersionEntry, ReburnEntry};
use diskrement_core::ContentHash;
```

- [x] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p diskrement-shell --lib
```

Expected: `test result: ok. 40 passed`.

If `several_placements_of_one_version_make_one_casualty` fails with two `b`
REBURN rows, the `group by` was dropped or the `max(case when …)` was replaced
by a per-row flag. If `a_copy_on_an_already_dead_disc_no_longer_shields_a_version`
fails, the `not exists (… disc_event …)` clause inside the shielding subquery is
missing — that clause is what makes death cumulative.

- [x] **Step 5: Verify lint and format, then commit**

```bash
cargo test --workspace \
  && cargo clippy --workspace --all-targets -- -D warnings \
  && cargo fmt --check
git add crates/diskrement-shell/src
git commit -m "feat(catalog): port markDead and the lost/reburn casualty split"
```

---

## Task 10: Snapshots and standalone updates

Port `snapshotTo` and `CatalogStore.applyUpdateTo`, and the assertion in
`SnapshotToSpec`.

`snapshot_to` is `VACUUM INTO`, which must run **outside** a transaction and
refuses to overwrite an existing destination. `apply_update_to` opens a second,
standalone catalog file and applies an update to it — that is how the post-burn
`catalog.sqlite` embedded on each disc is materialised without committing
anything to the live catalog. Never file-swap a SQLite database under an open
connection; the double-apply reaches the same end state through the proven path.

**Files:**
- Modify: `crates/diskrement-shell/src/catalog/store.rs`

- [x] **Step 1: Write the failing tests**

Append inside `mod tests` in `crates/diskrement-shell/src/catalog/store.rs`:

```rust
    #[test]
    fn snapshot_to_writes_a_self_contained_copy() {
        let (dir, mut cat) = temp_catalog();
        let pid = cat.create_project(&project_init("p")).expect("project");
        cat.apply_update(&update(
            pid,
            1,
            1,
            vec![fw("a", "a1", 10), fw("b", "b1", 20)],
            vec![],
            vec![],
        ))
        .expect("burn");

        let dest = dir.path().join("snapshot.sqlite");
        cat.snapshot_to(&dest).expect("vacuum into succeeds");
        assert!(dest.is_file());

        let copy = rusqlite::Connection::open(&dest).expect("the snapshot opens");
        let versions: i64 = copy
            .query_row("select count(*) from file_version", [], |r| r.get(0))
            .expect("the snapshot has the source's rows");
        assert_eq!(versions, 2);

        // VACUUM INTO refuses an existing destination; the burn flow relies on
        // that rather than on silently overwriting a stale snapshot.
        assert!(cat.snapshot_to(&dest).is_err());
    }

    #[test]
    fn apply_update_to_writes_a_standalone_catalog_without_touching_the_live_one() {
        let (dir, mut cat) = temp_catalog();
        let pid = cat.create_project(&project_init("p")).expect("project");
        cat.apply_update(&update(pid, 1, 1, vec![fw("a", "a1", 10)], vec![], vec![]))
            .expect("first burn");

        let work_copy = dir.path().join("burn-catalog.sqlite");
        cat.snapshot_to(&work_copy).expect("snapshot taken");

        let next = update(pid, 1, 2, vec![fw("b", "b1", 20)], vec![], vec![]);
        crate::catalog::Catalog::apply_update_to(&work_copy, &next)
            .expect("the work copy takes the update");

        let copy = crate::catalog::Catalog::open(&work_copy).expect("work copy opens");
        assert_eq!(count(&copy, "file_version"), 2);
        // The live catalog has not moved: this is the post-burn snapshot path,
        // not the commit point.
        assert_eq!(count(&cat, "file_version"), 1);
    }
```

- [x] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p diskrement-shell --lib
```

Expected: compilation errors — `no method named snapshot_to found`.

- [x] **Step 3: Write the implementation**

Add to the `impl Catalog` block in `crates/diskrement-shell/src/catalog/store.rs`:

```rust
    /// Write a consistent full-database snapshot to `dest` via `VACUUM INTO`.
    ///
    /// Runs outside a transaction, and `dest` must not already exist —
    /// `VACUUM INTO` fails if it does, which is the behaviour the burn flow
    /// relies on to notice a stale work-directory snapshot.
    pub fn snapshot_to(&self, dest: &Path) -> Result<(), CatalogError> {
        self.conn
            .execute("vacuum into ?1", params![dest.to_string_lossy()])
            .map_err(CatalogError::from_sqlite)?;
        Ok(())
    }

    /// Open a standalone catalog file, apply `update`, and close it.
    ///
    /// Used to materialise the post-burn catalog state into the per-disc
    /// snapshot without committing anything to the live catalog: the live
    /// commit happens separately, only after the discs are staged. The disc's
    /// embedded `catalog.sqlite` therefore describes the state *after* its own
    /// burn, so a single disc knows about itself and its siblings.
    pub fn apply_update_to(path: &Path, update: &CatalogUpdate) -> Result<(), CatalogError> {
        let mut catalog = Catalog::open(path)?;
        catalog.apply_update(update)
    }
```

Add `use std::path::Path;` to `store.rs`'s imports.

- [x] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p diskrement-shell --lib
```

Expected: `test result: ok. 42 passed`.

- [x] **Step 5: Verify lint and format, then commit**

```bash
cargo test --workspace \
  && cargo clippy --workspace --all-targets -- -D warnings \
  && cargo fmt --check
git add crates/diskrement-shell/src
git commit -m "feat(catalog): port snapshotTo and applyUpdateTo"
```

---

## Task 11: The append-only property

Port `AppendOnlySpec.scala` — the property that pins the catalog's central
invariant. It is a property test rather than an example test because the
invariant is about *every* sequence of scans, not one: whatever the history,
`read_view` equals the latest scan and `file_version` only ever grows.

This is also the first test that drives the store through `diff`, which is why
it comes after the write and read paths rather than beside `apply_update`.

**Files:**
- Modify: `crates/diskrement-shell/src/catalog/store.rs`

- [x] **Step 1: Write the failing test**

Append inside `mod tests` in `crates/diskrement-shell/src/catalog/store.rs`:

```rust
    use diskrement_core::{CatalogView, ChangeSet, ScannedFile};
    use proptest::prelude::*;
    use std::collections::BTreeMap;

    /// A generated scan: a set of paths, each mapped to a content hash.
    fn scanned_of(scan: &BTreeMap<String, String>) -> Vec<ScannedFile> {
        scan.iter()
            .map(|(path, hash)| ScannedFile {
                rel_path: RelPath(path.clone()),
                content_hash: ContentHash(hash.clone()),
                size_bytes: 1,
                mtime: ts(),
            })
            .collect()
    }

    /// Turn a change set into a one-disc update — the mini-orchestration the
    /// real pipeline does for real in P4.
    fn to_update(project: ProjectId, session_ordinal: u32, changes: &ChangeSet) -> CatalogUpdate {
        let as_write = |f: &ScannedFile| FileWrite {
            rel_path: f.rel_path.clone(),
            content_hash: f.content_hash.clone(),
            size_bytes: f.size_bytes,
            mtime: ts(),
            disc_path: format!("/data/{}", f.rel_path.as_str()),
            copy_index: 0,
        };
        update(
            project,
            1,
            session_ordinal,
            changes.added.iter().map(as_write).collect(),
            changes.modified.iter().map(as_write).collect(),
            changes.deleted.clone(),
        )
    }

    proptest! {
        // Forty cases, matching the Scala's `TestAspect.samples(40)`. Each case
        // builds a fresh catalog file, so this is the slowest test in the
        // crate; more cases buy little.
        #![proptest_config(ProptestConfig::with_cases(40))]

        #[test]
        fn the_view_tracks_the_latest_scan_while_file_version_only_grows(
            scans in prop::collection::vec(
                prop::collection::btree_map(
                    prop::sample::select(vec!["a", "b", "c", "d", "e"]).prop_map(String::from),
                    prop::sample::select(vec!["h1", "h2", "h3"]).prop_map(String::from),
                    0..5usize,
                ),
                1..6usize,
            )
        ) {
            let (_dir, mut cat) = temp_catalog();
            let pid = cat.create_project(&project_init("p")).expect("project");

            let mut view = CatalogView::empty();
            let mut previous_rows = 0i64;
            for (index, scan) in scans.iter().enumerate() {
                let scanned = scanned_of(scan);
                let changes = diskrement_core::diff::diff(&view, &scanned);
                cat.apply_update(&to_update(pid, index as u32 + 1, &changes))
                    .expect("the session applies");

                let rows = count(&cat, "file_version");
                prop_assert!(
                    rows >= previous_rows,
                    "file_version shrank: {} -> {}",
                    previous_rows,
                    rows
                );
                previous_rows = rows;
                view = CatalogView::from_scanned(&scanned);
            }

            let stored = cat.read_view(pid).expect("view reads");
            let expected: Vec<(&str, &str)> = view
                .current
                .values()
                .map(|e| (e.rel_path.as_str(), e.content_hash.as_str()))
                .collect();
            let actual: Vec<(&str, &str)> = stored
                .current
                .values()
                .map(|e| (e.rel_path.as_str(), e.content_hash.as_str()))
                .collect();
            prop_assert_eq!(actual, expected);
        }
    }
```

- [x] **Step 2: Run the test to verify it fails**

Before running, comment out the `prop_assert_eq!` on the final view and confirm
the rest passes; then restore it. A property test that has never failed has not
been shown to test anything — the two obvious mutations to try are dropping the
`l.kind <> 'DELETED'` clause from `read_view` and writing tombstones as new
`file_version` rows.

```bash
cargo test -p diskrement-shell --lib the_view_tracks
```

Expected first: compilation errors — `unresolved import diskrement_core::ChangeSet`
until the import is added. Then, with the deliberate mutation in place, a
shrunk counterexample naming the smallest scan sequence that breaks it.

- [x] **Step 3: Run the whole suite**

```bash
cargo test -p diskrement-shell --lib
```

Expected: `test result: ok. 43 passed`.

If it fails with `cannot tombstone unknown path`, `diff` and the store disagree
about which paths are deletable — the change set's `deleted` must only ever name
paths the view already held.

- [x] **Step 4: Verify lint and format, then commit**

```bash
cargo test --workspace \
  && cargo clippy --workspace --all-targets -- -D warnings \
  && cargo fmt --check
git add crates/diskrement-shell/src
git commit -m "test(catalog): port the append-only property"
```

---

## Task 12: Reading catalogs the Scala build wrote

The fixture corpus (spec §4) is the only cross-implementation evidence that
exists. This task points every read method at it.

Fixtures are never opened through `Catalog::open` in place — that would run the
migration runner against the corpus and write to it. Each test copies the
database into a `TempDir` first, which has the side benefit of proving the
migration runner is a no-op on an already-current catalog.

**Files:**
- Modify: `crates/diskrement-shell/tests/support/mod.rs`
- Create: `crates/diskrement-shell/tests/fixture_catalog.rs`

- [x] **Step 1: Add the copy helper**

Append to `crates/diskrement-shell/tests/support/mod.rs`:

```rust
/// Copy a fixture database into a fresh temp directory and return both.
///
/// Hold the `TempDir` for as long as the copy is in use. Opening the corpus
/// itself through `Catalog::open` would run migrations against it and write to
/// `fixtures/`; a copy is also the only honest way to test that opening an
/// already-current catalog changes nothing.
pub fn fixture_copy(rel: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::TempDir::new().expect("a writable temp directory");
    let dest = dir.path().join("catalog.sqlite");
    std::fs::copy(fixtures_dir().join(rel), &dest)
        .unwrap_or_else(|e| panic!("copying fixture {rel}: {e}"));
    (dir, dest)
}
```

- [x] **Step 2: Write the test file**

`crates/diskrement-shell/tests/fixture_catalog.rs`:

```rust
//! Every read method, against catalogs the Scala build actually wrote.
//!
//! The scenario behind the corpus (see `fixtures/README.md`): two 800-byte
//! files spill across discs 1-2, a modify+delete cycle lands a@2 on disc 3,
//! disc 3 is marked dead, a recovery burn puts a@2 on disc 4, and
//! `plan --finalize` seals disc 4 with a file-less session.

mod support;

use std::collections::BTreeMap;

use diskrement_core::{ContentHash, DiscState, DiscStatus, ManifestChangeKind, RelPath};
use diskrement_shell::catalog::model::{
    ArchivePointer, FileHistoryEntry, SessionState, VersionStatus,
};
use diskrement_shell::catalog::{time, Catalog};
use support::{fixture_copy, fixture_dbs};

const A1: &str = "8e19511b7991430a1950f27907234183151bb0dbf1fbf28cd5b765088feb9f70";
const A2: &str = "e148cf0e8c66a7c18ea6be52a5215dd2a770d74b6c014d51d1e1e0463e882e77";
const B1: &str = "3413ff9e166e40ad8798e36609f28433496ee3a0f8984f73d33b0a80ffef2a89";

fn open(rel: &str) -> (tempfile::TempDir, Catalog) {
    let (dir, path) = fixture_copy(rel);
    let catalog = Catalog::open(&path).expect("a Scala-written catalog opens");
    (dir, catalog)
}

fn rel(path: &str) -> RelPath {
    RelPath(path.to_string())
}

fn hash(hex: &str) -> ContentHash {
    ContentHash(hex.to_string())
}

#[test]
fn every_fixture_catalog_opens_and_reads_without_error() {
    let catalogs = fixture_dbs("catalog");
    assert_eq!(
        catalogs.len(),
        11,
        "the corpus should hold 11 catalog snapshots"
    );
    for name in &catalogs {
        let (_dir, cat) = open(name);
        let project = cat
            .find_project("p")
            .unwrap_or_else(|e| panic!("{name}: {e}"))
            .unwrap_or_else(|| panic!("{name}: the harvest project is missing"));
        cat.read_view(project.id).unwrap_or_else(|e| panic!("{name}: {e}"));
        cat.read_disc_states(project.id).unwrap_or_else(|e| panic!("{name}: {e}"));
        cat.fingerprints(project.id).unwrap_or_else(|e| panic!("{name}: {e}"));
        cat.archive_pointers(project.id).unwrap_or_else(|e| panic!("{name}: {e}"));
    }
}

#[test]
fn the_initial_catalog_holds_a_project_and_no_discs() {
    let (_dir, cat) = open("catalog/01-after-init.sqlite");
    let project = cat.find_project("p").expect("query").expect("project exists");
    assert_eq!(project.default_disc_type.as_str(), "BD_R");
    assert_eq!(project.copies, 1);
    assert_eq!(project.margin_percent, 2.0);
    // A nine-digit fractional timestamp, written by the Scala build and read
    // back through `catalog::time` unchanged.
    assert_eq!(
        time::format(&project.created_at),
        "2026-09-05T20:01:04.016371777Z"
    );

    assert!(cat.read_view(project.id).expect("view").current.is_empty());
    assert!(cat.read_disc_states(project.id).expect("states").is_empty());
    assert!(cat
        .sessions_by_state(project.id, SessionState::Burned)
        .expect("sessions")
        .is_empty());
}

#[test]
fn the_catalog_after_the_first_burn_reports_one_full_disc() {
    let (_dir, cat) = open("catalog/03-after-burn1.sqlite");
    let project = cat.find_project("p").expect("query").expect("project exists");

    let view = cat.read_view(project.id).expect("view");
    assert_eq!(view.current.len(), 1);
    assert_eq!(view.current[&rel("a.bin")].content_hash, hash(A1));
    assert_eq!(view.current[&rel("a.bin")].size_bytes, 800);

    assert_eq!(
        cat.read_disc_states(project.id).expect("states"),
        vec![DiscState {
            ordinal: 1,
            status: DiscStatus::Finalized,
            usable_bytes: 1000,
            used_bytes: 800,
        }]
    );

    let burned = cat
        .sessions_by_state(project.id, SessionState::Burned)
        .expect("sessions");
    assert_eq!(burned.len(), 1);
    assert_eq!((burned[0].disc_ordinal, burned[0].ordinal), (1, 1));
    assert_eq!(
        burned[0].burned_at.map(|at| time::format(&at)).as_deref(),
        Some("2026-09-05T20:01:06.623959846Z")
    );

    let fingerprints = cat.fingerprints(project.id).expect("fingerprints");
    assert_eq!(fingerprints.len(), 1);
    assert_eq!(fingerprints[&rel("a.bin")].content_hash, hash(A1));
    assert_eq!(
        time::format(&fingerprints[&rel("a.bin")].mtime),
        "2026-09-05T20:01:03.260Z"
    );
}

#[test]
fn a_marked_dead_disc_reads_dead_without_its_row_having_changed() {
    let (_dir, cat) = open("catalog/08-after-mark-dead.sqlite");
    let project = cat.find_project("p").expect("query").expect("project exists");

    assert_eq!(
        cat.read_disc_states(project.id).expect("states"),
        vec![
            DiscState {
                ordinal: 1,
                status: DiscStatus::Finalized,
                usable_bytes: 1000,
                used_bytes: 800,
            },
            DiscState {
                ordinal: 2,
                status: DiscStatus::Open,
                usable_bytes: 1000,
                used_bytes: 800,
            },
            DiscState {
                ordinal: 3,
                status: DiscStatus::Dead,
                usable_bytes: 1000,
                used_bytes: 800,
            },
        ]
    );

    // The stored row still says OPEN — DEAD came out of `disc_event`.
    let stored: String = cat
        .raw_connection_for_tests()
        .query_row("select status from disc where ordinal = 3", [], |r| r.get(0))
        .expect("the disc row exists");
    assert_eq!(stored, "OPEN");

    // a.bin's current version is on the dead disc and b.bin is deleted, so
    // nothing is live: this is exactly the state a recovery burn starts from.
    assert!(cat.read_view(project.id).expect("view").current.is_empty());
}

#[test]
fn fingerprints_are_deliberately_not_filtered_by_dead_disc() {
    // Ported as-is with the rest of the deferred debt (spec §1): a dead disc
    // does not change the source file, so the scanner may still reuse its hash.
    // The asymmetry with `read_view` above is intended, and this pins it.
    let (_dir, cat) = open("catalog/08-after-mark-dead.sqlite");
    let project = cat.find_project("p").expect("query").expect("project exists");

    assert!(cat.read_view(project.id).expect("view").current.is_empty());

    let fingerprints = cat.fingerprints(project.id).expect("fingerprints");
    assert_eq!(fingerprints.len(), 1);
    assert_eq!(fingerprints[&rel("a.bin")].content_hash, hash(A2));
}

#[test]
fn file_history_reports_lost_per_physical_copy() {
    // The invariant from CLAUDE.md, in its natural example: after the recovery
    // burn, a@2 exists twice — the dead disc-3 copy reads LOST and the disc-4
    // copy reads CURRENT, though both carry the same content hash.
    let (_dir, cat) = open("catalog/11-after-finalize.sqlite");
    let project = cat.find_project("p").expect("query").expect("project exists");

    assert_eq!(
        cat.file_history(project.id, &rel("a.bin")).expect("history"),
        vec![
            FileHistoryEntry {
                content_hash: hash(A1),
                size_bytes: 800,
                disc_ordinal: 1,
                session_ordinal: 1,
                status: VersionStatus::Superseded,
            },
            FileHistoryEntry {
                content_hash: hash(A2),
                size_bytes: 800,
                disc_ordinal: 3,
                session_ordinal: 1,
                status: VersionStatus::Lost,
            },
            FileHistoryEntry {
                content_hash: hash(A2),
                size_bytes: 800,
                disc_ordinal: 4,
                session_ordinal: 1,
                status: VersionStatus::Current,
            },
        ]
    );

    assert_eq!(
        cat.file_history(project.id, &rel("b.bin")).expect("history"),
        vec![FileHistoryEntry {
            content_hash: hash(B1),
            size_bytes: 800,
            disc_ordinal: 2,
            session_ordinal: 1,
            status: VersionStatus::Deleted,
        }]
    );
}

#[test]
fn the_final_catalog_reports_its_pointers_and_per_disc_contents() {
    let (_dir, cat) = open("catalog/11-after-finalize.sqlite");
    let project = cat.find_project("p").expect("query").expect("project exists");

    assert_eq!(
        cat.archive_pointers(project.id).expect("pointers"),
        vec![
            ArchivePointer {
                rel_path: rel("a.bin"),
                content_hash: hash(A1),
                size_bytes: 800,
                disc_ordinal: 1,
                session_ordinal: 1,
                disc_path: "/data/a.bin".to_string(),
                kind: ManifestChangeKind::Added,
            },
            ArchivePointer {
                rel_path: rel("a.bin"),
                content_hash: hash(A2),
                size_bytes: 800,
                disc_ordinal: 3,
                session_ordinal: 1,
                disc_path: "/data/a.bin".to_string(),
                kind: ManifestChangeKind::Modified,
            },
            ArchivePointer {
                rel_path: rel("b.bin"),
                content_hash: hash(B1),
                size_bytes: 800,
                disc_ordinal: 2,
                session_ordinal: 1,
                disc_path: "/data/b.bin".to_string(),
                kind: ManifestChangeKind::Deleted,
            },
        ]
    );

    // Disc 4's last session is the `plan --finalize` seal, which placed no
    // files — so the per-session view is empty while the whole-disc view is not.
    assert!(cat
        .placements_for_disc(project.id, 4)
        .expect("placements")
        .is_empty());
    assert_eq!(
        cat.disc_recorded_contents(project.id, 4)
            .expect("contents")
            .visible,
        BTreeMap::from([(rel("a.bin"), hash(A2))])
    );

    let disc3 = cat.disc_recorded_contents(project.id, 3).expect("contents");
    assert_eq!(disc3.visible, BTreeMap::from([(rel("a.bin"), hash(A2))]));
    assert!(disc3.superseded.is_empty());

    let verified = cat
        .sessions_by_state(project.id, SessionState::Verified)
        .expect("sessions");
    assert_eq!(verified.len(), 1);
    assert_eq!((verified[0].disc_ordinal, verified[0].ordinal), (1, 1));
}
```

- [x] **Step 3: Run the tests**

```bash
cargo test -p diskrement-shell --test fixture_catalog
```

Expected: `test result: ok. 7 passed`.

A failure here is a compatibility failure, not a test bug: the expected values
were read out of the corpus with `sqlite3` while this plan was written. If one
disagrees, re-derive it from the fixture before changing anything.

- [x] **Step 4: Confirm the corpus is untouched, then commit**

```bash
git status --porcelain fixtures/
cargo test --workspace \
  && cargo clippy --workspace --all-targets -- -D warnings \
  && cargo fmt --check
git add crates/diskrement-shell/tests
git commit -m "test(catalog): read Scala-written fixture catalogs through every query"
```

Expected: no output from `git status`, everything green.

---

## Task 13: Replaying a burn against the fixture

Reading the corpus proves the queries. This proves the *writer*: replay the
scenario's first burn cycle through `apply_update` and diff the resulting
database, row by row, against the catalog the Scala build produced at the same
point.

The inputs are read out of the fixture rather than hard-coded, so the test
compares implementations rather than comparing the plan's typing to itself.
What it actually pins is everything a writer can get wrong: insert order and
therefore rowids, timestamp rendering in three different columns, enum
spellings, `first_seen_at` being the session's burn time, and the choice to
record sessions as BURNED.

**Files:**
- Modify: `crates/diskrement-shell/tests/support/mod.rs`
- Create: `crates/diskrement-shell/tests/fixture_replay.rs`

- [x] **Step 1: Add the logical dump helper**

Append to `crates/diskrement-shell/tests/support/mod.rs`:

```rust
/// The catalog tables, in the order a dump reports them.
pub const CATALOG_TABLES: &[&str] = &[
    "schema_version",
    "project",
    "disc",
    "session",
    "file_version",
    "placement",
    "change_record",
    "disc_event",
    "lost_version",
];

fn render(value: rusqlite::types::ValueRef<'_>) -> String {
    use rusqlite::types::ValueRef;
    match value {
        ValueRef::Null => "NULL".to_string(),
        ValueRef::Integer(i) => i.to_string(),
        // `{:?}` so 2.0 stays "2.0" rather than collapsing to "2".
        ValueRef::Real(f) => format!("{f:?}"),
        ValueRef::Text(t) => format!("{:?}", String::from_utf8_lossy(t)),
        ValueRef::Blob(b) => format!("<{} bytes>", b.len()),
    }
}

/// Every row of every catalog table as sorted `table: col=value …` lines.
///
/// Logical, never byte-wise: SQLite file layout is not stable across library
/// builds, and xerial's SQLite is not `libsqlite3-sys`'s (spec §4).
pub fn dump_catalog(conn: &rusqlite::Connection) -> Vec<String> {
    let mut out = Vec::new();
    for table in CATALOG_TABLES {
        let mut stmt = conn
            .prepare(&format!("select * from {table}"))
            .unwrap_or_else(|e| panic!("preparing dump of {table}: {e}"));
        let columns: Vec<String> = stmt.column_names().iter().map(|c| c.to_string()).collect();
        let mut rows = stmt.query([]).expect("dump query runs");
        let mut lines = Vec::new();
        while let Some(row) = rows.next().expect("readable row") {
            let cells: Vec<String> = columns
                .iter()
                .enumerate()
                .map(|(i, name)| {
                    format!("{name}={}", render(row.get_ref(i).expect("column exists")))
                })
                .collect();
            lines.push(format!("{table}: {}", cells.join(" ")));
        }
        lines.sort();
        out.extend(lines);
    }
    out
}

/// Compare two dumps, reporting the first difference rather than two walls of
/// text.
pub fn assert_dumps_match(expected: &[String], actual: &[String]) {
    for (i, (want, got)) in expected.iter().zip(actual.iter()).enumerate() {
        assert_eq!(want, got, "dumps diverge at line {i}");
    }
    assert_eq!(
        expected.len(),
        actual.len(),
        "row counts differ:\nexpected {expected:#?}\nactual {actual:#?}"
    );
    assert!(!expected.is_empty(), "the dump was empty");
}
```

- [x] **Step 2: Write the replay test**

`crates/diskrement-shell/tests/fixture_replay.rs`:

```rust
//! Writes the scenario's first burn cycle with the Rust store and diffs the
//! result against the catalog the Scala build wrote at the same point.
//!
//! This is the one cross-implementation check on the *write* path available
//! before P5 produces catalogs through the real pipeline, and it covers the
//! failure the spec calls highest-consequence (§11): a Rust binary writing a
//! catalog the Scala version would not recognise.

mod support;

use std::str::FromStr;

use diskrement_core::{ContentHash, DiscStatus, DiscType, RelPath};
use diskrement_shell::catalog::model::{
    CatalogUpdate, DiscWrite, FileWrite, ProjectInit, SessionWrite,
};
use diskrement_shell::catalog::{time, Catalog};
use support::{assert_dumps_match, dump_catalog, open_fixture};

#[test]
fn replaying_the_first_burn_reproduces_the_scala_catalog_row_for_row() {
    let fixture = open_fixture("catalog/03-after-burn1.sqlite");

    // Every input is read out of the fixture, so this compares the two writers
    // rather than comparing the fixture to values typed into this file.
    let (name, source_root, work_dir, disc_type, margin_percent, copies, created_at): (
        String,
        String,
        String,
        String,
        f64,
        i64,
        String,
    ) = fixture
        .query_row(
            "select name, source_root, work_dir, default_disc_type,
                    margin_percent, copies, created_at
             from project where id = 1",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .expect("the fixture has a project row");

    let (disc_ordinal, disc_type_row, usable_bytes, status, label): (
        i64,
        String,
        i64,
        String,
        String,
    ) = fixture
        .query_row(
            "select ordinal, disc_type, usable_bytes, status, label from disc where id = 1",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .expect("the fixture has a disc row");

    let (session_ordinal, burned_at, manifest_hash): (i64, String, String) = fixture
        .query_row(
            "select ordinal, burned_at, manifest_hash from session where id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("the fixture has a session row");

    let (rel_path, content_hash, size_bytes, mtime): (String, String, i64, String) = fixture
        .query_row(
            "select rel_path, content_hash, size_bytes, mtime from file_version where id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("the fixture has a file version");

    let (disc_path, copy_index): (String, i64) = fixture
        .query_row(
            "select disc_path, copy_index from placement where id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("the fixture has a placement");

    let dir = tempfile::TempDir::new().expect("a writable temp directory");
    let mut catalog = Catalog::open(&dir.path().join("catalog.sqlite")).expect("catalog opens");

    let project = catalog
        .create_project(&ProjectInit {
            name,
            source_root,
            work_dir,
            default_disc_type: DiscType::from_str(&disc_type).expect("known disc type"),
            margin_percent,
            copies: copies as u32,
            created_at: time::parse(&created_at).expect("the fixture timestamp parses"),
        })
        .expect("project is created");

    catalog
        .apply_update(&CatalogUpdate {
            project_id: project,
            finalize_disc_ordinals: Vec::new(),
            discs: vec![DiscWrite {
                ordinal: disc_ordinal as u32,
                is_new: true,
                disc_type: DiscType::from_str(&disc_type_row).expect("known disc type"),
                usable_bytes: usable_bytes as u64,
                status: DiscStatus::from_str(&status).expect("known status"),
                label,
                session: SessionWrite {
                    ordinal: session_ordinal as u32,
                    burned_at: time::parse(&burned_at).expect("the fixture timestamp parses"),
                    manifest_hash,
                    added: vec![FileWrite {
                        rel_path: RelPath(rel_path),
                        content_hash: ContentHash(content_hash),
                        size_bytes: size_bytes as u64,
                        mtime: time::parse(&mtime).expect("the fixture timestamp parses"),
                        disc_path,
                        copy_index: copy_index as u32,
                    }],
                    modified: Vec::new(),
                    deleted: Vec::new(),
                },
            }],
        })
        .expect("the burn is recorded");

    assert_dumps_match(
        &dump_catalog(&fixture),
        &dump_catalog(catalog.raw_connection_for_tests()),
    );
}
```

- [x] **Step 3: Run the test**

```bash
cargo test -p diskrement-shell --test fixture_replay
```

Expected: `test result: ok. 1 passed`.

Failure modes worth recognising:
- a `first_seen_at` mismatch means `write_file` is not passing the session's
  `burned_at` through;
- a `state` mismatch means a session was written as something other than
  `'BURNED'`;
- an `id` mismatch means the insert order changed — file versions are written
  added-then-modified-then-tombstoned, in list order;
- a `created_at` or `mtime` mismatch is a `catalog::time` bug and should have
  been caught by `tests/timestamps.rs` first.

- [x] **Step 4: Commit**

```bash
git status --porcelain fixtures/
cargo test --workspace \
  && cargo clippy --workspace --all-targets -- -D warnings \
  && cargo fmt --check
git add crates/diskrement-shell/tests
git commit -m "test(catalog): replay the first burn and diff against the Scala catalog"
```

---

## Task 14: Document the crate and close the phase

**Files:**
- Create: `crates/diskrement-shell/README.md`
- Modify: `CLAUDE.md`
- Modify: `docs/superpowers/plans/2026-09-06-rust-rewrite-p2-catalog.md`

- [x] **Step 1: Write the crate README**

`crates/diskrement-shell/README.md`:

```markdown
# diskrement-shell

Every effect diskrement performs: the SQLite catalog, file scanning and
hashing, CoW snapshots and staging, and the burners. The functional core
(`diskrement-core`) knows nothing about any of it.

## The catalog (phase P2)

One `rusqlite::Connection` per catalog file, no pool: diskrement opens the
catalog, does its work, and exits. Readers take `&self`; writers take
`&mut self` and go through `Catalog::tx`, whose transaction guard rolls back on
drop unless committed.

Three things here are frozen by the data-compatibility contract
(`docs/superpowers/specs/2026-09-05-rust-rewrite-design.md`, §3.1) and must not
be "improved":

- **The schema**, in `catalog/schema.rs`, transcribed statement for statement
  including both `check` constraints. `disc.status` accepts only `OPEN` and
  `FINALIZED`; `DEAD` is derived from `disc_event` at read time and never
  stored.
- **The timestamp format**, in `catalog/time.rs`: `java.time.Instant.toString()`
  shape, with 0, 3, 6, or 9 fractional digits.
- **The enum spellings** — `BD_R`, `ADDED`, `FINALIZED`, `MARKED_DEAD` and the
  rest — which live as `as_str`/`FromStr` pairs rather than serde renames.

## Gotchas

- **Open fixture databases with `?immutable=1`.** A plain read-only open of a
  WAL database creates `-wal`/`-shm` files beside it, which would dirty
  `fixtures/` and invalidate `SHA256SUMS`. `tests/support/mod.rs` does this
  correctly; copy that helper rather than reinventing it.
- **Test against a real file, not `:memory:`.** An in-memory database reports
  `journal_mode=memory` no matter what pragma you set, so a WAL assertion
  against one is vacuous. `catalog::testutil::temp_catalog` gives every test a
  real file in its own `TempDir`.
- **`VACUUM INTO` refuses an existing destination**, and must run outside a
  transaction. Both are relied on by the burn flow.

## Tests

Unit tests live beside their code. The four files under `tests/` are the
cross-implementation compatibility suite and consume the P0 fixture corpus:
`timestamps.rs` (the wire format), `fixture_schema.rs` (the DDL),
`fixture_catalog.rs` (reads), `fixture_replay.rs` (writes).
```

- [x] **Step 2: Record the Rust-era gotchas in `CLAUDE.md`**

The spec (§7) says Rust-era gotchas are recorded in `CLAUDE.md` as they are
discovered; the file is not rewritten for the Rust build until P9, so add a
section rather than editing the Scala ones. Insert immediately before the
`## Repo etiquette` heading:

```markdown
## Rust rewrite gotchas (P2+)

These apply to the `crates/` tree only; the Scala gotchas above still govern
`core/`, `shell/` and `cli/` until P9 deletes them.

- **Fixture databases must be opened `?immutable=1`** (with
  `OpenFlags::SQLITE_OPEN_URI`). A plain read-only open of a WAL database
  creates `-wal`/`-shm` files beside it, dirtying `fixtures/`.
- **SQLite pragmas need a real file.** An in-memory database always reports
  `journal_mode=memory`, so a WAL assertion against one passes vacuously.
- **Never open a fixture catalog with `Catalog::open`** — it runs the migration
  runner and writes. Copy the file to a `TempDir` first.
```

- [x] **Step 3: Run the full verification**

```bash
cargo test --workspace \
  && cargo clippy --workspace --all-targets -- -D warnings \
  && cargo fmt --check
```

Expected, with no failures and no warnings:

| binary | tests |
|---|---|
| `diskrement-core` lib | 58 |
| `diskrement-shell` lib | 43 |
| `diskrement-shell` `tests/fixture_catalog.rs` | 7 |
| `diskrement-shell` `tests/fixture_replay.rs` | 1 |
| `diskrement-shell` `tests/fixture_schema.rs` | 1 |
| `diskrement-shell` `tests/timestamps.rs` | 2 |
| `diskrement-cli` `tests/binary.rs` | 1 |

- [x] **Step 4: Confirm the Scala build is untouched**

The Scala tree must keep working until P9 — it is how fixtures are re-harvested
and how behavior the tests do not pin stays readable.

```bash
git status --porcelain core/ shell/ cli/ build.sbt project/ fixtures/
```

Expected: no output.

- [x] **Step 5: Check off this plan and commit**

Mark every checkbox in this document, append any execution deviations to the
section at the end, and commit:

```bash
git add crates/diskrement-shell/README.md CLAUDE.md docs/superpowers/plans
git commit -m "docs(catalog): document diskrement-shell's catalog and record P2 gotchas"
```

- [x] **Step 6: Merge the phase branch**

Per `CLAUDE.md`, phase work merges back into the rewrite's integration branch
with an explicit merge commit, and the phase branch is then deleted.

```bash
git switch feat/rust-rewrite
git merge --no-ff feat/rewrite-p2 -m "Merge feat/rewrite-p2: the SQLite catalog, proven against the fixture corpus"
git branch -d feat/rewrite-p2
git push origin feat/rust-rewrite
git push origin --delete feat/rewrite-p2
```

---

## Phase P2 acceptance

- [x] The schema, both migrations, and every `check` constraint are transcribed
      verbatim, and a freshly migrated catalog's DDL matches a Scala-written
      one (`tests/fixture_schema.rs`).
- [x] `catalog::time` round-trips every timestamp in the fixture corpus
      byte-for-byte, covering 3- and 9-digit fractions and the whole-second case
      (`tests/timestamps.rs`).
- [x] All fifteen store operations are ported, with every assertion from the
      sixteen Scala catalog specs represented.
- [x] `AppendOnlySpec` is ported as a proptest: across a random sequence of
      scans, `read_view` equals the latest scan and `file_version` never
      shrinks (Task 11).
- [x] Marking a disc dead appends a `disc_event` and leaves the `disc` row
      unchanged, asserted on the row itself and not only on the derived value.
- [x] `LOST` reads per physical copy: fixture 11 shows a@2 as `Lost` on disc 3
      and `Current` on disc 4 with the same content hash.
- [x] Replaying burn cycle 1 through `apply_update` reproduces the Scala
      catalog row for row (`tests/fixture_replay.rs`).
- [x] `cargo test --workspace`, `cargo clippy --workspace --all-targets -D warnings`
      and `cargo fmt --check` are all clean, at the test counts in Task 14.
- [x] `fixtures/`, the Scala tree, and the sbt build are byte-identical to their
      state at the start of the phase.

**Next:** phase P3 (fs: scanner, hasher, fingerprint cache, snapshot, and the
first runnable binary — `init`, `scan`, `status`).

---

## P2 execution deviations (appended during execution)

*(Nothing yet. Record here what the implementation had to do differently and
why, rather than leaving it in commit messages.)*

### Task 2

- `#[expect(dead_code, reason = "first called by store.rs in task 3")]` was
  added to `Catalog::tx`, `CatalogError::from_sqlite`, and
  `is_unique_violation`. The brief's verbatim code defines them before their
  first caller exists, and `.cargo/config.toml` makes warnings failures for
  the whole workspace, so the brief as written could not compile. `expect`
  (not `allow`) so each marker warns once task 3 consumes its item.
- `tests/fixture_schema.rs` calls
  `schema_of(ours.raw_connection_for_tests())` without the brief's extra `&`:
  the method already returns `&Connection`, and `&&Connection` trips clippy's
  `needless_borrow` under `-D warnings`.
- Final `catalog/mod.rs` keeps the Step 1 `mod tests` appended below the Step 5
  replacement content: Step 5's block alone would leave the four new tests out
  and Step 7 expects `8 passed`. The "replace wholesale" instruction was read
  as replacing the task-1 skeleton, not the tests just written.

### Task 11

- The brief's verbatim `proptest!` block did not compile as pasted:
  `ContentHash`, `ProjectId` and `FileWrite` were not yet in scope inside
  `store.rs`'s child `mod tests` (only imported at the outer module level,
  which does not propagate into a nested module). Added
  `use crate::catalog::model::{FileWrite, ProjectId};` and extended the
  `diskrement_core` import with `ContentHash`. No logic changed; a spec
  reviewer independently confirmed the brief's snippet, taken literally, could
  not have compiled without this.

### Task 13

- The implementer subagent completed Step 1 (the `dump_catalog`/
  `assert_dumps_match` helpers in `tests/support/mod.rs`) verbatim, then hit
  an API rate limit and terminated before writing `tests/fixture_replay.rs`
  or committing. The controlling session finished Step 2 directly rather than
  redispatching a fresh subagent, wrote `fixture_replay.rs` byte-for-byte per
  the plan, and ran the full test/lint/format gate before committing. Both
  the spec-compliance and code-quality reviews for this task were run with
  extra scrutiny given the mixed provenance; both confirmed the committed
  code is byte-for-byte identical to the plan's prescribed text with zero
  deviation.
