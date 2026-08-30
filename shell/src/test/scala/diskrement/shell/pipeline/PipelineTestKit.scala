package diskrement.shell.pipeline

import diskrement.shell.burn.{Burner, DiscVerifier, SimulatedBurner, SnapshotWriter}
import diskrement.shell.catalog.{CatalogStore, Db, DbPath}
import diskrement.shell.fs.*
import zio.*
import java.nio.file.{Files, Path}

/** A fake SourceSnapshot that plain-copies the source tree (no CoW needed), so
  * pipeline tests run anywhere. `cowOk` controls the init CoW check. */
final case class FakeSnapshot(cowOk: Boolean = true) extends SourceSnapshot:
  def create(sourceRoot: Path, workDir: Path): IO[SnapshotError, SnapshotHandle] =
    ZIO.attemptBlockingIO {
      val dst = workDir.resolve("snapshot")
      if Files.exists(dst) then
        val s = Files.walk(dst)
        try s.sorted(java.util.Comparator.reverseOrder()).forEach(p => Files.deleteIfExists(p))
        finally s.close()
      Files.createDirectories(dst)
      val s = Files.walk(sourceRoot)
      try s.forEach { p =>
        val rel = sourceRoot.relativize(p)
        val target = dst.resolve(rel.toString)
        if Files.isDirectory(p) then Files.createDirectories(target)
        else { Files.createDirectories(target.getParent); Files.copy(p, target) }
      } finally s.close()
      SnapshotHandle(dst)
    }.mapError(SnapshotError.SnapshotIoError(_))
  def release(handle: SnapshotHandle): UIO[Unit] =
    ZIO.attemptBlockingIO {
      if Files.exists(handle.root) then
        val s = Files.walk(handle.root)
        try s.sorted(java.util.Comparator.reverseOrder()).forEach(p => Files.deleteIfExists(p))
        finally s.close()
    }.ignoreLogged
  def checkCowCapable(workDir: Path): IO[SnapshotError, Unit] =
    if cowOk then ZIO.unit else ZIO.fail(SnapshotError.NotCowCapable(workDir, "fake: not cow"))

object PipelineTestKit:
  def deleteRecursive(dir: Path): Unit =
    if Files.exists(dir) then
      val s = Files.walk(dir)
      try s.sorted(java.util.Comparator.reverseOrder()).forEach(p => Files.deleteIfExists(p))
      finally s.close()

  /** Fresh temp catalog + work dir per test, plus all real adapters except
    * SourceSnapshot (tests supply a FakeSnapshot via `mkPipeline`). */
  val env: ZLayer[Any, Throwable,
      ProjectPaths & CatalogStore & FileScanner & Burner & DiscVerifier & StagedStore & Hasher] =
    val paths: ZLayer[Any, Throwable, ProjectPaths] =
      ZLayer.scoped {
        ZIO.acquireRelease(
          ZIO.attempt {
            val base = Files.createTempDirectory("pipeline-test")
            ProjectPaths(base.resolve("catalog.sqlite"), Files.createDirectories(base.resolve("work")))
          })(p => ZIO.attempt(deleteRecursive(p.workDir.getParent)).orDie)
      }
    ZLayer.make[
      ProjectPaths & CatalogStore & FileScanner & Burner & DiscVerifier & StagedStore & Hasher](
      paths,
      ZLayer.fromFunction((p: ProjectPaths) => DbPath(p.catalogPath)),
      Db.poolLayer,
      FileSystem.live, Hasher.live, FileScanner.live,
      SnapshotWriter.live, SimulatedBurner.live, StagingDiscVerifier.live,
      StagedStore.live, CatalogStore.live
    )

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
    yield PipelineLive(catalog, scanner, snapshot, burner, verifier, staged, hasher, paths)

  /** A temp source tree on the same filesystem as the work dir (both under the system temp dir). */
  def makeSource(files: Map[String, String]): ZIO[Scope, Throwable, Path] =
    ZIO.acquireRelease(ZIO.attempt {
      val root = Files.createTempDirectory("pipeline-src")
      files.foreach { case (rel, content) =>
        val p = root.resolve(rel)
        Files.createDirectories(p.getParent)
        Files.writeString(p, content)
      }
      root
    })(root => ZIO.attempt(deleteRecursive(root)).orDie)
