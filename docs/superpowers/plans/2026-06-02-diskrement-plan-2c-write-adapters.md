# diskrement Plan 2c — Write-Side Adapters Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the write/output-side shell adapters — `SnapshotWriter` and `SimulatedBurner` — plus the JSON DTOs, the `ArchivePointer` model, and the `CatalogStore.snapshotTo`/`archivePointers` reads, so a planned disc can be staged to a directory as the on-disc `/.diskrement` + `/data` tree.

**Architecture:** Functional core / imperative shell. The pure `core` is unchanged and gains no dependency. New ZIO adapters live in the `shell` module under `diskrement.shell.burn`; JSON is serialized from shell-side DTOs (never by adding codecs to core types). `SimulatedBurner` composes the `Hasher` port (Plan 2b) and `SnapshotWriter`, re-hashing each file as it stages it. No CLI, prompts, or burn orchestration (Plan 3).

**Tech Stack:** Scala 3 (3.3.4 LTS), sbt, ZIO 2.1.19, zio-jdbc 0.1.2, sqlite-jdbc 3.49.1.0, **zio-json 0.7.3** (new), zio-test 2.1.19. JDK 21.

**Spec:** `docs/superpowers/specs/2026-06-02-diskrement-plan-2c-write-adapters-design.md` (and parent `2026-05-31-diskrement-core-engine-design.md` §6.2, §6.3, §9.3, §10.3).

---

## File Structure

All new code is in the `shell` module.

**Build:**
- Modify `build.sbt` — add a `zioJsonVersion` val + the `zio-json` dependency to the `shell` module.

**Main:**
- `shell/src/main/scala/diskrement/shell/model/ArchivePointer.scala` — the archive-pointer record.
- `shell/src/main/scala/diskrement/shell/burn/Burn.scala` — `DiscIdentity`, `DiscMetadata`, `DiscImage`, `BurnResult`, `BurnError`.
- `shell/src/main/scala/diskrement/shell/burn/Json.scala` — `ProjectJson`, `ManifestJson`/`ManifestEntryJson`, `PointersJson`/`ArchivePointerJson`, codecs, and mappings from domain types.
- `shell/src/main/scala/diskrement/shell/burn/SnapshotWriter.scala` — port + `SnapshotWriterLive` + layer.
- `shell/src/main/scala/diskrement/shell/burn/SimulatedBurner.scala` — port + `SimulatedBurnerLive` + layer.
- Modify `shell/src/main/scala/diskrement/shell/catalog/CatalogStore.scala` + `CatalogStoreLive.scala` — add `snapshotTo`, `archivePointers`.

**Test:**
- `shell/src/test/scala/diskrement/shell/burn/JsonSpec.scala`
- `shell/src/test/scala/diskrement/shell/catalog/SnapshotToSpec.scala`
- `shell/src/test/scala/diskrement/shell/catalog/ArchivePointersSpec.scala`
- `shell/src/test/scala/diskrement/shell/burn/SnapshotWriterSpec.scala`
- `shell/src/test/scala/diskrement/shell/burn/SimulatedBurnerSpec.scala`

---

## Environment prerequisite

Plans 2a + 2b are merged and green. Verify before starting:

```bash
sbt "shell/test"     # expect: 31 tests passed
```

---

### Task 1: zio-json dependency + burn bundles + JSON DTOs + `ArchivePointer`

**Files:**
- Modify: `build.sbt`
- Create: `shell/src/main/scala/diskrement/shell/model/ArchivePointer.scala`
- Create: `shell/src/main/scala/diskrement/shell/burn/Burn.scala`
- Create: `shell/src/main/scala/diskrement/shell/burn/Json.scala`
- Test: `shell/src/test/scala/diskrement/shell/burn/JsonSpec.scala`

- [ ] **Step 1: Add zio-json to `build.sbt`**

After the line `val sqliteVersion  = "3.49.1.0"` add:

```scala
val zioJsonVersion = "0.7.3"
```

In the `shell` module's `libraryDependencies ++= Seq(...)`, add this line (with the non-test deps, e.g. after the `sqlite-jdbc` line):

```scala
      "dev.zio"    %% "zio-json"     % zioJsonVersion,
```

(If `0.7.3` fails to resolve against ZIO 2.1.19, use the latest `0.7.x`.)

- [ ] **Step 2: Write the `ArchivePointer` model**

`shell/src/main/scala/diskrement/shell/model/ArchivePointer.scala`:

```scala
package diskrement.shell.model

import diskrement.core.{ContentHash, ManifestChangeKind, RelPath}

/** One non-current (superseded or deleted) file version and where its bytes
  * physically live. `kind` is the latest change-record kind for that version:
  * ADDED/MODIFIED for a superseded older version, DELETED for a tombstoned one. */
final case class ArchivePointer(
    relPath: RelPath,
    contentHash: ContentHash,
    sizeBytes: Long,
    discOrdinal: Int,
    sessionOrdinal: Int,
    discPath: String,
    kind: ManifestChangeKind
)
```

- [ ] **Step 3: Write the burn bundles**

