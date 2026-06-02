# diskrement Plan 2b — Read-Side Adapters Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the read/input-side shell adapters — `Hasher`, `FileSystem`, `SourceSnapshot`, `FileScanner`, plus the `CatalogStore.fingerprints` read — that turn a real source directory into a frozen, fully-hashed `List[ScannedFile]` for the pure core.

**Architecture:** Functional core / imperative shell. The pure `core` is unchanged. New ZIO ports live in the `shell` module under `diskrement.shell.fs`; each has a live implementation behind a `ZLayer`. `FileScanner` composes the `FileSystem` and `Hasher` ports; `SourceSnapshot` hides the real `cp --reflink=always` behind an internal `Cloner` seam so its logic is unit-testable with a fake. No CLI or orchestration (Plan 3); no write-side adapters (Plan 2c).

**Tech Stack:** Scala 3 (3.3.4 LTS), sbt, ZIO 2.1.19, zio-jdbc 0.1.2, sqlite-jdbc 3.49.1.0, zio-test 2.1.19. JDK 21. No new dependencies: `java.security.MessageDigest`, `java.nio.file`, `java.lang.ProcessBuilder`.

**Spec:** `docs/superpowers/specs/2026-06-02-diskrement-plan-2b-read-adapters-design.md` (and parent `2026-05-31-diskrement-core-engine-design.md` §4.2, §6, §7.1, §10.3).

---

## File Structure

All new code is in the `shell` module.

**Main:**
- `shell/src/main/scala/diskrement/shell/model/FileFingerprint.scala` — the `(size, mtime, hash)` record.
- `shell/src/main/scala/diskrement/shell/fs/Hasher.scala` — port + `HasherLive` (streaming SHA-256) + layer.
- `shell/src/main/scala/diskrement/shell/fs/FileSystem.scala` — `FileStat` + port + `FileSystemLive` (walk/stat) + layer.
- `shell/src/main/scala/diskrement/shell/fs/FileScanner.scala` — `ScanError` + port + `FileScannerLive` (fast-path hashing) + layer.
- `shell/src/main/scala/diskrement/shell/fs/SourceSnapshot.scala` — `SnapshotHandle`, `SnapshotError`, `Cloner` seam + `CpCloner`, port + `SourceSnapshotLive` + layer.
- Modify `shell/src/main/scala/diskrement/shell/catalog/CatalogStore.scala` + `CatalogStoreLive.scala` — add `fingerprints`.

**Test:**
- `shell/src/test/scala/diskrement/shell/catalog/FingerprintsSpec.scala`
- `shell/src/test/scala/diskrement/shell/fs/HasherSpec.scala`
- `shell/src/test/scala/diskrement/shell/fs/FileSystemSpec.scala`
- `shell/src/test/scala/diskrement/shell/fs/FileScannerSpec.scala`
- `shell/src/test/scala/diskrement/shell/fs/SourceSnapshotSpec.scala`

No `build.sbt` changes (no new dependencies).

---

## Environment prerequisite

Plan 2a is merged and green. Verify before starting:

```bash
sbt "shell/test"     # expect: 13 tests passed
```

---

### Task 1: `FileFingerprint` model + `CatalogStore.fingerprints`

**Files:**
- Create: `shell/src/main/scala/diskrement/shell/model/FileFingerprint.scala`
- Modify: `shell/src/main/scala/diskrement/shell/catalog/CatalogStore.scala`
- Modify: `shell/src/main/scala/diskrement/shell/catalog/CatalogStoreLive.scala`
- Test: `shell/src/test/scala/diskrement/shell/catalog/FingerprintsSpec.scala`

- [ ] **Step 1: Write the `FileFingerprint` model**

`shell/src/main/scala/diskrement/shell/model/FileFingerprint.scala`:

```scala
package diskrement.shell.model

import diskrement.core.ContentHash
import java.time.Instant

/** The prior-version identity the scanner needs to decide whether a file is
  * unchanged (size + mtime match -> reuse the stored content hash). */
final case class FileFingerprint(
    sizeBytes: Long,
    mtime: Instant,
    contentHash: ContentHash
)
```

- [ ] **Step 2: Write the failing test**

`shell/src/test/scala/diskrement/shell/catalog/FingerprintsSpec.scala`:

