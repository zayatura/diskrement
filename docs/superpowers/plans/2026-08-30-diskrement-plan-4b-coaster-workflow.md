# diskrement Plan 4b — Coaster Workflow Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A disc can be marked **dead** (destroyed/unreadable) via an append-only catalog event; its live content automatically re-plans onto fresh media, superseded versions are reported and recorded as permanently lost, and dead discs drop out of top-off planning and verify runs.

**Architecture:** Schema v2 adds two append-only tables (`disc_event`, `lost_version`); disc "deadness" is always **derived** from events, never an update-in-place of `disc.status`. The filtered burned view is implemented inside `CatalogStore.readView` (a current version with no placement on any live disc vanishes from the view, so `Diff` re-adds it). The planner is unchanged: a dead disc simply is not `OPEN`, so a fresh disc opens naturally. New `Pipeline.markDead` op + `disc mark-dead` CLI subcommand with confirmation.

**Tech Stack:** Scala 3, ZIO 2, zio-jdbc 0.1.2 + xerial sqlite-jdbc, zio-cli, zio-test, munit (core). Build uses `-Wunused:all` — keep imports clean.

**Spec:** `docs/superpowers/specs/2026-08-30-diskrement-cycle-2-real-burning-design.md` §7 (also §9–§11). Plan 4a (real burning) is DONE and merged; this plan touches only catalog/pipeline/CLI — **not** the burner.

---

## Context for implementers (read first)

- **Repo layout:** `core/` (pure, munit tests), `shell/` (adapters, zio-test), `cli/` (zio-cli, plus an `it` config integration suite driven by the staged launcher).
- **ZIO gotchas (learned in 4a):** ZIO for-comprehensions can NOT tuple-destructure a bind (`(a, b) <- eff` fails — no `withFilter`). Bind to one name, then destructure with `=` on the next line.
- **zio-jdbc/SQLite gotcha:** connections open with autoCommit=true; all transactional work must go through the existing `tx` helper in `CatalogStoreLive` (it calls `setAutoCommit(false)` first). Never write a new raw `transaction(...)` in main code without that.
- **Derived status rule (spec §7):** the `disc.status` column stays `OPEN`/`FINALIZED` forever (its CHECK constraint enforces this). `DEAD` exists only in the *derived* `DiscState` returned by `readDiscStates`, computed from `disc_event`.
- **Known caveat (spec §7, deferred):** the `copies > 1` duplicate-row caveat from Plan 3a also applies here; the "has a copy on another live disc" filter is written to be correct for that future, but no `copies > 1` behavior is activated now.
- **Verification commands:** `sbt core/test` (munit), `sbt shell/test`, `sbt cli/test`, `sbt cli/IntegrationTest/test` (needs a CoW-capable filesystem; skips itself cleanly otherwise). Expected counts before this plan: core 50+, shell 109, cli 53, IT 13.
- **Branch:** work happens on `feat/coaster-workflow` off `main` (Task 0).

## File structure

| File | Change | Responsibility |
|---|---|---|
| `core/src/main/scala/diskrement/core/Planner.scala` | modify (enum only) | `DiscStatus` gains `DEAD` |
| `core/src/test/scala/diskrement/core/PlannerTest.scala` | modify | dead-disc planner contract test |
| `shell/.../catalog/Migrations.scala` | modify | schema v2: `disc_event`, `lost_version` |
| `shell/.../model/LostReport.scala` | create | `LostVersionEntry`, `ReburnEntry`, `LostReport` |
| `shell/.../model/FileHistory.scala` | modify | `VersionStatus` gains `LOST` |
| `shell/.../catalog/CatalogStore.scala` | modify | `markDead` port method |
| `shell/.../catalog/CatalogStoreLive.scala` | modify | `markDead`, derived DEAD in `readDiscStates`, dead filter in `readView`, LOST in `fileHistory` |
| `shell/.../pipeline/PipelineError.scala` | modify | `UnknownDisc`, `DiscAlreadyDead`, `StalePlan` |
| `shell/.../pipeline/Pipeline.scala` | modify | `markDead` trait method |
| `shell/.../pipeline/PipelineLive.scala` | modify | `markDead` impl, verify exclusion, stale-plan burn guard |
| `cli/.../Args.scala`, `Commands.scala`, `Runner.scala`, `Render.scala` | modify | `disc mark-dead` subcommand, prompt, rendering, exit codes |
| `shell/src/test/.../catalog/MigrationsSpec.scala`, `MarkDeadSpec.scala` (new) | tests | schema + catalog behavior |
| `shell/src/test/.../pipeline/PipelineMarkDeadSpec.scala` (new) | tests | pipeline behavior |
| `cli/src/test/.../CommandsSpec.scala`, `RenderSpec.scala`, `ExitMappingSpec.scala` | tests | parsing/rendering/exit codes |
| `cli/src/it/.../scenarios/Scenario09MarkDead.scala` (new), `CliIntegrationSpec.scala` | tests | end-to-end recovery flow |
| `README.md` | modify | coaster workflow section |

---

### Task 0: Branch setup

- [ ] **Step 1: Create the feature branch**

```bash
cd ~/work/diskrement
git checkout main && git pull && git checkout -b feat/coaster-workflow
```

---

### Task 1: Core — `DiscStatus.DEAD` and the planner contract

**Files:**
- Modify: `core/src/main/scala/diskrement/core/Planner.scala` (lines 3–5, the enum)
- Test: `core/src/test/scala/diskrement/core/PlannerTest.scala`

The planner code itself does not change: it only ever tops off discs whose status is `OPEN` (`Planner.scala:45`), so a `DEAD` disc is ignored automatically. The test pins that contract.

- [ ] **Step 1: Write the failing test** (append to `PlannerTest`, matching its munit style — helpers `sf`, `changesOf`, `cap100` already exist in the file):

```scala
  test("a DEAD disc is never topped off; new files open the next fresh disc"):
    // dead disc 1 has plenty of free room, but must be ignored entirely
    val dead = DiscState(1, DiscStatus.DEAD, usableBytes = 100L, usedBytes = 10L)
    val cs   = changesOf(List(sf("a", 30)))
    val plan = Planner.plan(cs, discs = List(dead), newDiscCap = cap100)
    assertEquals(plan.discs.size, 1)
    val d = plan.discs.head
    assert(d.isNew)
    assert(!d.willFinalize)
    // the dead disc keeps its ordinal; the fresh disc takes the next one
    assertEquals(d.ordinal, 2)
```

