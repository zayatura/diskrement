# diskrement Plan 2a — Persistence Layer (SQLite CatalogStore) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the SQLite-backed `CatalogStore` — the first imperative-shell adapter — in a new ZIO `shell` module: append-only schema, forward migrations, the `CatalogView`/`DiscState` read models, and a single-transaction `CatalogUpdate` apply, all covered by ZIO Test integration tests against real temp-file databases.

**Architecture:** Functional core / imperative shell. The pure `core` module is unchanged and gains no dependencies. A new `shell` module depends on `core` + ZIO + zio-jdbc + sqlite-jdbc. The functional core never calls a port; the shell orchestrates. This plan implements only the `CatalogStore` port and its SQLite live implementation. Filesystem/scanner/snapshot/burner adapters are Plan 2b; the CLI and burn orchestration are Plan 3.

**Tech Stack:** Scala 3 (3.3.4 LTS), sbt, ZIO 2.1.19, zio-jdbc 0.1.2, sqlite-jdbc 3.49.1.0, zio-test 2.1.19. JDK 21.

**Spec:** `docs/superpowers/specs/2026-06-01-diskrement-plan-2a-persistence-design.md` (and parent `2026-05-31-diskrement-core-engine-design.md` §5, §5.2, §9.4, §10.1).

---

## File Structure

All new code lives in a new sbt module `shell`, package root `diskrement.shell`.

**Build:**
- Modify: `build.sbt` — add the `shell` module + dependency version vals; aggregate it.

**Main (`shell/src/main/scala/diskrement/shell/`):**
- `model/Project.scala` — `ProjectId`, `ProjectInit`, `Project`.
- `model/CatalogError.scala` — the `CatalogError` ADT (extends `Exception`).
- `model/CatalogUpdate.scala` — `CatalogUpdate`, `DiscWrite`, `SessionWrite`, `FileWrite`.
- `model/Session.scala` — `SessionState`, `Session`.
- `catalog/Db.scala` — `DbPath` + the SQLite `ZConnectionPool` layer (pragmas, single connection).
- `catalog/Migrations.scala` — DDL fragments + the forward-migration runner.
- `catalog/CatalogStore.scala` — the `CatalogStore` trait + its `live` `ZLayer`.
- `catalog/CatalogStoreLive.scala` — the SQLite implementation.

**Test (`shell/src/test/scala/diskrement/shell/`):**
- `SmokeSpec.scala`, `catalog/TestDb.scala` (shared test layer),
  `catalog/DbSpec.scala`, `catalog/MigrationsSpec.scala`, `catalog/ProjectSpec.scala`,
  `catalog/ApplyUpdateSpec.scala`, `catalog/ReadViewSpec.scala`,
  `catalog/DiscStatesSpec.scala`, `catalog/SessionsByStateSpec.scala`,
  `catalog/AppendOnlySpec.scala`.

Each file has one responsibility; persistence model types live together under `model/`, the SQLite adapter under `catalog/`.

---

## Environment prerequisite

Same toolchain as Plan 1 (JDK 21 + sbt). The new dependencies resolve from Maven Central on first compile. Verify Plan 1 is intact first:

```bash
sbt "core/test"     # expect: Passed: Total 50, Failed 0
```

Do not start Task 1 until that passes.

---

### Task 1: shell module scaffolding & ZIO Test smoke test

**Files:**
- Modify: `build.sbt`
- Test: `shell/src/test/scala/diskrement/shell/SmokeSpec.scala`

- [ ] **Step 1: Add the `shell` module to `build.sbt`**

Replace the existing `lazy val root = ...` line and add version vals + the module. The final `build.sbt` reads:

```scala
ThisBuild / scalaVersion := "3.3.4"
ThisBuild / organization := "io.github.zayatura.diskrement"
ThisBuild / scalacOptions ++= Seq("-deprecation", "-feature", "-Wunused:all")

val zioVersion     = "2.1.19"
val zioJdbcVersion = "0.1.2"
val sqliteVersion  = "3.49.1.0"

lazy val core = (project in file("core"))
  .settings(
    name := "diskrement-core",
    libraryDependencies ++= Seq(
      "org.scalameta"  %% "munit"            % "1.0.0"  % Test,
      "org.scalameta"  %% "munit-scalacheck" % "1.0.0"  % Test,
      "org.scalacheck" %% "scalacheck"       % "1.18.0" % Test
    )
  )

lazy val shell = (project in file("shell"))
  .dependsOn(core)
  .settings(
    name := "diskrement-shell",
    libraryDependencies ++= Seq(
      "dev.zio"    %% "zio"          % zioVersion,
      "dev.zio"    %% "zio-jdbc"     % zioJdbcVersion,
      "org.xerial"  % "sqlite-jdbc"  % sqliteVersion,
      "dev.zio"    %% "zio-test"     % zioVersion % Test,
      "dev.zio"    %% "zio-test-sbt" % zioVersion % Test
    ),
    testFrameworks += new TestFramework("zio.test.sbt.ZTestFramework")
  )

lazy val root = (project in file("."))
  .aggregate(core, shell)
  .settings(name := "diskrement")
```

- [ ] **Step 2: Write the failing smoke test**

```scala
package diskrement.shell

import zio.test._

object SmokeSpec extends ZIOSpecDefault:
  def spec = suite("shell toolchain")(
    test("zio-test compiles and runs"):
      assertTrue(1 + 1 == 2)
  )
```

- [ ] **Step 3: Run it to verify the toolchain resolves and runs**

Run: `sbt "shell/testOnly diskrement.shell.SmokeSpec"`
Expected: downloads ZIO/zio-jdbc/sqlite-jdbc on first run, then `1 tests passed`. If dependency resolution fails, fix versions before continuing.

- [ ] **Step 4: Commit**

```bash
git add build.sbt shell/src/test/scala/diskrement/shell/SmokeSpec.scala
git commit -m "$(cat <<'EOF'
chore(shell): scaffold ZIO shell module with zio-jdbc + sqlite

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: SQLite connection pool layer with pragmas

**Files:**
- Create: `shell/src/main/scala/diskrement/shell/catalog/Db.scala`
- Create: `shell/src/test/scala/diskrement/shell/catalog/TestDb.scala`
- Test: `shell/src/test/scala/diskrement/shell/catalog/DbSpec.scala`

- [ ] **Step 1: Write `TestDb` (shared test layer: fresh temp-file DB per test)**

```scala
package diskrement.shell.catalog

import zio._
import zio.jdbc._
import java.nio.file.{Files, Path}

/** Per-test SQLite database in a temp directory, deleted on release. */
object TestDb:
  val pathLayer: ZLayer[Any, Throwable, DbPath] =
    ZLayer.scoped {
      ZIO.acquireRelease(
        ZIO.attempt {
          val dir = Files.createTempDirectory("diskrement-test")
          DbPath(dir.resolve("catalog.sqlite"))
        }
      ) { dbPath =>
        ZIO.attempt {
          val dir = dbPath.value.getParent
          Files.list(dir).forEach(f => Files.deleteIfExists(f))
          Files.deleteIfExists(dir)
        }.orDie
      }
    }

  /** Just the connection pool (for tests that only run raw SQL). */
  val poolLayer: ZLayer[Any, Throwable, ZConnectionPool] =
    pathLayer >>> Db.poolLayer
