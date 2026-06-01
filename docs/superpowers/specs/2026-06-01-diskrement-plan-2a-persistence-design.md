# diskrement — Plan 2a Design: Persistence Layer (SQLite CatalogStore)

**Date:** 2026-06-01
**Status:** Approved for planning
**Parent spec:** `docs/superpowers/specs/2026-05-31-diskrement-core-engine-design.md`
**Predecessor:** Plan 1 (functional core) — complete on `feat/core-functional`.

---

## 1. Scope

This is the first of the imperative-shell plans. It delivers **only the
persistence layer**: a new `shell` sbt module, the port abstraction it
introduces, and the SQLite-backed `CatalogStore` (schema, migrations, the
`CatalogView`/`DiscState` read models, and a transactional `CatalogUpdate`
apply).

**In scope (this plan, "Plan 2a"):**
- New `shell` sbt module depending on `core` + ZIO + zio-jdbc + sqlite-jdbc.
- The `CatalogStore` port (ZIO service trait) and its SQLite live implementation.
- Append-only schema for the six entities of parent spec §5.1 + `schema_version`.
- Hand-rolled forward-migration runner.
- Reading the core's `CatalogView` (for `Diff`) and `List[DiscState]` (for
  `Planner`) out of SQL.
- Applying a `CatalogUpdate` in a single transaction (crash-safety, §9.4).
- ZIO Test integration suite against a real temp-file SQLite database.

**Out of scope (later plans):**
- **Plan 2b** — `FileSystem` walk, `FileScanner` (fast-path hashing + `--verify`),
  `SourceSnapshot` (reflink-cp + CoW hard-fail), `SnapshotWriter` (`/.diskrement`
  layout + manifest JSON + embedded catalog snapshot), `SimulatedBurner`.
- **Plan 3** — CLI commands, the staged scan/plan/burn pipeline, burn
  orchestration (which assembles a `CatalogUpdate` from a `BurnPlan` + manifests),
  orphaned-`PLANNED`-session handling, config/XDG + systemd path resolution, and
  the scripted end-to-end integration test.

---

## 2. Decisions (settled in brainstorming)

| Decision | Choice | Rationale |
|---|---|---|
| Shell programming style | **ZIO** (`ZIO[R, E, A]`, `ZLayer`, `Scope`, ZIO Test) | Typed errors, resource lifecycles, batteries-included test kit. |
| SQLite access | **zio-jdbc + hand-written SQL** | ZIO-native `transaction` combinator maps onto the single-transaction crash-safety requirement; full SQL control for the append-only model and current-view query. |
| Migrations | **Hand-rolled forward runner** | ~30 lines, no dependency, satisfies the spec's forward-migration requirement; cycle 1 ships v1. |
| CoW integration testing | (Plan 2b) best-effort + skip | Not exercised in 2a; recorded here for continuity. |
| Plan scope | **Split**: persistence (2a) before fs/burn (2b) | Two Plan-1-sized, independently reviewable plans; riskiest piece (catalog) stabilizes first. |

---

## 3. Module structure & dependency direction

A new sbt module **`shell`** joins `core`:

```
core   (pure, no ZIO)              — unchanged from Plan 1
shell  (ZIO + zio-jdbc + sqlite)   — NEW
  diskrement.shell.ports     CatalogStore (this plan).
                             Other port traits (FileSystem, SourceSnapshot,
                             Hasher, Burner, SnapshotWriter) are introduced
                             in Plan 2b, not stubbed here.
  diskrement.shell.catalog   SQLite live impl: connection/pool layer,
                             migration runner, query + update SQL.
  diskrement.shell.model     persistence types: Project, ProjectInit,
                             ProjectId, Session, SessionState (new); CatalogUpdate
                             and its sub-records; CatalogError. Reuses core's
                             DiscStatus (OPEN|FINALIZED), DiscType, CatalogView,
                             CatalogEntry, RelPath, ContentHash, DiscState.
```

**The functional core does not depend on the ports.** `Diff`, `Planner`, and
`ManifestBuilder` take pure data in and return pure data out; they never call a
port. The `shell` orchestrates — uses ports to gather facts, hands pure data to
the core, then uses ports to execute decisions. Therefore ports live in `shell`,
and the dependency arrow points one way: **`shell → core`**. `core` gains no new
dependencies and its build stays as-is.

Ports are **ZIO service traits** (methods return `ZIO[…]`) with a live `ZLayer`.
ZIO's built-in `Clock` is used for timestamps; no custom clock port.

**`build.sbt`** adds the `shell` module:

```scala
lazy val shell = (project in file("shell"))
  .dependsOn(core)
  .settings(
    name := "diskrement-shell",
    libraryDependencies ++= Seq(
      "dev.zio"     %% "zio"        % zioVersion,
      "dev.zio"     %% "zio-jdbc"   % zioJdbcVersion,
      "org.xerial"  %  "sqlite-jdbc"% sqliteVersion,
      "dev.zio"     %% "zio-test"     % zioVersion % Test,
      "dev.zio"     %% "zio-test-sbt" % zioVersion % Test
    ),
    testFrameworks += new TestFramework("zio.test.sbt.ZTestFramework")
  )
```