- [ ] **Step 2: Run it — expect a compile failure** (`DEAD` is not a member of `DiscStatus`):

Run: `sbt core/test`

- [ ] **Step 3: Add the enum case.** In `core/src/main/scala/diskrement/core/Planner.scala` replace:

```scala
/** Whether a disc can still receive new sessions. */
enum DiscStatus:
  case OPEN, FINALIZED
```

with:

```scala
/** Whether a disc can still receive new sessions. DEAD means the physical disc
  * is lost or unreadable (a "coaster"): derived from a catalog mark-dead event,
  * never stored in the disc row itself. */
enum DiscStatus:
  case OPEN, FINALIZED, DEAD
```

- [ ] **Step 4: Run `sbt core/test` — all green.**

- [ ] **Step 5: Commit**

```bash
git add core
git commit -m "feat(core): DiscStatus.DEAD — dead discs are never topped off"
```

---

### Task 2: Schema v2 — `disc_event` and `lost_version` tables

**Files:**
- Modify: `shell/src/main/scala/diskrement/shell/catalog/Migrations.scala`
- Test: `shell/src/test/scala/diskrement/shell/catalog/MigrationsSpec.scala`

- [ ] **Step 1: Update `MigrationsSpec`.** Both existing tests assert version 1 — they must move to version 2. Replace the two tests' bodies:

```scala
object MigrationsSpec extends ZIOSpecDefault:
  def spec = suite("Migrations")(
    test("fresh database migrates to version 2 with all domain tables"):
      for
        _      <- transaction(Migrations.run)
        ver    <- transaction(sql"select max(version) from schema_version".query[Int].selectOne)
        tables <- transaction(
                    sql"select name from sqlite_master where type='table' order by name"
                      .query[String].selectAll)
      yield assertTrue(
        ver.contains(2),
        tables.contains("project"),
        tables.contains("disc"),
        tables.contains("session"),
        tables.contains("file_version"),
        tables.contains("placement"),
        tables.contains("change_record"),
        tables.contains("disc_event"),
        tables.contains("lost_version")
      ),
    test("running migrations twice is idempotent"):
      for
        _    <- transaction(Migrations.run)
        _    <- transaction(Migrations.run)
        rows <- transaction(sql"select version from schema_version".query[Int].selectAll)
      yield assertTrue(rows == zio.Chunk(2))
  ).provideLayer(TestDb.poolLayer)
```

- [ ] **Step 2: Run `sbt "shell/testOnly diskrement.shell.catalog.MigrationsSpec"` — expect FAIL** (version is still 1, tables missing).

- [ ] **Step 3: Add v2 to `Migrations.scala`.** After the `v1` list, add:

```scala
  private val v2: List[SqlFragment] = List(
    sql"""create table if not exists disc_event (
            id      integer primary key autoincrement,
            disc_id integer not null references disc(id),
            kind    text    not null check (kind in ('MARKED_DEAD')),
            at      text    not null
          )""",
    sql"""create table if not exists lost_version (
            id              integer primary key autoincrement,
            event_id        integer not null references disc_event(id),
            file_version_id integer not null references file_version(id),
            fate            text    not null check (fate in ('LOST','REBURN'))
          )""",
    sql"create index if not exists idx_disc_event_disc on disc_event(disc_id)"
  )
```

and change the registry line to:

```scala
  private val migrations: List[(Int, List[SqlFragment])] = List(1 -> v1, 2 -> v2)
```

- [ ] **Step 4: Run `sbt shell/test` — all green** (the whole suite, to confirm v2 breaks nothing).

- [ ] **Step 5: Commit**

```bash
git add shell
git commit -m "feat(catalog): schema v2 — disc_event + lost_version tables"
```

---

### Task 3: Catalog — `markDead` with lost/reburn report; derived DEAD state

**Files:**
- Create: `shell/src/main/scala/diskrement/shell/model/LostReport.scala`
- Modify: `shell/src/main/scala/diskrement/shell/catalog/CatalogStore.scala` (trait)
- Modify: `shell/src/main/scala/diskrement/shell/catalog/CatalogStoreLive.scala` (`markDead` + `readDiscStates`)
- Test: `shell/src/test/scala/diskrement/shell/catalog/MarkDeadSpec.scala` (new)

Semantics (spec §7): among the file versions placed on the dead disc, consider only those with **no placement on any other live disc**. Each such version is either the path's *current* version (latest change_record is non-DELETED and points at it) → **REBURN** (the next scan/plan/burn re-writes it from source), or not current → **LOST** (the source has moved past it; the bytes are gone forever). Both fates are recorded append-only in `lost_version`.

- [ ] **Step 1: Write the model file** `shell/src/main/scala/diskrement/shell/model/LostReport.scala`:

```scala
package diskrement.shell.model

import diskrement.core.{ContentHash, RelPath}

/** A version whose only copy was on the now-dead disc and whose path has moved
  * past it — those bytes are permanently gone. */
final case class LostVersionEntry(
    relPath: RelPath, contentHash: ContentHash, sizeBytes: Long, sessionOrdinal: Int)

/** A live file whose only burned copy was on the dead disc; the next
  * scan/plan/burn writes it again from the source. */
final case class ReburnEntry(relPath: RelPath, contentHash: ContentHash, sizeBytes: Long)

/** What marking a disc dead cost. Recorded in `lost_version`, rendered by the CLI. */
final case class LostReport(
    discOrdinal: Int, lost: List[LostVersionEntry], reburn: List[ReburnEntry])
```

- [ ] **Step 2: Write the failing spec** `shell/src/test/scala/diskrement/shell/catalog/MarkDeadSpec.scala`:

```scala
package diskrement.shell.catalog

import diskrement.core._
import diskrement.shell.model._
import zio._
import zio.jdbc._
import zio.test._
import java.time.Instant

object MarkDeadSpec extends ZIOSpecDefault:
  private val init = ProjectInit("p", "/src", "/work", DiscType.BD_R, 2.0, 1,
    Instant.parse("2026-06-01T00:00:00Z"))
  private val ts     = Instant.parse("2026-06-01T10:00:00Z")
  private val deadAt = Instant.parse("2026-06-02T09:00:00Z")

  private def fw(path: String, hash: String, size: Long) =
    FileWrite(RelPath(path), ContentHash(hash), size, ts, "/data/" + path, 0)

  private def session(disc: Int, ord: Int, added: List[FileWrite] = Nil,
                      modified: List[FileWrite] = Nil, deleted: List[RelPath] = Nil) =
    CatalogUpdate(ProjectId(0), Nil, List(DiscWrite(
      ordinal = disc, isNew = ord == 1, discType = DiscType.BD_R, usableBytes = 1_000_000L,
      status = DiscStatus.OPEN, label = s"d$disc",
      session = SessionWrite(ord, ts, s"m$disc-$ord", added, modified, deleted))))

  /** Disc 1: session 1 adds a,b; session 2 modifies b — so b@b1 is superseded in place. */
  private def seed = for
    pid  <- ZIO.serviceWithZIO[CatalogStore](_.createProject(init))
    store = ZIO.serviceWithZIO[CatalogStore]
    _    <- store(_.applyUpdate(session(1, 1,
              added = List(fw("a", "a1", 10), fw("b", "b1", 20))).copy(projectId = pid)))
    _    <- store(_.applyUpdate(session(1, 2,
              modified = List(fw("b", "b2", 25))).copy(projectId = pid)))
  yield pid

  def spec = suite("CatalogStore.markDead")(
    test("splits the disc's versions into lost (superseded) and reburn (current), records both"):
      for
        pid    <- seed
        rep    <- ZIO.serviceWithZIO[CatalogStore](_.markDead(pid, 1, deadAt))
        states <- ZIO.serviceWithZIO[CatalogStore](_.readDiscStates(pid))
        fates  <- transaction(
                    sql"""select fv.rel_path, lv.fate from lost_version lv
                          join file_version fv on fv.id = lv.file_version_id"""
                      .query[(String, String)].selectAll)
      yield assertTrue(
        rep.discOrdinal == 1,
        rep.lost.map(e => (e.relPath.value, e.contentHash.value, e.sessionOrdinal)) ==
          List(("b", "b1", 1)),
        rep.reburn.map(e => (e.relPath.value, e.contentHash.value)).toSet ==
          Set(("a", "a1"), ("b", "b2")),
        states == List(DiscState(1, DiscStatus.DEAD, 1_000_000L, 55L)),
        fates.toList.sorted == List(("a", "REBURN"), ("b", "LOST"), ("b", "REBURN")))
    ,
    test("a version that also lives on another live disc is neither lost nor reburned"):
      for
        pid <- seed
        // disc 2 exists via its own file; then wire a's version onto it with a raw
        // extra placement — exactly what copies>1 will eventually do for real
        _   <- ZIO.serviceWithZIO[CatalogStore](_.applyUpdate(session(2, 1,
                 added = List(fw("c", "c1", 5))).copy(projectId = pid)))
        _   <- transaction(
                 sql"""insert into placement (file_version_id, session_id, disc_path, copy_index)
                       select (select id from file_version where rel_path = 'a'),
                              (select s.id from session s join disc d on d.id = s.disc_id
                               where d.ordinal = 2),
                              '/data/a', 1""".execute)
        rep <- ZIO.serviceWithZIO[CatalogStore](_.markDead(pid, 1, deadAt))
      yield assertTrue(
        rep.reburn.map(_.relPath.value) == List("b"),
        rep.lost.map(_.relPath.value) == List("b"))
    ,
    test("marking a dead disc again fails; unknown ordinal fails"):
      for
        pid <- seed
        _   <- ZIO.serviceWithZIO[CatalogStore](_.markDead(pid, 1, deadAt))
        dup <- ZIO.serviceWithZIO[CatalogStore](_.markDead(pid, 1, deadAt)).exit
        bad <- ZIO.serviceWithZIO[CatalogStore](_.markDead(pid, 9, deadAt)).exit
      yield assertTrue(
        dup.causeOption.flatMap(_.failureOption)
          .exists(_.isInstanceOf[CatalogError.IllegalStatusTransition]),
        bad.causeOption.flatMap(_.failureOption)
          .exists(_.isInstanceOf[CatalogError.ConstraintViolation]))
  ).provideLayer(TestDb.layer)
```

- [ ] **Step 3: Run `sbt "shell/testOnly diskrement.shell.catalog.MarkDeadSpec"` — expect compile FAIL** (`markDead` does not exist).

- [ ] **Step 4: Add the port method.** In `CatalogStore.scala` (trait), after `discRecordedContents`, add (needs `java.time.Instant` import — the file imports `java.nio.file.Path` already; add `import java.time.Instant`):

```scala
  /** Append a MARKED_DEAD event for `discOrdinal` and record its casualties:
    * every version whose only copy was on this disc becomes either REBURN (the
    * path's current version — the next scan/plan/burn re-writes it from source)
    * or LOST (a superseded/deleted version — permanently gone). Fails with
    * IllegalStatusTransition if the disc is already dead. */
  def markDead(project: ProjectId, discOrdinal: Int, at: Instant): IO[CatalogError, LostReport]
```

- [ ] **Step 5: Implement in `CatalogStoreLive.scala`.** Add after `discRecordedContents`:

```scala
  def markDead(project: ProjectId, discOrdinal: Int, at: Instant): IO[CatalogError, LostReport] =
    tx {
      for
        discId <- discIdByOrdinal(project, discOrdinal)
        dead   <- sql"""select count(*) from disc_event
                        where disc_id = $discId and kind = 'MARKED_DEAD'"""
                    .query[Int].selectOne.map(_.getOrElse(0))
        _      <- ZIO.when(dead > 0)(ZIO.fail(
                    IllegalStatusTransition(s"disc $discOrdinal is already dead")))
        rows   <- deadDiscVersions(project, discId)
        _      <- sql"""insert into disc_event (disc_id, kind, at)
                        values ($discId, 'MARKED_DEAD', ${at.toString})""".insert
        evId   <- lastId
        _      <- ZIO.foreachDiscard(rows) { case (fvId, _, _, _, _, isCurrent) =>
                    val fate = if isCurrent == 1 then "REBURN" else "LOST"
                    sql"""insert into lost_version (event_id, file_version_id, fate)
                          values ($evId, $fvId, $fate)""".insert
                  }
      yield
        val (cur, gone) = rows.partition(_._6 == 1)
        LostReport(
          discOrdinal,
          lost = gone.map { case (_, rp, ch, sz, sOrd, _) =>
            LostVersionEntry(RelPath(rp), ContentHash(ch), sz, sOrd) },
          reburn = cur.map { case (_, rp, ch, sz, _, _) =>
            ReburnEntry(RelPath(rp), ContentHash(ch), sz) })
    }

  /** Versions placed on the dying disc that have no copy on any other live disc,
    * flagged with whether each is its path's current version. */
  private def deadDiscVersions(pid: ProjectId, discId: Long)
      : ZIO[ZConnection, Throwable, List[(Long, String, String, Long, Int, Int)]] =
    sql"""select fv.id, fv.rel_path, fv.content_hash, fv.size_bytes, s.ordinal,
                 case when cur.fvid is not null then 1 else 0 end as is_current
          from placement p
          join session s on s.id = p.session_id
          join file_version fv on fv.id = p.file_version_id
          left join (
            select fvid from (
              select cr.file_version_id as fvid, cr.kind as kind,
                     row_number() over (partition by fv2.rel_path order by cr.id desc) as rn
              from change_record cr
              join file_version fv2 on fv2.id = cr.file_version_id
              where fv2.project_id = ${pid.value}
            ) c where c.rn = 1 and c.kind <> 'DELETED'
          ) cur on cur.fvid = fv.id
          where s.disc_id = $discId
            and not exists (
              select 1 from placement p2
              join session s2 on s2.id = p2.session_id
              join disc d2 on d2.id = s2.disc_id
              where p2.file_version_id = fv.id and d2.id <> $discId
                and not exists (select 1 from disc_event e
                                where e.disc_id = d2.id and e.kind = 'MARKED_DEAD'))
          order by fv.rel_path, s.ordinal"""
      .query[(Long, String, String, Long, Int, Int)].selectAll.map(_.toList)
```

- [ ] **Step 6: Derive DEAD in `readDiscStates`.** In `CatalogStoreLive.scala`, change the query's status column from `d.status` to a derived expression — the full replacement query:

```scala
      sql"""select d.ordinal,
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
            where d.project_id = ${project.value}
            order by d.ordinal"""
```

(The row mapping already goes through `DiscStatus.valueOf`, which now knows `DEAD` from Task 1.)

- [ ] **Step 7: Run `sbt shell/test` — all green** (MarkDeadSpec passes; DiscStatesSpec and the rest stay green).

- [ ] **Step 8: Commit**

```bash
git add shell
git commit -m "feat(catalog): markDead event with lost/reburn report; derived DEAD disc state"
```

---

### Task 4: Catalog — dead-filtered `readView`; `fileHistory` LOST flag

**Files:**
- Modify: `shell/src/main/scala/diskrement/shell/model/FileHistory.scala` (enum)
- Modify: `shell/src/main/scala/diskrement/shell/catalog/CatalogStoreLive.scala` (`readView`, `fileHistory`)
- Test: `shell/src/test/scala/diskrement/shell/catalog/MarkDeadSpec.scala` (append two tests)

This is the spec's "filtered burned view": a current version whose every placement sits on dead discs disappears from `readView`, so `Diff` classifies the source file as *added* and it re-plans automatically. `fingerprints` is intentionally **unchanged** — it only feeds hash-reuse during scanning; whether a file re-plans is decided by `Diff` against `readView`.

- [ ] **Step 1: Append two failing tests to `MarkDeadSpec`** (inside the same `suite(...)`, before the closing `).provideLayer`):

```scala
    ,
    test("readView drops current versions whose only copy is on a dead disc"):
      for
        pid    <- seed
        before <- ZIO.serviceWithZIO[CatalogStore](_.readView(pid))
        _      <- ZIO.serviceWithZIO[CatalogStore](_.markDead(pid, 1, deadAt))
        after  <- ZIO.serviceWithZIO[CatalogStore](_.readView(pid))
      yield assertTrue(
        before.current.keySet.map(_.value) == Set("a", "b"),
        after.current.isEmpty)
    ,
    test("fileHistory flags versions on a dead disc as LOST; the current version stays CURRENT"):
      for
        pid <- seed
        _   <- ZIO.serviceWithZIO[CatalogStore](_.markDead(pid, 1, deadAt))
        hb  <- ZIO.serviceWithZIO[CatalogStore](_.fileHistory(pid, RelPath("b")))
      yield assertTrue(hb.map(_.status) == List(VersionStatus.LOST, VersionStatus.CURRENT))
```

- [ ] **Step 2: Run `sbt "shell/testOnly diskrement.shell.catalog.MarkDeadSpec"` — expect FAIL** (view still shows a and b; `VersionStatus.LOST` does not exist yet → compile error first).

- [ ] **Step 3: Add `LOST` to `VersionStatus`** in `shell/src/main/scala/diskrement/shell/model/FileHistory.scala`:

```scala
/** Lifecycle status of one file version relative to its path's current state.
  * LOST = a non-current version whose disc was marked dead: the bytes are gone. */
enum VersionStatus:
  case CURRENT, SUPERSEDED, DELETED, LOST
```

- [ ] **Step 4: Filter `readView`.** In `CatalogStoreLive.readView`, extend the outer `where` clause. Full replacement query:

```scala
      sql"""select fv.rel_path, fv.content_hash, fv.size_bytes
            from (
              select cr.id as cr_id, cr.kind as kind, cr.file_version_id as fvid,
                     fv.rel_path as rp,
                     row_number() over (partition by fv.rel_path order by cr.id desc) as rn
              from change_record cr
              join file_version fv on fv.id = cr.file_version_id
              where fv.project_id = ${project.value}
            ) l
            join file_version fv on fv.id = l.fvid
            where l.rn = 1 and l.kind <> 'DELETED'
              and exists (
                select 1 from placement p
                join session s on s.id = p.session_id
                where p.file_version_id = fv.id
                  and not exists (select 1 from disc_event e
                                  where e.disc_id = s.disc_id and e.kind = 'MARKED_DEAD'))"""
```

- [ ] **Step 5: Flag LOST in `fileHistory`.** Two changes in `CatalogStoreLive.fileHistory`:

Add a `disc_dead` column to the select (after `is_current`):

```scala
      sql"""select fv.content_hash, fv.size_bytes, d.ordinal, s.ordinal, fvk.kind,
                   case when fv.id = cur.fvid then 1 else 0 end as is_current,
                   case when exists (select 1 from disc_event e
                                     where e.disc_id = d.id and e.kind = 'MARKED_DEAD')
                        then 1 else 0 end as disc_dead
```

