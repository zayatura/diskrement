# diskrement — Core Engine Design (Cycle 1)

**Date:** 2026-05-31
**Status:** Approved for planning
**Scope:** Cycle 1 of a long-term project. This document specifies the **core
engine** only: the pure business logic plus the persistence and CLI shell needed
to exercise it end to end. Real disc burning is **simulated** in this cycle; the
web interface and real burner integration are explicitly deferred to later cycles.

---

## 1. Overview

`diskrement` (disk + incremental) creates incremental backups of a directory tree
onto optical discs (primarily Blu-ray; DVDs supported). It tracks what has already
been backed up, detects added/modified/deleted files on each run, and plans which
files to write to which discs — topping off the current open (multisession) disc
first, then spilling onto new discs.

Optical media is **write-once**. This single physical fact drives the entire
design: nothing already burned can be modified or deleted. The system is therefore
modeled as an **append-only log** with a separate logical catalog that always
knows the current state.

### 1.1 Goals (cycle 1)

- A correct, exhaustively tested **functional core**: scan diffing + disc planning.
- A **catalog that never forgets**: full version history of every file, retained
  forever, enabling future point-in-time restore.
- Consistency under concurrent modification (the **TOCTOU** problem) via a
  copy-on-write **source snapshot** taken once per run.
- A **CLI** that drives the whole workflow against real directories with a
  **simulated burner**, making the system manually testable today.
- Self-describing discs: each disc carries a manifest and a full catalog snapshot,
  so a single disc can be understood and the catalog rebuilt without the host DB.

### 1.2 Non-goals (deferred to later cycles)

- Real disc burning (driving `xorriso`/`growisfs`/etc.). Cycle 1 simulates it.
- Web interface.
- Restoring arbitrary historical versions (the data model supports it; the feature
  is not built yet).
- Reclaiming space from superseded/deleted bytes (impossible on write-once media
  by design — see §3).
- Real same-disc extent reuse for the on-disc `/.archive` view (cycle 1 uses text
  pointers only — see §6.3).
- Splitting a single file across discs (oversized files are reported as errors).
- Enforcing redundant copies in the planner (the schema models copies; the planner
  targets a single copy this cycle).

---

## 2. Tech stack & style

- **Language:** Scala 3.
- **Style:** Functional core, imperative shell (hexagonal / ports & adapters). The
  core is pure (no I/O, deterministic); only the shell touches effects. The core
  does **not** wrap its logic in an effect type — purity is achieved by taking data
  in and returning data out.
- **Testing:** ScalaCheck for property tests over the core; example-based unit
  tests for edge cases; integration tests for the shell adapters; a documented
  manual test checklist for hardware-adjacent and filesystem behaviors.
- **Persistence:** SQLite (one database per project).
- **Distribution context:** runs either as a normal user (XDG directories) or as a
  dedicated systemd service (`StateDirectory`, `ConfigurationDirectory`).

Rationale: Scala is the author's strongest language and an excellent fit for
pure-FP business logic with property-based testing. The JVM is acceptable because
burning shells out to external tools. Alternatives considered and rejected for the
core: active-record/service-layer style (can't property-test the planner without a
DB), and an effect-system-heavy core (wrapping pure decisions in `IO`/`ZIO` adds
nothing and complicates property tests). The shell may use an effect type freely.

---

## 3. Core mental model

- The **catalog (SQLite) is the logical source of truth.** It knows the *current*
  version of every file and which disc + session physically holds it.
- The **discs are an immutable append-log.** A modified file appends a *new*
  version (old bytes remain, marked superseded). A deleted file is recorded as a
  logical tombstone in the catalog; the bytes remain on disc as "orphans."
- A file version can only be hidden/replaced **within the same disc's** multisession
  filesystem — never across discs.
- **Consequence (accepted):** deleted and modified files leave dead bytes on old
  discs forever. This is inherent to write-once archival media; the alternative
  (rewriting discs to reclaim space) defeats durability and is explicitly rejected.

---

## 4. Architecture

Three layers, dependencies pointing inward:

### 4.1 Functional core (pure — unit + property tested)

