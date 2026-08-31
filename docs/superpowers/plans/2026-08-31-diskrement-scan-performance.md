# Scan Performance (P1+P2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `scan` and `burn` staging fast on large libraries: parallel hashing, live progress, reflink staging (P1) and a sidecar fingerprint cache so rescans hash only new/changed files (P2).

**Architecture:** All changes live in `diskrement.shell` (fs, burn, pipeline) and `diskrement.cli`; the functional core is untouched. New ports: `ProgressSink` (CLI renders, tests no-op) and `FingerprintCache` (sidecar SQLite next to the catalog — the append-only catalog schema is NOT touched). `FileScannerLive` partitions fast-path hits from misses and hashes misses in bounded parallel. `Stager` clones via batched `cp --reflink=always` with a plain-copy fallback, then re-hashes in parallel.

**Tech Stack:** Scala 3.3 / ZIO 2.1 / sbt. Spec: `docs/superpowers/specs/2026-08-31-diskrement-scan-performance-design.md`. Baseline numbers and acceptance targets are in spec §1/§7.

**House rules for every task:**
- Branch: all work on `feat/scan-performance` (Task 1 creates it off `main`).
- Build uses `-Wunused:all`: keep imports minimal or compilation "fails" review.
- ZIO gotchas (learned in 4a): for-comprehensions cannot tuple-destructure in `<-` OR pattern-`=` binds (no `withFilter`) — bind to a single name, then destructure with plain `val`-style `x = pair._1` lines. `finalize` is unusable as an identifier.
- Test commands: unit `sbt shell/test cli/test core/test`; one suite `sbt "shell/testOnly <FQCN>"`; integration `sbt cli/IntegrationTest/test` (slow — run when a task says so).
- Commit after each green step-pair as instructed; short conventional messages.

---

### Task 1: Branch + `FingerprintCache` port and SQLite implementation

**Files:**
- Create: `shell/src/main/scala/diskrement/shell/fs/FingerprintCache.scala`
- Test: `shell/src/test/scala/diskrement/shell/fs/FingerprintCacheSpec.scala`

- [ ] **Step 1: Create the branch**

```bash
cd ~/work/diskrement
git checkout main && git pull && git checkout -b feat/scan-performance
```

- [ ] **Step 2: Write the failing tests**

Create `shell/src/test/scala/diskrement/shell/fs/FingerprintCacheSpec.scala`:

```scala
package diskrement.shell.fs

import diskrement.core.{ContentHash, RelPath}
import diskrement.shell.model.FileFingerprint
import zio._
import zio.test._
import java.nio.file.{Files, Path}
import java.time.Instant

object FingerprintCacheSpec extends ZIOSpecDefault:
  private val mt = Instant.parse("2026-08-31T00:00:00Z")
  private def fp(sz: Long, h: String) = FileFingerprint(sz, mt, ContentHash(h))

  /** Fresh cache path in a per-test temp dir (deleted with the JVM's tmpdir policy;
    * contents are tiny). */
  private val tmpCache: UIO[Path] =
    ZIO.attemptBlocking {
      val d = Files.createTempDirectory("fpcache-test")
      d.resolve("proj.scancache.sqlite")
    }.orDie

  def spec = suite("FingerprintCache")(
    test("readAll on a missing file returns empty"):
      for
        path <- tmpCache
        m    <- FingerprintCacheLive(path).readAll
      yield assertTrue(m.isEmpty)
    ,
    test("putAll then readAll round-trips; putAll upserts"):
      for
        path <- tmpCache
        c     = FingerprintCacheLive(path)
        _    <- c.putAll(Map(RelPath("a") -> fp(1, "h1"), RelPath("b") -> fp(2, "h2")))
        _    <- c.putAll(Map(RelPath("a") -> fp(9, "h9")))
        m    <- c.readAll
      yield assertTrue(
        m == Map(RelPath("a") -> fp(9, "h9"), RelPath("b") -> fp(2, "h2")))
    ,
    test("the schema version stamp is 1"):
      for
        path <- tmpCache
        c     = FingerprintCacheLive(path)
        _    <- c.putAll(Map(RelPath("a") -> fp(1, "h1")))
        ver  <- ZIO.attemptBlocking {
                  val conn = java.sql.DriverManager.getConnection(s"jdbc:sqlite:$path")
                  try
                    val rs = conn.createStatement().executeQuery("PRAGMA user_version")
                    rs.next(); rs.getInt(1)
                  finally conn.close()
                }.orDie
      yield assertTrue(ver == 1)
    ,
    test("a corrupt cache file is recreated empty and stays usable"):
      for
        path <- tmpCache
        _    <- ZIO.attemptBlocking(Files.write(path, "not a database at all".getBytes)).orDie
        c     = FingerprintCacheLive(path)
        m0   <- c.readAll
        _    <- c.putAll(Map(RelPath("a") -> fp(1, "h1")))
        m1   <- c.readAll
      yield assertTrue(m0.isEmpty, m1 == Map(RelPath("a") -> fp(1, "h1")))
    ,
    test("a future schema version is recreated empty (disposable cache)"):
      for
        path <- tmpCache
        c     = FingerprintCacheLive(path)
        _    <- c.putAll(Map(RelPath("a") -> fp(1, "h1")))
        _    <- ZIO.attemptBlocking {
                  val conn = java.sql.DriverManager.getConnection(s"jdbc:sqlite:$path")
                  try conn.createStatement().execute("PRAGMA user_version = 99")
                  finally conn.close()
                }.orDie
        m    <- c.readAll
      yield assertTrue(m.isEmpty)
  )
```

- [ ] **Step 3: Run to verify failure**

Run: `sbt "shell/testOnly diskrement.shell.fs.FingerprintCacheSpec"`
Expected: compile error — `FingerprintCacheLive` not found.

- [ ] **Step 4: Implement**

Create `shell/src/main/scala/diskrement/shell/fs/FingerprintCache.scala`:

```scala
package diskrement.shell.fs

import diskrement.core.{ContentHash, RelPath}
import diskrement.shell.model.FileFingerprint
import zio._
import java.nio.file.{Files, Path}
import java.sql.{Connection, DriverManager}
import java.time.Instant

/** Filesystem location of a project's sidecar scan-fingerprint cache
  * (`<catalog-stem>.scancache.sqlite`, resolved by the CLI). */
final case class CachePath(value: Path)

/** Expected cache failures. Callers treat them as warnings, never fatal. */
sealed abstract class CacheError(msg: String, cause: Throwable = null)
    extends Exception(msg, cause)
object CacheError:
  final case class CacheIoError(cause: Throwable)
      extends CacheError("fingerprint cache I/O error", cause)

/** Sidecar cache of size+mtime→hash observations for *scanned* files, so a
  * rescan hashes only new/changed content. Deliberately outside the append-only
  * catalog: it is disposable — deleting it costs one re-hash, nothing else. A
  * corrupt or version-incompatible file is deleted and recreated empty. */
trait FingerprintCache:
  def readAll: IO[CacheError, Map[RelPath, FileFingerprint]]
  def putAll(entries: Map[RelPath, FileFingerprint]): IO[CacheError, Unit]

final case class FingerprintCacheLive(path: Path) extends FingerprintCache:
  import FingerprintCacheLive.SchemaVersion

  def readAll: IO[CacheError, Map[RelPath, FileFingerprint]] =
    guarded { conn =>
      val st = conn.createStatement()
      try
        val rs = st.executeQuery(
          "select rel_path, size_bytes, mtime, content_hash from fingerprint")
        val b = Map.newBuilder[RelPath, FileFingerprint]
        while rs.next() do
          b += RelPath(rs.getString(1)) -> FileFingerprint(
            rs.getLong(2), Instant.parse(rs.getString(3)), ContentHash(rs.getString(4)))
        b.result()
      finally st.close()
    }

  def putAll(entries: Map[RelPath, FileFingerprint]): IO[CacheError, Unit] =
    guarded { conn =>
      conn.setAutoCommit(false)
      val ps = conn.prepareStatement(
        """insert into fingerprint (rel_path, size_bytes, mtime, content_hash, updated_at)
           values (?, ?, ?, ?, ?)
           on conflict(rel_path) do update set
             size_bytes = excluded.size_bytes, mtime = excluded.mtime,
             content_hash = excluded.content_hash, updated_at = excluded.updated_at""")
      try
        val now = Instant.now().toString
        entries.foreach { case (rel, fp) =>
          ps.setString(1, rel.value)
          ps.setLong(2, fp.sizeBytes)
          ps.setString(3, fp.mtime.toString)
          ps.setString(4, fp.contentHash.value)
          ps.setString(5, now)
          ps.addBatch()
        }
        ps.executeBatch()
        conn.commit()
      finally ps.close()
    }.unit

  /** Run `use` on a fresh connection; on ANY failure assume the cache file is
    * unusable, warn, wipe it, and retry once against a fresh empty cache. */
  private def guarded[A](use: Connection => A): IO[CacheError, A] =
    attempt(use).catchAll { e =>
      ZIO.logWarning(
        s"fingerprint cache at $path unreadable (${e.getMessage}); recreating empty") *>
        wipe *> attempt(use).mapError(CacheError.CacheIoError(_))
    }

  private def attempt[A](use: Connection => A): Task[A] =
    ZIO.attemptBlocking {
      val conn = openConn()
      try use(conn)
      finally conn.close()
    }

  /** Open + create-on-demand. A version-0 (fresh) file gets the schema; any
    * other version than SchemaVersion throws (caught by `guarded` → recreate). */
  private def openConn(): Connection =
    Option(path.getParent).foreach(Files.createDirectories(_))
    val conn = DriverManager.getConnection(s"jdbc:sqlite:$path")
    try
      val st = conn.createStatement()
      try
        st.execute("PRAGMA busy_timeout=5000")
        val rs  = st.executeQuery("PRAGMA user_version")
        val ver = { rs.next(); rs.getInt(1) }
        if ver == 0 then
          st.execute(
            """create table if not exists fingerprint (
                 rel_path     text primary key,
                 size_bytes   integer not null,
                 mtime        text not null,
                 content_hash text not null,
                 updated_at   text not null)""")
          st.execute(s"PRAGMA user_version = $SchemaVersion")
        else if ver != SchemaVersion then
          throw new IllegalStateException(s"cache schema v$ver, expected v$SchemaVersion")
      finally st.close()
      conn
    catch
      case e: Throwable =>
        conn.close()
        throw e

  private val wipe: UIO[Unit] =
    ZIO.attemptBlocking {
      Files.deleteIfExists(path)
      Files.deleteIfExists(Path.of(path.toString + "-wal"))
      Files.deleteIfExists(Path.of(path.toString + "-shm"))
    }.ignoreLogged

object FingerprintCacheLive:
  private val SchemaVersion = 1

object FingerprintCache:
  val live: ZLayer[CachePath, Nothing, FingerprintCache] =
    ZLayer.fromFunction((p: CachePath) => FingerprintCacheLive(p.value))
```