`shell/src/main/scala/diskrement/shell/burn/Burn.scala`:

```scala
package diskrement.shell.burn

import diskrement.core.{ContentHash, RelPath, ScannedFile, SessionManifest}
import diskrement.shell.model.ArchivePointer
import java.nio.file.Path
import java.time.Instant

/** Project + disc identity written to project.json. */
final case class DiscIdentity(
    projectName: String,
    discOrdinal: Int,
    discCount: Int,        // the "M" in "disc N-of-M"
    sessionOrdinal: Int,
    createdAt: Instant
)

/** Everything that goes under /.diskrement for one disc. */
final case class DiscMetadata(
    identity: DiscIdentity,
    manifest: SessionManifest,
    catalogSnapshot: Path,            // a prepared catalog.sqlite (from CatalogStore.snapshotTo)
    pointers: List[ArchivePointer]
)

/** A full disc to stage. */
final case class DiscImage(
    stagingDir: Path,
    snapshotRoot: Path,               // the frozen source root (Plan 2b SnapshotHandle.root)
    files: List[ScannedFile],         // the /data payload
    metadata: DiscMetadata
)

/** Result of staging one disc. */
final case class BurnResult(stagingDir: Path, imageBytes: Long)

/** Expected burn/write failures. Mirrors CatalogError (extends Exception). */
sealed abstract class BurnError(msg: String, cause: Throwable = null)
    extends Exception(msg, cause)
object BurnError:
  final case class HashMismatch(relPath: RelPath, expected: ContentHash, actual: ContentHash)
      extends BurnError(
        s"content hash mismatch for ${relPath.value}: expected ${expected.value}, got ${actual.value}")
  final case class SourceFileMissing(relPath: RelPath)
      extends BurnError(s"source file missing from snapshot: ${relPath.value}")
  final case class BurnIoError(cause: Throwable)
      extends BurnError("burn I/O error", cause)
```

- [ ] **Step 4: Write the JSON DTOs**

`shell/src/main/scala/diskrement/shell/burn/Json.scala`:

```scala
package diskrement.shell.burn

import diskrement.core.{ManifestEntry, SessionManifest}
import diskrement.shell.model.ArchivePointer
import zio.json._

/** Mirror of DiscIdentity for project.json. */
final case class ProjectJson(
    projectName: String,
    discOrdinal: Int,
    discCount: Int,
    sessionOrdinal: Int,
    createdAt: String
)
object ProjectJson:
  given JsonCodec[ProjectJson] = DeriveJsonCodec.gen[ProjectJson]
  def from(id: DiscIdentity): ProjectJson =
    ProjectJson(id.projectName, id.discOrdinal, id.discCount, id.sessionOrdinal, id.createdAt.toString)

final case class ManifestEntryJson(relPath: String, contentHash: String, sizeBytes: Long)
object ManifestEntryJson:
  given JsonCodec[ManifestEntryJson] = DeriveJsonCodec.gen[ManifestEntryJson]
  def from(e: ManifestEntry): ManifestEntryJson =
    ManifestEntryJson(e.relPath.value, e.contentHash.value, e.sizeBytes)

/** Mirror of the core SessionManifest for manifest.json (keeps zio-json off core). */
final case class ManifestJson(
    projectName: String,
    discOrdinal: Int,
    sessionOrdinal: Int,
    added: List[ManifestEntryJson],
    modified: List[ManifestEntryJson],
    deleted: List[String]
)
object ManifestJson:
  given JsonCodec[ManifestJson] = DeriveJsonCodec.gen[ManifestJson]
  def from(m: SessionManifest): ManifestJson =
    ManifestJson(
      m.projectName, m.discOrdinal, m.sessionOrdinal,
      m.added.map(ManifestEntryJson.from), m.modified.map(ManifestEntryJson.from),
      m.deleted.map(_.value))

final case class ArchivePointerJson(
    relPath: String,
    contentHash: String,
    sizeBytes: Long,
    discOrdinal: Int,
    sessionOrdinal: Int,
    discPath: String,
    kind: String
)
object ArchivePointerJson:
  given JsonCodec[ArchivePointerJson] = DeriveJsonCodec.gen[ArchivePointerJson]
  def from(p: ArchivePointer): ArchivePointerJson =
    ArchivePointerJson(p.relPath.value, p.contentHash.value, p.sizeBytes,
      p.discOrdinal, p.sessionOrdinal, p.discPath, p.kind.toString)

final case class PointersJson(pointers: List[ArchivePointerJson])
object PointersJson:
  given JsonCodec[PointersJson] = DeriveJsonCodec.gen[PointersJson]
  def from(ps: List[ArchivePointer]): PointersJson =
    PointersJson(ps.map(ArchivePointerJson.from))
```

- [ ] **Step 5: Write the failing test**

`shell/src/test/scala/diskrement/shell/burn/JsonSpec.scala`:

```scala
package diskrement.shell.burn

import diskrement.core._
import diskrement.shell.model.ArchivePointer
import zio.json._
import zio.test._
import java.time.Instant

object JsonSpec extends ZIOSpecDefault:
  private val ts = Instant.parse("2026-06-01T00:00:00Z")

  def spec = suite("burn JSON DTOs")(
    test("ProjectJson maps from DiscIdentity and round-trips"):
      val pj = ProjectJson.from(DiscIdentity("photos", 2, 3, 1, ts))
      assertTrue(
        pj == ProjectJson("photos", 2, 3, 1, "2026-06-01T00:00:00Z"),
        pj.toJson.fromJson[ProjectJson] == Right(pj)),
    test("ManifestJson maps from SessionManifest and round-trips"):
      val m = SessionManifest("photos", 1, 1,
        added = List(ManifestEntry(RelPath("a"), ContentHash("h1"), 10)),
        modified = List(ManifestEntry(RelPath("b"), ContentHash("h2"), 20)),
        deleted = List(RelPath("c")))
      val mj = ManifestJson.from(m)
      assertTrue(
        mj.added == List(ManifestEntryJson("a", "h1", 10)),
        mj.modified == List(ManifestEntryJson("b", "h2", 20)),
        mj.deleted == List("c"),
        mj.toJson.fromJson[ManifestJson] == Right(mj)),
    test("PointersJson maps from ArchivePointers and round-trips"):
      val pj = PointersJson.from(List(
        ArchivePointer(RelPath("a"), ContentHash("h0"), 10, 1, 1, "/data/a", ManifestChangeKind.MODIFIED)))
      assertTrue(
        pj.pointers == List(ArchivePointerJson("a", "h0", 10, 1, 1, "/data/a", "MODIFIED")),
        pj.toJson.fromJson[PointersJson] == Right(pj))
  )
```

- [ ] **Step 6: Run it to verify it fails, then passes**

Run: `sbt "shell/testOnly diskrement.shell.burn.JsonSpec"`
First expect a compile failure (types not found / zio-json not resolved). After Steps 1–4 are in place it should report `3 tests passed`. Then run `sbt "shell/test"` → `34 tests passed`.

- [ ] **Step 7: Commit**

```bash
git add build.sbt shell/src/main/scala/diskrement/shell/model/ArchivePointer.scala shell/src/main/scala/diskrement/shell/burn/Burn.scala shell/src/main/scala/diskrement/shell/burn/Json.scala shell/src/test/scala/diskrement/shell/burn/JsonSpec.scala
git commit -m "$(cat <<'EOF'
feat(shell): add burn bundles, ArchivePointer, and JSON DTOs (zio-json)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: `CatalogStore.snapshotTo` (VACUUM INTO)

**Files:**
- Modify: `shell/src/main/scala/diskrement/shell/catalog/CatalogStore.scala`
- Modify: `shell/src/main/scala/diskrement/shell/catalog/CatalogStoreLive.scala`
- Test: `shell/src/test/scala/diskrement/shell/catalog/SnapshotToSpec.scala`

- [ ] **Step 1: Write the failing test**

`shell/src/test/scala/diskrement/shell/catalog/SnapshotToSpec.scala`:

```scala
package diskrement.shell.catalog

import diskrement.core._
import diskrement.shell.model._
import zio._
import zio.test._
import java.nio.file.Files
import java.time.Instant

object SnapshotToSpec extends ZIOSpecDefault:
  private val init = ProjectInit("p", "/src", "/work", DiscType.BD_R, 2.0, 1,
    Instant.parse("2026-06-01T00:00:00Z"))
  private val ts = Instant.parse("2026-06-01T10:00:00Z")
  private def fw(path: String, hash: String, size: Long) =
    FileWrite(RelPath(path), ContentHash(hash), size, ts, "/data/" + path, 0)
  private val u = CatalogUpdate(ProjectId(0), Nil, List(DiscWrite(
    1, isNew = true, DiscType.BD_R, 100L, DiscStatus.OPEN, "d1",
    SessionWrite(1, ts, "m1", List(fw("a", "a1", 10), fw("b", "b1", 20)), Nil, Nil))))

  def spec = suite("CatalogStore.snapshotTo")(
    test("VACUUM INTO produces a self-contained DB with the source's rows"):
      for
        pid  <- ZIO.serviceWithZIO[CatalogStore](_.createProject(init))
        _    <- ZIO.serviceWithZIO[CatalogStore](_.applyUpdate(u.copy(projectId = pid)))
        dest <- ZIO.attempt(Files.createTempDirectory("snap-to").resolve("snapshot.sqlite"))
        _    <- ZIO.serviceWithZIO[CatalogStore](_.snapshotTo(dest))
        count <- ZIO.attempt {
                   val c = java.sql.DriverManager.getConnection(s"jdbc:sqlite:$dest")
                   try
                     val rs = c.createStatement().executeQuery("select count(*) from file_version")
                     rs.next(); rs.getInt(1)
                   finally c.close()
                 }
      yield assertTrue(Files.exists(dest), count == 2)
  ).provideLayer(TestDb.layer)