```

- [ ] **Step 2: Write the failing pool test**

```scala
package diskrement.shell.catalog

import zio.jdbc._
import zio.test._

object DbSpec extends ZIOSpecDefault:
  def spec = suite("Db.poolLayer")(
    test("opens SQLite and applies WAL + foreign_keys pragmas"):
      for
        jm <- transaction(sql"pragma journal_mode".query[String].selectOne)
        fk <- transaction(sql"pragma foreign_keys".query[Int].selectOne)
        one <- transaction(sql"select 1".query[Int].selectOne)
      yield assertTrue(jm.contains("wal"), fk.contains(1), one.contains(1))
  ).provideLayer(TestDb.poolLayer)
```

- [ ] **Step 3: Run it to verify it fails**

Run: `sbt "shell/testOnly diskrement.shell.catalog.DbSpec"`
Expected: FAIL — `DbPath`/`Db` not found (compile error).

- [ ] **Step 4: Write `Db.scala`**

```scala
package diskrement.shell.catalog

import zio._
import zio.jdbc._
import java.nio.file.Path

/** Filesystem location of a project's SQLite catalog. */
final case class DbPath(value: Path)

object Db:
  /** Acquire one SQLite connection with the project's pragmas applied.
    * `foreign_keys` is per-connection in SQLite, so it is set on every connection. */
  private def acquireConn(path: Path): Task[java.sql.Connection] =
    ZIO.attempt {
      val conn = java.sql.DriverManager.getConnection(s"jdbc:sqlite:${path.toString}")
      val st   = conn.createStatement()
      try
        st.execute("PRAGMA journal_mode=WAL")
        st.execute("PRAGMA foreign_keys=ON")
        st.execute("PRAGMA synchronous=FULL")
      finally st.close()
      conn
    }

  /** A single-connection pool: SQLite has one writer; serializing avoids lock churn. */
  val poolLayer: ZLayer[DbPath, Throwable, ZConnectionPool] =
    ZLayer.scoped {
      for
        dbPath <- ZIO.service[DbPath]
        cfg     = ZConnectionPoolConfig.default.copy(minConnections = 1, maxConnections = 1)
        zenv   <- ZConnectionPool.make(acquireConn(dbPath.value)).build
                    .provideSome[Scope](ZLayer.succeed(cfg))
      yield zenv.get[ZConnectionPool]
    }
```

- [ ] **Step 5: Run it to verify it passes**

Run: `sbt "shell/testOnly diskrement.shell.catalog.DbSpec"`
Expected: `1 tests passed`.

- [ ] **Step 6: Commit**

```bash
git add shell/src/main/scala/diskrement/shell/catalog/Db.scala shell/src/test/scala/diskrement/shell/catalog/TestDb.scala shell/src/test/scala/diskrement/shell/catalog/DbSpec.scala
git commit -m "$(cat <<'EOF'
feat(shell): add SQLite connection pool layer with WAL pragmas

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Schema DDL + forward-migration runner

**Files:**
- Create: `shell/src/main/scala/diskrement/shell/catalog/Migrations.scala`
- Test: `shell/src/test/scala/diskrement/shell/catalog/MigrationsSpec.scala`

- [ ] **Step 1: Write the failing migration test**

```scala
package diskrement.shell.catalog

import zio.jdbc._
import zio.test._

object MigrationsSpec extends ZIOSpecDefault:
  def spec = suite("Migrations")(
    test("fresh database migrates to version 1 with all six tables"):
      for
        _      <- transaction(Migrations.run)
        ver    <- transaction(sql"select max(version) from schema_version".query[Int].selectOne)
        tables <- transaction(
                    sql"select name from sqlite_master where type='table' order by name"
                      .query[String].selectAll)
      yield assertTrue(
        ver.contains(1),
        tables.contains("project"),
        tables.contains("disc"),
        tables.contains("session"),
        tables.contains("file_version"),
        tables.contains("placement"),
        tables.contains("change_record")
      ),
    test("running migrations twice is idempotent"):
      for
        _    <- transaction(Migrations.run)
        _    <- transaction(Migrations.run)
        rows <- transaction(sql"select version from schema_version".query[Int].selectAll)
      yield assertTrue(rows == zio.Chunk(1))
  ).provideLayer(TestDb.poolLayer)
```

- [ ] **Step 2: Run it to verify it fails**

Run: `sbt "shell/testOnly diskrement.shell.catalog.MigrationsSpec"`
Expected: FAIL — `Migrations` not found.

- [ ] **Step 3: Write `Migrations.scala`**

```scala
package diskrement.shell.catalog

import zio._
import zio.jdbc._

/** Hand-rolled forward-migration runner. Run inside a `transaction`. */
object Migrations:

  private val v1: List[SqlFragment] = List(
    sql"""create table if not exists project (
            id                integer primary key autoincrement,
            name              text    not null unique,
            source_root       text    not null,
            work_dir          text    not null,
            default_disc_type text    not null,
            margin_percent    real    not null,
            copies            integer not null default 1,
            created_at        text    not null
          )""",
    sql"""create table if not exists disc (
            id          integer primary key autoincrement,
            project_id  integer not null references project(id),
            ordinal     integer not null,
            disc_type   text    not null,
            usable_bytes integer not null,
            status      text    not null check (status in ('OPEN','FINALIZED')),
            label       text    not null,
            unique (project_id, ordinal)
          )""",
    sql"""create table if not exists session (
            id            integer primary key autoincrement,
            disc_id       integer not null references disc(id),
            ordinal       integer not null,
            state         text    not null check (state in ('PLANNED','BURNED','VERIFIED')),
            burned_at     text,
            manifest_hash text,
            unique (disc_id, ordinal)
          )""",
    sql"""create table if not exists file_version (
            id            integer primary key autoincrement,
            project_id    integer not null references project(id),
            rel_path      text    not null,
            content_hash  text    not null,
            size_bytes    integer not null,
            mtime         text    not null,
            first_seen_at text    not null
          )""",
    sql"""create table if not exists placement (
            id              integer primary key autoincrement,
            file_version_id integer not null references file_version(id),
            session_id      integer not null references session(id),
            disc_path       text    not null,
            copy_index      integer not null
          )""",
    sql"""create table if not exists change_record (
            id              integer primary key autoincrement,
            session_id      integer not null references session(id),
            file_version_id integer not null references file_version(id),
            kind            text    not null check (kind in ('ADDED','MODIFIED','DELETED'))
          )""",
    sql"create index if not exists idx_file_version_path on file_version(project_id, rel_path)",
    sql"create index if not exists idx_placement_session on placement(session_id)",
    sql"create index if not exists idx_change_record_fv on change_record(file_version_id)"
  )

  private val migrations: List[(Int, List[SqlFragment])] = List(1 -> v1)

  private val ensureVersionTable: ZIO[ZConnection, Throwable, Unit] =
    sql"create table if not exists schema_version (version integer not null)".execute

  private val currentVersion: ZIO[ZConnection, Throwable, Int] =
    sql"select coalesce(max(version), 0) from schema_version".query[Int].selectOne.map(_.getOrElse(0))

  private def setVersion(v: Int): ZIO[ZConnection, Throwable, Unit] =
    sql"delete from schema_version".execute *>
      sql"insert into schema_version (version) values ($v)".insert.unit

  /** Apply every migration whose version exceeds the stored one, in order. */
  val run: ZIO[ZConnection, Throwable, Unit] =
    for
      _   <- ensureVersionTable
      cur <- currentVersion
      _   <- ZIO.foreachDiscard(migrations.filter(_._1 > cur)) { case (v, stmts) =>
               ZIO.foreachDiscard(stmts)(_.execute) *> setVersion(v)
             }
    yield ()
```

