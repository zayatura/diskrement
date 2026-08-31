package diskrement.shell.burn

import diskrement.core.ScannedFile
import diskrement.shell.ProgressSink
import diskrement.shell.fs.{Hasher, Reflink}
import zio._
import java.nio.file.{Files, Path, StandardCopyOption}
import scala.jdk.StreamConverters.*

/** Stages a full disc tree (/data + /.diskrement) to image.stagingDir and
  * re-hashes every staged file against its scan hash (§6.2). The copy is a
  * batched reflink clone (near-free on the CoW filesystem the pipeline
  * requires), falling back to a plain copy where cloning is unsupported (e.g.
  * tests on tmpfs). The re-hash is the integrity gate and always runs.
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
      _     <- createDirs(dataDir, image.files)
      _     <- copyAll(image, dataDir)
      _     <- sink.phase("staging", image.files.map(_.sizeBytes).sum, image.files.size) {
                 ZIO.foreachParDiscard(image.files)(verifyOne(dataDir, hasher, sink, _))
                   .withParallelism(parallelism max 1)
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

  /** Pre-create every destination directory so `cp --parents -p` never applies
    * SOURCE directory modes/mtimes to the staged tree: a read-only source dir
    * would break a later batch writing into it, and directory mtimes would
    * otherwise depend on which batch created them. Staged directories keep the
    * default permissions and creation-time mtimes that burned discs already
    * carry; only FILE metadata is preserved from the source. */
  private def createDirs(dataDir: Path, files: List[ScannedFile]): IO[BurnError, Unit] =
    ZIO.attemptBlockingIO {
      files.map(f => dataDir.resolve(f.relPath.value).getParent).distinct.foreach { parent =>
        if parent != null then Files.createDirectories(parent)
      }
    }.mapError(BurnError.BurnIoError(_))

  /** Copy every planned file into dataDir, preserving structure and mtimes.
    * Tries batched reflink clones; on the first "cannot clone" it switches to
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
              else
                ZIO.logWarning(
                  "reflink clone unsupported here; falling back to plain copies for this " +
                    "burn (staging will read and write every byte)") *>
                  plainCopy(image, dataDir, b) *> go(rest, true)
            }
    go(image.files.map(_.relPath.value).grouped(BatchSize).toList, fallback = false)

  /** One `cp --reflink=always --parents -p` batch run from the snapshot root.
    * Returns false iff the fs cannot reflink (caller falls back). */
  private def reflinkBatch(
      image: DiscImage, dataDir: Path, rels: List[String]): IO[BurnError, Boolean] =
    ZIO.attemptBlockingIO {
      val cmd = List("cp", "--reflink=always", "--parents", "-p",
                     "-t", dataDir.toAbsolutePath.toString, "--") ++ rels
      val pb  = new ProcessBuilder(cmd*)
      pb.directory(image.snapshotRoot.toAbsolutePath.toFile)
      pb.environment().put("LC_ALL", "C")
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
        Files.copy(src, dst, StandardCopyOption.COPY_ATTRIBUTES, StandardCopyOption.REPLACE_EXISTING)
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