- **Domain model:** `Project`, `Disc`, `Session`, `FileVersion`, `Placement`,
  `ChangeRecord` (see §5).
- **`Diff`** — `(CatalogView, List[ScannedFile]) => ChangeSet` (see §7).
- **`Planner`** — `(ChangeSet, List[DiscState], Capacity) => BurnPlan` (see §8).
- **Manifest builder** — produces the per-session manifest data structure.

The core performs zero I/O and is fully deterministic: identical inputs always
produce identical outputs.

### 4.2 Ports (pure interfaces the core/shell depend on)

- `Clock` — current time.
- `Hasher` — content hashing (SHA-256, see §5.2).
- `FileSystem` — read-only directory walk + stat + stream bytes.
- `CatalogStore` — read a `CatalogView`, apply a `CatalogUpdate` transactionally.
- `SourceSnapshot` — freeze the source tree (see §6).
- `Burner` — write a planned disc; `SimulatedBurner` in cycle 1 (see §9.2).

Real implementations live in the shell; in-memory fakes back the core tests.

### 4.3 Imperative shell (effects — integration tested)

- **CLI** (see §9).
- **FileScanner** — walks the snapshot, applies fast-path hashing (see §7.1).
- **CatalogRepo** — SQLite adapter implementing `CatalogStore`.
- **SnapshotWriter** — writes the on-disc `/.diskrement` metadata (see §6.3).
- **SimulatedBurner** — stages disc contents to a directory tree.

**Data flow:** the shell gathers facts (snapshot → scan → `CatalogView`), the core
decides (`Diff` → `Planner` → manifest), the shell executes (simulated burn → write
snapshot → persist catalog update transactionally).

---

## 5. Domain model / database schema

Append-only. **No row is ever UPDATEd or DELETEd** in these tables; "current state"
is always derived by query, never stored as a mutable field. SQLite in WAL mode;
schema versioned via a `schema_version` table with forward migrations.

### 5.1 Entities

**Project**
- `id` (pk), `name`, `sourceRoot` (path), `workDir` (path),
- `defaultDiscType` (mutable; overwritten in place, no history kept),
- `marginPercent` (safety margin for filesystem/UDF overhead),
- `copies` (int, default 1; modeled now, planner targets 1 this cycle),
- `createdAt`.