(the rest of the query is unchanged), widen the row type to `.query[(String, Long, Int, Int, String, Int, Int)]`, and replace the mapping:

```scala
    }.map(_.toList.map { case (ch, sz, dOrd, sOrd, kind, isCurrent, discDead) =>
      val status =
        if isCurrent == 1 then VersionStatus.CURRENT
        else if discDead == 1 then VersionStatus.LOST
        else if kind == "DELETED" then VersionStatus.DELETED
        else VersionStatus.SUPERSEDED
      FileHistoryEntry(ContentHash(ch), sz, dOrd, sOrd, status)
    })
```

Note: a *current* version on a dead disc still renders CURRENT — the source still has it and it is about to re-burn; only versions the source has moved past become LOST.

- [ ] **Step 6: Run `sbt shell/test` — all green** (ReadViewSpec/FileHistorySpec exercise projects with no dead discs and must stay green).

- [ ] **Step 7: Commit**

```bash
git add shell
git commit -m "feat(catalog): dead-disc-filtered readView; fileHistory LOST flag"
```

---

### Task 5: Pipeline — `markDead` op, verify exclusion, stale-plan burn guard

**Files:**
- Modify: `shell/src/main/scala/diskrement/shell/pipeline/PipelineError.scala`
- Modify: `shell/src/main/scala/diskrement/shell/pipeline/Pipeline.scala` (trait)
- Modify: `shell/src/main/scala/diskrement/shell/pipeline/PipelineLive.scala`
- Test: `shell/src/test/scala/diskrement/shell/pipeline/PipelineMarkDeadSpec.scala` (new)