```

- [ ] **Step 2: Run it to verify it fails**

Run: `sbt "shell/testOnly diskrement.shell.catalog.SnapshotToSpec"`
Expected: FAIL — `snapshotTo` is not a member of `CatalogStore`.

- [ ] **Step 3: Add `snapshotTo` to the trait**

In `CatalogStore.scala`, add `import java.nio.file.Path` near the top, and add to the `trait CatalogStore`:

```scala
  def snapshotTo(dest: Path): IO[CatalogError, Unit]
```

- [ ] **Step 4: Implement `snapshotTo`**

In `CatalogStoreLive.scala`, add `import java.nio.file.Path` and add this method. It must NOT go through the `tx` helper: `tx` sets `autoCommit(false)`, but SQLite forbids `VACUUM` inside a transaction. Running a bare `transaction(...)` here keeps the connection in autocommit mode (zio-jdbc 0.1.2 does not disable autocommit on its own — that is exactly why `tx` adds it), so the `VACUUM INTO` executes outside any transaction.

```scala
  def snapshotTo(dest: Path): IO[CatalogError, Unit] =
    transaction(sql"vacuum into ${dest.toString}".execute.unit)
      .provideEnvironment(ZEnvironment(pool))
      .refineOrDie {
        case e: CatalogError                             => e
        case e: java.sql.SQLException if isConstraint(e) => ConstraintViolation(e)
      }
```

If sqlite-jdbc rejects the bound parameter in `VACUUM INTO ?` (error mentioning `near "?"`), replace the fragment with an escaped single-quoted literal: `SqlFragment("vacuum into '" + dest.toString.replace("'", "''") + "'")` — but try the bound-parameter form first.

- [ ] **Step 5: Run it to verify it passes**

Run: `sbt "shell/testOnly diskrement.shell.catalog.SnapshotToSpec"`
Expected: `1 tests passed`.

- [ ] **Step 6: Commit**

```bash
git add shell/src/main/scala/diskrement/shell/catalog/CatalogStore.scala shell/src/main/scala/diskrement/shell/catalog/CatalogStoreLive.scala shell/src/test/scala/diskrement/shell/catalog/SnapshotToSpec.scala
git commit -m "$(cat <<'EOF'
feat(shell): add CatalogStore.snapshotTo (VACUUM INTO full snapshot)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: `CatalogStore.archivePointers`

**Files:**
- Modify: `shell/src/main/scala/diskrement/shell/catalog/CatalogStore.scala`
- Modify: `shell/src/main/scala/diskrement/shell/catalog/CatalogStoreLive.scala`
- Test: `shell/src/test/scala/diskrement/shell/catalog/ArchivePointersSpec.scala`

- [ ] **Step 1: Write the failing test**

`shell/src/test/scala/diskrement/shell/catalog/ArchivePointersSpec.scala`:

```scala
package diskrement.shell.catalog

import diskrement.core._
import diskrement.shell.model._
import zio._
import zio.test._
import java.time.Instant

object ArchivePointersSpec extends ZIOSpecDefault:
  private val init = ProjectInit("p", "/src", "/work", DiscType.BD_R, 2.0, 1,
    Instant.parse("2026-06-01T00:00:00Z"))
  private val ts = Instant.parse("2026-06-01T10:00:00Z")
  private def fw(path: String, hash: String, size: Long) =
    FileWrite(RelPath(path), ContentHash(hash), size, ts, "/data/" + path, 0)
  private def session(ord: Int, added: List[FileWrite] = Nil,
                      modified: List[FileWrite] = Nil, deleted: List[RelPath] = Nil) =
    CatalogUpdate(ProjectId(0), Nil, List(DiscWrite(
      1, isNew = ord == 1, DiscType.BD_R, 1_000_000L, DiscStatus.OPEN, "d1",
      SessionWrite(ord, ts, "m" + ord, added, modified, deleted))))

  def spec = suite("CatalogStore.archivePointers")(
    test("returns superseded + deleted versions with locations; excludes live"):
      for
        pid <- ZIO.serviceWithZIO[CatalogStore](_.createProject(init))
        st   = ZIO.serviceWithZIO[CatalogStore]
        // session 1: add a(a1,10), b(b1,20), c(c1,30)
        _   <- st(_.applyUpdate(session(1,
                 added = List(fw("a","a1",10), fw("b","b1",20), fw("c","c1",30))).copy(projectId = pid)))
        // session 2: modify b -> b2, delete c
        _   <- st(_.applyUpdate(session(2,
                 modified = List(fw("b","b2",25)), deleted = List(RelPath("c"))).copy(projectId = pid)))
        ps  <- st(_.archivePointers(pid))
      yield assertTrue(
        ps.toSet == Set(
          ArchivePointer(RelPath("b"), ContentHash("b1"), 20, 1, 1, "/data/b", ManifestChangeKind.ADDED),
          ArchivePointer(RelPath("c"), ContentHash("c1"), 30, 1, 1, "/data/c", ManifestChangeKind.DELETED)))
  ).provideLayer(TestDb.layer)
```

- [ ] **Step 2: Run it to verify it fails**