- [ ] **Step 5: Run to verify pass**

Run: `sbt "shell/testOnly diskrement.shell.fs.FingerprintCacheSpec"`
Expected: 5 tests PASS.

- [ ] **Step 6: Full shell suite still green**

Run: `sbt shell/test`
Expected: all pass (109 tests before this plan; now 114).

- [ ] **Step 7: Commit**

```bash
git add shell/src/main/scala/diskrement/shell/fs/FingerprintCache.scala \
        shell/src/test/scala/diskrement/shell/fs/FingerprintCacheSpec.scala
git commit -m "feat(fs): FingerprintCache sidecar SQLite store (disposable scan-fingerprint cache)"
```

---

### Task 2: `ProgressSink` and `HashParallelism`

**Files:**
- Create: `shell/src/main/scala/diskrement/shell/ProgressSink.scala`
- Create: `shell/src/main/scala/diskrement/shell/fs/HashParallelism.scala`
- Test: `shell/src/test/scala/diskrement/shell/fs/HashParallelismSpec.scala`

- [ ] **Step 1: Write the failing test**

Create `shell/src/test/scala/diskrement/shell/fs/HashParallelismSpec.scala`:

```scala
package diskrement.shell.fs

import zio._
import zio.test._

object HashParallelismSpec extends ZIOSpecDefault:
  // ZIOSpecDefault routes zio.System.env to TestSystem, so building the layer
  // inside the test effect picks up putEnv (same pattern as the
  // DISKREMENT_DISC_BYTES tests in PipelineCapacityOverrideSpec).
  private val resolved: UIO[Int] =
    ZIO.service[HashParallelism].map(_.value).provide(HashParallelism.fromEnv)

  def spec = suite("HashParallelism")(
    test("defaults to 6 when the env var is unset"):
      resolved.map(v => assertTrue(v == 6))
    ,
    test("reads a positive DISKREMENT_HASH_PARALLELISM"):
      for
        _ <- TestSystem.putEnv("DISKREMENT_HASH_PARALLELISM", "3")
        v <- resolved
      yield assertTrue(v == 3)
    ,
    test("ignores non-numeric and non-positive values"):
      for
        _ <- TestSystem.putEnv("DISKREMENT_HASH_PARALLELISM", "banana")
        a <- resolved
        _ <- TestSystem.putEnv("DISKREMENT_HASH_PARALLELISM", "0")
        b <- resolved
      yield assertTrue(a == 6, b == 6)
  )
```

- [ ] **Step 2: Run to verify failure**

Run: `sbt "shell/testOnly diskrement.shell.fs.HashParallelismSpec"`
Expected: compile error — `HashParallelism` not found.

- [ ] **Step 3: Implement both files**

Create `shell/src/main/scala/diskrement/shell/ProgressSink.scala`:

```scala
package diskrement.shell

import zio._

/** Operator-facing progress for long byte-crunching phases (scan hashing, burn
  * staging). The pipeline reports through this port so it stays free of
  * console I/O; the CLI provides a stderr renderer, tests use the no-op.
  *
  * Lifecycle: wrap each phase in `phase(...)` — it calls `start` before the
  * work and guarantees `done` on success, failure, AND interrupt (so a TTY
  * renderer can always terminate its in-place line). One sink instance may run
  * several phases in sequence (e.g. scan hashing, then burn staging); `start`
  * resets any accumulated state. `advance` is called from parallel fibers —
  * implementations must be thread-safe and cheap. */
trait ProgressSink:
  def start(label: String, totalBytes: Long, totalFiles: Int): UIO[Unit]
  def advance(deltaBytes: Long, deltaFiles: Int): UIO[Unit]
  def done: UIO[Unit]

  /** Bracket one phase: `start` before `work`, `done` on any exit. */
  final def phase[R, E, A](label: String, totalBytes: Long, totalFiles: Int)(
      work: ZIO[R, E, A]): ZIO[R, E, A] =
    start(label, totalBytes, totalFiles) *> work.ensuring(done)

object ProgressSink:
  object Noop extends ProgressSink:
    def start(label: String, totalBytes: Long, totalFiles: Int): UIO[Unit] = ZIO.unit
    def advance(deltaBytes: Long, deltaFiles: Int): UIO[Unit]              = ZIO.unit
    def done: UIO[Unit]                                                    = ZIO.unit

  val noop: ZLayer[Any, Nothing, ProgressSink] = ZLayer.succeed(Noop)
```

Create `shell/src/main/scala/diskrement/shell/fs/HashParallelism.scala`:

```scala
package diskrement.shell.fs

import zio._

/** Bounded fiber count for hashing phases (scan + staging re-hash).
  * `DISKREMENT_HASH_PARALLELISM` overrides the default of 6; non-numeric or
  * non-positive values are ignored. Read via zio.System so tests can inject.
  * The value is read once at layer construction and fixed for the process;
  * values are trimmed and clamped to 64. */
final case class HashParallelism(value: Int)

object HashParallelism:
  val default: HashParallelism = HashParallelism(6)

  /** Values above this are almost certainly a misunderstanding of the knob
    * (each hashing fiber holds an open stream and a 64 KiB buffer). */
  private val MaxParallelism = 64

  val fromEnv: ZLayer[Any, Nothing, HashParallelism] =
    ZLayer {
      zio.System.env("DISKREMENT_HASH_PARALLELISM").orElseSucceed(None).flatMap {
        case None => ZIO.succeed(default)
        case Some(raw) if raw.trim.isEmpty => ZIO.succeed(default)
        case Some(raw) =>
          raw.trim.toIntOption.filter(_ > 0) match
            case Some(n) if n > MaxParallelism =>
              ZIO.logWarning(s"clamping DISKREMENT_HASH_PARALLELISM $n to $MaxParallelism")
                .as(HashParallelism(MaxParallelism))
            case Some(n) => ZIO.succeed(HashParallelism(n))
            case None =>
              ZIO.logWarning(
                s"ignoring unusable DISKREMENT_HASH_PARALLELISM '$raw' (want a positive int); using ${default.value}")
                .as(default)
      }
    }
```

- [ ] **Step 4: Run to verify pass**

Run: `sbt "shell/testOnly diskrement.shell.fs.HashParallelismSpec"`
Expected: 3 tests PASS. (If `TestSystem.putEnv` needs a different provideLayer
shape under this zio-test version, follow the pattern used by
`PipelineCapacityOverrideSpec`, which already injects env vars for
`DISKREMENT_DISC_BYTES`.)