```scala
package diskrement.shell.catalog

import diskrement.core._
import diskrement.shell.model._
import zio._
import zio.test._
import java.time.Instant

object FingerprintsSpec extends ZIOSpecDefault:
  private val init = ProjectInit("p", "/src", "/work", DiscType.BD_R, 2.0, 1,
    Instant.parse("2026-06-01T00:00:00Z"))
  private val ts = Instant.parse("2026-06-01T10:00:00Z")

  private def fw(path: String, hash: String, size: Long, mt: Instant) =
    FileWrite(RelPath(path), ContentHash(hash), size, mt, "/data/" + path, 0)

  private def session(ord: Int, added: List[FileWrite] = Nil,
                      modified: List[FileWrite] = Nil, deleted: List[RelPath] = Nil) =
    CatalogUpdate(ProjectId(0), Nil, List(DiscWrite(
      ordinal = 1, isNew = ord == 1, discType = DiscType.BD_R, usableBytes = 1_000_000L,
      status = DiscStatus.OPEN, label = "d1",
      session = SessionWrite(ord, ts, "m" + ord, added, modified, deleted))))

  def spec = suite("CatalogStore.fingerprints")(
    test("current version per path with mtime; modified supersedes; deleted excluded"):
      val mtA  = Instant.parse("2026-05-01T00:00:00Z")
      val mtB1 = Instant.parse("2026-05-02T00:00:00Z")
      val mtB2 = Instant.parse("2026-05-03T00:00:00Z")
      val mtC  = Instant.parse("2026-05-04T00:00:00Z")
      for
        pid <- ZIO.serviceWithZIO[CatalogStore](_.createProject(init))
        st   = ZIO.serviceWithZIO[CatalogStore]
        _   <- st(_.applyUpdate(session(1, added =
                 List(fw("a", "a1", 10, mtA), fw("b", "b1", 20, mtB1), fw("c", "c1", 30, mtC)))
                 .copy(projectId = pid)))
        _   <- st(_.applyUpdate(session(2,
                 modified = List(fw("b", "b2", 25, mtB2)), deleted = List(RelPath("c")))
                 .copy(projectId = pid)))
        fps <- st(_.fingerprints(pid))
      yield assertTrue(
        fps.keySet.map(_.value) == Set("a", "b"),
        fps(RelPath("a")) == FileFingerprint(10, mtA, ContentHash("a1")),
        fps(RelPath("b")) == FileFingerprint(25, mtB2, ContentHash("b2")),
        !fps.contains(RelPath("c")))
  ).provideLayer(TestDb.layer)
```

- [ ] **Step 3: Run it to verify it fails**

Run: `sbt "shell/testOnly diskrement.shell.catalog.FingerprintsSpec"`
Expected: FAIL — `fingerprints` is not a member of `CatalogStore`.

- [ ] **Step 4: Add `fingerprints` to the trait**

In `shell/src/main/scala/diskrement/shell/catalog/CatalogStore.scala`, add `RelPath` to the existing core import (it currently reads `import diskrement.core.{CatalogView, DiscState}`) so it becomes:

```scala
import diskrement.core.{CatalogView, DiscState, RelPath}
```

and add this method to the `trait CatalogStore` (after `sessionsByState`):

```scala
  def fingerprints(project: ProjectId): IO[CatalogError, Map[RelPath, FileFingerprint]]
```

(`FileFingerprint` resolves via the existing `import diskrement.shell.model._`.)

- [ ] **Step 5: Implement `fingerprints`**