The root project aggregates `core` and `shell`. (Exact dependency versions are
pinned during planning against current releases.)

---

## 4. Schema & append-only model

SQLite, WAL mode, one database per project. The parent spec's blanket
"no row is ever UPDATEd or DELETEd" is refined here into two table classes,
because lifecycle/config fields (§5.1 `Disc.status`, §9.4 `Session.state`,
`Project.defaultDiscType`) genuinely transition:

### 4.1 Immutable fact tables (never UPDATE/DELETE — the "never forgets" guarantee)

- **`file_version`** — `id` PK, `project_id` FK, `rel_path`, `content_hash`,
  `size_bytes`, `mtime`, `first_seen_at`. An immutable fact; a new version is a
  new row.
- **`placement`** — `id` PK, `file_version_id` FK, `session_id` FK, `disc_path`,
  `copy_index`. Where a version physically landed.
- **`change_record`** — `id` PK, `session_id` FK, `file_version_id` FK **NOT
  NULL**, `kind` ∈ `ADDED | MODIFIED | DELETED`. A `DELETED` row points at the
  version it tombstones (no *new* `file_version` is inserted for a delete), which
  lets every change record carry a `rel_path` via join and links the tombstone to
  what it removed.

### 4.2 Controlled-mutable rows (a small, fixed set of monotonic fields)

- **`project`** — `id` PK, `name` UNIQUE, `source_root`, `work_dir`,
  `default_disc_type` (mutable config, overwritten in place), `margin_percent`,
  `copies` (default 1), `created_at`.
- **`disc`** — `id` PK, `project_id` FK, `ordinal`, `disc_type`, `usable_bytes`,
  `status` ∈ `OPEN | FINALIZED` (one-way OPEN→FINALIZED), `label`.
  `UNIQUE(project_id, ordinal)`.
- **`session`** — `id` PK, `disc_id` FK, `ordinal`, `state` ∈
  `PLANNED | BURNED | VERIFIED` (one-way), `burned_at` (nullable),
  `manifest_hash` (nullable). `UNIQUE(disc_id, ordinal)`.

### 4.3 Bookkeeping

- **`schema_version`** — single-row `version INTEGER`.

### 4.4 Pragmas & indexes

- `journal_mode=WAL`, `foreign_keys=ON`, `synchronous=FULL` (archival durability
  over throughput; the tool runs occasionally, not on a hot path).
- Index on `file_version(project_id, rel_path)` for the current-view query;
  index on `placement(session_id)` and `change_record(file_version_id)`.

The purist alternative — deriving `disc.status`/`session.state` from event rows
rather than storing them — was considered and rejected: it complicates
finalization and orphaned-session queries without a real durability benefit,
since the immutable fact tables already carry the full, never-rewritten history.

---

## 5. Migrations

A hand-rolled forward runner:

```scala
val migrations: List[(Int, Seq[String])] = List(
  1 -> Seq(createProject, createDisc, createSession,
           createFileVersion, createPlacement, createChangeRecord,
           createIndexes)
)
```

On open: read `schema_version` (treat a missing table as version 0); for each
`(version, statements)` with `version > current`, run its statements and bump
`schema_version` **inside one transaction per version**. Running against an
already-current database applies nothing (idempotent). Cycle 1 ships version 1.
The `Hasher` port (Plan 2b) means a future alternate hash algorithm is additive
columns defaulting to SHA-256 — a forward migration, no rewrite.

---

## 6. The `CatalogStore` port

```scala
trait CatalogStore:
  def createProject(p: ProjectInit):       IO[CatalogError, ProjectId]
  def findProject(name: String):           IO[CatalogError, Option[Project]]
  def readView(project: ProjectId):        IO[CatalogError, CatalogView]
  def readDiscStates(project: ProjectId):  IO[CatalogError, List[DiscState]]
  def sessionsByState(project: ProjectId,
                      state: SessionState): IO[CatalogError, List[Session]]
  def applyUpdate(update: CatalogUpdate):  IO[CatalogError, Unit]
```

`readView` and `readDiscStates` return the **core's existing types**
(`CatalogView`, `DiscState`); all SQL→core mapping lives in the SQLite impl, so
the core stays oblivious to persistence.

### 6.1 Current-view query

"Current version = latest non-tombstoned version per `rel_path`." Because every
`change_record` references a `file_version` (including DELETEs, §4.1), the view
is "the latest `change_record` per `rel_path`, kept only when its kind ≠ DELETED":