- [ ] **Step 5: Commit**

```bash
git add shell/src/main/scala/diskrement/shell/ProgressSink.scala \
        shell/src/main/scala/diskrement/shell/fs/HashParallelism.scala \
        shell/src/test/scala/diskrement/shell/fs/HashParallelismSpec.scala
git commit -m "feat(shell): ProgressSink port and HashParallelism config"
```

---

### Task 3: `FileScanner` rework — `ScanOutput`, partition, parallel hash, progress

**Files:**
- Modify: `shell/src/main/scala/diskrement/shell/fs/FileScanner.scala` (full rewrite below)
- Modify: `shell/src/test/scala/diskrement/shell/fs/FileScannerSpec.scala`
- Modify: `shell/src/main/scala/diskrement/shell/pipeline/PipelineLive.scala:78` (compile fix only)
- Modify: `shell/src/main/scala/diskrement/shell/pipeline/Pipeline.scala` (layer requirements)
- Modify: `cli/src/main/scala/diskrement/cli/Runner.scala` (add `ProgressSink.noop` + `HashParallelism.fromEnv` to both stacks — replaced by the real renderer in Task 4)
- Possibly modify: pipeline test specs that build the `Pipeline` layer (grep step below)

- [ ] **Step 1: Update the spec (failing tests first)**

Rewrite `shell/src/test/scala/diskrement/shell/fs/FileScannerSpec.scala`. Keep the
four existing test intents, adapt to the new return type, and add three tests:

```scala
package diskrement.shell.fs

import diskrement.core.{ContentHash, RelPath}
import diskrement.shell.ProgressSink
import diskrement.shell.model.FileFingerprint
import zio._
import zio.test._
import java.io.IOException
import java.nio.file.{Path, Paths}
import java.time.Instant

object FileScannerSpec extends ZIOSpecDefault:
  private val mt1  = Instant.parse("2026-05-01T00:00:00Z")
  private val mt2  = Instant.parse("2026-05-02T00:00:00Z")
  private val root = Paths.get("/snap")

  /** In-memory FileSystem: a fixed relpath -> stat map under `root`. */
  private final case class FakeFs(files: Map[String, FileStat]) extends FileSystem:
    def walk(r: Path): IO[IOException, List[Path]] =
      ZIO.succeed(files.keys.toList.map(root.resolve))
    def stat(p: Path): IO[IOException, FileStat] =
      files.get(root.relativize(p).toString) match
        case Some(s) => ZIO.succeed(s)
        case None    => ZIO.fail(new IOException(s"no such file $p"))

  /** Hasher that returns "H:<filename>" and counts how many times it ran. */
  private final case class CountingHasher(ref: Ref[Int]) extends Hasher:
    def hash(p: Path): IO[IOException, ContentHash] =
      ref.update(_ + 1).as(ContentHash("H:" + p.getFileName.toString))

  /** Hasher that fails for one specific file name. */
  private final case class FailingHasher(bad: String) extends Hasher:
    def hash(p: Path): IO[IOException, ContentHash] =
      if p.getFileName.toString == bad then ZIO.fail(new IOException("boom"))
      else ZIO.succeed(ContentHash("H:" + p.getFileName.toString))

  /** Records start/advance/done calls. */
  private final case class RecordingSink(ref: Ref[List[String]]) extends ProgressSink:
    def start(label: String, totalBytes: Long, totalFiles: Int): UIO[Unit] =
      ref.update(_ :+ s"start:$label:$totalBytes:$totalFiles")
    def advance(deltaBytes: Long, deltaFiles: Int): UIO[Unit] =
      ref.update(_ :+ s"advance:$deltaBytes:$deltaFiles")
    def done: UIO[Unit] = ref.update(_ :+ "done")

  private def run(files: Map[String, FileStat],
                  prior: Map[RelPath, FileFingerprint],
                  verify: Boolean): UIO[(ScanOutput, Int)] =
    for
      counter <- Ref.make(0)
      scanner  = FileScannerLive(FakeFs(files), CountingHasher(counter), ProgressSink.Noop, 4)
      out     <- scanner.scan(root, prior, verify).orDie
      hashed  <- counter.get
    yield (out, hashed)

  def spec = suite("FileScanner")(
    test("fast path reuses the stored hash when size+mtime match (no hashing)"):
      val files = Map("a" -> FileStat(10, mt1), "b" -> FileStat(20, mt1))
      val prior = Map(
        RelPath("a") -> FileFingerprint(10, mt1, ContentHash("old-a")),
        RelPath("b") -> FileFingerprint(20, mt1, ContentHash("old-b")))
      run(files, prior, verify = false).map { case (out, hashed) =>
        assertTrue(
          hashed == 0,
          out.hashedFiles == 0,
          out.files.map(f => (f.relPath.value, f.contentHash.value)) ==
            List(("a", "old-a"), ("b", "old-b")))
      }
    ,
    test("a size or mtime change forces a re-hash"):
      val files = Map("a" -> FileStat(10, mt2), "b" -> FileStat(99, mt1))
      val prior = Map(
        RelPath("a") -> FileFingerprint(10, mt1, ContentHash("old-a")),
        RelPath("b") -> FileFingerprint(20, mt1, ContentHash("old-b")))
      run(files, prior, verify = false).map { case (out, hashed) =>
        assertTrue(hashed == 2, out.hashedFiles == 2,
          out.files.map(_.contentHash.value) == List("H:a", "H:b"))
      }
    ,
    test("verify forces a re-hash even when the fingerprint matches"):
      val files = Map("a" -> FileStat(10, mt1))
      val prior = Map(RelPath("a") -> FileFingerprint(10, mt1, ContentHash("old-a")))
      run(files, prior, verify = true).map { case (out, hashed) =>
        assertTrue(hashed == 1, out.hashedFiles == 1,
          out.files.head.contentHash.value == "H:a")
      }
    ,
    test("a file absent from prior is hashed; output is sorted by relPath"):
      val files = Map("z" -> FileStat(1, mt1), "a" -> FileStat(2, mt1))
      run(files, Map.empty, verify = false).map { case (out, hashed) =>
        assertTrue(hashed == 2, out.files.map(_.relPath.value) == List("a", "z"))
      }
    ,
    test("many files hash to the same sorted result under parallelism"):
      val files = (1 to 100).map(i => f"f$i%03d" -> FileStat(i.toLong, mt1)).toMap
      run(files, Map.empty, verify = false).map { case (out, hashed) =>
        assertTrue(hashed == 100, out.hashedFiles == 100,
          out.files.map(_.relPath.value) == (1 to 100).map(i => f"f$i%03d").toList)
      }
    ,
    test("a hash failure fails the scan with FileFailed for that file"):
      val files = Map("good" -> FileStat(1, mt1), "bad" -> FileStat(2, mt1))
      val scanner = FileScannerLive(FakeFs(files), FailingHasher("bad"), ProgressSink.Noop, 4)
      scanner.scan(root, Map.empty, verify = false).exit.map {
        case Exit.Failure(c) =>
          assertTrue(c.failureOption.exists {
            case ScanError.FileFailed(rel, _) => rel.value == "bad"
          })
        case Exit.Success(_) => assertTrue(false)
      }
    ,
    test("progress covers only the files that are actually hashed"):
      val files = Map("hit" -> FileStat(10, mt1), "miss" -> FileStat(32, mt1))
      val prior = Map(RelPath("hit") -> FileFingerprint(10, mt1, ContentHash("old")))
      for
        counter <- Ref.make(0)
        events  <- Ref.make(List.empty[String])
        scanner  = FileScannerLive(FakeFs(files), CountingHasher(counter), RecordingSink(events), 4)
        _       <- scanner.scan(root, prior, verify = false).orDie
        evs     <- events.get
      yield assertTrue(
        evs.head == "start:hashing:32:1",
        evs.count(_.startsWith("advance:")) == 1,
        evs.contains("advance:32:1"),
        evs.last == "done")
  )
```

- [ ] **Step 2: Run to verify failure**

Run: `sbt "shell/testOnly diskrement.shell.fs.FileScannerSpec"`
Expected: compile errors — `ScanOutput` not found, `FileScannerLive` arity.

- [ ] **Step 3: Rewrite `FileScanner.scala`**

Replace the whole file `shell/src/main/scala/diskrement/shell/fs/FileScanner.scala`:

```scala
package diskrement.shell.fs

import diskrement.core.{RelPath, ScannedFile}
import diskrement.shell.ProgressSink
import diskrement.shell.model.FileFingerprint
import zio._
import java.nio.file.Path

/** Expected scan failure: a specific file could not be walked/statted/hashed. */
sealed abstract class ScanError(msg: String, cause: Throwable = null)
    extends Exception(msg, cause)
object ScanError:
  final case class FileFailed(relPath: RelPath, cause: Throwable)
      extends ScanError(s"failed scanning ${relPath.value}", cause)

/** The scanner's result: hashed files plus how many actually got hashed this
  * run (fast-path fingerprint hits are the difference). */
final case class ScanOutput(files: List[ScannedFile], hashedFiles: Int)

/** Walks a frozen snapshot and produces fully-hashed ScannedFiles, reusing a
  * stored hash when size+mtime match (fast path) unless `verify` is set.
  * Fast-path misses are hashed in bounded parallel; progress is reported for
  * the miss set only (that is the work the operator is waiting on). */
trait FileScanner:
  def scan(
      snapshotRoot: Path,
      prior: Map[RelPath, FileFingerprint],
      verify: Boolean
  ): IO[ScanError, ScanOutput]

final case class FileScannerLive(
    fs: FileSystem, hasher: Hasher, sink: ProgressSink, parallelism: Int
) extends FileScanner:

  def scan(
      snapshotRoot: Path,
      prior: Map[RelPath, FileFingerprint],
      verify: Boolean
  ): IO[ScanError, ScanOutput] =
    for
      paths  <- fs.walk(snapshotRoot)
                  .mapError(e => ScanError.FileFailed(RelPath(snapshotRoot.toString), e))
      stats  <- ZIO.foreach(paths) { abs =>
                  val rel = RelPath(snapshotRoot.relativize(abs).toString)
                  fs.stat(abs).mapBoth(ScanError.FileFailed(rel, _), st => (abs, rel, st))
                }
      parts   = stats.partition { case (_, rel, st) =>
                  !verify && prior.get(rel)
                    .exists(fp => fp.sizeBytes == st.sizeBytes && fp.mtime == st.mtime)
                }
      hits    = parts._1
      misses  = parts._2
      reused  = hits.map { case (_, rel, st) =>
                  ScannedFile(rel, prior(rel).contentHash, st.sizeBytes, st.mtime)
                }
      // phase() brackets the hash work: start before, done on ANY exit
      // (success, failure, interrupt) — never leave the TTY line half-drawn.
      hashed <- sink.phase("hashing", misses.map(_._3.sizeBytes).sum, misses.size) {
                  ZIO.foreachPar(misses) { case (abs, rel, st) =>
                    hasher.hash(abs)
                      .mapBoth(ScanError.FileFailed(rel, _),
                               h => ScannedFile(rel, h, st.sizeBytes, st.mtime))
                      .zipLeft(sink.advance(st.sizeBytes, 1))
                  }.withParallelism(parallelism)
                }
    yield ScanOutput((reused ++ hashed).sortBy(_.relPath), hashed.size)

object FileScanner:
  val live: ZLayer[FileSystem & Hasher & ProgressSink & HashParallelism, Nothing, FileScanner] =
    ZLayer {
      for
        fs   <- ZIO.service[FileSystem]
        h    <- ZIO.service[Hasher]
        sink <- ZIO.service[ProgressSink]
        par  <- ZIO.service[HashParallelism]
      yield FileScannerLive(fs, h, sink, par.value)
    }
```

- [ ] **Step 4: Compile fixes in pipeline and CLI**

`shell/src/main/scala/diskrement/shell/pipeline/PipelineLive.scala` — in
`freshScan`, the scanner now returns `ScanOutput`:

```scala
      scanned <- scanner.scan(handle.root, prior, verify).mapError(PipelineError.Failed(_))
      _       <- staged.writeScan(paths.workDir, scanned.files)
      res     <- countOf(p.id, scanned.files)
```

(The `hashedFiles` count is wired into `ScanResult` in Task 5; ignore it here.)

`cli/src/main/scala/diskrement/cli/Runner.scala` — add to BOTH backend stacks in
`withPipeline` (imports: `diskrement.shell.ProgressSink`; `HashParallelism` comes
in with the existing `diskrement.shell.fs.*` wildcard):

```scala
          ProgressSink.noop, HashParallelism.fromEnv,
```

(placed alongside `FileSystem.live, Hasher.live, FileScanner.live`).

`shell/src/test/scala/diskrement/shell/pipeline/PipelineTestKit.scala` — `env`
builds `FileScanner.live`, so add to its `ZLayer.make` inputs:

```scala
      diskrement.shell.ProgressSink.noop, ZLayer.succeed(HashParallelism(4)),
```

Then grep for any other constructor of the scanner or `Pipeline` layer in tests
and give it the same treatment:

```bash
grep -rn "FileScannerLive\|FileScanner.live\|Pipeline.live" shell/src/test cli/src/test
```

- [ ] **Step 5: Run to verify pass**

Run: `sbt shell/test cli/test`
Expected: all green, including the 7 FileScannerSpec tests.

- [ ] **Step 6: Commit**

```bash
git add -A shell/src cli/src
git commit -m "feat(scan): parallel hashing with progress; scanner returns ScanOutput with hashed count"
```

---

### Task 4: CLI progress renderer

**Files:**
- Create: `cli/src/main/scala/diskrement/cli/ConsoleProgress.scala`
- Test: `cli/src/test/scala/diskrement/cli/ConsoleProgressSpec.scala`
- Modify: `cli/src/main/scala/diskrement/cli/Runner.scala` (swap `ProgressSink.noop` → `ConsoleProgress.live` in both stacks)

- [ ] **Step 1: Write the failing test** (the line formatting is pure; test that)

Create `cli/src/test/scala/diskrement/cli/ConsoleProgressSpec.scala`:

```scala
package diskrement.cli

import zio.test._

object ConsoleProgressSpec extends ZIOSpecDefault:
  def spec = suite("ConsoleProgress.format")(
    test("renders bytes, files, rate and ETA"):
      val line = ConsoleProgress.format(
        label = "hashing", doneBytes = 12_300_000_000L, totalBytes = 413_600_000_000L,
        doneFiles = 3021, totalFiles = 75618, rateBytesPerSec = 840_000_000d, etaSeconds = 432L)
      assertTrue(line == "hashing 12.3/413.6 GB · 3021/75618 files · 840 MB/s · ETA 7m12s")
    ,
    test("zero totals do not divide by zero"):
      val line = ConsoleProgress.format("hashing", 0L, 0L, 0, 0, 0d, 0L)
      assertTrue(line == "hashing 0.0/0.0 GB · 0/0 files · 0 MB/s · ETA 0m00s")
  )
```

- [ ] **Step 2: Run to verify failure**

Run: `sbt "cli/testOnly diskrement.cli.ConsoleProgressSpec"`
Expected: compile error — `ConsoleProgress` not found.

- [ ] **Step 3: Implement**

Create `cli/src/main/scala/diskrement/cli/ConsoleProgress.scala`:

```scala
package diskrement.cli

import diskrement.shell.ProgressSink
import zio._

/** Stderr progress renderer. On a TTY: one carriage-return-refreshed line,
  * throttled to ~1/s. Not a TTY: a plain line every ~10 s (so logs stay small
  * and IT stdout parsing is never touched). Writes ONLY to stderr. */
object ConsoleProgress:

  def format(label: String, doneBytes: Long, totalBytes: Long,
             doneFiles: Int, totalFiles: Int,
             rateBytesPerSec: Double, etaSeconds: Long): String =
    f"$label ${doneBytes / 1e9}%.1f/${totalBytes / 1e9}%.1f GB · " +
      f"$doneFiles%d/$totalFiles%d files · ${rateBytesPerSec / 1e6}%.0f MB/s · " +
      f"ETA ${etaSeconds / 60}%dm${etaSeconds % 60}%02ds"

  private final case class State(
      label: String, totalBytes: Long, totalFiles: Int,
      doneBytes: Long, doneFiles: Int,
      startedNanos: Long, lastEmitNanos: Long, emitted: Boolean)

  private final class Live(ref: Ref[Option[State]], tty: Boolean) extends ProgressSink:
    private val intervalNanos = if tty then 1_000_000_000L else 10_000_000_000L

    def start(label: String, totalBytes: Long, totalFiles: Int): UIO[Unit] =
      Clock.nanoTime.flatMap(now =>
        ref.set(Some(State(label, totalBytes, totalFiles, 0L, 0, now, now, false))))

    def advance(deltaBytes: Long, deltaFiles: Int): UIO[Unit] =
      Clock.nanoTime.flatMap { now =>
        ref.modify {
          case Some(s) =>
            val s2 = s.copy(doneBytes = s.doneBytes + deltaBytes,
                            doneFiles = s.doneFiles + deltaFiles)
            if now - s.lastEmitNanos >= intervalNanos then
              (Some(s2), Some(s2.copy(lastEmitNanos = now, emitted = true)))
            else (None, Some(s2))
          case None => (None, None)
        }.flatMap {
          case Some(snap) => emit(snap, now)
          case None       => ZIO.unit
        }
      }

    def done: UIO[Unit] =
      ref.getAndSet(None).flatMap {
        case Some(s) if s.emitted && tty =>
          ZIO.succeed(java.lang.System.err.println())
        case _ => ZIO.unit
      }

    private def emit(s: State, now: Long): UIO[Unit] =
      ZIO.succeed {
        val elapsedSec = math.max((now - s.startedNanos) / 1e9, 1e-9)
        val rate       = s.doneBytes / elapsedSec
        val eta        = if rate > 0 then ((s.totalBytes - s.doneBytes) / rate).toLong else 0L
        val line       = format(s.label, s.doneBytes, s.totalBytes,
                                s.doneFiles, s.totalFiles, rate, eta)
        if tty then java.lang.System.err.print("\r" + line)
        else java.lang.System.err.println(line)
      }

  val live: ULayer[ProgressSink] =
    ZLayer {
      Ref.make(Option.empty[State])
        .map(ref => Live(ref, java.lang.System.console() != null))
    }
```