Three behaviors: (1) `markDead` validates the ordinal and delegates to the catalog; (2) `verify` skips dead discs; (3) `burn` refuses a staged plan that targets a disc marked dead after `plan` ran (the spec's recovery flow interleaves `reset` — without this guard the simulated burner would happily commit a session onto a dead disc).

- [ ] **Step 1: Write the failing spec** `shell/src/test/scala/diskrement/shell/pipeline/PipelineMarkDeadSpec.scala`:

```scala
package diskrement.shell.pipeline

import diskrement.core._
import diskrement.shell.model._
import zio._
import zio.test._
import java.nio.file.Files
import java.time.Instant

object PipelineMarkDeadSpec extends ZIOSpecDefault:
  private def initOf(name: String, src: java.nio.file.Path, work: java.nio.file.Path) =
    ProjectInit(name, src.toString, work.toString, DiscType.BD_R, 2.0, 1,
      Instant.parse("2026-06-01T00:00:00Z"))

  def spec = suite("Pipeline.markDead")(
    test("mark-dead re-plans the dead disc's live files onto a new disc; verify skips it"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.txt" -> "aaa", "b.txt" -> "bbb"))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          _     <- pipe.scan("p", verify = false, force = false)
          _     <- pipe.plan("p", None, finalize = false)
          _     <- pipe.burn("p")
          // second session on the same disc supersedes b.txt in place
          _     <- ZIO.attempt(Files.writeString(src.resolve("b.txt"), "bbbb"))
          _     <- pipe.scan("p", verify = false, force = true)
          _     <- pipe.plan("p", None, finalize = false)
          _     <- pipe.burn("p")
          rep   <- pipe.markDead("p", 1)
          st    <- pipe.status("p")
          // recovery cycle: everything current re-plans onto a fresh disc 2
          s3    <- pipe.scan("p", verify = false, force = true)
          plan3 <- pipe.plan("p", None, finalize = false)
          _     <- pipe.burn("p")
          vreps <- pipe.verify("p", None)
          hb    <- pipe.history("p", RelPath("b.txt"))
        yield assertTrue(
          rep.discOrdinal == 1,
          rep.lost.map(_.relPath.value) == List("b.txt"),
          rep.reburn.map(_.relPath.value).toSet == Set("a.txt", "b.txt"),
          st.discs.find(_.ordinal == 1).exists(_.status == DiscStatus.DEAD),
          st.openDiscOrdinal.isEmpty,
          s3 == ScanResult(added = 2, modified = 0, deleted = 0, unchanged = 0),
          plan3.disc.isNew,
          plan3.disc.ordinal == 2,
          plan3.disc.files.map(_.relPath.value) == List("a.txt", "b.txt"),
          vreps.map(_.discOrdinal) == List(2),
          hb.map(_.status) == List(VersionStatus.LOST, VersionStatus.LOST, VersionStatus.CURRENT))
      }
    ,
    test("unknown ordinal and double mark fail with dedicated errors"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.txt" -> "aaa"))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          _     <- pipe.scan("p", verify = false, force = false)
          _     <- pipe.plan("p", None, finalize = false)
          _     <- pipe.burn("p")
          bad   <- pipe.markDead("p", 9).exit
          _     <- pipe.markDead("p", 1)
          dup   <- pipe.markDead("p", 1).exit
        yield assertTrue(
          bad.causeOption.flatMap(_.failureOption).contains(PipelineError.UnknownDisc(9)),
          dup.causeOption.flatMap(_.failureOption).contains(PipelineError.DiscAlreadyDead(1)))
      }
    ,
    test("burn refuses a staged plan whose target disc was marked dead after planning"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.txt" -> "aaa"))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          _     <- pipe.scan("p", verify = false, force = false)
          _     <- pipe.plan("p", None, finalize = false)
          _     <- pipe.burn("p")
          // stage a top-off of disc 1, then kill disc 1 before burning
          _     <- ZIO.attempt(Files.writeString(src.resolve("b.txt"), "bb"))
          _     <- pipe.scan("p", verify = false, force = true)
          _     <- pipe.plan("p", None, finalize = false)
          _     <- pipe.markDead("p", 1)
          res   <- pipe.burn("p").exit
        yield assertTrue(
          res.causeOption.flatMap(_.failureOption)
            .exists(_.isInstanceOf[PipelineError.StalePlan]))
      }
  ).provideLayer(PipelineTestKit.env.orDie)
```

- [ ] **Step 2: Run `sbt "shell/testOnly diskrement.shell.pipeline.PipelineMarkDeadSpec"` — expect compile FAIL** (`markDead`, error cases missing).

- [ ] **Step 3: Add the error cases.** In `PipelineError.scala`, after `OversizedFiles`:

```scala
  final case class UnknownDisc(ordinal: Int)
      extends PipelineError(s"no disc with ordinal $ordinal")
  final case class DiscAlreadyDead(ordinal: Int)
      extends PipelineError(s"disc $ordinal is already marked dead")
  final case class StalePlan(detail: String)
      extends PipelineError(s"staged plan is stale: $detail (run reset, then scan/plan)")
```

- [ ] **Step 4: Add the trait method.** In `Pipeline.scala`, after `verify`:

```scala
  def markDead(name: String, discOrdinal: Int): IO[PipelineError, LostReport]
```

(`LostReport` is covered by the existing `diskrement.shell.model.*` import.)

- [ ] **Step 5: Implement in `PipelineLive.scala`.** Add the method (e.g. after `verifyDisc`):

```scala
  def markDead(name: String, discOrdinal: Int): IO[PipelineError, LostReport] =
    for
      p      <- project(name)
      states <- catalog.readDiscStates(p.id).mapError(PipelineError.Failed(_))
      _      <- states.find(_.ordinal == discOrdinal) match
                  case None => ZIO.fail(PipelineError.UnknownDisc(discOrdinal))
                  case Some(s) if s.status == DiscStatus.DEAD =>
                    ZIO.fail(PipelineError.DiscAlreadyDead(discOrdinal))
                  case Some(_) => ZIO.unit
      now    <- Clock.instant
      report <- catalog.markDead(p.id, discOrdinal, now).mapError(PipelineError.Failed(_))
    yield report
```

- [ ] **Step 6: Exclude dead discs from verify.** In `PipelineLive.verify`, replace the `targets` line with:

```scala
      targets   = states.filter(_.status != DiscStatus.DEAD).map(_.ordinal)
                    .filter(o => discOrdinal.forall(_ == o))
```

- [ ] **Step 7: Guard burn against a stale plan.** In `PipelineLive.burn`, directly after the `report <- staged.readPlan(...)` bind and the existing oversized check, add:

```scala
      states    <- catalog.readDiscStates(p.id).mapError(PipelineError.Failed(_))
      _         <- ZIO.when(states.exists(s =>
                       s.ordinal == report.disc.ordinal && s.status == DiscStatus.DEAD))(
                     ZIO.fail(PipelineError.StalePlan(
                       s"disc ${report.disc.ordinal} was marked dead after this plan was staged")))
```

- [ ] **Step 8: Run `sbt shell/test` — all green** (including the untouched PipelineVerifySpec/PipelineFullLoopSpec).

- [ ] **Step 9: Commit**

```bash
git add shell
git commit -m "feat(pipeline): markDead op, verify excludes dead discs, stale-plan burn guard"
```

---

### Task 6: CLI — `disc mark-dead` command, prompt, rendering, exit codes

**Files:**
- Modify: `cli/src/main/scala/diskrement/cli/Args.scala`
- Modify: `cli/src/main/scala/diskrement/cli/Commands.scala`
- Modify: `cli/src/main/scala/diskrement/cli/Runner.scala`
- Modify: `cli/src/main/scala/diskrement/cli/Render.scala`
- Test: `cli/src/test/scala/diskrement/cli/CommandsSpec.scala`, `RenderSpec.scala`, `ExitMappingSpec.scala`

CLI shape: `diskrement disc mark-dead <name> <disc-ordinal> [--yes]`. A declined confirmation prints `Aborted.` and exits 0 (same convention as `burn`). `UnknownDisc`/`DiscAlreadyDead`/`StalePlan` map to exit 2 (user-fixable state errors, like `NoStagedPlan`).

- [ ] **Step 1: Write the failing tests.**

Append to `CommandsSpec`:

```scala
    test("disc mark-dead with ordinal and --yes") {
      for a <- parse("disc", "mark-dead", "photos", "1")
          b <- parse("disc", "mark-dead", "photos", "2", "--yes")
      yield assertTrue(
        a._2 == DiskrementCommand.MarkDead("photos", 1, assumeYes = false),
        b._2 == DiskrementCommand.MarkDead("photos", 2, assumeYes = true))
    },
```

Append to `RenderSpec` (match the file's existing test style; `LostReport` etc. come from `diskrement.shell.model` — add to imports if the wildcard is not already there):

```scala
    test("markDead lists lost and re-burn files") {
      val r = Render.markDead(LostReport(1,
        List(LostVersionEntry(RelPath("b"), ContentHash("b1"), 20L, 1)),
        List(ReburnEntry(RelPath("a"), ContentHash("a1"), 10L))))
      assertTrue(
        r.contains("Disc 1 marked DEAD."),
        r.contains("Permanently lost (1):"),
        r.contains("b1"),
        r.contains("Re-burn on next scan/plan/burn (1):"),
        r.contains("  a"))
    },
    test("markDead with no casualties") {
      val r = Render.markDead(LostReport(2, Nil, Nil))
      assertTrue(
        r.contains("No versions permanently lost."),
        r.contains("No live files to re-burn."))
    },
```

Append to `ExitMappingSpec`:

```scala
    test("UnknownDisc / DiscAlreadyDead / StalePlan -> exit 2") {
      assertTrue(
        Render.handle(PipelineError.UnknownDisc(3))._2 == ExitCode(2),
        Render.handle(PipelineError.DiscAlreadyDead(3))._2 == ExitCode(2),
        Render.handle(PipelineError.StalePlan("disc 3 was marked dead after this plan was staged"))._2 == ExitCode(2))
    },
```

- [ ] **Step 2: Run `sbt cli/test` — expect compile FAIL.**

- [ ] **Step 3: Add the command variant.** In `Args.scala`, append to `DiskrementCommand`:

```scala
  case MarkDead(name: String, disc: Int, assumeYes: Boolean)
```

- [ ] **Step 4: Add the subcommand.** In `Commands.scala`, after `history`:

```scala
  // --- disc mark-dead ---
  private val disc: Command[DiskrementCommand] =
    val yes = Options.boolean("yes").alias("y")
    val markDead =
      Command("mark-dead", yes, nameArg ++ Args.integer("disc"))
        .withHelp(HelpDoc.p("Mark a disc dead (destroyed/unreadable); its live files re-plan."))
        .map { case (y, (name, disc)) => DiskrementCommand.MarkDead(name, disc.toInt, y) }
    // zio-cli's Reducable elides the parent Command's Unit value here; if the
    // compiler reports (Unit, DiskrementCommand) instead, append `.map(_._2)`.
    Command("disc").subcommands(markDead)
```

and register it in the root: `.subcommands(init, scan, plan, burn, reset, status, verify, history, disc)`.

- [ ] **Step 5: Render.** In `Render.scala`, add after `verify`:

```scala
  def markDead(r: LostReport): String =
    val header = s"Disc ${r.discOrdinal} marked DEAD."
    val lost =
      if r.lost.isEmpty then List("No versions permanently lost.")
      else s"Permanently lost (${r.lost.size}):" ::
        r.lost.map(e =>
          s"  ${e.relPath.value}  ${gb(e.sizeBytes)}  ${e.contentHash.value}  (was session ${e.sessionOrdinal})")
    val reburn =
      if r.reburn.isEmpty then List("No live files to re-burn.")
      else s"Re-burn on next scan/plan/burn (${r.reburn.size}):" ::
        r.reburn.map(e => s"  ${e.relPath.value}")
    (header :: lost ::: reburn).mkString("\n")
```

and extend `handle`'s code mapping (before the `Burn` cases):

```scala
      case _: PipelineError.UnknownDisc       => ExitCode(2)
      case _: PipelineError.DiscAlreadyDead   => ExitCode(2)
      case _: PipelineError.StalePlan         => ExitCode(2)
```

- [ ] **Step 6: Runner.** In `Runner.scala`: add to `projectName`:

```scala
    case c: DiskrementCommand.MarkDead => c.name
```

and to `dispatch` (after the `History` case):

```scala
      case c: DiskrementCommand.MarkDead =>
        withProject(catalogPath, c.name, backend) { (_, pipeline) =>
          for
            ok   <- Prompt.confirm(
                      s"Mark disc ${c.disc} of ${c.name} DEAD? Superseded versions on it are lost forever; its live files will re-plan.",
                      c.assumeYes)
            code <- if !ok then printOut("Aborted.").as(ExitCode.success)
                    else pipeline.markDead(c.name, c.disc)
                           .flatMap(r => printOut(Render.markDead(r))).as(ExitCode.success)
          yield code
        }
```

- [ ] **Step 7: Run `sbt cli/test` — all green** (CliSmokeSpec must stay green; it pins the simulated backend and is unaffected).

- [ ] **Step 8: Commit**

```bash
git add cli
git commit -m "feat(cli): disc mark-dead command with confirmation and lost-history rendering"
```

---

### Task 7: Integration test — mark-dead recovery flow (scenario 9)

**Files:**
- Create: `cli/src/it/scala/diskrement/cli/it/scenarios/Scenario09MarkDead.scala`
- Modify: `cli/src/it/scala/diskrement/cli/it/CliIntegrationSpec.scala`

End-to-end drill of the spec's recovery flow on the simulated backend: burn → top-off (supersede a version in place) → `disc mark-dead 1` → re-plan picks up all live content on a fresh disc → verify excludes the dead disc → history shows LOST. Plus the decline path (no `--yes`, stdin EOF → declines).

- [ ] **Step 1: Write the scenario** `cli/src/it/scala/diskrement/cli/it/scenarios/Scenario09MarkDead.scala`:

```scala
package diskrement.cli.it
package scenarios

import zio.ZIO
import zio.test.*

/** The coaster recovery flow: a top-off disc dies; its live content re-plans
  * onto a fresh disc, the superseded version is reported permanently lost, and
  * verify no longer looks at the dead disc. */
object Scenario09MarkDead:
  val suite = zio.test.suite("scenario 9: mark-dead -> re-plan")(
    test("dead disc's live content re-plans; superseded version reported lost"):
      ZIO.scoped {
        for
          s    <- Support.scratch
          cli   = new Cli(s, discBytes = None)
          _    <- ZIO.attempt {
                    Tree.write(s.source, "a.txt", 128)
                    Tree.write(s.source, "b.txt", 128)
                  }
          _    <- Support.run(cli, "init", "p",
                    "--source", s.source.toString, "--work-dir", s.work.toString)
          // cycle 1: burn a+b to disc 1 (left OPEN)
          _    <- Support.run(cli, "scan", "p")
          _    <- Support.run(cli, "plan", "p")
          _    <- Support.run(cli, "burn", "p", "--yes")
          // cycle 2: modify b -> session 2 tops off disc 1, superseding b@v0 in place
          _    <- ZIO.attempt(Tree.modify(s.source, "b.txt", 128))
          _    <- Support.run(cli, "scan", "p")
          _    <- Support.run(cli, "plan", "p")
          _    <- Support.run(cli, "burn", "p", "--yes")
          // the coaster event
          dead <- Support.run(cli, "disc", "mark-dead", "p", "1", "--yes")
          st   <- Support.run(cli, "status", "p")
          // recovery cycle: everything live re-plans onto a fresh disc 2
          _    <- Support.run(cli, "scan", "p")
          pl   <- Support.run(cli, "plan", "p")
          b2   <- Support.run(cli, "burn", "p", "--yes")
          d2   <- ZIO.attemptBlocking(DiscOracle.dataFiles(s.discDir(2)))
          ver  <- Support.run(cli, "verify", "p")
          hb   <- Support.run(cli, "history", "p", "b.txt")
        yield assertTrue(
          dead.exitCode == 0,
          dead.stdout.contains("Disc 1 marked DEAD."),
          dead.stdout.contains("Permanently lost (1):"),
          dead.stdout.contains("b.txt"),
          dead.stdout.contains("Re-burn on next scan/plan/burn (2):"),
          st.stdout.contains("DEAD"),
          pl.exitCode == 0,
          pl.stdout.contains("new disc"),
          b2.exitCode == 0,
          b2.stdout.contains("Wrote disc 2"),
          d2 == Set("a.txt", "b.txt"),
          ver.exitCode == 0,
          !ver.stdout.contains("disc 1"),
          ver.stdout.contains("disc 2"),
          hb.exitCode == 0,
          hb.stdout.contains("LOST"))
      }
    ,
    test("declined confirmation aborts cleanly and marks nothing"):
      ZIO.scoped {
        for
          s    <- Support.scratch
          cli   = new Cli(s, discBytes = None)
          _    <- ZIO.attempt(Tree.write(s.source, "a.txt", 128))
          _    <- Support.run(cli, "init", "p",
                    "--source", s.source.toString, "--work-dir", s.work.toString)
          _    <- Support.run(cli, "scan", "p")
          _    <- Support.run(cli, "plan", "p")
          _    <- Support.run(cli, "burn", "p", "--yes")
          // no --yes: the harness closes stdin, so the prompt reads EOF -> decline
          dead <- Support.run(cli, "disc", "mark-dead", "p", "1")
          st   <- Support.run(cli, "status", "p")
        yield assertTrue(
          dead.exitCode == 0,
          dead.stdout.contains("Aborted."),
          !st.stdout.contains("DEAD"))
      }
  )
```

- [ ] **Step 2: Register it.** In `CliIntegrationSpec.scala`, add `scenarios.Scenario09MarkDead.suite,` after `scenarios.Scenario08Idempotent.suite,`.

- [ ] **Step 3: Stage the launcher and run the IT suite:**

Run: `sbt cli/stage && sbt cli/IntegrationTest/test`
Expected: all scenarios PASS (15 IT tests now). If the environment lacks CoW support the suite self-ignores — then at minimum confirm compilation via `sbt cli/IntegrationTest/compile` and report the limitation.

- [ ] **Step 4: Commit**

```bash
git add cli
git commit -m "test(cli): IT scenario 9 — mark-dead recovery flow"
```

---

### Task 8: Docs — README coaster section

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Update the README.** Read `README.md` first. In the status table (the row layout is visible in the file), mark the cycle-2 coaster workflow as done / add its row alongside the "Real burning" entry. Then, after the "Real burning" section, add:

```markdown
## Dead discs (coasters)

A burned disc that is later lost, destroyed, or unreadable can be written off:

    diskrement disc mark-dead <project> <disc-ordinal>

This records an append-only `MARKED_DEAD` event (disc rows are never rewritten)
and prints a **lost-history report**: file versions whose only copy was on that
disc and whose path has moved on are *permanently lost* (flagged `LOST` in
`history`); live files whose only burned copy was there simply drop out of the
burned view, so the next `scan` / `plan` / `burn` writes them again to fresh
media. Dead discs are excluded from `verify`, can never be topped off, and show
as `DEAD` in `status`. If a plan was staged before the disc was marked dead,
`burn` refuses it — run `reset`, then `scan`/`plan` again.
```

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "docs: README coaster workflow section"
```

---

### Task 9: Final verification and merge

- [ ] **Step 1: Full test run**

```bash
sbt core/test shell/test cli/test
sbt cli/stage "cli/IntegrationTest/test"
```

Expected: everything green (`-Wunused:all` — zero warnings from new code).

- [ ] **Step 2: Final review, then merge to main** (per repo convention: merge `feat/coaster-workflow` into `main`, push origin). Use superpowers:finishing-a-development-branch.

---

## Self-review notes (already applied)

- **Spec §7 coverage:** append-only event ✓ (Task 2/3), never-top-off + planner-unchanged ✓ (Task 1), verify exclusion + status flag ✓ (Task 5, Render prints derived `DEAD`), Diff-input filter ✓ (Task 4 `readView`), CLI command + confirmation + printed & recorded lost report ✓ (Task 3 `lost_version`, Task 6), lost versions stay in catalog flagged lost ✓ (Task 4 `VersionStatus.LOST`), recovery flow ✓ (Tasks 5/7; the `StalePlan` guard covers the mark-dead-without-reset hole), copies>1 interaction deferred but the "other live disc" filter is written future-correct ✓.
- **Type consistency:** `LostReport(discOrdinal, lost: List[LostVersionEntry], reburn: List[ReburnEntry])` is used identically in Tasks 3, 5, 6. `markDead(project: ProjectId, discOrdinal: Int, at: Instant)` (catalog) vs `markDead(name: String, discOrdinal: Int)` (pipeline) — intentional, mirrors `markVerified`.
- **`fingerprints` intentionally unchanged** — documented in Task 4.
- **Explicit `verify --disc N` on a dead disc** returns an empty report list ("Nothing to verify"); accepted as-is for this cycle.

---

## Deviations applied during execution (2026-09-04)

The plan text above is preserved as written. These review-driven changes were made
during execution and are what actually shipped — where the two disagree, the code wins.

- **Task 5's `StalePlan` message** says `re-run plan`, NOT "run reset, then scan/plan".
  `reset` is unnecessary and harmful: it deletes the work dir and staging trees, after
  which `StagingDiscVerifier` reports every disc `skipped`, destroying verify evidence.
  Pinned by `ExitMappingSpec` (`re-run plan`, and `!contains("reset")`). The Task 8
  README draft above carries the same wrong advice; the shipped README does not.
- **The Task 8 README draft's `LOST` wording** ("whose path has moved on") describes how
  the mark-dead *report* classifies casualties, not what `history` renders. Every version
  that lived only on the dead disc reads `LOST` in `history` after the recovery burn,
  including one whose content is still live (it reappears as a new `CURRENT` row, same
  hash). Per-physical-copy semantics; pinned by `PipelineMarkDeadSpec`.
- **`verify --disc <dead|unknown>`** fails with `DiscIsDead` / `UnknownDisc` (exit 2)
  rather than returning an empty report list. The self-review note below that says
  "accepted as-is for this cycle" is superseded. `DiscIsDead` is a fourth `PipelineError`
  case the plan never named.
- **The `Render.handle` exit-code mapping was pulled forward** from Task 6 into Task 5:
  adding sealed `PipelineError` cases made the existing match inexhaustive, and
  `StalePlan` is reachable from the existing CLI burn path (a live `MatchError`).
- **`idx_placement_fv` was amended into schema v2** rather than added as a v3 — v2 had
  never shipped on `main`.
- **`deadDiscVersions` groups by version**, so one version is one casualty regardless of
  how many placements it has on the dying disc.
- **Task 6's `Args.integer("disc")` does not compile** — zio-cli 0.5.0 has no named
  `Args.integer(String)` overload. Shipped as `Args.integer ?? "<description>"` with a
  `mapOrFail` rejecting non-positive and out-of-`Int`-range ordinals (`BigInt#toInt`
  would otherwise silently truncate `2^32+1` to 1 and kill the wrong disc).

Known follow-ups, deliberately not in this cycle: `archivePointers` still names dead
discs and that output is baked into subsequently burned media; `writeDisc` does not
itself refuse a session onto a dead disc (only `PipelineLive.burn`'s `StalePlan` guard
does); `fingerprints` is intentionally unfiltered.