In `shell/src/main/scala/diskrement/shell/catalog/CatalogStoreLive.scala`, add this method to the class (it mirrors `readView`'s latest-change-per-path window query, selecting `size_bytes` + `mtime` too). `RelPath`, `ContentHash`, and `Instant` are already imported in this file.

```scala
  def fingerprints(project: ProjectId): IO[CatalogError, Map[RelPath, FileFingerprint]] =
    tx {
      sql"""select fv.rel_path, fv.size_bytes, fv.mtime, fv.content_hash
            from (
              select cr.kind as kind, cr.file_version_id as fvid,
                     row_number() over (partition by fv.rel_path order by cr.id desc) as rn
              from change_record cr
              join file_version fv on fv.id = cr.file_version_id
              where fv.project_id = ${project.value}
            ) l
            join file_version fv on fv.id = l.fvid
            where l.rn = 1 and l.kind <> 'DELETED'"""
        .query[(String, Long, String, String)].selectAll
    }.map { rows =>
      rows.iterator.map { case (p, sz, mt, h) =>
        RelPath(p) -> FileFingerprint(sz, Instant.parse(mt), ContentHash(h))
      }.toMap
    }
```

- [ ] **Step 6: Run it to verify it passes**

Run: `sbt "shell/testOnly diskrement.shell.catalog.FingerprintsSpec"`
Expected: `1 tests passed`.

- [ ] **Step 7: Commit**

```bash
git add shell/src/main/scala/diskrement/shell/model/FileFingerprint.scala shell/src/main/scala/diskrement/shell/catalog/CatalogStore.scala shell/src/main/scala/diskrement/shell/catalog/CatalogStoreLive.scala shell/src/test/scala/diskrement/shell/catalog/FingerprintsSpec.scala
git commit -m "$(cat <<'EOF'
feat(shell): add catalog fingerprints read for fast-path scanning

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: `Hasher` (streaming SHA-256)

**Files:**
- Create: `shell/src/main/scala/diskrement/shell/fs/Hasher.scala`
- Test: `shell/src/test/scala/diskrement/shell/fs/HasherSpec.scala`

- [ ] **Step 1: Write the failing test**

`shell/src/test/scala/diskrement/shell/fs/HasherSpec.scala`:

```scala
package diskrement.shell.fs

import zio._
import zio.test._
import java.nio.file.{Files, Path}

object HasherSpec extends ZIOSpecDefault:
  // Known SHA-256 vectors:
  //   sha256("")    = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
  //   sha256("abc") = ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad

  private def withFile[A](content: Array[Byte])(
      use: Path => ZIO[Hasher, Throwable, A]): ZIO[Hasher, Throwable, A] =
    ZIO.acquireReleaseWith(
      ZIO.attempt { val p = Files.createTempFile("hashspec", ".bin"); Files.write(p, content); p })(
      p => ZIO.attempt(Files.deleteIfExists(p)).orDie)(use)

  def spec = suite("Hasher")(
    test("hashes an empty file to the SHA-256 of empty"):
      withFile(Array.emptyByteArray) { p =>
        for h <- ZIO.serviceWithZIO[Hasher](_.hash(p))
        yield assertTrue(
          h.value == "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
      },
    test("hashes \"abc\""):
      withFile("abc".getBytes("UTF-8")) { p =>
        for h <- ZIO.serviceWithZIO[Hasher](_.hash(p))
        yield assertTrue(
          h.value == "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
      },
    test("chunked digest matches a one-shot digest for a large input"):
      val big = Array.tabulate(300_000)(i => (i % 251).toByte)
      val expected =
        java.security.MessageDigest.getInstance("SHA-256").digest(big)
          .map(b => f"${b & 0xff}%02x").mkString
      withFile(big) { p =>
        for h <- ZIO.serviceWithZIO[Hasher](_.hash(p))
        yield assertTrue(h.value == expected)
      }
  ).provideLayer(Hasher.live)
```

- [ ] **Step 2: Run it to verify it fails**

Run: `sbt "shell/testOnly diskrement.shell.fs.HasherSpec"`
Expected: FAIL — `Hasher` not found (compile error).

- [ ] **Step 3: Write `Hasher.scala`**

`shell/src/main/scala/diskrement/shell/fs/Hasher.scala`:

```scala
package diskrement.shell.fs

import diskrement.core.ContentHash
import zio._
import java.io.IOException
import java.nio.file.{Files, Path}

/** Content hashing port. SHA-256 in cycle 1; a port so the algorithm can change
  * later without touching callers (engine design §5.3). */
trait Hasher:
  def hash(path: Path): IO[IOException, ContentHash]

final case class HasherLive() extends Hasher:
  def hash(path: Path): IO[IOException, ContentHash] =
    ZIO.attemptBlockingIO {
      val md  = java.security.MessageDigest.getInstance("SHA-256")
      val buf = new Array[Byte](64 * 1024)
      val in  = Files.newInputStream(path)
      try
        var n = in.read(buf)
        while n >= 0 do
          md.update(buf, 0, n)
          n = in.read(buf)
      finally in.close()
      ContentHash(md.digest().map(b => f"${b & 0xff}%02x").mkString)
    }

object Hasher:
  val live: ZLayer[Any, Nothing, Hasher] = ZLayer.succeed(HasherLive())
```

- [ ] **Step 4: Run it to verify it passes**

Run: `sbt "shell/testOnly diskrement.shell.fs.HasherSpec"`
Expected: `3 tests passed`.

- [ ] **Step 5: Commit**

```bash
git add shell/src/main/scala/diskrement/shell/fs/Hasher.scala shell/src/test/scala/diskrement/shell/fs/HasherSpec.scala
git commit -m "$(cat <<'EOF'
feat(shell): add streaming SHA-256 Hasher adapter

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: `FileSystem` (read-only walk + stat)

**Files:**
- Create: `shell/src/main/scala/diskrement/shell/fs/FileSystem.scala`
- Test: `shell/src/test/scala/diskrement/shell/fs/FileSystemSpec.scala`

- [ ] **Step 1: Write the failing test**

`shell/src/test/scala/diskrement/shell/fs/FileSystemSpec.scala`:

```scala
package diskrement.shell.fs

import zio._
import zio.test._
import java.nio.file.{Files, Path}

object FileSystemSpec extends ZIOSpecDefault:

  /** root/{a.txt, .hidden, sub/b.txt, sub/c.txt} plus a symlink that must be
    * skipped. Deleted recursively on release. */
  private val makeTree: ZIO[Scope, Throwable, Path] =
    ZIO.acquireRelease(
      ZIO.attempt {
        val root = Files.createTempDirectory("fsspec")
        Files.writeString(root.resolve("a.txt"), "aaa")
        Files.writeString(root.resolve(".hidden"), "h")
        val sub = Files.createDirectory(root.resolve("sub"))
        Files.writeString(sub.resolve("b.txt"), "bbbb")
        Files.writeString(sub.resolve("c.txt"), "ccccc")
        try Files.createSymbolicLink(root.resolve("link.txt"), root.resolve("a.txt"))
        catch case _: Throwable => ()   // symlink creation may be unsupported; fine
        root
      })(root =>
        ZIO.attempt(
          Files.walk(root).sorted(java.util.Comparator.reverseOrder())
            .forEach(p => Files.deleteIfExists(p))).orDie)

  def spec = suite("FileSystemLive")(
    test("walk returns regular files (incl dotfiles); skips symlinks and dirs"):
      ZIO.scoped {
        for
          root  <- makeTree
          paths <- ZIO.serviceWithZIO[FileSystem](_.walk(root))
          rels   = paths.map(p => root.relativize(p).toString).sorted
        yield assertTrue(rels == List(".hidden", "a.txt", "sub/b.txt", "sub/c.txt"))
      },
    test("stat returns size and an mtime truncated to millis"):
      ZIO.scoped {
        for
          root <- makeTree
          st   <- ZIO.serviceWithZIO[FileSystem](_.stat(root.resolve("sub/c.txt")))
        yield assertTrue(
          st.sizeBytes == 5L,
          st.mtime == st.mtime.truncatedTo(java.time.temporal.ChronoUnit.MILLIS))
      }
  ).provideLayer(FileSystem.live)
```

- [ ] **Step 2: Run it to verify it fails**

Run: `sbt "shell/testOnly diskrement.shell.fs.FileSystemSpec"`
Expected: FAIL — `FileSystem` not found (compile error).

- [ ] **Step 3: Write `FileSystem.scala`**

`shell/src/main/scala/diskrement/shell/fs/FileSystem.scala`:

```scala
package diskrement.shell.fs

import zio._
import java.io.IOException
import java.nio.file.{Files, LinkOption, Path}
import java.nio.file.attribute.BasicFileAttributes
import java.time.Instant
import java.time.temporal.ChronoUnit
import scala.jdk.StreamConverters.*

/** Size and last-modified time of a regular file. mtime is truncated to
  * milliseconds so it matches the precision persisted in the catalog (this is
  * the single point where mtime precision is normalized). */
final case class FileStat(sizeBytes: Long, mtime: Instant)

/** Read-only filesystem port: enumerate regular files and stat them. */
trait FileSystem:
  def walk(root: Path): IO[IOException, List[Path]]
  def stat(path: Path): IO[IOException, FileStat]

final case class FileSystemLive() extends FileSystem:
  def walk(root: Path): IO[IOException, List[Path]] =
    ZIO.attemptBlockingIO {
      val stream = Files.walk(root)   // does NOT follow symlinks by default
      try
        stream.toScala(List)
          .filter(p => Files.isRegularFile(p, LinkOption.NOFOLLOW_LINKS))
          .sortBy(_.toString)
      finally stream.close()
    }

  def stat(path: Path): IO[IOException, FileStat] =
    ZIO.attemptBlockingIO {
      val attrs = Files.readAttributes(
        path, classOf[BasicFileAttributes], LinkOption.NOFOLLOW_LINKS)
      FileStat(attrs.size(), attrs.lastModifiedTime().toInstant.truncatedTo(ChronoUnit.MILLIS))
    }

object FileSystem:
  val live: ZLayer[Any, Nothing, FileSystem] = ZLayer.succeed(FileSystemLive())
```

- [ ] **Step 4: Run it to verify it passes**

Run: `sbt "shell/testOnly diskrement.shell.fs.FileSystemSpec"`
Expected: `2 tests passed`.

- [ ] **Step 5: Commit**

```bash
git add shell/src/main/scala/diskrement/shell/fs/FileSystem.scala shell/src/test/scala/diskrement/shell/fs/FileSystemSpec.scala
git commit -m "$(cat <<'EOF'
feat(shell): add read-only FileSystem walk/stat adapter

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: `FileScanner` (fast-path hashing)

**Files:**
- Create: `shell/src/main/scala/diskrement/shell/fs/FileScanner.scala`
- Test: `shell/src/test/scala/diskrement/shell/fs/FileScannerSpec.scala`

Depends on Task 1 (`FileFingerprint`), Task 2 (`Hasher`), Task 3 (`FileSystem`).

- [ ] **Step 1: Write the failing test (with in-memory fakes)**

`shell/src/test/scala/diskrement/shell/fs/FileScannerSpec.scala`:

```scala
package diskrement.shell.fs

import diskrement.core.{ContentHash, RelPath}
import diskrement.shell.model.FileFingerprint
import zio._
import zio.test._
import java.io.IOException
import java.nio.file.{Path, Paths}
import java.time.Instant

object FileScannerSpec extends ZIOSpecDefault:
  private val mt1 = Instant.parse("2026-05-01T00:00:00Z")
  private val mt2 = Instant.parse("2026-05-02T00:00:00Z")
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

  private def run(files: Map[String, FileStat],
                  prior: Map[RelPath, FileFingerprint],
                  verify: Boolean): UIO[(List[diskrement.core.ScannedFile], Int)] =
    for
      counter <- Ref.make(0)
      scanner  = FileScannerLive(FakeFs(files), CountingHasher(counter))
      out     <- scanner.scan(root, prior, verify).orDie
      hashed  <- counter.get
    yield (out, hashed)

  def spec = suite("FileScanner")(
    test("fast path reuses the stored hash when size+mtime match (no hashing)"):
      val files = Map("a" -> FileStat(10, mt1), "b" -> FileStat(20, mt1))
      val prior = Map(
        RelPath("a") -> FileFingerprint(10, mt1, ContentHash("old-a")),
        RelPath("b") -> FileFingerprint(20, mt1, ContentHash("old-b")))
      for (out, hashed) <- run(files, prior, verify = false)
      yield assertTrue(
        hashed == 0,
        out.map(f => (f.relPath.value, f.contentHash.value)) ==
          List(("a", "old-a"), ("b", "old-b")))
    ,
    test("a size or mtime change forces a re-hash"):
      val files = Map("a" -> FileStat(10, mt2), "b" -> FileStat(99, mt1))
      val prior = Map(
        RelPath("a") -> FileFingerprint(10, mt1, ContentHash("old-a")),
        RelPath("b") -> FileFingerprint(20, mt1, ContentHash("old-b")))
      for (out, hashed) <- run(files, prior, verify = false)
      yield assertTrue(hashed == 2, out.map(_.contentHash.value) == List("H:a", "H:b"))
    ,
    test("verify forces a re-hash even when the fingerprint matches"):
      val files = Map("a" -> FileStat(10, mt1))
      val prior = Map(RelPath("a") -> FileFingerprint(10, mt1, ContentHash("old-a")))
      for (out, hashed) <- run(files, prior, verify = true)
      yield assertTrue(hashed == 1, out.head.contentHash.value == "H:a")
    ,
    test("a file absent from prior is hashed; output is sorted by relPath"):
      val files = Map("z" -> FileStat(1, mt1), "a" -> FileStat(2, mt1))
      for (out, hashed) <- run(files, Map.empty, verify = false)
      yield assertTrue(hashed == 2, out.map(_.relPath.value) == List("a", "z"))
  )
```

- [ ] **Step 2: Run it to verify it fails**

Run: `sbt "shell/testOnly diskrement.shell.fs.FileScannerSpec"`
Expected: FAIL — `FileScannerLive` / `FileScanner` not found (compile error).

- [ ] **Step 3: Write `FileScanner.scala`**

`shell/src/main/scala/diskrement/shell/fs/FileScanner.scala`:

```scala
package diskrement.shell.fs

import diskrement.core.{RelPath, ScannedFile}
import diskrement.shell.model.FileFingerprint
import zio._
import java.nio.file.Path

/** Expected scan failure: a specific file could not be walked/statted/hashed. */
sealed abstract class ScanError(msg: String, cause: Throwable = null)
    extends Exception(msg, cause)
object ScanError:
  final case class FileFailed(relPath: RelPath, cause: Throwable)
      extends ScanError(s"failed scanning ${relPath.value}", cause)

/** Walks a frozen snapshot and produces fully-hashed ScannedFiles, reusing a
  * stored hash when size+mtime match (fast path) unless `verify` is set. */
trait FileScanner:
  def scan(
      snapshotRoot: Path,
      prior: Map[RelPath, FileFingerprint],
      verify: Boolean
  ): IO[ScanError, List[ScannedFile]]

final case class FileScannerLive(fs: FileSystem, hasher: Hasher) extends FileScanner:
  def scan(
      snapshotRoot: Path,
      prior: Map[RelPath, FileFingerprint],
      verify: Boolean
  ): IO[ScanError, List[ScannedFile]] =
    for
      paths <- fs.walk(snapshotRoot)
                 .mapError(e => ScanError.FileFailed(RelPath(snapshotRoot.toString), e))
      files <- ZIO.foreach(paths)(scanOne(snapshotRoot, _, prior, verify))
    yield files.sortBy(_.relPath)

  private def scanOne(
      root: Path, abs: Path,
      prior: Map[RelPath, FileFingerprint], verify: Boolean
  ): IO[ScanError, ScannedFile] =
    val rel = RelPath(root.relativize(abs).toString)
    val program =
      for
        st   <- fs.stat(abs)
        hash <- prior.get(rel) match
                  case Some(fp) if !verify && fp.sizeBytes == st.sizeBytes && fp.mtime == st.mtime =>
                    ZIO.succeed(fp.contentHash)
                  case _ => hasher.hash(abs)
      yield ScannedFile(rel, hash, st.sizeBytes, st.mtime)
    program.mapError(e => ScanError.FileFailed(rel, e))

object FileScanner:
  val live: ZLayer[FileSystem & Hasher, Nothing, FileScanner] =
    ZLayer {
      for
        fs     <- ZIO.service[FileSystem]
        hasher <- ZIO.service[Hasher]
      yield FileScannerLive(fs, hasher)
    }
```

- [ ] **Step 4: Run it to verify it passes**

Run: `sbt "shell/testOnly diskrement.shell.fs.FileScannerSpec"`
Expected: `4 tests passed`.

- [ ] **Step 5: Commit**

```bash
git add shell/src/main/scala/diskrement/shell/fs/FileScanner.scala shell/src/test/scala/diskrement/shell/fs/FileScannerSpec.scala
git commit -m "$(cat <<'EOF'
feat(shell): add FileScanner with fast-path hashing

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: `SourceSnapshot` (reflink freeze + CoW hard-fail)

**Files:**
- Create: `shell/src/main/scala/diskrement/shell/fs/SourceSnapshot.scala`
- Test: `shell/src/test/scala/diskrement/shell/fs/SourceSnapshotSpec.scala`

- [ ] **Step 1: Write the failing test (fake `Cloner` + an auto-skipping real-`cp` test)**

`shell/src/test/scala/diskrement/shell/fs/SourceSnapshotSpec.scala`:

```scala
package diskrement.shell.fs

import zio._
import zio.test._
import java.nio.file.{Files, Path}

object SourceSnapshotSpec extends ZIOSpecDefault:

  /** Fake cloner: plain recursive copy (so the snapshot really appears), or a
    * configured failure for the error-mapping tests. */
  private final case class FakeCloner(fail: Option[SnapshotError]) extends Cloner:
    def reflinkCopy(src: Path, dst: Path): IO[SnapshotError, Unit] =
      fail match
        case Some(e) => ZIO.fail(e)
        case None    =>
          ZIO.attempt {
            Files.createDirectories(dst)
            Files.list(src).forEach(p => Files.copy(p, dst.resolve(p.getFileName)))
          }.mapError(SnapshotError.SnapshotIoError(_))

  /** base/{src/f.txt, work} ; deleted recursively on release. */
  private val makeDirs: ZIO[Scope, Throwable, (Path, Path)] =
    ZIO.acquireRelease(
      ZIO.attempt {
        val base = Files.createTempDirectory("snapspec")
        val src  = Files.createDirectory(base.resolve("src"))
        Files.writeString(src.resolve("f.txt"), "data")
        val work = Files.createDirectory(base.resolve("work"))
        (src, work)
      })((dirs) =>
        ZIO.attempt(
          Files.walk(dirs._1.getParent).sorted(java.util.Comparator.reverseOrder())
            .forEach(p => Files.deleteIfExists(p))).orDie)

  def spec = suite("SourceSnapshot")(
    test("create reflinks the source into workDir/snapshot and returns the handle"):
      ZIO.scoped {
        for
          (src, work) <- makeDirs
          snap = SourceSnapshotLive(FakeCloner(None))
          h    <- snap.create(src, work)
          ok   <- ZIO.attempt(Files.exists(h.root.resolve("f.txt")))
        yield assertTrue(h.root == work.resolve("snapshot"), ok)
      },
    test("create clears a stale snapshot dir first"):
      ZIO.scoped {
        for
          (src, work) <- makeDirs
          stale = work.resolve("snapshot")
          _    <- ZIO.attempt {
                    Files.createDirectories(stale)
                    Files.writeString(stale.resolve("old.txt"), "stale")
                  }
          snap = SourceSnapshotLive(FakeCloner(None))
          h    <- snap.create(src, work)
          oldGone  <- ZIO.attempt(!Files.exists(h.root.resolve("old.txt")))
          newThere <- ZIO.attempt(Files.exists(h.root.resolve("f.txt")))
        yield assertTrue(oldGone, newThere)
      },
    test("a missing source fails with SourceMissing"):
      ZIO.scoped {
        for
          (src, work) <- makeDirs
          snap = SourceSnapshotLive(FakeCloner(None))
          err  <- snap.create(work.resolve("does-not-exist"), work).flip
        yield assertTrue(err.isInstanceOf[SnapshotError.SourceMissing])
      },
    test("a NotCowCapable clone error propagates"):
      ZIO.scoped {
        for
          (src, work) <- makeDirs
          snap = SourceSnapshotLive(FakeCloner(Some(
                   SnapshotError.NotCowCapable(work, "operation not supported"))))
          err  <- snap.create(src, work).flip
        yield assertTrue(err.isInstanceOf[SnapshotError.NotCowCapable])
      },
    test("release removes the snapshot dir"):
      ZIO.scoped {
        for
          (src, work) <- makeDirs
          snap = SourceSnapshotLive(FakeCloner(None))
          h    <- snap.create(src, work)
          _    <- snap.release(h)
          gone <- ZIO.attempt(!Files.exists(h.root))
        yield assertTrue(gone)
      },
    test("real cp --reflink reflinks on a CoW filesystem (auto-skips otherwise)"):
      ZIO.scoped {
        for
          (src, work) <- makeDirs
          snap = SourceSnapshotLive(CpCloner())
          res  <- snap.create(src, work).either
        yield res match
          case Right(h)                             => assertTrue(Files.exists(h.root.resolve("f.txt")))
          case Left(_: SnapshotError.NotCowCapable) => assertCompletes // not a CoW fs: skip
          case Left(other)                          => assertTrue(false) // unexpected real failure
      }
  )
```

- [ ] **Step 2: Run it to verify it fails**

Run: `sbt "shell/testOnly diskrement.shell.fs.SourceSnapshotSpec"`
Expected: FAIL — `SourceSnapshotLive` / `Cloner` / `CpCloner` / `SnapshotError` not found (compile error).

- [ ] **Step 3: Write `SourceSnapshot.scala`**

`shell/src/main/scala/diskrement/shell/fs/SourceSnapshot.scala`:

```scala
package diskrement.shell.fs

import zio._
import java.nio.file.{Files, Path}

/** The frozen source root the rest of the run reads from. */
final case class SnapshotHandle(root: Path)

/** Expected snapshot failures. Mirrors CatalogError (extends Exception). */
sealed abstract class SnapshotError(msg: String, cause: Throwable = null)
    extends Exception(msg, cause)
object SnapshotError:
  final case class NotCowCapable(workDir: Path, detail: String)
      extends SnapshotError(
        s"work dir is not on a CoW-capable filesystem (need btrfs or XFS-with-reflink): $workDir [$detail]")
  final case class SourceMissing(sourceRoot: Path)
      extends SnapshotError(s"source root does not exist: $sourceRoot")
  final case class SnapshotIoError(cause: Throwable)
      extends SnapshotError("snapshot I/O error", cause)

/** Internal seam over the actual reflink clone, so SourceSnapshot's logic is
  * unit-testable with a fake and the real `cp` runs only in an env-gated test. */
private[fs] trait Cloner:
  def reflinkCopy(src: Path, dst: Path): IO[SnapshotError, Unit]

/** Live cloner: `cp --reflink=always -a SRC DST`. A clone-unsupported failure
  * maps to NotCowCapable; everything else to SnapshotIoError. */
private[fs] final case class CpCloner() extends Cloner:
  def reflinkCopy(src: Path, dst: Path): IO[SnapshotError, Unit] =
    ZIO.attemptBlockingIO {
      val proc = new ProcessBuilder(
        "cp", "--reflink=always", "-a", src.toString, dst.toString).start()
      val stderr = new String(proc.getErrorStream.readAllBytes(), "UTF-8")
      val code   = proc.waitFor()
      (code, stderr)
    }.mapError(SnapshotError.SnapshotIoError(_))
      .flatMap { case (code, stderr) =>
        if code == 0 then ZIO.unit
        else if isCloneUnsupported(stderr) then
          ZIO.fail(SnapshotError.NotCowCapable(dst.getParent, stderr.trim))
        else
          ZIO.fail(SnapshotError.SnapshotIoError(
            new RuntimeException(s"cp exited $code: ${stderr.trim}")))
      }

  private def isCloneUnsupported(stderr: String): Boolean =
    val s = stderr.toLowerCase
    s.contains("failed to clone") || s.contains("operation not supported")

/** Freezes the source tree by reflinking it to a deterministic dir under the
  * work dir. Lifecycle is explicit (create/release) and on-disk so it can span
  * the staged CLI invocations (engine design §6; spec §2.1 deviation 3). */
trait SourceSnapshot:
  def create(sourceRoot: Path, workDir: Path): IO[SnapshotError, SnapshotHandle]
  def release(handle: SnapshotHandle): UIO[Unit]

private[fs] final case class SourceSnapshotLive(cloner: Cloner) extends SourceSnapshot:
  def create(sourceRoot: Path, workDir: Path): IO[SnapshotError, SnapshotHandle] =
    val dst = workDir.resolve("snapshot")
    for
      exists <- ZIO.attemptBlockingIO(Files.exists(sourceRoot))
                  .mapError(SnapshotError.SnapshotIoError(_))
      _      <- ZIO.unless(exists)(ZIO.fail(SnapshotError.SourceMissing(sourceRoot)))
      _      <- removeDir(dst)
      _      <- ZIO.attemptBlockingIO(Files.createDirectories(workDir))
                  .mapError(SnapshotError.SnapshotIoError(_))
      _      <- cloner.reflinkCopy(sourceRoot, dst)
    yield SnapshotHandle(dst)

  def release(handle: SnapshotHandle): UIO[Unit] = removeDir(handle.root)

  private def removeDir(dir: Path): UIO[Unit] =
    ZIO.attemptBlockingIO {
      if Files.exists(dir) then
        Files.walk(dir).sorted(java.util.Comparator.reverseOrder())
          .forEach(p => Files.deleteIfExists(p))
    }.ignoreLogged

object SourceSnapshot:
  val live: ZLayer[Any, Nothing, SourceSnapshot] =
    ZLayer.succeed(SourceSnapshotLive(CpCloner()))
```

- [ ] **Step 4: Run it to verify it passes**

Run: `sbt "shell/testOnly diskrement.shell.fs.SourceSnapshotSpec"`
Expected: `6 tests passed` (the real-`cp` test passes-as-skipped on a non-CoW filesystem).

- [ ] **Step 5: Commit**

```bash
git add shell/src/main/scala/diskrement/shell/fs/SourceSnapshot.scala shell/src/test/scala/diskrement/shell/fs/SourceSnapshotSpec.scala
git commit -m "$(cat <<'EOF'
feat(shell): add reflink SourceSnapshot with CoW hard-fail

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

### Task 6: Final green sweep

**Files:** none (verification only).

- [ ] **Step 1: Run the entire suite green (both modules)**

Run: `sbt clean core/test shell/test`
Expected: core `Passed: Total 50, Failed 0`; shell `29 tests passed` (13 from Plan 2a + Task 1 `1` + Task 2 `3` + Task 3 `2` + Task 4 `4` + Task 5 `6`). Fix any `-Wunused:all` warnings (unused imports) before finishing.

- [ ] **Step 2: Confirm no uncommitted changes**

Run: `git status --porcelain`
Expected: empty (all work committed across Tasks 1–5). If the sweep required a fix, commit it with an appropriate message.

> **Manual test (documented, not automated — engine design §10.4 / §6.2):** On a real **btrfs** or **XFS-with-reflink** work dir, call `SourceSnapshot.create` against a sample tree and confirm the snapshot is a reflink copy (near-instant, shares extents). On **ext4**, confirm `create` fails with `NotCowCapable` carrying an actionable message. This is the path the auto-skipping integration test cannot cover in CI.

---

## Self-Review (completed by plan author)

**Spec coverage (Plan 2b design):**
- §3.1 `FileFingerprint` → Task 1.
- §3.2 `Hasher` (streaming SHA-256, port) → Task 2.
- §3.3 `FileSystem` (walk regular-files-only/no-follow-symlinks/dotfiles-in; stat with millis-truncated mtime) → Task 3.
- §3.5 `FileScanner` (fast-path reuse on size+mtime match; `verify` forces re-hash; depends only on FileSystem+Hasher; deletions not its concern; sorted output) → Task 4.
- §3.4 `SourceSnapshot` (deterministic workDir/snapshot location, stale-dir clear, CoW hard-fail via `Cloner` seam + `CpCloner`, total `release`) → Task 5.
- §3.6 `CatalogStore.fingerprints` (latest non-tombstoned per path + mtime) → Task 1.
- §2.1 deviation 1 (no custom `Clock`) → honored (none defined). Deviation 2 (mtime shell-side) → Task 1 keeps core untouched. Deviation 3 (explicit on-disk create/release) → Task 5.
- §5 testing: Hasher vectors + large input → Task 2; FileSystem real tree → Task 3; FileScanner fakes (fast-path/verify/new/sorted) → Task 4; SourceSnapshot fake-`Cloner` unit tests + auto-skipping real-`cp` + documented manual → Task 5; fingerprints SQLite integration → Task 1.

**Correctly deferred (Plan 2c/3):** `SnapshotWriter`, `SimulatedBurner`; CLI + staged orchestration + when create/release/scan fire; XDG/systemd path resolution; btrfs subvolume snapshot; ignore patterns; symlink/special-file handling.

**Type consistency:** `FileFingerprint(sizeBytes, mtime, contentHash)` is defined in Task 1 and consumed identically in Tasks 1 (catalog) and 4 (scanner). `FileStat(sizeBytes, mtime)` defined in Task 3, consumed in Task 4. Ports `Hasher.hash(Path): IO[IOException, ContentHash]`, `FileSystem.walk/stat`, `FileScanner.scan(Path, Map[RelPath, FileFingerprint], Boolean)`, `SourceSnapshot.create/release` are referenced with matching signatures across tasks and their fakes. `ScanError.FileFailed` / `SnapshotError.{NotCowCapable, SourceMissing, SnapshotIoError}` names match between source and tests. zio-jdbc API for `fingerprints` matches the verified Plan 2a usage (`tx`, `sql"".query[...].selectAll`, window function).

**No placeholders:** every step has concrete code and exact commands; dependency set unchanged (pinned by Plan 2a).