- [ ] **Step 4: Swap it into `Runner`**

In `cli/src/main/scala/diskrement/cli/Runner.scala`, replace `ProgressSink.noop,`
with `ConsoleProgress.live,` in BOTH backend stacks (the `ProgressSink` import
can then go if unused).

- [ ] **Step 5: Run to verify pass**

Run: `sbt cli/test shell/test`
Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add cli/src/main/scala/diskrement/cli/ConsoleProgress.scala \
        cli/src/test/scala/diskrement/cli/ConsoleProgressSpec.scala \
        cli/src/main/scala/diskrement/cli/Runner.scala
git commit -m "feat(cli): stderr progress renderer for scan/staging"
```

---

### Task 5: Wire the cache into the pipeline; `ScanResult.hashed`; render + paths

**Files:**
- Modify: `shell/src/main/scala/diskrement/shell/pipeline/Pipeline.scala` (`ScanResult`, layer)
- Modify: `shell/src/main/scala/diskrement/shell/pipeline/PipelineLive.scala` (`freshScan`, `countOf`, constructor)
- Modify: `cli/src/main/scala/diskrement/cli/PathResolver.scala` (+ scancache path)
- Modify: `cli/src/main/scala/diskrement/cli/Runner.scala` (CachePath layer, pass-through)
- Modify: `cli/src/main/scala/diskrement/cli/Render.scala` (`scan` second line)
- Create: `shell/src/test/scala/diskrement/shell/pipeline/FakeFingerprintCache.scala`
- Test (modify): `cli/src/test/scala/diskrement/cli/RenderSpec.scala`, `cli/src/test/scala/diskrement/cli/PathResolverSpec.scala`, pipeline specs
- Test (create): `shell/src/test/scala/diskrement/shell/pipeline/PipelineRescanSpec.scala`

- [ ] **Step 1: Failing tests — render, path, and the crown-jewel rescan behavior**

In `cli/src/test/scala/diskrement/cli/RenderSpec.scala`, update the scan
expectation (find the existing `Render.scan` test) to:

```scala
      Render.scan(ScanResult(2, 1, 0, 3, hashed = 3)) ==
        "Scanned: 2 added, 1 modified, 0 deleted, 3 unchanged.\n" +
        "Hashed 3 of 6 files (3 fingerprints reused)."
```

In `cli/src/test/scala/diskrement/cli/PathResolverSpec.scala`, add:

```scala
    test("derives the scancache path next to the catalog"):
      val r = PathResolver.resolve("p",
        GlobalOpts(catalog = Some(Paths.get("/data/cat/proj.sqlite")), dataDir = None,
                   device = None, burner = None),
        _ => None, Paths.get("/home/u"))
      assertTrue(r.scanCachePath == Paths.get("/data/cat/proj.scancache.sqlite"))
```

(Match the actual `GlobalOpts` constructor — check `Args.scala` — and the
existing spec's assertion style.)

Create `shell/src/test/scala/diskrement/shell/pipeline/FakeFingerprintCache.scala`:

```scala
package diskrement.shell.pipeline

import diskrement.core.RelPath
import diskrement.shell.fs.{CacheError, FingerprintCache}
import diskrement.shell.model.FileFingerprint
import zio._

/** In-memory cache for pipeline specs. */
final case class FakeFingerprintCache(ref: Ref[Map[RelPath, FileFingerprint]])
    extends FingerprintCache:
  def readAll: IO[CacheError, Map[RelPath, FileFingerprint]] = ref.get
  def putAll(entries: Map[RelPath, FileFingerprint]): IO[CacheError, Unit] =
    ref.update(_ ++ entries)

object FakeFingerprintCache:
  val layer: ULayer[FingerprintCache] =
    ZLayer(Ref.make(Map.empty[RelPath, FileFingerprint]).map(FakeFingerprintCache(_)))
```

Update `PipelineTestKit.mkPipeline` for `PipelineLive`'s new constructor by
creating a fresh in-memory cache internally — existing call sites stay
`mkPipeline(FakeSnapshot())`, and a scan + rescan through the same pipeline
instance share the cache:

```scala
  def mkPipeline(snapshot: SourceSnapshot): URIO[
      ProjectPaths & CatalogStore & FileScanner & Burner & DiscVerifier & StagedStore & Hasher,
      Pipeline] =
    for
      paths    <- ZIO.service[ProjectPaths]
      catalog  <- ZIO.service[CatalogStore]
      scanner  <- ZIO.service[FileScanner]
      burner   <- ZIO.service[Burner]
      verifier <- ZIO.service[DiscVerifier]
      staged   <- ZIO.service[StagedStore]
      hasher   <- ZIO.service[Hasher]
      cacheRef <- Ref.make(Map.empty[diskrement.core.RelPath, diskrement.shell.model.FileFingerprint])
    yield PipelineLive(catalog, scanner, snapshot, burner, verifier, staged, hasher,
      FakeFingerprintCache(cacheRef), paths)
```

Create `shell/src/test/scala/diskrement/shell/pipeline/PipelineRescanSpec.scala`
(same fixture shape as `PipelineBurnSpec`):

```scala
package diskrement.shell.pipeline

import diskrement.core._
import diskrement.shell.model._
import zio._
import zio.test._
import java.time.Instant

object PipelineRescanSpec extends ZIOSpecDefault:
  private def initOf(name: String, src: java.nio.file.Path, work: java.nio.file.Path) =
    ProjectInit(name, src.toString, work.toString, DiscType.BD_R, 2.0, 1,
      Instant.parse("2026-08-31T00:00:00Z"))

  def spec = suite("Pipeline rescan with fingerprint cache")(
    test("a forced rescan after a first scan hashes nothing (cache primed)"):
      ZIO.scoped {
        for
          paths  <- ZIO.service[ProjectPaths]
          src    <- PipelineTestKit.makeSource(
                      Map("a.txt" -> "aaa", "b.txt" -> "bb", "sub/c.txt" -> "c"))
          pipe   <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _      <- pipe.init(initOf("p", src, paths.workDir))
          first  <- pipe.scan("p", verify = false, force = false)
          second <- pipe.scan("p", verify = false, force = true)
        yield assertTrue(first.hashed == 3, second.hashed == 0,
          second.added == 3, second.unchanged == 0)
      }
  ).provideLayer(PipelineTestKit.env.orDie)
```

(Check how `PipelineBurnSpec` provides `PipelineTestKit.env` and mirror it
exactly — `.orDie` vs `mapError` may differ.)

- [ ] **Step 2: Run to verify failure**

Run: `sbt shell/test cli/test`
Expected: compile errors (`ScanResult` arity, `scanCachePath` missing).

- [ ] **Step 3: Implement**

`Pipeline.scala`:

```scala
final case class ScanResult(added: Int, modified: Int, deleted: Int, unchanged: Int, hashed: Int)
```

and add `FingerprintCache` to the layer:

```scala
object Pipeline:
  val live: ZLayer[
    ProjectPaths & CatalogStore & FileScanner & SourceSnapshot & Burner & DiscVerifier &
      StagedStore & Hasher & FingerprintCache,
    Nothing, Pipeline] =
    ZLayer {
      for
        paths    <- ZIO.service[ProjectPaths]
        catalog  <- ZIO.service[CatalogStore]
        scanner  <- ZIO.service[FileScanner]
        snapshot <- ZIO.service[SourceSnapshot]
        burner   <- ZIO.service[Burner]
        verifier <- ZIO.service[DiscVerifier]
        staged   <- ZIO.service[StagedStore]
        hasher   <- ZIO.service[Hasher]
        cache    <- ZIO.service[FingerprintCache]
      yield PipelineLive(catalog, scanner, snapshot, burner, verifier, staged, hasher, cache, paths)
    }