Run: `sbt "shell/testOnly diskrement.shell.catalog.ArchivePointersSpec"`
Expected: FAIL — `archivePointers` is not a member of `CatalogStore`.

- [ ] **Step 3: Add `archivePointers` to the trait**

In `CatalogStore.scala`, add to the `trait CatalogStore` (`ArchivePointer` resolves via the existing `import diskrement.shell.model._`):

```scala
  def archivePointers(project: ProjectId): IO[CatalogError, List[ArchivePointer]]
```

- [ ] **Step 4: Implement `archivePointers`**

In `CatalogStoreLive.scala`, add this method. It returns one pointer per file version that is NOT the current live version of its path, joined to its placement (where its bytes physically live), with `kind` = that version's latest change-record kind (ADDED/MODIFIED for a superseded version, DELETED for a tombstoned one). `ManifestChangeKind` and `ArchivePointer` are already importable (core import + `model._`).

```scala
  def archivePointers(project: ProjectId): IO[CatalogError, List[ArchivePointer]] =
    tx {
      sql"""select fv.rel_path, fv.content_hash, fv.size_bytes,
                   d.ordinal, s.ordinal, p.disc_path, fvk.kind
            from file_version fv
            join placement p on p.file_version_id = fv.id
            join session s   on s.id = p.session_id
            join disc d      on d.id = s.disc_id
            join (
              select cr.file_version_id as fvid, cr.kind as kind,
                     row_number() over (partition by cr.file_version_id order by cr.id desc) as rn
              from change_record cr
            ) fvk on fvk.fvid = fv.id and fvk.rn = 1
            where fv.project_id = ${project.value}
              and fv.id not in (
                select fvid from (
                  select cr.file_version_id as fvid, cr.kind as kind,
                         row_number() over (partition by fv2.rel_path order by cr.id desc) as rn
                  from change_record cr
                  join file_version fv2 on fv2.id = cr.file_version_id
                  where fv2.project_id = ${project.value}
                ) cur
                where cur.rn = 1 and cur.kind <> 'DELETED'
              )
            order by fv.rel_path, d.ordinal, s.ordinal"""
        .query[(String, String, Long, Int, Int, String, String)].selectAll
    }.map(_.toList.map { case (rp, ch, sz, dOrd, sOrd, dpath, kind) =>
      ArchivePointer(RelPath(rp), ContentHash(ch), sz, dOrd, sOrd, dpath, ManifestChangeKind.valueOf(kind))
    })
```

- [ ] **Step 5: Run it to verify it passes**

Run: `sbt "shell/testOnly diskrement.shell.catalog.ArchivePointersSpec"`
Expected: `1 tests passed`.

- [ ] **Step 6: Commit**

```bash
git add shell/src/main/scala/diskrement/shell/catalog/CatalogStore.scala shell/src/main/scala/diskrement/shell/catalog/CatalogStoreLive.scala shell/src/test/scala/diskrement/shell/catalog/ArchivePointersSpec.scala
git commit -m "$(cat <<'EOF'
feat(shell): add CatalogStore.archivePointers (superseded/deleted index)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: `SnapshotWriter`

**Files:**
- Create: `shell/src/main/scala/diskrement/shell/burn/SnapshotWriter.scala`
- Test: `shell/src/test/scala/diskrement/shell/burn/SnapshotWriterSpec.scala`

Depends on Task 1 (`DiscMetadata`, JSON DTOs).

- [ ] **Step 1: Write the failing test**

`shell/src/test/scala/diskrement/shell/burn/SnapshotWriterSpec.scala`:

```scala
package diskrement.shell.burn

import diskrement.core._
import diskrement.shell.model.ArchivePointer
import zio._
import zio.json._
import zio.test._
import java.nio.file.{Files, Path}
import java.time.Instant

object SnapshotWriterSpec extends ZIOSpecDefault:
  private val ts = Instant.parse("2026-06-01T00:00:00Z")

  private def deleteRecursive(dir: Path): Unit =
    val s = Files.walk(dir)
    try s.sorted(java.util.Comparator.reverseOrder()).forEach(p => Files.deleteIfExists(p))
    finally s.close()

  def spec = suite("SnapshotWriter")(
    test("writes the four /.diskrement files with correct content"):
      ZIO.scoped {
        for
          tmp <- ZIO.acquireRelease(ZIO.attempt(Files.createTempDirectory("snapwriter")))(
                   d => ZIO.attempt(deleteRecursive(d)).orDie)
          catSrc = tmp.resolve("src-catalog.sqlite")
          _   <- ZIO.attempt(Files.write(catSrc, Array[Byte](1, 2, 3, 4)))
          ddir = tmp.resolve("disc/.diskrement")
          meta = DiscMetadata(
                   identity = DiscIdentity("photos", 1, 1, 1, ts),
                   manifest = SessionManifest("photos", 1, 1,
                     added = List(ManifestEntry(RelPath("a"), ContentHash("h1"), 10)),
                     modified = Nil, deleted = Nil),
                   catalogSnapshot = catSrc,
                   pointers = List(ArchivePointer(
                     RelPath("b"), ContentHash("h0"), 5, 1, 1, "/data/b", ManifestChangeKind.ADDED)))
          _   <- ZIO.serviceWithZIO[SnapshotWriter](_.write(ddir, meta))
          pjson  <- ZIO.attempt(Files.readString(ddir.resolve("project.json")))
          mjson  <- ZIO.attempt(Files.readString(ddir.resolve("manifest.json")))
          ptjson <- ZIO.attempt(Files.readString(ddir.resolve("pointers.json")))
          catB   <- ZIO.attempt(Files.readAllBytes(ddir.resolve("catalog.sqlite")).toList)
        yield assertTrue(
          pjson.fromJson[ProjectJson] == Right(ProjectJson("photos", 1, 1, 1, "2026-06-01T00:00:00Z")),
          mjson.fromJson[ManifestJson] == Right(ManifestJson.from(meta.manifest)),
          ptjson.fromJson[PointersJson] == Right(PointersJson.from(meta.pointers)),
          catB == List[Byte](1, 2, 3, 4))
      }
  ).provideLayer(SnapshotWriter.live)
