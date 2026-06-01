# diskrement Core Engine — Plan 1: Functional Core Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the pure, deterministic functional core of diskrement — the domain model, the scan `Diff`, the disc `Planner`, and the manifest builder — fully covered by example and property tests, with zero I/O.

**Architecture:** Functional core / imperative shell. This plan implements only the *core* (pure functions over immutable data). No filesystem, SQLite, snapshot, or CLI code — those are Plans 2 and 3. Every function here takes data in and returns data out, making the whole module property-testable without any fakes.

**Tech Stack:** Scala 3 (3.3.4 LTS), sbt 1.10.x, munit 1.0.0 + munit-scalacheck + ScalaCheck 1.18.0 for tests. JDK 21.

**Spec:** `docs/superpowers/specs/2026-05-31-diskrement-core-engine-design.md` — this plan implements §5 (domain model, the pure subset), §7 (Diff), §8 (Planner + the 8 invariants), and the manifest-builder data structure from §4.3 / §6.3. Persistence, snapshots, and CLI are explicitly out of scope here.

---

## Environment prerequisite

The executor needs **JDK 21** and **sbt** on the PATH. This repo's host currently has neither. On Arch Linux:

```bash
sudo pacman -S --needed jdk21-openjdk sbt
java -version   # expect: openjdk version "21..."
sbt --version   # expect: sbt runner version 1.x
```

If you cannot install system-wide, use Coursier: `cs setup --jvm 21` then `cs install sbt`. Do not proceed to Task 1 until `java -version` and `sbt --version` both succeed.

---

## File Structure

All core code lives under a single sbt sub-module `core`, package `diskrement.core`.

**Build:**
- Create: `build.sbt` — sbt build definition, one `core` module.
- Create: `project/build.properties` — pins sbt version.

**Main (`core/src/main/scala/diskrement/core/`):**
- `DiscType.scala` — `DiscType` enum (nominal capacities) + `Capacity` + margin math.
- `Model.scala` — `RelPath`, `ContentHash`, `ScannedFile`, `CatalogEntry`, `CatalogView`.
- `ChangeSet.scala` — the `ChangeSet` result type.
- `Diff.scala` — the pure `Diff.diff` function.
- `Planner.scala` — `DiscStatus`, `DiscState`, `PlannedDisc`, `BurnPlan`, `Planner.plan`.
- `Manifest.scala` — `ManifestEntry`, `ManifestChangeKind`, `SessionManifest`, `ManifestBuilder`.

**Test (`core/src/test/scala/diskrement/core/`):**
- `SmokeTest.scala`, `DiscTypeTest.scala`, `DiffTest.scala`, `DiffPropertyTest.scala`,
  `PlannerTest.scala`, `PlannerPropertyTest.scala`, `ManifestBuilderTest.scala`.
- `Generators.scala` — shared ScalaCheck generators.

Each file has one clear responsibility; planner types live with the planner because they change together.

---

### Task 1: Project scaffolding & toolchain smoke test

**Files:**
- Create: `build.sbt`
- Create: `project/build.properties`
- Test: `core/src/test/scala/diskrement/core/SmokeTest.scala`

- [ ] **Step 1: Create `project/build.properties`**

```properties
sbt.version=1.10.2
```

- [ ] **Step 2: Create `build.sbt`**

```scala
ThisBuild / scalaVersion := "3.3.4"
ThisBuild / organization := "io.github.zayatura.diskrement"
ThisBuild / scalacOptions ++= Seq("-deprecation", "-feature", "-Wunused:all")

lazy val core = (project in file("core"))
  .settings(
    name := "diskrement-core",
    libraryDependencies ++= Seq(
      "org.scalameta"  %% "munit"            % "1.0.0"  % Test,
      "org.scalameta"  %% "munit-scalacheck" % "1.0.0"  % Test,
      "org.scalacheck" %% "scalacheck"       % "1.18.0" % Test
    )
  )

lazy val root = (project in file("."))
  .aggregate(core)
  .settings(name := "diskrement")
```

- [ ] **Step 3: Write the failing smoke test**

```scala
package diskrement.core

class SmokeTest extends munit.FunSuite:
  test("toolchain compiles and runs a test"):
    assertEquals(1 + 1, 2)
```

- [ ] **Step 4: Run it to verify the toolchain works**

Run: `sbt "core/testOnly diskrement.core.SmokeTest"`
Expected: downloads dependencies on first run, then `Tests: 1, Passed: 1`. (If this fails to resolve dependencies, fix the build before continuing.)

- [ ] **Step 5: Commit**

```bash
git add build.sbt project/build.properties core/src/test/scala/diskrement/core/SmokeTest.scala
git commit -m "$(cat <<'EOF'
chore: scaffold sbt build and core module

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: DiscType and Capacity (with safety margin)

**Files:**
- Create: `core/src/main/scala/diskrement/core/DiscType.scala`
- Test: `core/src/test/scala/diskrement/core/DiscTypeTest.scala`

- [ ] **Step 1: Write the failing test**

```scala
package diskrement.core

class DiscTypeTest extends munit.FunSuite:
  test("disc types expose nominal capacities"):
    assertEquals(DiscType.BD_R.nominalBytes, 25_000_000_000L)
    assertEquals(DiscType.BD_R_DL.nominalBytes, 50_000_000_000L)
    assertEquals(DiscType.DVD.nominalBytes, 4_700_000_000L)
    assertEquals(DiscType.DVD_DL.nominalBytes, 8_500_000_000L)

  test("Capacity.fromDiscType subtracts the margin percent and floors"):
    val cap = Capacity.fromDiscType(DiscType.BD_R, marginPercent = 2.0)
    assertEquals(cap.usableBytes, 24_500_000_000L)

  test("zero margin yields nominal capacity"):
    assertEquals(
      Capacity.fromDiscType(DiscType.DVD, 0.0).usableBytes,
      4_700_000_000L
    )
```

- [ ] **Step 2: Run test to verify it fails**

Run: `sbt "core/testOnly diskrement.core.DiscTypeTest"`
Expected: FAIL — compilation error, `DiscType`/`Capacity` not found.

- [ ] **Step 3: Write minimal implementation**

```scala
package diskrement.core