```

`PipelineLive.scala` — constructor gains `cache: FingerprintCache` (before
`paths`); `freshScan` becomes:

```scala
  private def freshScan(p: Project, verify: Boolean): IO[PipelineError, ScanResult] =
    for
      srcPath <- ZIO.attempt(Path.of(p.sourceRoot)).mapError(PipelineError.Failed(_))
      handle  <- snapshot.create(srcPath, paths.workDir)
                   .mapError(e => PipelineError.Failed(e))
      prior   <- catalog.fingerprints(p.id).mapError(PipelineError.Failed(_))
      cached  <- cache.readAll.catchAll(e =>
                   ZIO.logWarning(s"fingerprint cache read failed: ${e.getMessage}")
                     .as(Map.empty[RelPath, FileFingerprint]))
      // The cache wins on conflict: it is fresher (it re-fingerprints
      // touched-but-identical files so later scans fast-path them).
      scanned <- scanner.scan(handle.root, prior ++ cached, verify)
                   .mapError(PipelineError.Failed(_))
      _       <- staged.writeScan(paths.workDir, scanned.files)
      _       <- cache.putAll(scanned.files.iterator.map(f =>
                     f.relPath -> FileFingerprint(f.sizeBytes, f.mtime, f.contentHash)).toMap)
                   .catchAll(e =>
                     ZIO.logWarning(s"fingerprint cache write failed: ${e.getMessage}"))
      res     <- countOf(p.id, scanned.files, scanned.hashedFiles)
    yield res
```

`countOf` gains the count (staged-reuse call site passes `hashed = 0`):

```scala
  private def countOf(pid: ProjectId, scanned: List[ScannedFile], hashed: Int): IO[PipelineError, ScanResult] =
    catalog.readView(pid).mapError(PipelineError.Failed(_)).map { view =>
      val cs = Diff.diff(view, scanned)
      ScanResult(cs.added.size, cs.modified.size, cs.deleted.size, cs.unchanged.size, hashed)
    }
```

(Imports: `FileFingerprint` is in `diskrement.shell.model.*`, already wildcarded;
add `FingerprintCache` to the `diskrement.shell.fs` selective import.)

`Render.scala`:

```scala
  def scan(r: ScanResult): String =
    val total  = r.added + r.modified + r.unchanged
    val reused = total - r.hashed
    s"Scanned: ${r.added} added, ${r.modified} modified, ${r.deleted} deleted, ${r.unchanged} unchanged.\n" +
      s"Hashed ${r.hashed} of $total files ($reused fingerprints reused)."
```

`PathResolver.scala` — `ResolvedPaths` gains the cache path, derived in both
branches:

```scala
final case class ResolvedPaths(catalogPath: Path, dataDir: Path, scanCachePath: Path)
```

```scala
  private def cacheFor(catalog: Path): Path =
    val n    = catalog.getFileName.toString
    val stem = if n.endsWith(".sqlite") then n.dropRight(".sqlite".length) else n
    catalog.resolveSibling(s"$stem.scancache.sqlite")
```

— `--catalog` branch: `ResolvedPaths(abs, parent, cacheFor(abs))`; default
branch: `ResolvedPaths(dataDir.resolve(s"$projectName.sqlite"), dataDir, cacheFor(dataDir.resolve(s"$projectName.sqlite")))`
(bind the catalog path to a val first).

`Runner.scala` — thread the cache path into `withPipeline`:

```scala
  private def withPipeline[A](catalogPath: Path, cachePath: Path, workDir: Path, backend: Backend)(...)
```

both call sites pass `resolved.scanCachePath` (thread it through `withProject`
too), and both layer stacks add:

```scala
          ZLayer.succeed(CachePath(cachePath)), FingerprintCache.live,
```

- [ ] **Step 4: Run to verify pass**

Run: `sbt shell/test cli/test`
Expected: all green, including `PipelineRescanSpec` (first.hashed == 3,
second.hashed == 0) — this is the P2 acceptance behavior in miniature.

- [ ] **Step 5: Commit**

```bash
git add -A shell/src cli/src
git commit -m "feat(pipeline): sidecar fingerprint cache wiring; scan reports hashed/reused counts"
```

---

### Task 6: Reflink staging with parallel re-hash

**Files:**
- Create: `shell/src/main/scala/diskrement/shell/fs/Reflink.scala`
- Modify: `shell/src/main/scala/diskrement/shell/fs/SourceSnapshot.scala` (use `Reflink.cloneUnsupported`, delete private duplicate)
- Modify: `shell/src/main/scala/diskrement/shell/burn/Stager.scala` (full rewrite below)
- Modify: `shell/src/main/scala/diskrement/shell/burn/SimulatedBurner.scala`, `shell/src/main/scala/diskrement/shell/burn/XorrisoBurner.scala` (layers gain `ProgressSink & HashParallelism`, pass into `Stager.stage`)
- Test (modify): `shell/src/test/scala/diskrement/shell/burn/SimulatedBurnerSpec.scala` and any other spec constructing these layers / calling `Stager.stage`

- [ ] **Step 1: Failing test — staging produces identical results on any filesystem**

Add to `SimulatedBurnerSpec`, reusing its `makeWorld`/`meta`/`sha256` helpers
(the fixture lives under `Files.createTempDirectory` — often tmpfs = no reflink
→ exercises the fallback; on a CoW temp dir it exercises the reflink path; the
staged result must be identical either way, and the existing first test already
pins content equality on the repo fs):

```scala
    test("staging preserves the source mtimes on the staged tree"):
      ZIO.scoped {
        for
          world <- makeWorld
          (snap, staging, cat) = world
          srcMtime <- ZIO.attempt(Files.getLastModifiedTime(snap.resolve("a.txt")))
          files = List(ScannedFile(RelPath("a.txt"), ContentHash(sha256("aaa")), 3, ts))
          image = DiscImage(staging, snap, files, meta(cat, files), closeDisc = false)
          _     <- ZIO.serviceWithZIO[Burner](_.burn(image))
          dstMtime <- ZIO.attempt(Files.getLastModifiedTime(staging.resolve("data/a.txt")))
        yield assertTrue(dstMtime == srcMtime)
      },
```

and update the suite's layer for the new `SimulatedBurner.live` requirements:

```scala
  ).provideLayer(
    (Hasher.live ++ SnapshotWriter.live ++ diskrement.shell.ProgressSink.noop ++
      ZLayer.succeed(diskrement.shell.fs.HashParallelism(2))) >>> SimulatedBurner.live)
```

Also update every other call site of `Stager.stage(image, hasher, writer)` /
`SimulatedBurner.live` / `XorrisoBurner.live` for the new signatures using
`ProgressSink.Noop` / `ProgressSink.noop` and `HashParallelism(2)` — grep:

```bash
grep -rn "Stager.stage\|SimulatedBurner.live\|XorrisoBurner.live" shell/src cli/src
```

(`PipelineTestKit.env` already provides both layers after Task 3, so its
`SimulatedBurner.live` line needs no change.)

- [ ] **Step 2: Run to verify failure**

Run: `sbt "shell/testOnly diskrement.shell.burn.SimulatedBurnerSpec"`
Expected: compile error (arity) or new test FAIL.

- [ ] **Step 3: Implement**

Create `shell/src/main/scala/diskrement/shell/fs/Reflink.scala`:

```scala
package diskrement.shell.fs

/** Shared detection of "this filesystem cannot reflink" from cp's stderr, used
  * by the snapshot cloner (hard failure: CoW is required there) and the stager
  * (soft failure: falls back to plain copy). */
object Reflink:
  def cloneUnsupported(stderr: String): Boolean =
    val s = stderr.toLowerCase
    s.contains("failed to clone") || s.contains("operation not supported")
```

In `SourceSnapshot.scala`, delete the private `isCloneUnsupported` and call
`Reflink.cloneUnsupported(stderr)` instead.

Rewrite `shell/src/main/scala/diskrement/shell/burn/Stager.scala`:

```scala
package diskrement.shell.burn

import diskrement.core.ScannedFile
import diskrement.shell.ProgressSink
import diskrement.shell.fs.{Hasher, Reflink}
import zio._
import java.nio.file.{Files, Path, StandardCopyOption}
import scala.jdk.StreamConverters.*