```sql
SELECT fv.rel_path, fv.content_hash, fv.size_bytes
FROM (
  SELECT cr.id, cr.kind, cr.file_version_id,
         ROW_NUMBER() OVER (PARTITION BY fv.rel_path ORDER BY cr.id DESC) AS rn
  FROM change_record cr
  JOIN file_version fv ON fv.id = cr.file_version_id
  WHERE fv.project_id = ?
) l
JOIN file_version fv ON fv.id = l.file_version_id
WHERE l.rn = 1 AND l.kind <> 'DELETED';
```

`change_record.id` is monotonic and reflects burn order (updates apply per
session in ascending order), so ordering by it yields the true latest event.
SQLite window functions require 3.25+, satisfied by the bundled `sqlite-jdbc`.

### 6.2 Disc states

`readDiscStates` maps each `disc` row to `DiscState(ordinal, status,
usableBytes, usedBytes)`, where `usedBytes` is derived
(`SUM(file_version.size_bytes)` over the disc's placements) — never a stored
counter. The result feeds `Planner.plan` directly.

### 6.3 `CatalogUpdate` (write model)

A structured, pure description of one burn run, assembled by the burn
orchestration (Plan 2b/3) and applied here:

- The pre-existing OPEN disc ordinal(s) to flip to FINALIZED (if spillover
  finalized them).
- For each disc touched/created this run: its identity (ordinal, `isNew`,
  `discType`, `usableBytes`, resulting `status`, `label`) and its one new
  `session` (`state = BURNED`, `burned_at`, `manifest_hash`).
- For each session: the new `file_version`s (added + modified), their
  `placement`s (`disc_path`, `copy_index`), and the `change_record`s
  (ADDED/MODIFIED → the new version; DELETED → the prior version being
  tombstoned).

`applyUpdate` writes the entire structure in **one zio-jdbc `transaction`**:
all-or-nothing (§9.4). Fact-table writes are insert-only; status transitions are
validated monotonic (OPEN→FINALIZED only) and reject illegal transitions with
`IllegalStatusTransition`.

### 6.4 Crash-safety note (boundary with Plan 2b/3)

`applyUpdate` is the **atomic post-success commit**: the catalog is updated only
after a burn reports success, in a single transaction, so a mid-burn crash leaves
the last known-good state. §9.4's "orphaned `PLANNED` session detected on next
run" depends on the burn-orchestration flow (whether a session row is written
before the burn) and is a Plan 2b/3 concern. `sessionsByState` is provided now so
that detection can be built later with no schema change.

---

## 7. Error model

`CatalogStore` returns `IO[CatalogError, A]`. `CatalogError` is a small sealed
ADT of *expected* domain failures:

- `DuplicateProject(name)` — `createProject` on an existing name.
- `ProjectNotFound(...)`.
- `IllegalStatusTransition(...)` — e.g. FINALIZED→OPEN.
- `MigrationFailed(cause)`.
- `ConstraintViolation(cause)` — a rejected insert (FK/unique/check).

Genuinely unexpected JDBC faults surface as ZIO **defects** (die), not typed
errors, so callers handle only what is meaningful.

---

## 8. Testing strategy

ZIO Test, against a **real temp-file SQLite database** (WAL needs a file). A
`ZLayer` provisions a fresh database per test and deletes it via `Scope`.

- **Migration runner:** fresh DB → `schema_version` = 1, all six tables present,
  `journal_mode=WAL` and `foreign_keys=ON` verified; a second run applies nothing.
- **Round-trips:** `createProject`/`findProject` (incl. `DuplicateProject`);
  after `applyUpdate`, `readView` shows added files; a MODIFIED supersedes the old
  hash; a DELETED drops the path from the view while its history rows remain;
  `readDiscStates` reports correct `usedBytes` and OPEN/FINALIZED status.
- **Transactional atomicity:** an `applyUpdate` that violates a constraint partway
  leaves the database unchanged and surfaces `ConstraintViolation` — proving the
  single-transaction guarantee.
- **Append-only property:** generate a sequence of change-sets, apply them in
  order, and assert (a) `readView` always equals a pure reference view (cumulative
  adds/modifies minus deletes) and (b) the `file_version` row count only ever
  grows. This realizes parent spec §10.1's "catalog append-only / current view
  never returns superseded or tombstoned versions."

---

## 9. Self-review checklist (for the plan author)

- **Parent-spec coverage:** §5.1 entities → §4; §5.2 derived views → §6.1/§6.2;
  §9.4 single-transaction crash-safety → §6.3/§6.4; §10.1 catalog append-only
  property → §8.
- **Correctly deferred:** scanner/snapshot/burner/snapshot-writer (2b); CLI,
  burn orchestration that builds `CatalogUpdate`, orphan-session handling,
  config/XDG paths, full-loop test (3).
- **Type consistency:** `readView`/`readDiscStates` return the unchanged core
  types `CatalogView` and `DiscState`; `CatalogUpdate`/`CatalogError`/`Project`
  are new `shell` types defined once.
- **No placeholders:** dependency *versions* are intentionally pinned at planning
  time against current releases; every other element is concrete.