**Disc** (logical record; identity is the catalog row, not media-embedded)
- `id` (pk), `projectId` (fk), `ordinal` (disc # within project),
- `discType`, `usableBytes` (this disc's own capacity = nominal − margin),
- `status` = `OPEN | FINALIZED`, `label` (human text to write on the disc).

Disc type and usable capacity are **per-disc physical properties** fixed when the
disc is first planned. This allows mixed-capacity projects (e.g. early BD-R 25 GB
discs alongside later BD-R DL 50 GB discs).

**Session** (one burn onto a disc)
- `id` (pk), `discId` (fk), `ordinal` (session # on that disc),
- `state` = `PLANNED | BURNED | VERIFIED`, `burnedAt`, `manifestHash`.

**FileVersion** (immutable fact — a new version is a new row)
- `id` (pk), `projectId` (fk), `relPath`, `contentHash`, `sizeBytes`, `mtime`,
  `firstSeenAt`.

**Placement** (a version × where it physically landed)
- `id` (pk), `fileVersionId` (fk), `sessionId` (fk), `discPath`
  (e.g. `/data/2024/01/01/a.jpg`), `copyIndex` (0…copies−1).

**ChangeRecord** (per session classification)
- `id` (pk), `sessionId` (fk), `fileVersionId` (fk),
  `kind` = `ADDED | MODIFIED | DELETED`.
  A `DELETED` record with no corresponding new FileVersion is a logical tombstone.

### 5.2 Derived views (queries, not stored)

- **Full history of a path** = all `FileVersion` rows for that `relPath`, ordered
  by time.
- **Current version** = latest non-tombstoned `FileVersion` per `relPath`.
- **Restore source** = that version's `Placement` row(s).

### 5.3 Hashing

SHA-256, hardcoded in cycle 1. `Hasher` is a port, so an alternate algorithm can be
introduced later via optional columns defaulting to SHA-256 — no migration pain.

---

## 6. Source snapshot (TOCTOU protection)

### 6.1 The problem

Between the start of a scan and the last byte written during a (possibly long)
burn, source files may be modified, deleted, or added. Acting on a mutating tree
risks a burn that no longer fits, or silent file-content changes that corrupt the
catalog. This is a time-of-check-to-time-of-use race.

### 6.2 The solution

Take **one immutable snapshot up front**; scan, plan, and burn all read from it, so
the entire operation observes a single consistent point in time. The snapshot is
released at the end of the run.

- **`SourceSnapshot` port:** `create(root) => SnapshotHandle`,
  `path(handle) => frozenRoot`, `release(handle)`.
- **Default implementation (cycle 1):** `cp --reflink=always` of the source into
  the project's **work directory** (works on btrfs and XFS-reflink). A btrfs
  subvolume-snapshot implementation may be added later.
- **CoW is a hard requirement.** If the work directory's filesystem does not
  support CoW reflinks, the tool **refuses to run** with an actionable error
  explaining why and which filesystems qualify. (No live-tree or full-copy
  fallback in cycle 1.)
- **Belt-and-suspenders:** even with the snapshot, the burner re-reads and re-hashes
  each file as it writes it and aborts rather than recording a hash that disagrees
  with what the scan recorded.

### 6.3 On-disc layout

Each burned session writes:

```
/.diskrement/
  project.json        # project name, disc N-of-M, session #, created-at
  manifest.json       # this session: added/modified/deleted + sizes + hashes
  catalog.sqlite      # FULL catalog snapshot at burn time (self-recovery)
  pointers.json       # /.archive text pointers to old versions on other discs
/data/...             # the live tree: current versions, mirroring source layout
```

- **`/data`** — the human-browsable live view (current version of each file at its
  natural path).
- **`/.archive`** — superseded and last-known-deleted versions, surfaced as **text
  pointers** (which disc/session/hash holds each old version). Real byte-level
  linking (same-disc extent reuse) is deferred to a burn-focused cycle.
- **`/.diskrement/catalog.sqlite`** — a **full** catalog snapshot on every disc
  (growth accepted: metadata is megabytes against 25–50 GB of data, and any single
  recent disc can rebuild the entire host catalog).

---

## 7. Diff (pure)

`diff(catalog: CatalogView, scanned: List[ScannedFile]): ChangeSet`

Classifies every scanned file:

- **Added** — `relPath` not in the catalog's current view → new `FileVersion`.
- **Modified** — `relPath` present but `contentHash` differs → new (superseding)
  `FileVersion`.
- **Deleted** — in the catalog's current view, absent from the snapshot → tombstone
  `ChangeRecord`.
- **Unchanged** — `relPath` + `contentHash` match → nothing to burn.

`ChangeSet = { added, modified, deleted, unchanged }`, each a list carrying
`relPath`, hash, size, mtime. This is the **only** input the planner needs from
reality; everything downstream is a pure function of it plus disc state.

### 7.1 Fast-path hashing (in the scanner, not the diff)

The **scanner** (shell) decides per file whether to reuse the catalog's stored hash
(when size + mtime match) or to stream-hash (new file, or size/mtime differ). The
`diff` always receives fully-hashed `ScannedFile`s, keeping it pure. A `--verify`
flag forces a full re-hash of every file, bypassing the fast path.

---

## 8. Planner (pure)

`plan(changes: ChangeSet, discs: List[DiscState], newDiscCap: Capacity): BurnPlan`

Deterministic ordered-fill bin packing (not optimal bin-packing). Each existing
`DiscState` carries its own usable capacity (per-disc model, §5.1); `newDiscCap` is
the usable capacity (disc type − margin) applied to any **new** discs opened during
this run. This is what makes mixed-capacity plans expressible.

### 8.1 Placement rules

1. Sort added + modified files lexicographically by `relPath`.
2. Fill the existing **OPEN** disc to capacity first.
3. A file that does not fit the remaining space spills **wholly** to the next disc
   (files are never split).
4. **Strict order, leave gaps:** once a file does not fit the current disc, stop
   filling it and spill all remaining files to the next disc. Tail space may be
   wasted; in exchange, files stay in perfect lexicographic order within and across
   discs (ideal for browsing a photo archive disc by disc). No backfill.
5. A disc filled to capacity is marked **FINALIZE**; only the last, partially-full
   disc is left **OPEN** for future sessions (auto-finalize policy). New discs use
   the disc type chosen for this run (`plan --disc-type`), falling back to the
   project's mutable default. Existing OPEN discs keep their original capacity, so
   mixed-capacity plans are supported.
6. Deletions/tombstones cost **zero bytes**.

### 8.2 Oversized files

A file that cannot fit on **any** disc available this run cannot be placed (no
splitting in cycle 1). It is reported as a **hard error** in the plan — listed
separately and skipped — rather than silently dropped.

"Any disc available" is the **larger** of: a new blank disc's usable capacity
(`newDiscCap`), or the existing OPEN disc's **full** usable capacity. So in a
mixed-capacity project, a file that exceeds a new blank disc but still fits the
(larger) open disc is placed on the open disc, not rejected. A file is oversized
only when it fits neither — including the case where it fits the open disc's total
capacity but not its remaining free space and is too big for a fresh disc.

### 8.3 Planner invariants (become ScalaCheck properties)

For any random `ChangeSet` × disc state × capacity:

1. **Conservation** — every added/modified file appears in exactly one placement in
   the plan, or in the oversized-error list; none lost, none duplicated.
2. **Capacity** — no disc in the plan exceeds its own usable capacity.
3. **No split** — each file lands wholly on one disc.
4. **Order preserved** — placed files are in lexicographic `relPath` order within
   and across discs.
5. **Determinism** — identical inputs produce an identical plan regardless of input
   list ordering.
6. **Open-disc-first** — the existing OPEN disc is filled before any new disc opens.
7. **Finalize invariant** — at most one disc is left OPEN after the plan (the last);
   every other touched disc is FINALIZE.
8. **Tombstones cost nothing** — deletions add zero bytes to any disc.

---

## 9. CLI (imperative shell)

### 9.1 Working directory & staged pipeline

- `init` accepts `--work-dir`, validated at init to be **on the same filesystem as
  the source root** and on a **CoW-capable** filesystem (hard-fail otherwise).
- All snapshots and staged scan/plan results live in the work directory — never
  `/tmp` (which may be a different filesystem; this matters when the library lives
  on a separate disk from the system).
- The pipeline is **staged**: `scan` persists its result, `plan` consumes the
  staged scan and persists the plan, `burn` consumes the staged plan.
- **Restage policy:** if fresh staged scan results exist, `scan` reuses them;
  `--force` (or `reset`) is required to re-snapshot and re-hash. `plan` always
  recomputes from the staged scan (cheap).

  > **Amended 2026-10-12 (#4):** superseded. `plan` now snapshots and scans the
  > source itself on every run, and the `scan` command is gone; nothing is reused.
  > See `2026-10-12-merge-scan-into-plan-design.md`.
- **Staleness:** `burn` executes the staged plan as-is (it does not re-diff), but it
  reads file bytes from the **frozen snapshot** and performs the per-file hash
  re-check (§6.2) as it writes. If the snapshot is gone, those reads fail loudly.

### 9.2 Commands

- `init <name> --source <dir> --work-dir <dir> [--disc-type BD_R] [--margin 2%] [--copies 1]`
  Create a project; run both filesystem checks now. `--disc-type` here only sets the
  mutable default for future plans.
- `scan <name> [--verify]` — snapshot + diff, staged to the work dir.
- `plan <name> [--disc-type BD_R_DL]` — consume staged scan; `--disc-type` sets the
  type for NEW discs this run.
- `burn <name> [--yes]` — consume staged plan; `SimulatedBurner` drives the
  interactive insert/label prompts and stages disc trees; on success writes the
  `/.diskrement` snapshot and commits the catalog update in one transaction.
- `reset <name>` — discard staged scan/plan state and release any lingering snapshot.
- `status <name>` — discs, sessions, totals, open disc + free space, last burn, any
  orphaned `PLANNED` session.
- `verify <name> [--disc N]` — re-read a (simulated) disc's snapshot, re-hash its
  files, compare against the catalog, report integrity.
- `history <name> <relPath>` — full version history of one file: every version,
  hash, disc/session, and current vs. superseded vs. deleted status.

### 9.3 SimulatedBurner

Writes the exact disc contents (`/data` + `/.diskrement`) into a per-disc staging
directory in the work dir, computes the would-be image size, and simulates the
insert/label interaction via scripted prompts. This lets integration tests assert
on the real on-disc layout and manifests, and lets `verify` run against the staged
output. Real ISO/UDF image generation and hardware burning are deferred.

### 9.4 Crash safety

A session progresses `PLANNED → BURNED → VERIFIED`. The host catalog is updated only
**after** the simulated burn reports success, in a **single transaction**. If the
process dies mid-burn, the catalog still reflects the last known-good state; the
orphaned `PLANNED` session is detected on the next run and offered for
retry/cleanup.

### 9.5 Configuration & paths

- Per-project SQLite catalog default location:
  - **systemd service:** `StateDirectory`, e.g. `/var/lib/diskrement/<project>.sqlite`;
    config under `ConfigurationDirectory` (e.g. `/etc/diskrement`).
  - **normal user:** XDG directories (`$XDG_DATA_HOME`, defaulting to
    `~/.local/share/diskrement/<project>.sqlite`).
- The catalog path is overridable per project.

---

## 10. Testing strategy

### 10.1 Property tests (ScalaCheck)

- The **8 planner invariants** (§8.3), including mixed-capacity discs.
- **Diff idempotence:** applying a diff and then re-scanning yields an empty diff.
- **Catalog append-only:** the current-view query never returns superseded or
  tombstoned versions; full history remains retrievable.
- **Generators:** random file trees, sizes, capacities, and multi-burn edit
  sequences (add/modify/delete across sessions and mixed disc types).

### 10.2 Unit tests (example-based)

- Edge cases: empty directory; single oversized file; exact-fit file; all-deleted;
  no-changes; move (delete + add); mixed-capacity spill (BD-R OPEN + BD-50 NEW).
- Manifest shape; capacity math with margin; path sorting (unicode, deep nesting).

### 10.3 Integration tests (shell against real effects)

- **SourceSnapshot:** reflink-cp on a real CoW temp filesystem; the hard-fail path
  on a non-CoW filesystem; release cleanup.
- **CatalogStore (SQLite):** migrations; transaction rollback on failure; recovery
  from an orphaned `PLANNED` session; rebuild catalog from a disc snapshot.
- **FileScanner:** real temp trees; fast-path vs. `--verify` agreement; staged scan
  reuse and `--force`.
- **SimulatedBurner:** stages disc trees → assert `/data` + `/.diskrement` layout,
  manifest JSON, embedded catalog snapshot.
- **Full loop (one scripted end-to-end test):**
  `init → scan → plan → burn → modify → scan → plan(--disc-type BD_R_DL) → burn →
  verify → history`, exercising multi-disc spill, mixed capacity, and `reset`. This
  proves the pieces work together (e.g. that a catalog written by `burn` is readable
  by the next `scan`, and that the disc-snapshot format matches what `verify`
  expects) — integration bugs the isolated tests cannot catch.

### 10.4 Manual test plan (documented checklist)

- Real btrfs snapshot of a large photo library (`plan` dry-run); hard-fail demo on
  ext4.
- **TOCTOU drill:** modify/delete files mid-run → confirm the snapshot isolates them,
  the catalog stays consistent, and `burn` re-validates the snapshot/per-file hash.
- Run as a systemd service (`StateDirectory` `/var/lib/diskrement`) vs. as a normal
  user (XDG directories).

---

## 11. Future cycles (not in scope now)

- Real burner integration (`xorriso`/`growisofs`), real ISO/UDF image generation.
- Web interface: browse projects, discs, sessions; preview photos/videos/documents;
  drive the interactive burn flow.
- Point-in-time restore of arbitrary historical versions.
- Enforced redundant copies (`copies > 1`) in the planner and burn flow.
- Real same-disc `/.archive` byte-level linking.
- btrfs subvolume-snapshot `SourceSnapshot` implementation.
- Oversized-file handling via a split/container format.