/** Stages a full disc tree (/data + /.diskrement) to image.stagingDir and
  * re-hashes every staged file against its scan hash (§6.2). The copy is a
  * batched reflink clone (free on the CoW fs the pipeline requires), falling
  * back to a plain copy where cloning is unsupported (e.g. tests on tmpfs).
  * Shared by the simulated and real burners: for the real burner the staged
  * tree is exactly what xorriso masters. */
object Stager:
  private val BatchSize = 500

  def stage(
      image: DiscImage, hasher: Hasher, writer: SnapshotWriter,
      sink: ProgressSink, parallelism: Int
  ): IO[BurnError, BurnResult] =
    val dataDir = image.stagingDir.resolve("data")
    for
      _     <- clearAndCreate(dataDir)
      _     <- ZIO.foreachDiscard(image.files)(checkExists(image, _))
      _     <- copyAll(image, dataDir)
      _     <- sink.phase("staging", image.files.map(_.sizeBytes).sum, image.files.size) {
                 ZIO.foreachParDiscard(image.files)(verifyOne(dataDir, hasher, sink, _))
                   .withParallelism(parallelism)
               }
      _     <- writer.write(image.stagingDir.resolve(".diskrement"), image.metadata)
      bytes <- totalBytes(image.stagingDir)
    yield BurnResult(image.stagingDir, bytes)

  private def clearAndCreate(dataDir: Path): IO[BurnError, Unit] =
    ZIO.attemptBlockingIO {
      val stagingDir = dataDir.getParent
      if Files.exists(stagingDir) then
        val s = Files.walk(stagingDir)
        try s.sorted(java.util.Comparator.reverseOrder()).forEach(p => Files.deleteIfExists(p))
        finally s.close()
      Files.createDirectories(dataDir)
      ()
    }.mapError(BurnError.BurnIoError(_))

  private def checkExists(image: DiscImage, f: ScannedFile): IO[BurnError, Unit] =
    ZIO.attemptBlockingIO(Files.exists(image.snapshotRoot.resolve(f.relPath.value)))
      .mapError(BurnError.BurnIoError(_))
      .flatMap(e => ZIO.unless(e)(ZIO.fail(BurnError.SourceFileMissing(f.relPath))).unit)

  /** Copy every planned file into dataDir, preserving structure and mtimes.
    * Tries batched reflink clones; on the first "cannot clone" switches to
    * plain copies for the rest of the run. */
  private def copyAll(image: DiscImage, dataDir: Path): IO[BurnError, Unit] =
    def go(batches: List[List[String]], fallback: Boolean): IO[BurnError, Unit] =
      batches match
        case Nil => ZIO.unit
        case b :: rest =>
          if fallback then plainCopy(image, dataDir, b) *> go(rest, true)
          else
            reflinkBatch(image, dataDir, b).flatMap { cloned =>
              if cloned then go(rest, false)
              else plainCopy(image, dataDir, b) *> go(rest, true)
            }
    go(image.files.map(_.relPath.value).grouped(BatchSize).toList, fallback = false)

  /** One `cp --reflink=always --parents -p` batch run from the snapshot root.
    * Returns false iff the fs cannot reflink (caller falls back). */
  private def reflinkBatch(
      image: DiscImage, dataDir: Path, rels: List[String]): IO[BurnError, Boolean] =
    ZIO.attemptBlockingIO {
      val cmd = List("cp", "--reflink=always", "--parents", "-p", "-t", dataDir.toString) ++ rels
      val pb  = new ProcessBuilder(cmd*)
      pb.directory(image.snapshotRoot.toFile)
      val proc   = pb.start()
      val stderr = new String(proc.getErrorStream.readAllBytes(), "UTF-8")
      (proc.waitFor(), stderr)
    }.mapError(BurnError.BurnIoError(_)).flatMap { res =>
      val code   = res._1
      val stderr = res._2
      if code == 0 then ZIO.succeed(true)
      else if Reflink.cloneUnsupported(stderr) then ZIO.succeed(false)
      else ZIO.fail(BurnError.BurnIoError(
        new RuntimeException(s"cp exited $code: ${stderr.trim}")))
    }

  private def plainCopy(
      image: DiscImage, dataDir: Path, rels: List[String]): IO[BurnError, Unit] =
    ZIO.foreachDiscard(rels) { rel =>
      ZIO.attemptBlockingIO {
        val src = image.snapshotRoot.resolve(rel)
        val dst = dataDir.resolve(rel)
        Files.createDirectories(dst.getParent)
        Files.copy(src, dst, StandardCopyOption.COPY_ATTRIBUTES)
        ()
      }.mapError(BurnError.BurnIoError(_))
    }

  private def verifyOne(
      dataDir: Path, hasher: Hasher, sink: ProgressSink, f: ScannedFile): IO[BurnError, Unit] =
    for
      actual <- hasher.hash(dataDir.resolve(f.relPath.value)).mapError(BurnError.BurnIoError(_))
      _      <- ZIO.unless(actual == f.contentHash)(
                  ZIO.fail(BurnError.HashMismatch(f.relPath, f.contentHash, actual)))
      _      <- sink.advance(f.sizeBytes, 1)
    yield ()

  private def totalBytes(dir: Path): IO[BurnError, Long] =
    ZIO.attemptBlockingIO {
      val s = Files.walk(dir)
      try s.toScala(List).filter(p => Files.isRegularFile(p)).map(p => Files.size(p)).sum
      finally s.close()
    }.mapError(BurnError.BurnIoError(_))
```

Note the behavior details preserved: missing source → `SourceFileMissing`
(pre-check, before any copy); mismatch → `HashMismatch`; `clearAndCreate` still
wipes the whole staging dir. New: `COPY_ATTRIBUTES`/`-p` preserve mtimes on the
staged tree (previously fresh copy mtimes — an improvement for mastered discs).

`SimulatedBurner.scala`:

```scala
final case class SimulatedBurnerLive(
    hasher: Hasher, writer: SnapshotWriter, sink: ProgressSink, parallelism: Int
) extends Burner:
  def burn(image: DiscImage): IO[BurnError, BurnResult] =
    Stager.stage(image, hasher, writer, sink, parallelism)

object SimulatedBurner:
  val live: ZLayer[Hasher & SnapshotWriter & ProgressSink & HashParallelism, Nothing, Burner] =
    ZLayer {
      for
        hasher <- ZIO.service[Hasher]
        writer <- ZIO.service[SnapshotWriter]
        sink   <- ZIO.service[ProgressSink]
        par    <- ZIO.service[HashParallelism]
      yield SimulatedBurnerLive(hasher, writer, sink, par.value)
    }
```

(imports: add `diskrement.shell.ProgressSink` and `diskrement.shell.fs.HashParallelism`.)

`XorrisoBurner.scala`: same pattern — add `sink`/`parallelism` to the case
class, extend the layer's requirements identically, and change its
`Stager.stage(image, hasher, writer)` call (around line 27) to
`Stager.stage(image, hasher, writer, sink, parallelism)`.

- [ ] **Step 4: Run to verify pass**

Run: `sbt shell/test cli/test`
Expected: all green (Runner stacks already provide `ConsoleProgress.live` and
`HashParallelism.fromEnv` from Tasks 3–4, so `SimulatedBurner.live` /
`XorrisoBurner.live` resolve).

- [ ] **Step 5: Commit**

```bash
git add -A shell/src cli/src
git commit -m "feat(burn): reflink staging with plain-copy fallback; parallel staging re-hash with progress"
```

---### Task 7: Benchmark script

**Files:**
- Create: `scripts/perf-bench.sh` (mode 755)

- [ ] **Step 1: Write the script**

```bash
#!/usr/bin/env bash
# perf-bench.sh — end-to-end diskrement performance benchmark.
#
#   scripts/perf-bench.sh SOURCE_DIR WORK_AREA [BIN]
#
# Runs init → scan → plan → burn (simulated) → scan against SOURCE_DIR using a
# throwaway project under WORK_AREA (same filesystem as SOURCE_DIR; needs free
# space for one staged disc, ~25 GB with default BD_R sizing). SOURCE_DIR is
# only ever read. Reports per-step wall time and the JVM's cumulative block
# reads. Cleans up everything it created.
set -euo pipefail

SRC=${1:?usage: perf-bench.sh SOURCE_DIR WORK_AREA [BIN]}
AREA=${2:?usage: perf-bench.sh SOURCE_DIR WORK_AREA [BIN]}
BIN=${3:-"$(dirname "$0")/../cli/target/universal/stage/bin/diskrement"}

BASE="$AREA/perf-bench-$$"
WORK="$BASE/work"
CAT="$BASE/data/bench.sqlite"
mkdir -p "$WORK" "$BASE/data"
export DISKREMENT_BURNER=simulated