/** Physical optical media types and their nominal byte capacities. */
enum DiscType(val nominalBytes: Long):
  case BD_R    extends DiscType(25_000_000_000L)
  case BD_R_DL extends DiscType(50_000_000_000L)
  case DVD     extends DiscType(4_700_000_000L)
  case DVD_DL  extends DiscType(8_500_000_000L)

/** Usable bytes on a disc after subtracting filesystem/UDF overhead margin. */
final case class Capacity(usableBytes: Long)

object Capacity:
  /** Nominal capacity reduced by `marginPercent` (e.g. 2.0 == 2%), floored. */
  def fromDiscType(discType: DiscType, marginPercent: Double): Capacity =
    val usable = math.floor(discType.nominalBytes * (1.0 - marginPercent / 100.0))
    Capacity(usable.toLong)
```

- [ ] **Step 4: Run test to verify it passes**

Run: `sbt "core/testOnly diskrement.core.DiscTypeTest"`
Expected: `Tests: 3, Passed: 3`.

- [ ] **Step 5: Commit**

```bash
git add core/src/main/scala/diskrement/core/DiscType.scala core/src/test/scala/diskrement/core/DiscTypeTest.scala
git commit -m "$(cat <<'EOF'
feat(core): add DiscType and Capacity with margin math

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Core model types (RelPath, ContentHash, ScannedFile, CatalogView)

**Files:**
- Create: `core/src/main/scala/diskrement/core/Model.scala`
- Test: `core/src/test/scala/diskrement/core/ModelTest.scala`

- [ ] **Step 1: Write the failing test**

```scala
package diskrement.core

import java.time.Instant

class ModelTest extends munit.FunSuite:
  test("RelPath orders lexicographically by value"):
    val sorted = List(RelPath("b/a"), RelPath("a/z"), RelPath("a/a"))
      .sorted
      .map(_.value)
    assertEquals(sorted, List("a/a", "a/z", "b/a"))

  test("CatalogView.empty has no current entries"):
    assertEquals(CatalogView.empty.current.size, 0)

  test("CatalogView lookups are by RelPath"):
    val entry = CatalogEntry(RelPath("x/y"), ContentHash("ab"), 10L)
    val view = CatalogView(Map(entry.relPath -> entry))
    assertEquals(view.current.get(RelPath("x/y")), Some(entry))

  test("ScannedFile carries path, hash, size, mtime"):
    val f = ScannedFile(RelPath("p"), ContentHash("cd"), 42L, Instant.EPOCH)
    assertEquals(f.sizeBytes, 42L)
    assertEquals(f.contentHash, ContentHash("cd"))
```

- [ ] **Step 2: Run test to verify it fails**

Run: `sbt "core/testOnly diskrement.core.ModelTest"`
Expected: FAIL — compilation error, types not found.

- [ ] **Step 3: Write minimal implementation**

```scala
package diskrement.core

import java.time.Instant

/** Path of a file relative to the project's source root, using '/' separators. */
final case class RelPath(value: String)

object RelPath:
  given Ordering[RelPath] = Ordering.by(_.value)

/** Content hash of a file (SHA-256 hex in production; opaque to the core). */
final case class ContentHash(value: String)

/** A file observed during a scan, always fully hashed by the scanner. */
final case class ScannedFile(
    relPath: RelPath,
    contentHash: ContentHash,
    sizeBytes: Long,
    mtime: Instant
)

/** The current (live) version of one file as recorded in the catalog. */
final case class CatalogEntry(
    relPath: RelPath,
    contentHash: ContentHash,
    sizeBytes: Long
)

/** Immutable snapshot of the catalog's current view: relPath -> live version. */
final case class CatalogView(current: Map[RelPath, CatalogEntry])

object CatalogView:
  val empty: CatalogView = CatalogView(Map.empty)
```

- [ ] **Step 4: Run test to verify it passes**

Run: `sbt "core/testOnly diskrement.core.ModelTest"`
Expected: `Tests: 4, Passed: 4`.

- [ ] **Step 5: Commit**

```bash
git add core/src/main/scala/diskrement/core/Model.scala core/src/test/scala/diskrement/core/ModelTest.scala
git commit -m "$(cat <<'EOF'
feat(core): add core model types (RelPath, ContentHash, ScannedFile, CatalogView)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: ChangeSet type

**Files:**
- Create: `core/src/main/scala/diskrement/core/ChangeSet.scala`
- Test: `core/src/test/scala/diskrement/core/ChangeSetTest.scala`

- [ ] **Step 1: Write the failing test**

```scala
package diskrement.core

class ChangeSetTest extends munit.FunSuite:
  test("ChangeSet.empty has no entries and reports nothing to do"):
    assert(ChangeSet.empty.isEmpty)

  test("a ChangeSet with any added/modified/deleted is non-empty"):
    val cs = ChangeSet.empty.copy(deleted = List(RelPath("gone")))
    assert(!cs.isEmpty)
```

- [ ] **Step 2: Run test to verify it fails**

Run: `sbt "core/testOnly diskrement.core.ChangeSetTest"`
Expected: FAIL — `ChangeSet` not found.

- [ ] **Step 3: Write minimal implementation**

```scala
package diskrement.core

/** Classification of a scan against the catalog's current view.
  *
  * `unchanged` is retained for reporting; it never contributes to a burn.
  */
final case class ChangeSet(
    added: List[ScannedFile],
    modified: List[ScannedFile],
    deleted: List[RelPath],
    unchanged: List[ScannedFile]
):
  /** True when there is nothing to back up (added/modified/deleted all empty). */
  def isEmpty: Boolean =
    added.isEmpty && modified.isEmpty && deleted.isEmpty

object ChangeSet:
  val empty: ChangeSet = ChangeSet(Nil, Nil, Nil, Nil)
```

- [ ] **Step 4: Run test to verify it passes**

Run: `sbt "core/testOnly diskrement.core.ChangeSetTest"`
Expected: `Tests: 2, Passed: 2`.

- [ ] **Step 5: Commit**

```bash
git add core/src/main/scala/diskrement/core/ChangeSet.scala core/src/test/scala/diskrement/core/ChangeSetTest.scala
git commit -m "$(cat <<'EOF'
feat(core): add ChangeSet result type

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: Diff function (example-based)

**Files:**
- Create: `core/src/main/scala/diskrement/core/Diff.scala`
- Test: `core/src/test/scala/diskrement/core/DiffTest.scala`

- [ ] **Step 1: Write the failing test**