- [ ] **Step 4: Run it to verify it passes**

Run: `sbt "shell/testOnly diskrement.shell.catalog.MigrationsSpec"`
Expected: `2 tests passed`.

- [ ] **Step 5: Commit**

```bash
git add shell/src/main/scala/diskrement/shell/catalog/Migrations.scala shell/src/test/scala/diskrement/shell/catalog/MigrationsSpec.scala
git commit -m "$(cat <<'EOF'
feat(shell): add append-only schema and forward-migration runner

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: Project model + CatalogError + CatalogStore trait + createProject/findProject

**Files:**
- Create: `shell/src/main/scala/diskrement/shell/model/Project.scala`
- Create: `shell/src/main/scala/diskrement/shell/model/CatalogError.scala`
- Create: `shell/src/main/scala/diskrement/shell/catalog/CatalogStore.scala`
- Create: `shell/src/main/scala/diskrement/shell/catalog/CatalogStoreLive.scala`
- Modify: `shell/src/test/scala/diskrement/shell/catalog/TestDb.scala`
- Test: `shell/src/test/scala/diskrement/shell/catalog/ProjectSpec.scala`

- [ ] **Step 1: Extend `TestDb` with a layer exposing both the pool and the store**

Add to `object TestDb` (below `poolLayer`):

```scala
  /** Pool + migrated CatalogStore, both visible to tests (>+> keeps the pool). */
  val layer: ZLayer[Any, Throwable, ZConnectionPool & CatalogStore] =
    pathLayer >>> (Db.poolLayer >+> CatalogStore.live)
```

- [ ] **Step 2: Write the failing project test**

```scala
package diskrement.shell.catalog

import diskrement.core.DiscType
import diskrement.shell.model._
import zio._
import zio.test._
import java.time.Instant

object ProjectSpec extends ZIOSpecDefault:
  private val init = ProjectInit(
    name = "photos", sourceRoot = "/src", workDir = "/work",
    defaultDiscType = DiscType.BD_R, marginPercent = 2.0, copies = 1,
    createdAt = Instant.parse("2026-06-01T00:00:00Z"))

  def spec = suite("CatalogStore projects")(
    test("createProject then findProject round-trips"):
      for
        id  <- ZIO.serviceWithZIO[CatalogStore](_.createProject(init))
        got <- ZIO.serviceWithZIO[CatalogStore](_.findProject("photos"))
      yield assertTrue(
        got.exists(p => p.id == id && p.name == "photos" && p.defaultDiscType == DiscType.BD_R)),
    test("createProject on a duplicate name fails with DuplicateProject"):
      for
        _   <- ZIO.serviceWithZIO[CatalogStore](_.createProject(init))
        err <- ZIO.serviceWithZIO[CatalogStore](_.createProject(init)).flip
      yield assertTrue(err == CatalogError.DuplicateProject("photos")),
    test("findProject returns None for an unknown name"):
      for got <- ZIO.serviceWithZIO[CatalogStore](_.findProject("nope"))
      yield assertTrue(got.isEmpty)
  ).provideLayer(TestDb.layer)
```

- [ ] **Step 3: Run it to verify it fails**

Run: `sbt "shell/testOnly diskrement.shell.catalog.ProjectSpec"`
Expected: FAIL — `ProjectInit`/`CatalogStore` not found.

- [ ] **Step 4: Write the model types**

`model/Project.scala`:

```scala
package diskrement.shell.model

import diskrement.core.DiscType
import java.time.Instant

/** Primary key of a project row. */
final case class ProjectId(value: Long)

/** Fields needed to create a project (no id yet). */
final case class ProjectInit(
    name: String,
    sourceRoot: String,
    workDir: String,
    defaultDiscType: DiscType,
    marginPercent: Double,
    copies: Int,
    createdAt: Instant
)

/** A persisted project row. */
final case class Project(
    id: ProjectId,
    name: String,
    sourceRoot: String,
    workDir: String,
    defaultDiscType: DiscType,
    marginPercent: Double,
    copies: Int,
    createdAt: Instant
)
```

`model/CatalogError.scala`:

```scala
package diskrement.shell.model

/** Expected, recoverable persistence failures. Extends Exception so it can flow
  * through zio-jdbc's `Throwable` error channel and be kept by `refineOrDie`. */
sealed abstract class CatalogError(message: String, cause: Throwable = null)
    extends Exception(message, cause)

object CatalogError:
  final case class DuplicateProject(name: String)
      extends CatalogError(s"project already exists: $name")
  final case class ProjectNotFound(name: String)
      extends CatalogError(s"project not found: $name")
  final case class IllegalStatusTransition(detail: String)
      extends CatalogError(detail)
  final case class MigrationFailed(cause: Throwable)
      extends CatalogError("migration failed", cause)
  final case class ConstraintViolation(cause: Throwable)
      extends CatalogError("constraint violation", cause)
```

- [ ] **Step 5: Write the `CatalogStore` trait + live layer**

`catalog/CatalogStore.scala`:

```scala
package diskrement.shell.catalog

import diskrement.shell.model._
import zio._
import zio.jdbc._

/** Persistence port: reads the core's view/disc-states, applies updates atomically. */
trait CatalogStore:
  def createProject(p: ProjectInit): IO[CatalogError, ProjectId]
  def findProject(name: String):     IO[CatalogError, Option[Project]]

object CatalogStore:
  /** Build the store from a pool, running migrations once at layer construction. */
  val live: ZLayer[ZConnectionPool, Throwable, CatalogStore] =
    ZLayer {
      for
        pool <- ZIO.service[ZConnectionPool]
        _    <- transaction(Migrations.run)
                  .provideEnvironment(ZEnvironment(pool))
                  .mapError(CatalogError.MigrationFailed(_))
      yield CatalogStoreLive(pool)
    }
```

- [ ] **Step 6: Write the live implementation (createProject/findProject only for now)**

`catalog/CatalogStoreLive.scala`:

```scala
package diskrement.shell.catalog

