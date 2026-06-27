package diskrement.shell.pipeline

import diskrement.core.*
import diskrement.shell.catalog.CatalogStore
import diskrement.shell.burn.{DiscIdentity, DiscImage, DiscMetadata, ManifestJson, SimulatedBurner}
import zio.json.*
import diskrement.shell.fs.{FileScanner, Hasher, SnapshotHandle, SourceSnapshot}
import diskrement.shell.model.*
import zio.*
import java.nio.file.{Files, Path}
import scala.jdk.StreamConverters.*

/** Per-disc data derived once during burn and reused for the embedded catalog,
  * the staged manifest, and the live commit. */
private final case class DiscPlanData(
    disc: PlannedDisc, tombs: List[RelPath], sessOrd: Int, manifest: SessionManifest)

private[pipeline] final case class PipelineLive(
    catalog: CatalogStore,
    scanner: FileScanner,
    snapshot: SourceSnapshot,
    burner: SimulatedBurner,
    staged: StagedStore,
    hasher: Hasher,
    paths: ProjectPaths
) extends Pipeline:

  /** Resolve a project by name or fail ProjectNotFound. */
  private def project(name: String): IO[PipelineError, Project] =
    catalog.findProject(name).mapError(PipelineError.Failed(_))
      .flatMap {
        case Some(p) => ZIO.succeed(p)
        case None    => ZIO.fail(PipelineError.ProjectNotFound(name))
      }

  def init(p: ProjectInit): IO[PipelineError, ProjectId] =
    for
      srcPath <- ZIO.attempt(Path.of(p.sourceRoot)).mapError(PipelineError.Failed(_))
      _  <- sameFilesystem(srcPath, paths.workDir)
      _  <- snapshot.checkCowCapable(paths.workDir)
              .mapError(e => PipelineError.NotCowCapable(paths.workDir, e.getMessage))
      id <- catalog.createProject(p).mapError(PipelineError.Failed(_))
    yield id

  private def sameFilesystem(src: Path, work: Path): IO[PipelineError, Unit] =
    ZIO.attemptBlockingIO(Files.getFileStore(src) == Files.getFileStore(work))
      .mapError(PipelineError.Failed(_))
      .flatMap(same => ZIO.unless(same)(ZIO.fail(PipelineError.NotSameFilesystem(src, work))).unit)

  def status(name: String): IO[PipelineError, StatusReport] =
    for
      p        <- project(name)
      discs    <- catalog.readDiscStates(p.id).mapError(PipelineError.Failed(_))
      view     <- catalog.readView(p.id).mapError(PipelineError.Failed(_))
      burned   <- catalog.sessionsByState(p.id, SessionState.BURNED).mapError(PipelineError.Failed(_))
      verified <- catalog.sessionsByState(p.id, SessionState.VERIFIED).mapError(PipelineError.Failed(_))
    yield StatusReport(
      discs = discs,
      openDiscOrdinal = discs.find(_.status == DiscStatus.OPEN).map(_.ordinal),
      totalCurrentFiles = view.current.size,
      lastBurnAt = (burned ++ verified).flatMap(_.burnedAt).maxOption)

  def history(name: String, relPath: RelPath): IO[PipelineError, List[FileHistoryEntry]] =
    project(name).flatMap(p =>
      catalog.fileHistory(p.id, relPath).mapError(PipelineError.Failed(_)))

  def scan(name: String, verify: Boolean, force: Boolean): IO[PipelineError, ScanResult] =
    for
      p      <- project(name)
      reuse  <- staged.readScan(paths.workDir)
      result <- reuse match
                  case Some(s) if !force => countOf(p.id, s)
                  case _                 => freshScan(p, verify)
    yield result

  private def freshScan(p: Project, verify: Boolean): IO[PipelineError, ScanResult] =
    for
      srcPath <- ZIO.attempt(Path.of(p.sourceRoot)).mapError(PipelineError.Failed(_))
      handle  <- snapshot.create(srcPath, paths.workDir)
                   .mapError(e => PipelineError.Failed(e))
      prior   <- catalog.fingerprints(p.id).mapError(PipelineError.Failed(_))
      scanned <- scanner.scan(handle.root, prior, verify).mapError(PipelineError.Failed(_))
      _       <- staged.writeScan(paths.workDir, scanned)
      res     <- countOf(p.id, scanned)
    yield res

  private def countOf(pid: ProjectId, scanned: List[ScannedFile]): IO[PipelineError, ScanResult] =
    catalog.readView(pid).mapError(PipelineError.Failed(_)).map { view =>
      val cs = Diff.diff(view, scanned)
      ScanResult(cs.added.size, cs.modified.size, cs.deleted.size, cs.unchanged.size)
    }

  def plan(name: String, discTypeOverride: Option[DiscType]): IO[PipelineError, BurnPlan] =
    for
      p       <- project(name)
      scanned <- staged.readScan(paths.workDir).flatMap {
                   case Some(s) => ZIO.succeed(s)
                   case None    => ZIO.fail(PipelineError.NoStagedScan(name))
                 }
      view    <- catalog.readView(p.id).mapError(PipelineError.Failed(_))
      states  <- catalog.readDiscStates(p.id).mapError(PipelineError.Failed(_))
      discType = discTypeOverride.getOrElse(p.defaultDiscType)
      cap      = Capacity.fromDiscType(discType, p.marginPercent)
      cs       = Diff.diff(view, scanned)
      burnPlan = Planner.plan(cs, states, cap)
      _       <- staged.writePlan(paths.workDir, burnPlan, discType)
    yield burnPlan

  def burn(name: String): IO[PipelineError, BurnReport] =
    for
      p           <- project(name)
      scanned     <- staged.readScan(paths.workDir).flatMap {
                       case Some(s) => ZIO.succeed(s); case None => ZIO.fail(PipelineError.NoStagedScan(name)) }
      planAndType <- staged.readPlan(paths.workDir).flatMap {
                       case Some(pt) => ZIO.succeed(pt); case None => ZIO.fail(PipelineError.NoStagedPlan(name)) }
      (burnPlan, discType) = planAndType
      _           <- ZIO.when(burnPlan.oversized.nonEmpty)(
                       ZIO.fail(PipelineError.OversizedFiles(burnPlan.oversized.map(_.relPath))))
      snapRoot     = paths.workDir.resolve("snapshot")
      _           <- ZIO.attemptBlockingIO(Files.exists(snapRoot)).mapError(PipelineError.Failed(_))
                       .flatMap(e => ZIO.unless(e)(ZIO.fail(PipelineError.SnapshotMissing(name))).unit)
      view        <- catalog.readView(p.id).mapError(PipelineError.Failed(_))
      changeSet    = Diff.diff(view, scanned)
      now         <- Clock.instant
      existing    <- existingSessions(p.id)
      discCount    = burnPlan.discs.map(_.ordinal).maxOption.getOrElse(0)
      lastIdx      = burnPlan.discs.size - 1
      // Per-disc data derived once, so the embedded catalog, the staged manifests,
      // and the live commit are all built from the same session ordinals/manifests.
      discData     = burnPlan.discs.zipWithIndex.map { case (d, i) =>
                       val tombs    = if i == lastIdx then burnPlan.tombstones else Nil
                       val sessOrd  = existing.getOrElse(d.ordinal, 0) + 1
                       val manifest = ManifestBuilder.build(name, sessOrd, d, changeSet, tombs)
                       DiscPlanData(d, tombs, sessOrd, manifest)
                     }
      update       = CatalogUpdate(p.id, Nil, discData.map(dd =>
                       toDiscWrite(name, dd.disc, discType, dd.sessOrd, now, changeSet, dd.tombs, dd.manifest)))
      tmpCatalog   = paths.workDir.resolve("burn-catalog.sqlite")
      _           <- ZIO.attemptBlockingIO {
                       Files.createDirectories(tmpCatalog.getParent); Files.deleteIfExists(tmpCatalog) }
                       .mapError(PipelineError.Failed(_))
      _           <- catalog.snapshotTo(tmpCatalog).mapError(PipelineError.Failed(_))
      // Materialize the post-burn state into the snapshot that each disc embeds,
      // so a single disc fully describes the archive *including itself*. This
      // applies the update to the copy only; the live catalog is committed below.
      _           <- CatalogStore.applyUpdateTo(tmpCatalog, update).mapError(PipelineError.Failed(_))
      pointers    <- catalog.archivePointers(p.id).mapError(PipelineError.Failed(_))
      reports     <- ZIO.foreach(discData) { dd =>
                       val identity = DiscIdentity(name, dd.disc.ordinal, discCount, dd.sessOrd, now)
                       val meta  = DiscMetadata(identity, dd.manifest, tmpCatalog, pointers)
                       val image = DiscImage(
                         paths.workDir.resolve("staging").resolve(s"disc-${dd.disc.ordinal}"),
                         snapRoot, dd.disc.files, meta)
                       burner.burn(image).mapError(PipelineError.Failed(_))
                         .map(r => DiscBurn(dd.disc.ordinal, r.stagingDir, r.imageBytes))
                     }
      // Commit to the live catalog only after every disc is staged — the
      // crash-safe commit point. The committed state equals the embedded snapshot.
      _           <- catalog.applyUpdate(update).mapError(PipelineError.Failed(_))
      _           <- snapshot.release(SnapshotHandle(snapRoot))
      _           <- staged.clear(paths.workDir)
      _           <- ZIO.attemptBlockingIO(Files.deleteIfExists(tmpCatalog)).mapError(PipelineError.Failed(_))
      // burnPlan.oversized is always empty here (burn hard-fails OversizedFiles above);
      // the field is kept for the BurnReport shape the CLI (3b) renders.
    yield BurnReport(reports, burnPlan.oversized.map(_.relPath))

  /** Highest existing session ordinal per disc ordinal (for the next ordinal). */
  private def existingSessions(pid: ProjectId): IO[PipelineError, Map[Int, Int]] =
    for
      b <- catalog.sessionsByState(pid, SessionState.BURNED).mapError(PipelineError.Failed(_))
      v <- catalog.sessionsByState(pid, SessionState.VERIFIED).mapError(PipelineError.Failed(_))
    yield (b ++ v).groupBy(_.discOrdinal).view.mapValues(_.map(_.ordinal).max).toMap

  private def toDiscWrite(
      name: String, d: PlannedDisc, discType: DiscType, sessOrd: Int, now: java.time.Instant,
      cs: ChangeSet, tombs: List[RelPath], manifest: SessionManifest): DiscWrite =
    val addedPaths = cs.added.iterator.map(_.relPath).toSet
    val (addedFiles, modifiedFiles) = d.files.partition(f => addedPaths.contains(f.relPath))
    def fw(f: ScannedFile) =
      FileWrite(f.relPath, f.contentHash, f.sizeBytes, f.mtime, "/data/" + f.relPath.value, 0)
    DiscWrite(
      ordinal = d.ordinal, isNew = d.isNew, discType = discType, usableBytes = d.usableBytes,
      status = if d.willFinalize then DiscStatus.FINALIZED else DiscStatus.OPEN,
      label = s"$name disc ${d.ordinal}",
      session = SessionWrite(sessOrd, now, manifestHash(manifest),
        addedFiles.map(fw), modifiedFiles.map(fw), tombs))

  private def manifestHash(m: SessionManifest): String =
    val json = ManifestJson.from(m).toJson
    java.security.MessageDigest.getInstance("SHA-256")
      .digest(json.getBytes("UTF-8")).map(b => f"${b & 0xff}%02x").mkString

  def reset(name: String): IO[PipelineError, Unit] =
    for
      _ <- project(name)
      _ <- staged.clear(paths.workDir)
      _ <- ZIO.attemptBlockingIO {
             Files.deleteIfExists(paths.workDir.resolve("burn-catalog.sqlite"))
             val staging = paths.workDir.resolve("staging")
             if Files.exists(staging) then
               val s = Files.walk(staging)
               try s.sorted(java.util.Comparator.reverseOrder()).forEach(x => Files.deleteIfExists(x))
               finally s.close()
           }.mapError(PipelineError.Failed(_))
      _ <- snapshot.release(SnapshotHandle(paths.workDir.resolve("snapshot")))
    yield ()

  def verify(name: String, discOrdinal: Option[Int]): IO[PipelineError, List[VerifyReport]] =
    for
      p        <- project(name)
      view     <- catalog.readView(p.id).mapError(PipelineError.Failed(_))
      states   <- catalog.readDiscStates(p.id).mapError(PipelineError.Failed(_))
      sessions <- catalog.sessionsByState(p.id, SessionState.BURNED).mapError(PipelineError.Failed(_))
      targets   = states.map(_.ordinal).filter(o => discOrdinal.forall(_ == o))
      reps     <- ZIO.foreach(targets)(o => verifyDisc(p.id, o, view, sessions))
    yield reps

  private def verifyDisc(
      pid: ProjectId, ordinal: Int, view: CatalogView, sessions: List[Session]
  ): IO[PipelineError, VerifyReport] =
    val dataDir = paths.workDir.resolve("staging").resolve(s"disc-$ordinal").resolve("data")
    for
      existed <- ZIO.attemptBlockingIO(Files.exists(dataDir)).mapError(PipelineError.Failed(_))
      files <- ZIO.attemptBlockingIO {
                 if existed then
                   val s = Files.walk(dataDir)
                   try s.toScala(List).filter(p => Files.isRegularFile(p))
                   finally s.close()
                 else Nil
               }.mapError(PipelineError.Failed(_))
      mismatches <- ZIO.foreach(files) { f =>
                      val rel = RelPath(dataDir.relativize(f).toString)
                      hasher.hash(f).mapError(PipelineError.Failed(_)).map { h =>
                        Option.when(!view.current.get(rel).exists(_.contentHash == h))(rel)
                      }
                    }.map(_.flatten)
      _ <- ZIO.when(existed && mismatches.isEmpty)(
             sessions.filter(_.discOrdinal == ordinal).lastOption match
               case Some(s) => catalog.markVerified(pid, ordinal, s.ordinal).mapError(PipelineError.Failed(_))
               case None    => ZIO.unit)
    yield VerifyReport(ordinal, files.size, mismatches)