```scala
package diskrement.core

import java.time.Instant

class DiffTest extends munit.FunSuite:
  private def sf(path: String, hash: String, size: Long = 1L): ScannedFile =
    ScannedFile(RelPath(path), ContentHash(hash), size, Instant.EPOCH)

  private def entry(path: String, hash: String, size: Long = 1L): CatalogEntry =
    CatalogEntry(RelPath(path), ContentHash(hash), size)

  private def viewOf(es: CatalogEntry*): CatalogView =
    CatalogView(es.map(e => e.relPath -> e).toMap)

  test("new path is classified as added"):
    val cs = Diff.diff(CatalogView.empty, List(sf("a", "h1")))
    assertEquals(cs.added.map(_.relPath), List(RelPath("a")))
    assert(cs.modified.isEmpty && cs.deleted.isEmpty && cs.unchanged.isEmpty)

  test("same path with different hash is modified"):
    val cs = Diff.diff(viewOf(entry("a", "old")), List(sf("a", "new")))
    assertEquals(cs.modified.map(_.contentHash), List(ContentHash("new")))
    assert(cs.added.isEmpty)

  test("same path and hash is unchanged"):
    val cs = Diff.diff(viewOf(entry("a", "h")), List(sf("a", "h")))
    assertEquals(cs.unchanged.map(_.relPath), List(RelPath("a")))
    assert(cs.added.isEmpty && cs.modified.isEmpty)

  test("catalog path absent from scan is deleted"):
    val cs = Diff.diff(viewOf(entry("a", "h")), List.empty)
    assertEquals(cs.deleted, List(RelPath("a")))

  test("outputs are sorted by relPath for determinism"):
    val cs = Diff.diff(CatalogView.empty, List(sf("b", "h"), sf("a", "h")))
    assertEquals(cs.added.map(_.relPath.value), List("a", "b"))

  test("mixed scenario classifies every file exactly once"):
    val catalog = viewOf(entry("keep", "h"), entry("edit", "old"), entry("rm", "h"))
    val scan = List(sf("keep", "h"), sf("edit", "new"), sf("add", "h"))
    val cs = Diff.diff(catalog, scan)
    assertEquals(cs.added.map(_.relPath.value), List("add"))
    assertEquals(cs.modified.map(_.relPath.value), List("edit"))
    assertEquals(cs.unchanged.map(_.relPath.value), List("keep"))
    assertEquals(cs.deleted.map(_.value), List("rm"))
```

- [ ] **Step 2: Run test to verify it fails**

Run: `sbt "core/testOnly diskrement.core.DiffTest"`
Expected: FAIL — `Diff` not found.

- [ ] **Step 3: Write minimal implementation**

```scala
package diskrement.core

/** Pure classification of a scan against the catalog's current view. */
object Diff:

  def diff(catalog: CatalogView, scanned: List[ScannedFile]): ChangeSet =
    val scannedPaths = scanned.iterator.map(_.relPath).toSet

    val added = scanned.filter(f => !catalog.current.contains(f.relPath))

    val present: List[(ScannedFile, CatalogEntry)] =
      scanned.flatMap(f => catalog.current.get(f.relPath).map(e => (f, e)))

    val (modifiedPairs, unchangedPairs) =
      present.partition((f, e) => f.contentHash != e.contentHash)

    val deleted =
      catalog.current.keys.filterNot(scannedPaths.contains).toList

    ChangeSet(
      added = added.sortBy(_.relPath),
      modified = modifiedPairs.map(_._1).sortBy(_.relPath),
      deleted = deleted.sorted,
      unchanged = unchangedPairs.map(_._1).sortBy(_.relPath)
    )
```

- [ ] **Step 4: Run test to verify it passes**

Run: `sbt "core/testOnly diskrement.core.DiffTest"`
Expected: `Tests: 6, Passed: 6`.

- [ ] **Step 5: Commit**

```bash
git add core/src/main/scala/diskrement/core/Diff.scala core/src/test/scala/diskrement/core/DiffTest.scala
git commit -m "$(cat <<'EOF'
feat(core): add pure Diff classification

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 6: Shared ScalaCheck generators

**Files:**
- Create: `core/src/test/scala/diskrement/core/Generators.scala`
- Test: `core/src/test/scala/diskrement/core/GeneratorsTest.scala`

- [ ] **Step 1: Write the failing test**

```scala
package diskrement.core

import org.scalacheck.Prop.forAll

class GeneratorsTest extends munit.ScalaCheckSuite:
  property("generated scanned-file lists have unique relPaths"):
    forAll(Generators.genScannedFiles): files =>
      val paths = files.map(_.relPath)
      paths.distinct.size == paths.size

  property("generated file sizes are positive"):
    forAll(Generators.genScannedFile): f =>
      f.sizeBytes > 0L
```

- [ ] **Step 2: Run test to verify it fails**

Run: `sbt "core/testOnly diskrement.core.GeneratorsTest"`
Expected: FAIL — `Generators` not found.

- [ ] **Step 3: Write minimal implementation**

```scala
package diskrement.core

import java.time.Instant
import org.scalacheck.Gen