cleanup() {
  "$BIN" --burner simulated --catalog "$CAT" reset bench >/dev/null 2>&1 || true
  rm -rf "$BASE"
}
trap cleanup EXIT

# Track the peak cumulative read_bytes of the CLI's JVM while CMD runs.
run_step() { # NAME ARGS...
  local name=$1; shift
  local t0 t1 rb=0
  t0=$(date +%s)
  "$BIN" --burner simulated --catalog "$CAT" "$@" > "$BASE/$name.out" 2> "$BASE/$name.err" &
  local wrapper=$!
  while kill -0 "$wrapper" 2>/dev/null; do
    local jpid
    jpid=$(pgrep -f 'diskrement.cli.Main' | head -1 || true)
    if [ -n "$jpid" ] && [ -r "/proc/$jpid/io" ]; then
      local cur
      cur=$(awk '/^read_bytes/{print $2}' "/proc/$jpid/io" 2>/dev/null || echo 0)
      [ "${cur:-0}" -gt "$rb" ] && rb=$cur
    fi
    sleep 1
  done
  wait "$wrapper"; local code=$?
  t1=$(date +%s)
  printf '%-12s %5ss  read=%s GB  exit=%s\n' "$name" "$((t1 - t0))" "$((rb / 1000000000))" "$code"
  [ "$code" -eq 0 ] || { echo "step $name failed:"; cat "$BASE/$name.out" "$BASE/$name.err"; exit "$code"; }
}

echo "== diskrement perf-bench: $SRC =="
run_step init  init bench --source "$SRC" --work-dir "$WORK"
run_step scan1 scan bench
run_step plan  plan bench
run_step burn  burn bench --yes
run_step scan2 scan bench
grep -h "Hashed" "$BASE/scan1.out" "$BASE/scan2.out"
echo "== done (scratch cleaned) =="
```

```bash
chmod +x scripts/perf-bench.sh
```

- [ ] **Step 2: Smoke-test it on a tiny tree**

```bash
sbt cli/stage
# Source and work area must share a filesystem (init enforces it), and the work
# dir must be CoW-capable — target/ on the repo's btrfs satisfies both.
mkdir -p target/bench-src && echo hi > target/bench-src/a.txt && echo yo > target/bench-src/b.txt
scripts/perf-bench.sh "$(pwd)/target/bench-src" "$(pwd)/target"
rm -rf target/bench-src
```

Expected: five step lines, all `exit=0`; `scan2` shows `Hashed 0 of 2 files (2 fingerprints reused).`; `target/perf-bench-*` gone afterwards.

- [ ] **Step 3: Commit**

```bash
git add scripts/perf-bench.sh
git commit -m "feat(bench): end-to-end performance benchmark script"
```

---

### Task 8: Integration scenario — warm-cache rescan

**Files:**
- Create: `cli/src/it/scala/diskrement/cli/it/scenarios/Scenario10WarmRescan.scala`
  (10, not 9: scenario 9 is reserved by the written-but-unexecuted plan 4b)
- Modify: `cli/src/it/scala/diskrement/cli/it/CliIntegrationSpec.scala` (register it)

- [ ] **Step 1: Write the scenario**

```scala
package diskrement.cli.it
package scenarios

import zio.ZIO
import zio.test.*

object Scenario10WarmRescan:
  val suite = zio.test.suite("scenario 10: warm-cache rescan hashes only changes")(
    test("scan hashes all; post-burn rescan hashes none; a touched file hashes one"):
      ZIO.scoped {
        for
          s     <- Support.scratch
          cli    = new Cli(s, discBytes = None)
          _     <- ZIO.attempt(Tree.write(s.source, "a.txt", 100))
          _     <- ZIO.attempt(Tree.write(s.source, "b.txt", 200))
          _     <- ZIO.attempt(Tree.write(s.source, "c.txt", 300))
          _     <- Support.run(cli, "init", "p",
                     "--source", s.source.toString, "--work-dir", s.work.toString)
          scan1 <- Support.run(cli, "scan", "p")
          _     <- Support.run(cli, "plan", "p")
          _     <- Support.run(cli, "burn", "p", "--yes")
          scan2 <- Support.run(cli, "scan", "p")
          _     <- ZIO.attempt(Tree.write(s.source, "b.txt", 999)) // modify one file
          scan3 <- Support.run(cli, "scan", "p", "--force")
        yield assertTrue(
          scan1.exitCode == 0,
          scan1.stdout.contains("Hashed 3 of 3 files (0 fingerprints reused)."),
          scan2.exitCode == 0,
          scan2.stdout.contains("Scanned: 0 added, 0 modified, 0 deleted, 3 unchanged."),
          scan2.stdout.contains("Hashed 0 of 3 files (3 fingerprints reused)."),
          scan3.exitCode == 0,
          scan3.stdout.contains("Scanned: 0 added, 1 modified, 0 deleted, 2 unchanged."),
          scan3.stdout.contains("Hashed 1 of 3 files (2 fingerprints reused)."))
      }
  )
```

Check `Tree.write`'s actual signature (`(dir, relPath, sizeOrContent)`) in
`cli/src/it/scala/diskrement/cli/it/Tree.scala` and adapt the calls; the modify
step must change size or content-with-new-mtime so the fingerprint misses.
Register `scenarios.Scenario10WarmRescan.suite` in `CliIntegrationSpec`'s list.

- [ ] **Step 2: Run the IT suite**

Run: `sbt cli/IntegrationTest/test`
Expected: all previous scenarios green (their `Scanned:` assertions still pass —
the old line is unchanged; `Hashed …` is a separate second line) plus scenario 10.
If any scenario asserted a FULL stdout equality (not `contains`), update it for
the new second line.

- [ ] **Step 3: Commit**

```bash
git add cli/src/it
git commit -m "it: scenario 10 — warm-cache rescan hashes only changed files"
```

---

### Task 9: README

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Add a "Performance" subsection** (after the "Real burning" section):

```markdown
### Performance

`scan` hashes new/changed content with up to 6 parallel workers
(`DISKREMENT_HASH_PARALLELISM` overrides) and reports progress on stderr.
Observed size+mtime→hash fingerprints are kept in a sidecar cache next to the
catalog (`<name>.scancache.sqlite`), so a rescan hashes only what changed —
the first scan of a large library is the only expensive one. The cache is
disposable: deleting it is always safe and merely costs one full re-hash
(`scan --verify` re-hashes everything regardless). Burn staging clones files
via reflink (free on the required CoW filesystem) and re-hashes the staged
tree in parallel. `scripts/perf-bench.sh SOURCE_DIR WORK_AREA` measures the
full cycle end to end.
```

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "docs: README performance section (parallel hashing, scancache, bench script)"
```

---

### Task 10: Real-library validation (spec §7 acceptance)

Not a subagent task — run in the main session (touches the real library,
read-only, and the user's working area).

**Files:** none (results recorded in the final summary and the plan checklist).

- [ ] **Step 1: Build and run the benchmark against the photo library**

```bash
sbt cli/stage
scripts/perf-bench.sh \
  /data/ext/services/immich/store/library/d30dd4c8-53c2-4f26-81f2-f7b743ac76e0 \
  /data/ext/services/working-area/diskrement
```

(~25 GB of scratch appears under the working area during the run; the script
cleans it up.)

- [ ] **Step 2: Correctness spot-check while the bench project exists** — pick
~10 files from `scan1`'s staged output and compare `sha256sum` of the source
files against the hashes recorded on the staged disc's manifest (or re-run a
tiny fixture and compare). At minimum verify `scan2` reports
`Scanned: 61391 added, …` consistent counts with the baseline run and
`Hashed 0 of` on an untouched rescan.

- [ ] **Step 3: Record results against targets**

| metric | baseline | target | measured |
|---|---|---|---|
| scan #1 | 450 s | ≤ 180 s | |
| scan #2 (post-burn rescan) | 410 s | ≤ 30 s | |
| burn staging | 38 s | ≤ 25 s | |
| scan #2 bytes read | 388.6 GB | ~0 | |

If a target is missed, investigate before merging (parallelism tuning,
progress-throttle overhead, cache read path) — do not relax targets silently.

---

### Task 11: Merge

- [ ] **Step 1: Full verification**

```bash
sbt clean core/test shell/test cli/test cli/IntegrationTest/test
```

Expected: everything green (core 50 munit; shell/cli ZIO suites; IT 14 scenarios-worth).

- [ ] **Step 2: Merge and push**

```bash
git checkout main && git pull
git merge --no-ff feat/scan-performance -m "Merge feat/scan-performance: parallel hashing, progress, reflink staging, fingerprint cache"
git push
git branch -d feat/scan-performance
```