```

- [ ] **Step 2: Run it to verify it fails**

Run: `sbt "shell/testOnly diskrement.shell.burn.SnapshotWriterSpec"`
Expected: FAIL — `SnapshotWriter` not found.

- [ ] **Step 3: Write `SnapshotWriter.scala`**

`shell/src/main/scala/diskrement/shell/burn/SnapshotWriter.scala`:

```scala
package diskrement.shell.burn

import zio._
import zio.json._
import java.nio.file.{Files, Path, StandardCopyOption}

/** Writes the /.diskrement metadata for one disc. */
trait SnapshotWriter:
  def write(diskrementDir: Path, meta: DiscMetadata): IO[BurnError, Unit]

final case class SnapshotWriterLive() extends SnapshotWriter:
  def write(diskrementDir: Path, meta: DiscMetadata): IO[BurnError, Unit] =
    ZIO.attemptBlockingIO {
      Files.createDirectories(diskrementDir)
      Files.writeString(diskrementDir.resolve("project.json"),
        ProjectJson.from(meta.identity).toJsonPretty)
      Files.writeString(diskrementDir.resolve("manifest.json"),
        ManifestJson.from(meta.manifest).toJsonPretty)
      Files.writeString(diskrementDir.resolve("pointers.json"),
        PointersJson.from(meta.pointers).toJsonPretty)
      Files.copy(meta.catalogSnapshot, diskrementDir.resolve("catalog.sqlite"),
        StandardCopyOption.REPLACE_EXISTING)
      ()
    }.mapError(BurnError.BurnIoError(_))

object SnapshotWriter:
  val live: ZLayer[Any, Nothing, SnapshotWriter] = ZLayer.succeed(SnapshotWriterLive())
```

- [ ] **Step 4: Run it to verify it passes**

Run: `sbt "shell/testOnly diskrement.shell.burn.SnapshotWriterSpec"`
Expected: `1 tests passed`.

- [ ] **Step 5: Commit**

```bash
git add shell/src/main/scala/diskrement/shell/burn/SnapshotWriter.scala shell/src/test/scala/diskrement/shell/burn/SnapshotWriterSpec.scala
git commit -m "$(cat <<'EOF'
feat(shell): add SnapshotWriter for the /.diskrement metadata layout

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: `SimulatedBurner`

**Files:**
- Create: `shell/src/main/scala/diskrement/shell/burn/SimulatedBurner.scala`
- Test: `shell/src/test/scala/diskrement/shell/burn/SimulatedBurnerSpec.scala`

Depends on Task 1 (bundles), Task 4 (`SnapshotWriter`), and the Plan 2b `Hasher` port.

- [ ] **Step 1: Write the failing test**

`shell/src/test/scala/diskrement/shell/burn/SimulatedBurnerSpec.scala`:

```scala
package diskrement.shell.burn

import diskrement.core._
import diskrement.shell.fs.Hasher
import zio._
import zio.test._
import java.nio.file.{Files, Path}
import java.time.Instant

object SimulatedBurnerSpec extends ZIOSpecDefault:
  private val ts = Instant.parse("2026-06-01T00:00:00Z")

  private def sha256(s: String): String =
    java.security.MessageDigest.getInstance("SHA-256")
      .digest(s.getBytes("UTF-8")).map(b => f"${b & 0xff}%02x").mkString

  private def deleteRecursive(dir: Path): Unit =
    val st = Files.walk(dir)
    try st.sorted(java.util.Comparator.reverseOrder()).forEach(p => Files.deleteIfExists(p))
    finally st.close()

  /** base/{snapshot/{a.txt,sub/b.txt}, catalog-snapshot.sqlite} + a staging path. */
  private val makeWorld: ZIO[Scope, Throwable, (Path, Path, Path)] =
    ZIO.acquireRelease(ZIO.attempt {
      val base = Files.createTempDirectory("burnspec")
      val snap = Files.createDirectories(base.resolve("snapshot"))
      Files.writeString(snap.resolve("a.txt"), "aaa")
      Files.createDirectories(snap.resolve("sub"))
      Files.writeString(snap.resolve("sub/b.txt"), "bbbb")
      val cat = base.resolve("catalog-snapshot.sqlite")
      Files.write(cat, Array[Byte](9, 9))
      (snap, base.resolve("staging"), cat)
    })(t => ZIO.attempt(deleteRecursive(t._1.getParent)).orDie)

  private def meta(cat: Path, files: List[ScannedFile]) =
    DiscMetadata(
      DiscIdentity("p", 1, 1, 1, ts),
      SessionManifest("p", 1, 1,
        files.map(f => ManifestEntry(f.relPath, f.contentHash, f.sizeBytes)), Nil, Nil),
      cat, Nil)

  def spec = suite("SimulatedBurner")(
    test("stages /data + /.diskrement and computes the image size"):
      ZIO.scoped {
        for
          (snap, staging, cat) <- makeWorld
          files = List(
            ScannedFile(RelPath("a.txt"), ContentHash(sha256("aaa")), 3, ts),
            ScannedFile(RelPath("sub/b.txt"), ContentHash(sha256("bbbb")), 4, ts))
          image = DiscImage(staging, snap, files, meta(cat, files))
          res   <- ZIO.serviceWithZIO[SimulatedBurner](_.burn(image))
          aTxt  <- ZIO.attempt(Files.readString(staging.resolve("data/a.txt")))
          bTxt  <- ZIO.attempt(Files.readString(staging.resolve("data/sub/b.txt")))
          hasManifest <- ZIO.attempt(Files.exists(staging.resolve(".diskrement/manifest.json")))
          hasCatalog  <- ZIO.attempt(Files.exists(staging.resolve(".diskrement/catalog.sqlite")))
        yield assertTrue(aTxt == "aaa", bTxt == "bbbb", hasManifest, hasCatalog, res.imageBytes > 7L)
      },
    test("aborts with HashMismatch when bytes don't match the expected hash"):
      ZIO.scoped {
        for
          (snap, staging, cat) <- makeWorld
          files = List(ScannedFile(RelPath("a.txt"), ContentHash("deadbeef"), 3, ts))
          image = DiscImage(staging, snap, files, meta(cat, files))
          err   <- ZIO.serviceWithZIO[SimulatedBurner](_.burn(image)).flip
        yield assertTrue(err.isInstanceOf[BurnError.HashMismatch])
      },
    test("aborts with SourceFileMissing when a planned file is absent"):
      ZIO.scoped {
        for
          (snap, staging, cat) <- makeWorld
          files = List(ScannedFile(RelPath("ghost.txt"), ContentHash(sha256("x")), 1, ts))
          image = DiscImage(staging, snap, files, meta(cat, files))
          err   <- ZIO.serviceWithZIO[SimulatedBurner](_.burn(image)).flip
        yield assertTrue(err.isInstanceOf[BurnError.SourceFileMissing])
      }
  ).provideLayer((Hasher.live ++ SnapshotWriter.live) >>> SimulatedBurner.live)
```

- [ ] **Step 2: Run it to verify it fails**

Run: `sbt "shell/testOnly diskrement.shell.burn.SimulatedBurnerSpec"`
Expected: FAIL — `SimulatedBurner` not found.

- [ ] **Step 3: Write `SimulatedBurner.scala`**

`shell/src/main/scala/diskrement/shell/burn/SimulatedBurner.scala`:

```scala
package diskrement.shell.burn

import diskrement.core.ScannedFile
import diskrement.shell.fs.Hasher
import zio._
import java.nio.file.{Files, Path}
import scala.jdk.StreamConverters.*

/** Stages a full disc (/data + /.diskrement) to a directory, re-hashing each
  * file as it copies it (§6.2). Non-interactive; prompts are a Plan 3 concern. */
trait SimulatedBurner:
  def burn(image: DiscImage): IO[BurnError, BurnResult]

final case class SimulatedBurnerLive(hasher: Hasher, writer: SnapshotWriter) extends SimulatedBurner:
  def burn(image: DiscImage): IO[BurnError, BurnResult] =
    for
      _     <- clearAndCreate(image.stagingDir)
      _     <- ZIO.foreachDiscard(image.files)(copyOne(image, _))
      _     <- writer.write(image.stagingDir.resolve(".diskrement"), image.metadata)
      bytes <- totalBytes(image.stagingDir)
    yield BurnResult(image.stagingDir, bytes)

  private def clearAndCreate(dir: Path): IO[BurnError, Unit] =
    ZIO.attemptBlockingIO {
      if Files.exists(dir) then
        val s = Files.walk(dir)
        try s.sorted(java.util.Comparator.reverseOrder()).forEach(p => Files.deleteIfExists(p))
        finally s.close()
      Files.createDirectories(dir)
      ()
    }.mapError(BurnError.BurnIoError(_))

  private def copyOne(image: DiscImage, f: ScannedFile): IO[BurnError, Unit] =
    val src = image.snapshotRoot.resolve(f.relPath.value)
    val dst = image.stagingDir.resolve("data").resolve(f.relPath.value)
    for
      exists <- ZIO.attemptBlockingIO(Files.exists(src)).mapError(BurnError.BurnIoError(_))
      _      <- ZIO.unless(exists)(ZIO.fail(BurnError.SourceFileMissing(f.relPath)))
      _      <- ZIO.attemptBlockingIO {
                  Files.createDirectories(dst.getParent)
                  Files.copy(src, dst)
                  ()
                }.mapError(BurnError.BurnIoError(_))
      actual <- hasher.hash(dst).mapError(BurnError.BurnIoError(_))
      _      <- ZIO.unless(actual == f.contentHash)(
                  ZIO.fail(BurnError.HashMismatch(f.relPath, f.contentHash, actual)))
    yield ()

  private def totalBytes(dir: Path): IO[BurnError, Long] =
    ZIO.attemptBlockingIO {
      val s = Files.walk(dir)
      try s.toScala(List).filter(p => Files.isRegularFile(p)).map(p => Files.size(p)).sum
      finally s.close()
    }.mapError(BurnError.BurnIoError(_))

object SimulatedBurner:
  val live: ZLayer[Hasher & SnapshotWriter, Nothing, SimulatedBurner] =
    ZLayer {
      for
        hasher <- ZIO.service[Hasher]
        writer <- ZIO.service[SnapshotWriter]
      yield SimulatedBurnerLive(hasher, writer)
    }
```