/** Shared generators for property tests over the functional core. */
object Generators:

  val genRelPath: Gen[RelPath] =
    for
      depth <- Gen.choose(1, 4)
      segs  <- Gen.listOfN(depth, Gen.identifier.suchThat(_.nonEmpty))
    yield RelPath(segs.mkString("/"))

  val genHash: Gen[ContentHash] =
    Gen.listOfN(8, Gen.hexChar).map(cs => ContentHash(cs.mkString))

  val genScannedFile: Gen[ScannedFile] =
    for
      p    <- genRelPath
      h    <- genHash
      size <- Gen.choose(1L, 5_000_000_000L)
    yield ScannedFile(p, h, size, Instant.EPOCH)

  /** A list of scanned files with unique relPaths (deduped, order arbitrary). */
  val genScannedFiles: Gen[List[ScannedFile]] =
    Gen.listOf(genScannedFile).map { fs =>
      fs.groupBy(_.relPath).values.map(_.head).toList
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `sbt "core/testOnly diskrement.core.GeneratorsTest"`
Expected: `+ ... OK` for both properties.

- [ ] **Step 5: Commit**

```bash
git add core/src/test/scala/diskrement/core/Generators.scala core/src/test/scala/diskrement/core/GeneratorsTest.scala
git commit -m "$(cat <<'EOF'
test(core): add shared ScalaCheck generators

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 7: Diff property test (idempotence)

**Files:**
- Create: `core/src/test/scala/diskrement/core/DiffPropertyTest.scala`

This task adds a property and a small helper (`CatalogView.fromScanned`) used to express it. The helper belongs in main because it models "what the catalog would look like after recording these files."

- [ ] **Step 1: Write the failing test**

```scala
package diskrement.core

import org.scalacheck.Prop.forAll

class DiffPropertyTest extends munit.ScalaCheckSuite:

  property("every scanned file is classified exactly once"):
    forAll(Generators.genScannedFiles): files =>
      val cs = Diff.diff(CatalogView.empty, files)
      val classified =
        cs.added.size + cs.modified.size + cs.unchanged.size
      // against an empty catalog, nothing is deleted, all are added
      classified == files.size && cs.deleted.isEmpty

  property("diff is idempotent: recording a scan yields an empty diff next time"):
    forAll(Generators.genScannedFiles): files =>
      val catalog = CatalogView.fromScanned(files)
      val cs = Diff.diff(catalog, files)
      cs.isEmpty && cs.unchanged.size == files.size
```

- [ ] **Step 2: Run test to verify it fails**

Run: `sbt "core/testOnly diskrement.core.DiffPropertyTest"`
Expected: FAIL — `CatalogView.fromScanned` not found.

- [ ] **Step 3: Add the helper to `CatalogView`**

In `core/src/main/scala/diskrement/core/Model.scala`, replace the `CatalogView` companion object with:

```scala
object CatalogView:
  val empty: CatalogView = CatalogView(Map.empty)

  /** The catalog view that results from recording exactly these files as the
    * current versions (used to express diff idempotence). */
  def fromScanned(files: List[ScannedFile]): CatalogView =
    CatalogView(
      files.map(f => f.relPath -> CatalogEntry(f.relPath, f.contentHash, f.sizeBytes)).toMap
    )
```

- [ ] **Step 4: Run test to verify it passes**

Run: `sbt "core/testOnly diskrement.core.DiffPropertyTest"`
Expected: both properties `OK`.

- [ ] **Step 5: Commit**

```bash
git add core/src/main/scala/diskrement/core/Model.scala core/src/test/scala/diskrement/core/DiffPropertyTest.scala
git commit -m "$(cat <<'EOF'
test(core): add diff idempotence + conservation properties

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 8: Planner types

**Files:**
- Create: `core/src/main/scala/diskrement/core/Planner.scala` (types only this task; the `plan` function comes in Task 9)
- Test: `core/src/test/scala/diskrement/core/PlannerTypesTest.scala`

- [ ] **Step 1: Write the failing test**

```scala
package diskrement.core

import java.time.Instant

class PlannerTypesTest extends munit.FunSuite:
  test("DiscState reports free bytes"):
    val d = DiscState(ordinal = 1, status = DiscStatus.OPEN,
                      usableBytes = 100L, usedBytes = 30L)
    assertEquals(d.freeBytes, 70L)

  test("PlannedDisc reports added and total bytes"):
    val f = ScannedFile(RelPath("a"), ContentHash("h"), 25L, Instant.EPOCH)
    val pd = PlannedDisc(ordinal = 2, isNew = true, willFinalize = false,
                         files = List(f, f), priorUsedBytes = 10L, usableBytes = 100L)
    assertEquals(pd.addedBytes, 50L)
    assertEquals(pd.totalUsedBytes, 60L)

  test("BurnPlan groups planned discs, tombstones, and oversized files"):
    val plan = BurnPlan(discs = Nil, tombstones = List(RelPath("gone")), oversized = Nil)
    assertEquals(plan.tombstones, List(RelPath("gone")))
```

- [ ] **Step 2: Run test to verify it fails**

Run: `sbt "core/testOnly diskrement.core.PlannerTypesTest"`
Expected: FAIL — types not found.

- [ ] **Step 3: Write minimal implementation**

```scala
package diskrement.core

/** Whether a disc can still receive new sessions. */
enum DiscStatus:
  case OPEN, FINALIZED

/** An existing disc as known to the catalog when planning begins. */
final case class DiscState(
    ordinal: Int,
    status: DiscStatus,
    usableBytes: Long,
    usedBytes: Long
):
  def freeBytes: Long = usableBytes - usedBytes

/** One disc as it will be written by the upcoming burn. */
final case class PlannedDisc(
    ordinal: Int,
    isNew: Boolean,
    willFinalize: Boolean,
    files: List[ScannedFile],
    priorUsedBytes: Long,
    usableBytes: Long
):
  def addedBytes: Long      = files.map(_.sizeBytes).sum
  def totalUsedBytes: Long  = priorUsedBytes + addedBytes

/** The full plan for the next burn session(s). */
final case class BurnPlan(
    discs: List[PlannedDisc],
    tombstones: List[RelPath],
    oversized: List[ScannedFile]
)
```

- [ ] **Step 4: Run test to verify it passes**

Run: `sbt "core/testOnly diskrement.core.PlannerTypesTest"`
Expected: `Tests: 3, Passed: 3`.

- [ ] **Step 5: Commit**

```bash
git add core/src/main/scala/diskrement/core/Planner.scala core/src/test/scala/diskrement/core/PlannerTypesTest.scala
git commit -m "$(cat <<'EOF'
feat(core): add planner data types

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 9: Planner core algorithm (fill, spill, finalize)

**Files:**
- Modify: `core/src/main/scala/diskrement/core/Planner.scala` (add `Planner.plan`)
- Test: `core/src/test/scala/diskrement/core/PlannerTest.scala`

**Policy reminder (from spec §8):** sort added+modified by relPath; fill the existing OPEN disc first; a file that doesn't fit the remaining space spills *wholly* to the next disc; once a file doesn't fit, stop filling that disc (strict order, no backfill, leave gaps); a disc that spills is FINALIZED; only the last disc stays OPEN. **Documented edge case:** if there is any spillover to a new disc, a pre-existing OPEN disc is finalized even if it received zero files this session (it appears in the plan with empty `files` and `willFinalize = true`). This keeps the single-open-disc invariant.

**Oversized rule (spec §8.2):** the oversized check considers **both** the currently open disc and the next blank disc — a file is oversized only when it fits neither. Concretely, compare each file against the **larger** of `newDiscCap` and the existing OPEN disc's full usable capacity; a file that exceeds a new blank disc but still fits the (larger) open disc is placed there, not rejected. (A file that passes this up-front check but still doesn't fit the open disc's *remaining* free space and is too big for a fresh disc is detected as oversized during placement.)

**Ordinal rule:** new (spilled) discs take ordinals strictly above the highest existing ordinal (`maxOrdinal`), never merely the open disc's ordinal + 1, so they cannot collide with an existing disc.

> **Note:** Tasks 9–12's code blocks below reflect the *initial* implementation. The oversized and ordinal rules above were tightened after a later review; the committed `Planner.scala` is the source of truth for those two behaviors.

- [ ] **Step 1: Write the failing test**

```scala
package diskrement.core

import java.time.Instant

class PlannerTest extends munit.FunSuite:
  private def sf(path: String, size: Long): ScannedFile =
    ScannedFile(RelPath(path), ContentHash("h-" + path), size, Instant.EPOCH)

  private def changesOf(added: List[ScannedFile]): ChangeSet =
    ChangeSet(added = added, modified = Nil, deleted = Nil, unchanged = Nil)

  private val cap100 = Capacity(100L)

  test("all files fit on one fresh disc, which stays open"):
    val cs = changesOf(List(sf("a", 30), sf("b", 30)))
    val plan = Planner.plan(cs, discs = Nil, newDiscCap = cap100)
    assertEquals(plan.discs.size, 1)
    val d = plan.discs.head
    assert(d.isNew)
    assert(!d.willFinalize)
    assertEquals(d.files.map(_.relPath.value), List("a", "b"))
    assertEquals(d.ordinal, 1)

  test("files are placed in lexicographic order regardless of input order"):
    val cs = changesOf(List(sf("c", 10), sf("a", 10), sf("b", 10)))
    val plan = Planner.plan(cs, Nil, cap100)
    assertEquals(plan.discs.head.files.map(_.relPath.value), List("a", "b", "c"))

  test("a file that doesn't fit spills wholly to a new disc; full disc finalized"):
    // 60 + 60 cannot share a 100-byte disc
    val cs = changesOf(List(sf("a", 60), sf("b", 60)))
    val plan = Planner.plan(cs, Nil, cap100)
    assertEquals(plan.discs.size, 2)
    val first = plan.discs(0)
    val last  = plan.discs(1)
    assertEquals(first.files.map(_.relPath.value), List("a"))
    assert(first.willFinalize, "spilled disc must be finalized")
    assertEquals(last.files.map(_.relPath.value), List("b"))
    assert(!last.willFinalize, "last disc stays open")
    assertEquals(last.ordinal, 2)

  test("existing OPEN disc is topped off before opening a new disc"):
    // open disc ordinal 1: 100 usable, 90 used -> 10 free
    val open = DiscState(1, DiscStatus.OPEN, usableBytes = 100L, usedBytes = 90L)
    val cs = changesOf(List(sf("a", 10), sf("b", 50)))
    val plan = Planner.plan(cs, discs = List(open), newDiscCap = cap100)
    assertEquals(plan.discs.size, 2)
    val topped = plan.discs(0)
    assertEquals(topped.ordinal, 1)
    assert(!topped.isNew)
    assertEquals(topped.files.map(_.relPath.value), List("a"))
    assert(topped.willFinalize)               // spilled -> finalized
    assertEquals(topped.totalUsedBytes, 100L)
    val fresh = plan.discs(1)
    assertEquals(fresh.ordinal, 2)
    assert(fresh.isNew)
    assertEquals(fresh.files.map(_.relPath.value), List("b"))
    assert(!fresh.willFinalize)

  test("deletions become tombstones and cost zero bytes"):
    val cs = ChangeSet(added = Nil, modified = Nil,
                       deleted = List(RelPath("x"), RelPath("y")), unchanged = Nil)
    val plan = Planner.plan(cs, Nil, cap100)
    assertEquals(plan.tombstones, List(RelPath("x"), RelPath("y")))
    assertEquals(plan.discs.flatMap(_.files), Nil)

  test("modified files are planned alongside added, by relPath"):
    val cs = ChangeSet(added = List(sf("a", 10)), modified = List(sf("b", 10)),
                       deleted = Nil, unchanged = Nil)
    val plan = Planner.plan(cs, Nil, cap100)
    assertEquals(plan.discs.head.files.map(_.relPath.value), List("a", "b"))

  test("a file larger than a fresh disc is reported oversized, not placed"):
    val cs = changesOf(List(sf("big", 150), sf("ok", 10)))
    val plan = Planner.plan(cs, Nil, cap100)
    assertEquals(plan.oversized.map(_.relPath.value), List("big"))
    assertEquals(plan.discs.flatMap(_.files).map(_.relPath.value), List("ok"))
```

- [ ] **Step 2: Run test to verify it fails**

Run: `sbt "core/testOnly diskrement.core.PlannerTest"`
Expected: FAIL — `Planner` not found.

- [ ] **Step 3: Write minimal implementation**

Append to `core/src/main/scala/diskrement/core/Planner.scala`:

```scala
/** Pure, deterministic ordered-fill disc planner. */
object Planner:

  def plan(
      changes: ChangeSet,
      discs: List[DiscState],
      newDiscCap: Capacity
  ): BurnPlan =
    val toPlace = (changes.added ++ changes.modified).sortBy(_.relPath)
    val (oversized, placeable) =
      toPlace.partition(_.sizeBytes > newDiscCap.usableBytes)

    val existingOpen = discs.filter(_.status == DiscStatus.OPEN)
      .sortBy(_.ordinal)
      .lastOption
    val maxOrdinal = discs.map(_.ordinal).maxOption.getOrElse(0)

    // Mutable-free in-progress disc representation, local to the planner.
    final case class Bin(
        ordinal: Int,
        isNew: Boolean,
        usableBytes: Long,
        priorUsedBytes: Long,
        files: Vector[ScannedFile]
    ):
      def usedBytes: Long = priorUsedBytes + files.map(_.sizeBytes).sum
      def freeBytes: Long = usableBytes - usedBytes
      def fits(f: ScannedFile): Boolean = f.sizeBytes <= freeBytes
      def add(f: ScannedFile): Bin = copy(files = files :+ f)

    def freshBin(prevOrdinal: Int): Bin =
      Bin(prevOrdinal + 1, isNew = true, newDiscCap.usableBytes, 0L, Vector.empty)

    val initialBin: Bin = existingOpen match
      case Some(d) => Bin(d.ordinal, isNew = false, d.usableBytes, d.usedBytes, Vector.empty)
      case None    => freshBin(maxOrdinal)

    val (completed, current) =
      placeable.foldLeft((Vector.empty[Bin], initialBin)) {
        case ((done, cur), f) =>
          if cur.fits(f) then (done, cur.add(f))
          else (done :+ cur, freshBin(cur.ordinal).add(f))
      }

    val bins = completed :+ current
    val lastIdx = bins.size - 1
    val plannedDiscs = bins.zipWithIndex.map { case (b, i) =>
      PlannedDisc(
        ordinal = b.ordinal,
        isNew = b.isNew,
        willFinalize = i != lastIdx,
        files = b.files.toList,
        priorUsedBytes = b.priorUsedBytes,
        usableBytes = b.usableBytes
      )
    }.toList

    BurnPlan(
      discs = plannedDiscs,
      tombstones = changes.deleted,
      oversized = oversized
    )
```

- [ ] **Step 4: Run test to verify it passes**

Run: `sbt "core/testOnly diskrement.core.PlannerTest"`
Expected: `Tests: 7, Passed: 7`.

- [ ] **Step 5: Commit**

```bash
git add core/src/main/scala/diskrement/core/Planner.scala core/src/test/scala/diskrement/core/PlannerTest.scala
git commit -m "$(cat <<'EOF'
feat(core): add ordered-fill disc planner

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 10: Planner property tests (the 8 invariants)

**Files:**
- Modify: `core/src/test/scala/diskrement/core/Generators.scala` (add planner-input generators)
- Test: `core/src/test/scala/diskrement/core/PlannerPropertyTest.scala`

- [ ] **Step 1: Add generators**

Append to `core/src/test/scala/diskrement/core/Generators.scala` (inside the `object Generators`):

```scala
  import org.scalacheck.Gen as G

  /** A capacity comfortably larger than the largest single generated file,
    * so that "oversized" is a deliberate, separate concern. */
  val genNewDiscCap: Gen[Capacity] =
    G.choose(1_000_000_000L, 10_000_000_000L).map(Capacity.apply)

  /** Files guaranteed to fit on a disc of the given capacity. */
  def genFilesWithin(cap: Capacity): Gen[List[ScannedFile]] =
    for
      files <- genScannedFiles
    yield files.map { f =>
      val size = math.max(1L, f.sizeBytes % cap.usableBytes)
      f.copy(sizeBytes = size)
    }

  /** An optional pre-existing OPEN disc with the same usable capacity. */
  def genOptOpenDisc(cap: Capacity): Gen[List[DiscState]] =
    G.option(
      G.choose(0L, cap.usableBytes).map { used =>
        DiscState(ordinal = 1, status = DiscStatus.OPEN,
                  usableBytes = cap.usableBytes, usedBytes = used)
      }
    ).map(_.toList)

  /** A full planner input: capacity, optional open disc, and fitting changes. */
  val genPlannerInput: Gen[(ChangeSet, List[DiscState], Capacity)] =
    for
      cap   <- genNewDiscCap
      open  <- genOptOpenDisc(cap)
      files <- genFilesWithin(cap)
      // split files arbitrarily into added vs modified
      split <- G.choose(0, files.size)
    yield
      val (add, mod) = files.splitAt(split)
      (ChangeSet(add, mod, Nil, Nil), open, cap)
```

- [ ] **Step 2: Write the failing properties**

```scala
package diskrement.core

import org.scalacheck.Prop.forAll

class PlannerPropertyTest extends munit.ScalaCheckSuite:

  private def placeable(cs: ChangeSet): List[ScannedFile] =
    cs.added ++ cs.modified

  property("1. conservation: every placeable file is placed or oversized, exactly once"):
    forAll(Generators.genPlannerInput): (cs, discs, cap) =>
      val plan = Planner.plan(cs, discs, cap)
      val placedPaths = plan.discs.flatMap(_.files).map(_.relPath)
      val oversizedPaths = plan.oversized.map(_.relPath)
      val all = placedPaths ++ oversizedPaths
      val expected = placeable(cs).map(_.relPath)
      all.sortBy(_.value) == expected.sortBy(_.value) &&
        all.distinct.size == all.size

  property("2. capacity: no disc exceeds its usable capacity"):
    forAll(Generators.genPlannerInput): (cs, discs, cap) =>
      val plan = Planner.plan(cs, discs, cap)
      plan.discs.forall(d => d.totalUsedBytes <= d.usableBytes)

  property("3. no split: each placed file appears on exactly one disc"):
    forAll(Generators.genPlannerInput): (cs, discs, cap) =>
      val plan = Planner.plan(cs, discs, cap)
      val placed = plan.discs.flatMap(_.files).map(_.relPath)
      placed.distinct.size == placed.size

  property("4. order: placed files are in lexicographic relPath order across discs"):
    forAll(Generators.genPlannerInput): (cs, discs, cap) =>
      val plan = Planner.plan(cs, discs, cap)
      val placed = plan.discs.flatMap(_.files).map(_.relPath.value)
      placed == placed.sorted

  property("5. determinism: shuffling inputs yields an identical plan"):
    forAll(Generators.genPlannerInput): (cs, discs, cap) =>
      val shuffled = cs.copy(added = cs.added.reverse, modified = cs.modified.reverse)
      Planner.plan(cs, discs, cap) == Planner.plan(shuffled, discs, cap)

  property("6. open-disc-first: if an open disc exists, it is the first planned disc"):
    forAll(Generators.genPlannerInput): (cs, discs, cap) =>
      val plan = Planner.plan(cs, discs, cap)
      discs.find(_.status == DiscStatus.OPEN) match
        case Some(open) if plan.discs.nonEmpty =>
          val first = plan.discs.head
          first.ordinal == open.ordinal && !first.isNew
        case _ => true

  property("7. finalize: at most one planned disc stays open (the last)"):
    forAll(Generators.genPlannerInput): (cs, discs, cap) =>
      val plan = Planner.plan(cs, discs, cap)
      val openCount = plan.discs.count(!_.willFinalize)
      openCount <= 1 &&
        (plan.discs.isEmpty || !plan.discs.last.willFinalize)

  property("8. tombstones cost nothing: deletions add no bytes to any disc"):
    forAll(Generators.genPlannerInput): (cs, discs, cap) =>
      val withDeletes = cs.copy(deleted = List(RelPath("zzz/deleted")))
      val a = Planner.plan(cs, discs, cap).discs.map(_.addedBytes).sum
      val b = Planner.plan(withDeletes, discs, cap).discs.map(_.addedBytes).sum
      a == b
```

- [ ] **Step 3: Run to verify they fail first (red), if generators were missing**

Run: `sbt "core/testOnly diskrement.core.PlannerPropertyTest"`
Expected: compiles only after Step 1 generators exist; properties should then **pass**. If any property fails, the planner has a real bug — fix `Planner.plan`, not the property. (These properties are the spec's §8.3 invariants and must hold.)

- [ ] **Step 4: Run the full suite**

Run: `sbt core/test`
Expected: all tests and properties pass.

- [ ] **Step 5: Commit**

```bash
git add core/src/test/scala/diskrement/core/Generators.scala core/src/test/scala/diskrement/core/PlannerPropertyTest.scala
git commit -m "$(cat <<'EOF'
test(core): add the 8 planner invariants as properties

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 11: Manifest builder

**Files:**
- Create: `core/src/main/scala/diskrement/core/Manifest.scala`
- Test: `core/src/test/scala/diskrement/core/ManifestBuilderTest.scala`

The manifest is the pure data structure for a single session on a single disc. JSON serialization is a shell concern (Plan 2); this task builds only the structure. `added`/`modified` are split using the ChangeSet, since a `PlannedDisc.files` list mixes both. Tombstones are attached to the disc the caller chooses (in practice the last/open disc).

- [ ] **Step 1: Write the failing test**

```scala
package diskrement.core

import java.time.Instant

class ManifestBuilderTest extends munit.FunSuite:
  private def sf(path: String, size: Long): ScannedFile =
    ScannedFile(RelPath(path), ContentHash("h-" + path), size, Instant.EPOCH)

  test("manifest splits a disc's files into added vs modified using the changeset"):
    val added = List(sf("new1", 10))
    val modified = List(sf("edit1", 10))
    val cs = ChangeSet(added = added, modified = modified, deleted = Nil, unchanged = Nil)
    val disc = PlannedDisc(ordinal = 1, isNew = true, willFinalize = false,
                           files = added ++ modified, priorUsedBytes = 0L, usableBytes = 100L)
    val m = ManifestBuilder.build("photos", sessionOrdinal = 1, disc = disc,
                                  changes = cs, tombstones = List(RelPath("gone")))
    assertEquals(m.projectName, "photos")
    assertEquals(m.discOrdinal, 1)
    assertEquals(m.sessionOrdinal, 1)
    assertEquals(m.added.map(_.relPath.value), List("new1"))
    assertEquals(m.modified.map(_.relPath.value), List("edit1"))
    assertEquals(m.deleted, List(RelPath("gone")))

  test("manifest entry carries hash and size"):
    val f = sf("a", 42)
    val cs = ChangeSet(added = List(f), modified = Nil, deleted = Nil, unchanged = Nil)
    val disc = PlannedDisc(1, true, false, List(f), 0L, 100L)
    val m = ManifestBuilder.build("p", 1, disc, cs, Nil)
    assertEquals(m.added.head, ManifestEntry(RelPath("a"), ContentHash("h-a"), 42L))
```

- [ ] **Step 2: Run test to verify it fails**

Run: `sbt "core/testOnly diskrement.core.ManifestBuilderTest"`
Expected: FAIL — `ManifestBuilder` not found.

- [ ] **Step 3: Write minimal implementation**

```scala
package diskrement.core

/** One file's identity as recorded in a session manifest. */
final case class ManifestEntry(
    relPath: RelPath,
    contentHash: ContentHash,
    sizeBytes: Long
)

enum ManifestChangeKind:
  case ADDED, MODIFIED, DELETED

/** The pure manifest data for one session burned onto one disc.
  * Serialization to JSON is a shell concern (not part of the core). */
final case class SessionManifest(
    projectName: String,
    discOrdinal: Int,
    sessionOrdinal: Int,
    added: List[ManifestEntry],
    modified: List[ManifestEntry],
    deleted: List[RelPath]
)

object ManifestBuilder:

  /** Build the manifest for `disc`, classifying its files via `changes` and
    * attaching the given `tombstones` (typically only on the last/open disc). */
  def build(
      projectName: String,
      sessionOrdinal: Int,
      disc: PlannedDisc,
      changes: ChangeSet,
      tombstones: List[RelPath]
  ): SessionManifest =
    val addedPaths = changes.added.iterator.map(_.relPath).toSet
    val (addedFiles, modifiedFiles) =
      disc.files.partition(f => addedPaths.contains(f.relPath))
    SessionManifest(
      projectName = projectName,
      discOrdinal = disc.ordinal,
      sessionOrdinal = sessionOrdinal,
      added = addedFiles.map(toEntry),
      modified = modifiedFiles.map(toEntry),
      deleted = tombstones
    )

  private def toEntry(f: ScannedFile): ManifestEntry =
    ManifestEntry(f.relPath, f.contentHash, f.sizeBytes)
```

- [ ] **Step 4: Run test to verify it passes**

Run: `sbt "core/testOnly diskrement.core.ManifestBuilderTest"`
Expected: `Tests: 2, Passed: 2`.

- [ ] **Step 5: Commit**

```bash
git add core/src/main/scala/diskrement/core/Manifest.scala core/src/test/scala/diskrement/core/ManifestBuilderTest.scala
git commit -m "$(cat <<'EOF'
feat(core): add session manifest builder

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 12: Edge-case unit tests & final green sweep

**Files:**
- Test: `core/src/test/scala/diskrement/core/PlannerEdgeTest.scala`

Covers the spec's named edge cases not yet explicitly asserted: empty input, exact-fit, all-deleted, move (delete+add), and mixed-capacity spill (BD-R open + BD-50 new).

- [ ] **Step 1: Write the failing test**

```scala
package diskrement.core

import java.time.Instant

class PlannerEdgeTest extends munit.FunSuite:
  private def sf(path: String, size: Long): ScannedFile =
    ScannedFile(RelPath(path), ContentHash("h-" + path), size, Instant.EPOCH)
  private def added(fs: ScannedFile*): ChangeSet =
    ChangeSet(fs.toList, Nil, Nil, Nil)

  test("empty changeset yields a single empty open disc"):
    val plan = Planner.plan(ChangeSet.empty, Nil, Capacity(100L))
    assertEquals(plan.discs.size, 1)
    assert(plan.discs.head.files.isEmpty)
    assert(!plan.discs.head.willFinalize)
    assert(plan.oversized.isEmpty)

  test("a file exactly equal to free space fits"):
    val plan = Planner.plan(added(sf("a", 100)), Nil, Capacity(100L))
    assertEquals(plan.discs.size, 1)
    assertEquals(plan.discs.head.files.map(_.relPath.value), List("a"))
    assert(!plan.discs.head.willFinalize)

  test("only deletions: tombstones recorded, no data discs filled"):
    val cs = ChangeSet(Nil, Nil, List(RelPath("x")), Nil)
    val plan = Planner.plan(cs, Nil, Capacity(100L))
    assertEquals(plan.tombstones, List(RelPath("x")))
    assert(plan.discs.forall(_.files.isEmpty))

  test("a move (delete old path + add new path) plans the add and tombstones the old"):
    val cs = ChangeSet(added = List(sf("new/a", 10)), modified = Nil,
                       deleted = List(RelPath("old/a")), unchanged = Nil)
    val plan = Planner.plan(cs, Nil, Capacity(100L))
    assertEquals(plan.discs.head.files.map(_.relPath.value), List("new/a"))
    assertEquals(plan.tombstones, List(RelPath("old/a")))

  test("mixed capacity: BD-R open disc tops off, new disc uses BD-50 capacity"):
    val bdrUsable  = Capacity.fromDiscType(DiscType.BD_R, 0.0).usableBytes
    val bd50Usable = Capacity.fromDiscType(DiscType.BD_R_DL, 0.0).usableBytes
    // open BD-R with only 5 bytes free
    val open = DiscState(1, DiscStatus.OPEN, usableBytes = bdrUsable, usedBytes = bdrUsable - 5L)
    val cs = added(sf("a", 5), sf("b", bd50Usable)) // b only fits a BD-50
    val plan = Planner.plan(cs, discs = List(open), newDiscCap = Capacity(bd50Usable))
    assertEquals(plan.discs.map(_.ordinal), List(1, 2))
    assertEquals(plan.discs(0).files.map(_.relPath.value), List("a"))
    assertEquals(plan.discs(0).usableBytes, bdrUsable)
    assert(plan.discs(0).willFinalize)
    assertEquals(plan.discs(1).files.map(_.relPath.value), List("b"))
    assertEquals(plan.discs(1).usableBytes, bd50Usable)
    assert(!plan.discs(1).willFinalize)
    assert(plan.oversized.isEmpty)
```

- [ ] **Step 2: Run test to verify it fails or passes**

Run: `sbt "core/testOnly diskrement.core.PlannerEdgeTest"`
Expected: these should PASS against the Task 9 implementation. If any fail, it reveals a planner bug — fix `Planner.plan` and re-run. (Do not weaken the assertions.)

- [ ] **Step 3: Run the entire suite green**

Run: `sbt clean core/test`
Expected: every suite passes, no warnings-as-errors failures (we set `-Wunused:all`). Fix any unused-import warnings.

- [ ] **Step 4: Commit**

```bash
git add core/src/test/scala/diskrement/core/PlannerEdgeTest.scala
git commit -m "$(cat <<'EOF'
test(core): cover planner edge cases (empty, exact-fit, move, mixed capacity)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review (completed by plan author)

**Spec coverage (cycle-1 core subset):**
- §5 domain model (pure subset: RelPath, ContentHash, ScannedFile, CatalogView/Entry; DiscType/Capacity) → Tasks 2, 3. (SQLite-backed entities Project/Disc/Session/Placement/ChangeRecord are persistence — Plan 2.)
- §7 Diff + fast-path note (fast-path is the scanner's job in Plan 2; Diff stays pure) → Tasks 5, 7.
- §8 Planner, placement rules, oversized handling → Task 9; the 8 invariants → Task 10; named edge cases → Task 12.
- §4.3 / §6.3 manifest data structure → Task 11 (JSON serialization deferred to Plan 2).

**Out of scope here (correctly deferred to Plans 2/3):** SourceSnapshot, CatalogStore/SQLite, FileScanner fast-path, SimulatedBurner, CLI, on-disc layout writing, crash-safety transactions, config/XDG paths, the full-loop integration test.

**Placeholder scan:** none — every step has concrete code and exact commands.

**Type consistency check:** `ScannedFile`, `RelPath`, `ContentHash`, `CatalogView`/`CatalogEntry`, `ChangeSet`, `Capacity`, `DiscState`/`DiscStatus`, `PlannedDisc`/`BurnPlan`, `SessionManifest`/`ManifestEntry` are defined once and referenced consistently. `Planner.plan` signature `(ChangeSet, List[DiscState], Capacity): BurnPlan` matches the spec's revised signature and all call sites. `CatalogView.fromScanned` is introduced in Task 7 before use. Generators in Task 6 are extended (not redefined) in Task 10.

---

## Next plans (separate documents, written when ready to execute)

- **Plan 2 — Persistence & shell adapters:** SQLite `CatalogStore` (append-only schema, WAL, migrations, `CatalogView` read / `CatalogUpdate` apply in one transaction), `FileSystem` walk, `FileScanner` with fast-path hashing + `--verify`, `SourceSnapshot` (reflink-cp, CoW hard-fail), `SnapshotWriter` (`/.diskrement` layout + manifest JSON + catalog snapshot), `SimulatedBurner` (stage disc trees), crash-safety transactions, config/XDG + systemd paths.
- **Plan 3 — CLI & full-loop integration:** `init/scan/plan/burn/reset/status/verify/history`, staged work-dir pipeline + restage policy, and the scripted end-to-end integration test (init → scan → plan → burn → modify → scan → plan --disc-type BD_R_DL → burn → verify → history) covering multi-disc spill, mixed capacity, and reset.