import diskrement.core.DiscType
import diskrement.shell.model._
import diskrement.shell.model.CatalogError._
import zio._
import zio.jdbc._
import java.time.Instant

final case class CatalogStoreLive(pool: ZConnectionPool) extends CatalogStore:

  /** Run a connection program as one transaction, mapping expected faults to
    * CatalogError and dying on anything unexpected. */
  private def tx[A](z: ZIO[ZConnection, Throwable, A]): IO[CatalogError, A] =
    transaction(z)
      .provideEnvironment(ZEnvironment(pool))
      .refineOrDie {
        case e: CatalogError                              => e
        case e: java.sql.SQLException if isConstraint(e)  => ConstraintViolation(e)
      }

  private def isConstraint(e: java.sql.SQLException): Boolean =
    Option(e.getMessage).exists(_.toLowerCase.contains("constraint"))

  /** Last auto-increment id on the current connection (valid within a transaction). */
  private val lastId: ZIO[ZConnection, Throwable, Long] =
    sql"select last_insert_rowid()".query[Long].selectOne.map(_.getOrElse(0L))

  def createProject(p: ProjectInit): IO[CatalogError, ProjectId] =
    tx {
      sql"""insert into project
              (name, source_root, work_dir, default_disc_type, margin_percent, copies, created_at)
            values
              (${p.name}, ${p.sourceRoot}, ${p.workDir}, ${p.defaultDiscType.toString},
               ${p.marginPercent}, ${p.copies}, ${p.createdAt.toString})""".insert
        *> lastId.map(ProjectId(_))
    }.catchSome {
      case ConstraintViolation(_) => ZIO.fail(DuplicateProject(p.name))
    }

  def findProject(name: String): IO[CatalogError, Option[Project]] =
    tx {
      sql"""select id, name, source_root, work_dir, default_disc_type,
                   margin_percent, copies, created_at
            from project where name = $name"""
        .query[(Long, String, String, String, String, Double, Int, String)].selectOne
    }.map(_.map { case (id, n, sr, wd, dt, mp, cp, ca) =>
      Project(ProjectId(id), n, sr, wd, DiscType.valueOf(dt), mp, cp, Instant.parse(ca))
    })
```

- [ ] **Step 7: Run it to verify it passes**

Run: `sbt "shell/testOnly diskrement.shell.catalog.ProjectSpec"`
Expected: `3 tests passed`.

- [ ] **Step 8: Commit**

```bash
git add shell/src/main/scala/diskrement/shell/model/Project.scala shell/src/main/scala/diskrement/shell/model/CatalogError.scala shell/src/main/scala/diskrement/shell/catalog/CatalogStore.scala shell/src/main/scala/diskrement/shell/catalog/CatalogStoreLive.scala shell/src/test/scala/diskrement/shell/catalog/TestDb.scala shell/src/test/scala/diskrement/shell/catalog/ProjectSpec.scala
git commit -m "$(cat <<'EOF'
feat(shell): add CatalogStore with project create/find

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: CatalogUpdate model + transactional applyUpdate

**Files:**
- Create: `shell/src/main/scala/diskrement/shell/model/CatalogUpdate.scala`
- Modify: `shell/src/main/scala/diskrement/shell/catalog/CatalogStore.scala` (add `applyUpdate`)
- Modify: `shell/src/main/scala/diskrement/shell/catalog/CatalogStoreLive.scala`
- Test: `shell/src/test/scala/diskrement/shell/catalog/ApplyUpdateSpec.scala`

- [ ] **Step 1: Write the failing applyUpdate test**

```scala
package diskrement.shell.catalog

import diskrement.core._
import diskrement.shell.model._
import zio._
import zio.jdbc._
import zio.test._
import java.time.Instant

object ApplyUpdateSpec extends ZIOSpecDefault:
  private val init = ProjectInit("p", "/src", "/work", DiscType.BD_R, 2.0, 1,
    Instant.parse("2026-06-01T00:00:00Z"))
  private val ts = Instant.parse("2026-06-01T10:00:00Z")

  private def file(path: String, size: Long) =
    FileWrite(RelPath(path), ContentHash("h-" + path), size, ts, "/data/" + path, 0)

  private def newDiscUpdate(pid: ProjectId) =
    CatalogUpdate(
      projectId = pid,
      finalizeDiscOrdinals = Nil,
      discs = List(DiscWrite(
        ordinal = 1, isNew = true, discType = DiscType.BD_R, usableBytes = 100L,
        status = DiscStatus.OPEN, label = "disc 1",
        session = SessionWrite(ordinal = 1, burnedAt = ts, manifestHash = "m1",
          added = List(file("a", 10), file("b", 20)), modified = Nil, deleted = Nil))))

  private def rowCount(q: SqlFragment): URIO[ZConnectionPool, Int] =
    transaction(q.query[Int].selectOne).map(_.getOrElse(0)).orDie

  def spec = suite("CatalogStore.applyUpdate")(
    test("writes discs, sessions, file_versions, placements and change_records atomically"):
      for
        pid <- ZIO.serviceWithZIO[CatalogStore](_.createProject(init))
        _   <- ZIO.serviceWithZIO[CatalogStore](_.applyUpdate(newDiscUpdate(pid)))
        d   <- rowCount(sql"select count(*) from disc")
        s   <- rowCount(sql"select count(*) from session")
        fv  <- rowCount(sql"select count(*) from file_version")
        pl  <- rowCount(sql"select count(*) from placement")
        cr  <- rowCount(sql"select count(*) from change_record")
      yield assertTrue(d == 1, s == 1, fv == 2, pl == 2, cr == 2),
    test("a failing applyUpdate rolls back every insert"):
      // second disc references a non-existent ordinal (isNew=false) -> fails after
      // the first disc's rows were inserted; the whole transaction must roll back.
      val bad = newDiscUpdate(ProjectId(0)).copy(discs =
        newDiscUpdate(ProjectId(0)).discs :+ DiscWrite(
          ordinal = 99, isNew = false, discType = DiscType.BD_R, usableBytes = 100L,
          status = DiscStatus.FINALIZED, label = "ghost",
          session = SessionWrite(2, ts, "m2", Nil, Nil, Nil)))
      for
        pid <- ZIO.serviceWithZIO[CatalogStore](_.createProject(init))
        err <- ZIO.serviceWithZIO[CatalogStore](_.applyUpdate(bad.copy(projectId = pid))).flip
        fv  <- rowCount(sql"select count(*) from file_version")
      yield assertTrue(err.isInstanceOf[CatalogError], fv == 0)
  ).provideLayer(TestDb.layer)
```

- [ ] **Step 2: Run it to verify it fails**

Run: `sbt "shell/testOnly diskrement.shell.catalog.ApplyUpdateSpec"`
Expected: FAIL — `CatalogUpdate`/`applyUpdate` not found.

- [ ] **Step 3: Write the `CatalogUpdate` model**

`model/CatalogUpdate.scala`:

```scala
package diskrement.shell.model

import diskrement.core.{ContentHash, DiscStatus, DiscType, RelPath}
import java.time.Instant

/** One file written this run: its identity, where it landed, and which copy. */
final case class FileWrite(
    relPath: RelPath,
    contentHash: ContentHash,
    sizeBytes: Long,
    mtime: Instant,
    discPath: String,
    copyIndex: Int
)

/** One session burned onto one disc this run. */
final case class SessionWrite(
    ordinal: Int,
    burnedAt: Instant,
    manifestHash: String,
    added: List[FileWrite],
    modified: List[FileWrite],
    deleted: List[RelPath]   // tombstones: reference the current version of each path
)

/** One disc touched or created this run, with its single new session. */
final case class DiscWrite(
    ordinal: Int,
    isNew: Boolean,
    discType: DiscType,
    usableBytes: Long,
    status: DiscStatus,      // resulting status (OPEN for the last disc, else FINALIZED)
    label: String,
    session: SessionWrite
)

/** The full set of catalog changes from one successful burn run. */
final case class CatalogUpdate(
    projectId: ProjectId,
    finalizeDiscOrdinals: List[Int],   // pre-existing OPEN discs to flip to FINALIZED
    discs: List[DiscWrite]
)
```

- [ ] **Step 4: Add `applyUpdate` to the trait**

In `catalog/CatalogStore.scala`, add to the `trait CatalogStore`:

```scala
  def applyUpdate(update: CatalogUpdate): IO[CatalogError, Unit]
```

- [ ] **Step 5: Implement `applyUpdate` in the live impl**