- [ ] **Step 4: Run it to verify it passes**

Run: `sbt "shell/testOnly diskrement.shell.burn.SimulatedBurnerSpec"`
Expected: `3 tests passed`.

- [ ] **Step 5: Commit**

```bash
git add shell/src/main/scala/diskrement/shell/burn/SimulatedBurner.scala shell/src/test/scala/diskrement/shell/burn/SimulatedBurnerSpec.scala
git commit -m "$(cat <<'EOF'
feat(shell): add SimulatedBurner staging discs with re-hash verify

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 6: Final green sweep

**Files:** none (verification only).

- [ ] **Step 1: Run the entire suite green (both modules)**

Run: `sbt clean core/test shell/test`
Expected: core `Passed: Total 50, Failed 0`; shell `40 tests passed` (31 from Plan 2b + Task 1 `3` + Task 2 `1` + Task 3 `1` + Task 4 `1` + Task 5 `3`). Fix any `-Wunused:all` warnings before finishing.

- [ ] **Step 2: Confirm no uncommitted changes**

Run: `git status --porcelain`
Expected: empty (all work committed across Tasks 1–5). If the sweep required a fix, commit it.

---

## Self-Review (completed by plan author)

**Spec coverage (Plan 2c design):**
- §3.1 `ArchivePointer` → Task 1.
- §3.2 burn bundles (`DiscIdentity`/`DiscMetadata`/`DiscImage`/`BurnResult`), §3.3 `BurnError` → Task 1.
- §3.6 JSON DTOs + codecs + mappings (shell-side, core untouched — decision §2.1.1) → Task 1.
- §3.7 `snapshotTo` (VACUUM INTO, outside a transaction — decision §2.1.2) → Task 2; `archivePointers` → Task 3.
- §3.4 `SnapshotWriter` (four `/.diskrement` files, catalog copy) → Task 4.
- §3.5 `SimulatedBurner` (stage `/data`, re-hash-on-write abort §6.2, delegate `/.diskrement`, image size; non-interactive — decision §2.1.3) → Task 5.
- §6 zio-json dependency → Task 1. §8 testing → the test in each task.

**Correctly deferred (Plan 3):** CLI + scripted insert/label prompts; burn orchestration assembling `DiscImage`s from a `BurnPlan` + manifests; catalog-update vs. snapshot timing and `PLANNED→BURNED→VERIFIED` crash-safety ordering; real ISO/UDF + hardware; real `/.archive` byte-linking; full-loop integration test.

**Type consistency:** `DiscIdentity`/`DiscMetadata`/`DiscImage`/`BurnResult`/`BurnError` defined in Task 1 and consumed unchanged in Tasks 4–5. `ArchivePointer(relPath, contentHash, sizeBytes, discOrdinal, sessionOrdinal, discPath, kind)` defined in Task 1, produced by `archivePointers` (Task 3) and serialized by `PointersJson.from` (Task 1). JSON DTO field names/order are stable across the mapping functions and their tests. `SnapshotWriter.write(Path, DiscMetadata)` and `SimulatedBurner.burn(DiscImage)` signatures match their layers and tests. `snapshotTo(Path)`/`archivePointers(ProjectId)` match the trait and `CatalogStoreLive`. The `archivePointers` SQL reuses the verified latest-change-per-path window pattern from `readView`/`fingerprints`.

**No placeholders:** every step has concrete code + exact commands. The one runtime risk (bound parameter in `VACUUM INTO`) has a concrete primary form and a concrete fallback in Task 2 Step 4. zio-json pinned to `0.7.3` with a fallback note.