In `catalog/CatalogStoreLive.scala`, add these methods to the class (the `kind`
text reuses the core's `ManifestChangeKind`):

```scala
  import diskrement.core.{DiscStatus, ManifestChangeKind, RelPath}

  def applyUpdate(u: CatalogUpdate): IO[CatalogError, Unit] =
    tx {
      ZIO.foreachDiscard(u.finalizeDiscOrdinals)(finalizeDisc(u.projectId, _)) *>
        ZIO.foreachDiscard(u.discs)(writeDisc(u.projectId, _))
    }

  private def finalizeDisc(pid: ProjectId, ordinal: Int): ZIO[ZConnection, Throwable, Unit] =
    sql"""update disc set status = 'FINALIZED'
          where project_id = ${pid.value} and ordinal = $ordinal and status = 'OPEN'""".update
      .flatMap(n =>
        ZIO.when(n == 0L)(
          ZIO.fail(IllegalStatusTransition(s"disc $ordinal is not OPEN"))).unit)

  private def discIdByOrdinal(pid: ProjectId, ordinal: Int): ZIO[ZConnection, Throwable, Long] =
    sql"select id from disc where project_id = ${pid.value} and ordinal = $ordinal"
      .query[Long].selectOne
      .flatMap {
        case Some(id) => ZIO.succeed(id)
        case None     => ZIO.fail(ConstraintViolation(
                           new RuntimeException(s"no disc with ordinal $ordinal")))
      }

  private def writeDisc(pid: ProjectId, d: DiscWrite): ZIO[ZConnection, Throwable, Unit] =
    for
      discId <- if d.isNew then
                  sql"""insert into disc
                          (project_id, ordinal, disc_type, usable_bytes, status, label)
                        values
                          (${pid.value}, ${d.ordinal}, ${d.discType.toString},
                           ${d.usableBytes}, ${d.status.toString}, ${d.label})""".insert *> lastId
                else
                  for
                    id <- discIdByOrdinal(pid, d.ordinal)
                    _  <- ZIO.when(d.status == DiscStatus.FINALIZED)(
                            sql"update disc set status = 'FINALIZED' where id = $id and status = 'OPEN'"
                              .update.unit)
                  yield id
      sessId <- sql"""insert into session (disc_id, ordinal, state, burned_at, manifest_hash)
                      values ($discId, ${d.session.ordinal}, 'BURNED',
                              ${d.session.burnedAt.toString}, ${d.session.manifestHash})""".insert *> lastId
      _ <- ZIO.foreachDiscard(d.session.added)(
             writeFile(pid, sessId, _, ManifestChangeKind.ADDED, d.session.burnedAt))
      _ <- ZIO.foreachDiscard(d.session.modified)(
             writeFile(pid, sessId, _, ManifestChangeKind.MODIFIED, d.session.burnedAt))
      _ <- ZIO.foreachDiscard(d.session.deleted)(writeTombstone(pid, sessId, _))
    yield ()

  private def writeFile(pid: ProjectId, sessId: Long, f: FileWrite,
                        kind: ManifestChangeKind, firstSeenAt: Instant): ZIO[ZConnection, Throwable, Unit] =
    for
      fvId <- sql"""insert into file_version
                      (project_id, rel_path, content_hash, size_bytes, mtime, first_seen_at)
                    values
                      (${pid.value}, ${f.relPath.value}, ${f.contentHash.value},
                       ${f.sizeBytes}, ${f.mtime.toString}, ${firstSeenAt.toString})""".insert *> lastId
      _ <- sql"""insert into placement (file_version_id, session_id, disc_path, copy_index)
                 values ($fvId, $sessId, ${f.discPath}, ${f.copyIndex})""".insert
      _ <- sql"insert into change_record (session_id, file_version_id, kind) values ($sessId, $fvId, ${kind.toString})".insert
    yield ()

  /** Tombstone the path's current version (the latest file_version for that path). */
  private def writeTombstone(pid: ProjectId, sessId: Long, rel: RelPath): ZIO[ZConnection, Throwable, Unit] =
    sql"""select id from file_version
          where project_id = ${pid.value} and rel_path = ${rel.value}
          order by id desc limit 1""".query[Long].selectOne
      .flatMap {
        case Some(fvId) =>
          sql"insert into change_record (session_id, file_version_id, kind) values ($sessId, $fvId, 'DELETED')".insert.unit
        case None =>
          ZIO.fail(ConstraintViolation(
            new RuntimeException(s"cannot tombstone unknown path ${rel.value}")))
      }
```

- [ ] **Step 6: Run it to verify it passes**

Run: `sbt "shell/testOnly diskrement.shell.catalog.ApplyUpdateSpec"`
Expected: `2 tests passed`.

- [ ] **Step 7: Commit**

```bash
git add shell/src/main/scala/diskrement/shell/model/CatalogUpdate.scala shell/src/main/scala/diskrement/shell/catalog/CatalogStore.scala shell/src/main/scala/diskrement/shell/catalog/CatalogStoreLive.scala shell/src/test/scala/diskrement/shell/catalog/ApplyUpdateSpec.scala
git commit -m "$(cat <<'EOF'
feat(shell): apply a CatalogUpdate in a single transaction

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 6: readView (current view, tombstone-aware)

**Files:**
- Modify: `shell/src/main/scala/diskrement/shell/catalog/CatalogStore.scala` (add `readView`)
- Modify: `shell/src/main/scala/diskrement/shell/catalog/CatalogStoreLive.scala`
- Test: `shell/src/test/scala/diskrement/shell/catalog/ReadViewSpec.scala`

- [ ] **Step 1: Write the failing test**

```scala
package diskrement.shell.catalog

import diskrement.core._
import diskrement.shell.model._
import zio._
import zio.test._
import java.time.Instant

object ReadViewSpec extends ZIOSpecDefault:
  private val init = ProjectInit("p", "/src", "/work", DiscType.BD_R, 2.0, 1,
    Instant.parse("2026-06-01T00:00:00Z"))
  private val ts = Instant.parse("2026-06-01T10:00:00Z")

  private def fw(path: String, hash: String, size: Long) =
    FileWrite(RelPath(path), ContentHash(hash), size, ts, "/data/" + path, 0)

  private def session(ord: Int, added: List[FileWrite] = Nil,
                      modified: List[FileWrite] = Nil, deleted: List[RelPath] = Nil) =
    CatalogUpdate(ProjectId(0), Nil, List(DiscWrite(
      ordinal = 1, isNew = ord == 1, discType = DiscType.BD_R, usableBytes = 1_000_000L,
      status = DiscStatus.OPEN, label = "d1",
      session = SessionWrite(ord, ts, "m" + ord, added, modified, deleted))))

  def spec = suite("CatalogStore.readView")(
    test("added files appear; a modified file supersedes; a deleted file disappears"):
      for
        pid  <- ZIO.serviceWithZIO[CatalogStore](_.createProject(init))
        store = ZIO.serviceWithZIO[CatalogStore]
        // session 1: add a, b, c
        _    <- store(_.applyUpdate(session(1,
                  added = List(fw("a", "a1", 10), fw("b", "b1", 20), fw("c", "c1", 30))).copy(projectId = pid)))
        // session 2: modify b (new hash), delete c
        _    <- store(_.applyUpdate(session(2,
                  modified = List(fw("b", "b2", 25)), deleted = List(RelPath("c"))).copy(projectId = pid)))
        view <- store(_.readView(pid))
      yield assertTrue(
        view.current.keySet.map(_.value) == Set("a", "b"),
        view.current(RelPath("a")).contentHash == ContentHash("a1"),
        view.current(RelPath("b")).contentHash == ContentHash("b2"),
        !view.current.contains(RelPath("c")))
  ).provideLayer(TestDb.layer)
```

- [ ] **Step 2: Run it to verify it fails**

Run: `sbt "shell/testOnly diskrement.shell.catalog.ReadViewSpec"`
Expected: FAIL — `readView` not found.

- [ ] **Step 3: Add `readView` to the trait**

In `catalog/CatalogStore.scala`, add to the trait (and ensure `import diskrement.core._` or specific imports are present — add `import diskrement.core.{CatalogView, DiscState}` at the top of the file):

```scala
  def readView(project: ProjectId): IO[CatalogError, CatalogView]
```

- [ ] **Step 4: Implement `readView`**

In `catalog/CatalogStoreLive.scala`, add `import diskrement.core.{CatalogEntry, CatalogView, ContentHash}` to the existing imports and add this method:

```scala
  def readView(project: ProjectId): IO[CatalogError, CatalogView] =
    tx {
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
            where l.rn = 1 and l.kind <> 'DELETED'"""
        .query[(String, String, Long)].selectAll
    }.map { rows =>
      CatalogView(rows.iterator.map { case (p, h, s) =>
        val rel = RelPath(p)
        rel -> CatalogEntry(rel, ContentHash(h), s)
      }.toMap)
    }
```

- [ ] **Step 5: Run it to verify it passes**

Run: `sbt "shell/testOnly diskrement.shell.catalog.ReadViewSpec"`
Expected: `1 tests passed`.

- [ ] **Step 6: Commit**

```bash
git add shell/src/main/scala/diskrement/shell/catalog/CatalogStore.scala shell/src/main/scala/diskrement/shell/catalog/CatalogStoreLive.scala shell/src/test/scala/diskrement/shell/catalog/ReadViewSpec.scala
git commit -m "$(cat <<'EOF'
feat(shell): read the catalog current view (tombstone-aware)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 7: readDiscStates (derived usedBytes + status)

**Files:**
- Modify: `shell/src/main/scala/diskrement/shell/catalog/CatalogStore.scala` (add `readDiscStates`)
- Modify: `shell/src/main/scala/diskrement/shell/catalog/CatalogStoreLive.scala`
- Test: `shell/src/test/scala/diskrement/shell/catalog/DiscStatesSpec.scala`

- [ ] **Step 1: Write the failing test**

```scala
package diskrement.shell.catalog

import diskrement.core._
import diskrement.shell.model._
import zio._
import zio.test._
import java.time.Instant

object DiscStatesSpec extends ZIOSpecDefault:
  private val init = ProjectInit("p", "/src", "/work", DiscType.BD_R, 2.0, 1,
    Instant.parse("2026-06-01T00:00:00Z"))
  private val ts = Instant.parse("2026-06-01T10:00:00Z")

  private def fw(path: String, size: Long) =
    FileWrite(RelPath(path), ContentHash("h-" + path), size, ts, "/data/" + path, 0)

  def spec = suite("CatalogStore.readDiscStates")(
    test("derives usedBytes from placements and reports status"):
      // disc 1: created OPEN with 30 bytes of files, then finalized; disc 2: OPEN, 10 bytes
      val u = CatalogUpdate(ProjectId(0), Nil, List(
        DiscWrite(1, isNew = true, DiscType.BD_R, 100L, DiscStatus.FINALIZED, "d1",
          SessionWrite(1, ts, "m1", List(fw("a", 10), fw("b", 20)), Nil, Nil)),
        DiscWrite(2, isNew = true, DiscType.BD_R, 100L, DiscStatus.OPEN, "d2",
          SessionWrite(1, ts, "m2", List(fw("c", 10)), Nil, Nil))))
      for
        pid    <- ZIO.serviceWithZIO[CatalogStore](_.createProject(init))
        _      <- ZIO.serviceWithZIO[CatalogStore](_.applyUpdate(u.copy(projectId = pid)))
        states <- ZIO.serviceWithZIO[CatalogStore](_.readDiscStates(pid))
      yield assertTrue(
        states.map(_.ordinal) == List(1, 2),
        states.head == DiscState(1, DiscStatus.FINALIZED, 100L, 30L),
        states(1)   == DiscState(2, DiscStatus.OPEN, 100L, 10L))
  ).provideLayer(TestDb.layer)
```

- [ ] **Step 2: Run it to verify it fails**

Run: `sbt "shell/testOnly diskrement.shell.catalog.DiscStatesSpec"`
Expected: FAIL — `readDiscStates` not found.

- [ ] **Step 3: Add `readDiscStates` to the trait**

In `catalog/CatalogStore.scala`, add to the trait:

```scala
  def readDiscStates(project: ProjectId): IO[CatalogError, List[DiscState]]
```

- [ ] **Step 4: Implement `readDiscStates`**

In `catalog/CatalogStoreLive.scala`, add `import diskrement.core.DiscState` and add:

```scala
  def readDiscStates(project: ProjectId): IO[CatalogError, List[DiscState]] =
    tx {
      sql"""select d.ordinal, d.status, d.usable_bytes,
                   coalesce((select sum(fv.size_bytes)
                             from placement p
                             join session s on s.id = p.session_id
                             join file_version fv on fv.id = p.file_version_id
                             where s.disc_id = d.id), 0) as used
            from disc d
            where d.project_id = ${project.value}
            order by d.ordinal"""
        .query[(Int, String, Long, Long)].selectAll
    }.map(_.toList.map { case (ord, status, usable, used) =>
      DiscState(ord, DiscStatus.valueOf(status), usable, used)
    })
```

- [ ] **Step 5: Run it to verify it passes**

Run: `sbt "shell/testOnly diskrement.shell.catalog.DiscStatesSpec"`
Expected: `1 tests passed`.

- [ ] **Step 6: Commit**

```bash
git add shell/src/main/scala/diskrement/shell/catalog/CatalogStore.scala shell/src/main/scala/diskrement/shell/catalog/CatalogStoreLive.scala shell/src/test/scala/diskrement/shell/catalog/DiscStatesSpec.scala
git commit -m "$(cat <<'EOF'
feat(shell): read disc states with derived usedBytes

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 8: sessionsByState read

**Files:**
- Create: `shell/src/main/scala/diskrement/shell/model/Session.scala`
- Modify: `shell/src/main/scala/diskrement/shell/catalog/CatalogStore.scala` (add `sessionsByState`)
- Modify: `shell/src/main/scala/diskrement/shell/catalog/CatalogStoreLive.scala`
- Test: `shell/src/test/scala/diskrement/shell/catalog/SessionsByStateSpec.scala`

- [ ] **Step 1: Write the failing test**

```scala
package diskrement.shell.catalog

import diskrement.core._
import diskrement.shell.model._
import zio._
import zio.test._
import java.time.Instant

object SessionsByStateSpec extends ZIOSpecDefault:
  private val init = ProjectInit("p", "/src", "/work", DiscType.BD_R, 2.0, 1,
    Instant.parse("2026-06-01T00:00:00Z"))
  private val ts = Instant.parse("2026-06-01T10:00:00Z")

  private val u = CatalogUpdate(ProjectId(0), Nil, List(
    DiscWrite(1, isNew = true, DiscType.BD_R, 100L, DiscStatus.OPEN, "d1",
      SessionWrite(1, ts, "m1",
        List(FileWrite(RelPath("a"), ContentHash("h"), 10L, ts, "/data/a", 0)), Nil, Nil))))

  def spec = suite("CatalogStore.sessionsByState")(
    test("applyUpdate records sessions as BURNED; PLANNED is empty"):
      for
        pid     <- ZIO.serviceWithZIO[CatalogStore](_.createProject(init))
        _       <- ZIO.serviceWithZIO[CatalogStore](_.applyUpdate(u.copy(projectId = pid)))
        burned  <- ZIO.serviceWithZIO[CatalogStore](_.sessionsByState(pid, SessionState.BURNED))
        planned <- ZIO.serviceWithZIO[CatalogStore](_.sessionsByState(pid, SessionState.PLANNED))
      yield assertTrue(
        burned.map(s => (s.discOrdinal, s.ordinal, s.state)) == List((1, 1, SessionState.BURNED)),
        planned.isEmpty)
  ).provideLayer(TestDb.layer)
```

- [ ] **Step 2: Run it to verify it fails**

Run: `sbt "shell/testOnly diskrement.shell.catalog.SessionsByStateSpec"`
Expected: FAIL — `SessionState`/`Session`/`sessionsByState` not found.

- [ ] **Step 3: Write the `Session` model**

`model/Session.scala`:

```scala
package diskrement.shell.model

import java.time.Instant

/** Lifecycle state of a burn session. */
enum SessionState:
  case PLANNED, BURNED, VERIFIED

/** A persisted session row, joined to its disc's ordinal for convenience. */
final case class Session(
    discOrdinal: Int,
    ordinal: Int,
    state: SessionState,
    burnedAt: Option[Instant],
    manifestHash: Option[String]
)
```

- [ ] **Step 4: Add `sessionsByState` to the trait**

In `catalog/CatalogStore.scala`, add to the trait:

```scala
  def sessionsByState(project: ProjectId, state: SessionState): IO[CatalogError, List[Session]]
```

- [ ] **Step 5: Implement `sessionsByState`**

In `catalog/CatalogStoreLive.scala`, add `import diskrement.shell.model.SessionState` (the package is already imported via `diskrement.shell.model._`; ensure `Session` resolves) and add:

```scala
  def sessionsByState(project: ProjectId, state: SessionState): IO[CatalogError, List[Session]] =
    tx {
      sql"""select d.ordinal, s.ordinal, s.state, s.burned_at, s.manifest_hash
            from session s
            join disc d on d.id = s.disc_id
            where d.project_id = ${project.value} and s.state = ${state.toString}
            order by d.ordinal, s.ordinal"""
        .query[(Int, Int, String, Option[String], Option[String])].selectAll
    }.map(_.toList.map { case (dOrd, sOrd, st, burnedAt, manifestHash) =>
      Session(dOrd, sOrd, SessionState.valueOf(st),
              burnedAt.map(Instant.parse), manifestHash)
    })
```

- [ ] **Step 6: Run it to verify it passes**

Run: `sbt "shell/testOnly diskrement.shell.catalog.SessionsByStateSpec"`
Expected: `1 tests passed`.

- [ ] **Step 7: Commit**

```bash
git add shell/src/main/scala/diskrement/shell/model/Session.scala shell/src/main/scala/diskrement/shell/catalog/CatalogStore.scala shell/src/main/scala/diskrement/shell/catalog/CatalogStoreLive.scala shell/src/test/scala/diskrement/shell/catalog/SessionsByStateSpec.scala
git commit -m "$(cat <<'EOF'
feat(shell): query sessions by lifecycle state

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 9: Append-only property test & final green sweep

**Files:**
- Test: `shell/src/test/scala/diskrement/shell/catalog/AppendOnlySpec.scala`

This property realizes parent spec §10.1: applying a sequence of scans always
leaves `readView` equal to the latest scan's live set, and `file_version` only
grows. A small test-local helper turns a core `ChangeSet` into a one-disc
`CatalogUpdate` (true burn orchestration is Plan 3).

- [ ] **Step 1: Write the property test**

```scala
package diskrement.shell.catalog

import diskrement.core._
import diskrement.shell.model._
import zio._
import zio.jdbc._
import zio.test._
import java.time.Instant

object AppendOnlySpec extends ZIOSpecDefault:
  private val init = ProjectInit("p", "/src", "/work", DiscType.BD_R, 2.0, 1,
    Instant.parse("2026-06-01T00:00:00Z"))
  private val ts = Instant.parse("2026-06-01T10:00:00Z")

  /** A scan = a set of paths each mapped to a content hash. */
  private val genScan: Gen[Any, Map[String, String]] =
    Gen.mapOf(Gen.elements("a", "b", "c", "d", "e"),
              Gen.elements("h1", "h2", "h3"))

  private val genScans: Gen[Any, List[Map[String, String]]] =
    Gen.listOfBounded(1, 6)(genScan)

  private def scannedOf(scan: Map[String, String]): List[ScannedFile] =
    scan.toList.map { case (p, h) => ScannedFile(RelPath(p), ContentHash(h), 1L, ts) }

  /** Turn a diff into a one-disc CatalogUpdate (test-local mini-orchestration). */
  private def updateOf(pid: ProjectId, ord: Int, cs: ChangeSet): CatalogUpdate =
    def fw(f: ScannedFile) =
      FileWrite(f.relPath, f.contentHash, f.sizeBytes, ts, "/data/" + f.relPath.value, 0)
    CatalogUpdate(pid, Nil, List(DiscWrite(
      ordinal = 1, isNew = ord == 1, discType = DiscType.BD_R, usableBytes = 1_000_000_000L,
      status = DiscStatus.OPEN, label = "d1",
      session = SessionWrite(ord, ts, "m" + ord,
        added = cs.added.map(fw), modified = cs.modified.map(fw), deleted = cs.deleted))))

  private def fvCount: URIO[ZConnectionPool, Int] =
    transaction(sql"select count(*) from file_version".query[Int].selectOne).map(_.getOrElse(0)).orDie

  def spec = suite("catalog append-only")(
    test("readView equals the latest scan; file_version only grows"):
      check(genScans) { scans =>
        for
          // unique project per sample: `check` runs many samples against one DB
          name <- Random.nextUUID.map("p-" + _.toString)
          pid  <- ZIO.serviceWithZIO[CatalogStore](_.createProject(init.copy(name = name)))
          // apply each scan as a session; track that file_version count never drops
          finalView <- ZIO.foldLeft(scans.zipWithIndex)
                         ((CatalogView.empty, 0)) { case ((prevView, prevCount), (scan, idx)) =>
            val scanned = scannedOf(scan)
            val cs      = Diff.diff(prevView, scanned)
            for
              _        <- ZIO.serviceWithZIO[CatalogStore](_.applyUpdate(updateOf(pid, idx + 1, cs)))
              count    <- fvCount
              _        <- ZIO.when(count < prevCount)(
                            ZIO.die(new AssertionError("file_version count shrank")))
              nextView  = CatalogView.fromScanned(scanned)
            yield (nextView, count)
          }
          (expectedView, _) = finalView
          got <- ZIO.serviceWithZIO[CatalogStore](_.readView(pid))
        yield assertTrue(
          got.current.view.mapValues(_.contentHash).toMap ==
            expectedView.current.view.mapValues(_.contentHash).toMap)
      }
  ).provideLayer(TestDb.layer) @@ TestAspect.samples(40)
```

- [ ] **Step 2: Run the property to verify it passes**

Run: `sbt "shell/testOnly diskrement.shell.catalog.AppendOnlySpec"`
Expected: `1 tests passed`. If it fails, the bug is in the store (current-view query or applyUpdate), not the property — fix the store.

> Note: each scan is applied to a fresh new disc (`ordinal = 1`, `isNew = (ord == 1)`) only for the first; for later sessions `isNew = false` reuses disc 1, so files accumulate on one open disc with a huge capacity. This keeps the test focused on the catalog (planning/spill is the core's concern, already tested in Plan 1).

- [ ] **Step 3: Run the entire suite green (both modules)**

Run: `sbt clean core/test shell/test`
Expected: core `Passed: Total 50, Failed 0`; shell all specs pass. Fix any `-Wunused:all` warnings (unused imports) before finishing.

- [ ] **Step 4: Commit**

```bash
git add shell/src/test/scala/diskrement/shell/catalog/AppendOnlySpec.scala
git commit -m "$(cat <<'EOF'
test(shell): add catalog append-only / current-view property

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review (completed by plan author)

**Spec coverage (Plan 2a design):**
- §3 module structure & one-way `shell → core` dependency → Task 1 (`build.sbt`, module).
- §4 schema (immutable fact tables + controlled-mutable rows), WAL/`synchronous=FULL` → Tasks 2 (pragmas) + 3 (DDL).
- §5 forward-migration runner → Task 3.
- §6 `CatalogStore` port: `createProject`/`findProject` → Task 4; `applyUpdate` (single transaction) → Task 5; `readView` (tombstone-aware current view, §6.1) → Task 6; `readDiscStates` (derived `usedBytes`, §6.2) → Task 7; `sessionsByState` (§6.4) → Task 8.
- §7 error model (`CatalogError` ADT, typed vs defect) → Task 4 (`CatalogError`, `tx`/`refineOrDie`).
- §8 testing: migrations, round-trips, transactional atomicity → Tasks 3–5; append-only property → Task 9.

**Correctly deferred (Plan 2b/3):** `FileSystem`, `FileScanner`, `SourceSnapshot`, `SnapshotWriter`, `SimulatedBurner`; CLI; burn orchestration that assembles `CatalogUpdate` from a `BurnPlan` + manifests; orphaned-`PLANNED`-session handling; config/XDG + systemd path resolution; full-loop integration test.

**Type consistency:** `CatalogStore` returns the unchanged core types `CatalogView` and `DiscState`; `ProjectId`/`ProjectInit`/`Project`/`CatalogError`/`CatalogUpdate`/`DiscWrite`/`SessionWrite`/`FileWrite`/`Session`/`SessionState` are new `shell.model` types, each defined once before use. `change_record.kind` reuses the core's `ManifestChangeKind` (ADDED/MODIFIED/DELETED) — wiring in the previously-unused enum. zio-jdbc API verified against v0.1.2: `ZConnectionPool.make`, `transaction`, `sql"".execute/.insert/.update`, `.query[T].selectOne/.selectAll`.

**No placeholders:** dependency versions are pinned (ZIO 2.1.19, zio-jdbc 0.1.2, sqlite-jdbc 3.49.1.0); every step has concrete code and exact commands.

---

## Next plans (separate documents)

- **Plan 2b — Filesystem & burn adapters:** `FileSystem` walk, `FileScanner` (fast-path hashing + `--verify`), `SourceSnapshot` (reflink-cp + CoW hard-fail; best-effort + skip integration testing), `SnapshotWriter` (`/.diskrement` layout + manifest JSON + embedded catalog snapshot), `SimulatedBurner`.
- **Plan 3 — CLI & full-loop integration:** the staged `init/scan/plan/burn/reset/status/verify/history` pipeline, burn orchestration that builds a `CatalogUpdate` from a `BurnPlan` + manifests, orphaned-`PLANNED`-session detection, config/XDG + systemd paths, and the scripted end-to-end integration test.
