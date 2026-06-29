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

  def plan(name: String, discTypeOverride: Option[DiscType], finalize: Boolean): IO[PipelineError, PlanReport] =
    for
      p       <- project(name)
      scanned <- staged.readScan(paths.workDir).flatMap {
                   case Some(s) => ZIO.succeed(s)
                   case None    => ZIO.fail(PipelineError.NoStagedScan(name))
                 }
      view    <- catalog.readView(p.id).mapError(PipelineError.Failed(_))
      states  <- catalog.readDiscStates(p.id).mapError(PipelineError.Failed(_))
      // Test-only: DISKREMENT_DISC_BYTES overrides usable capacity directly
      // (bypassing margin) so multi-disc scenarios are cheap and deterministic.
      // Not documented in --help; read via zio.System so TestSystem can inject it.
      capBytes <- zio.System.env("DISKREMENT_DISC_BYTES").mapError(PipelineError.Failed(_))
                    .map(_.flatMap(_.toLongOption).filter(_ > 0L))
      discType  = discTypeOverride.getOrElse(p.defaultDiscType)
      cap       = capBytes.fold(Capacity.fromDiscType(discType, p.marginPercent))(Capacity(_))
      cs       = Diff.diff(view, scanned)
      full     = Planner.plan(cs, states, cap)
      // The planner already finalizes the first disc iff the plan spilled; the
      // --finalize flag forces a seal even when everything fit.
      // Planner.plan always returns at least one disc in BurnPlan.discs.
      // One disc per cycle: burn the first disc that actually carries new files.
      // The planner keeps the open disc first by contract; when a new file can't
      // top it off, that open disc is an empty finalizing placeholder — skip it so
      // this cycle writes the new disc instead of wasting a cycle on an empty
      // session (the skipped disc stays open). If no disc has files (a no-op, or an
      // explicit --finalize of an empty open disc), keep the head as-is.
      headIdx    = full.discs.indexWhere(_.files.nonEmpty)
      idx        = if headIdx < 0 then 0 else headIdx
      head       = full.discs(idx)
      sealedDisc = head.copy(willFinalize = head.willFinalize || finalize)
      tail       = full.discs.drop(idx + 1)
      // full.tombstones are this scan's deletions, not per-disc: on a cross-cycle spill (idx > 0 skips the empty open disc) they attach to the chosen new disc's session.
      remaining  = RemainingEstimate(
                     tail.size, tail.map(_.files.size).sum, tail.flatMap(_.files).map(_.sizeBytes).sum)
      report     = PlanReport(sealedDisc, discType, full.tombstones, remaining, full.oversized)
      _       <- staged.writePlan(paths.workDir, report)
    yield report

  def burn(name: String): IO[PipelineError, BurnReport] =
    for
      p         <- project(name)
      scanned   <- staged.readScan(paths.workDir).flatMap {
                     case Some(s) => ZIO.succeed(s); case None => ZIO.fail(PipelineError.NoStagedScan(name)) }
      report    <- staged.readPlan(paths.workDir).flatMap {
                     case Some(r) => ZIO.succeed(r); case None => ZIO.fail(PipelineError.NoStagedPlan(name)) }
      _         <- ZIO.when(report.oversized.nonEmpty)(
                     ZIO.fail(PipelineError.OversizedFiles(report.oversized.map(_.relPath))))
      snapRoot   = paths.workDir.resolve("snapshot")
      _         <- ZIO.attemptBlockingIO(Files.exists(snapRoot)).mapError(PipelineError.Failed(_))
                     .flatMap(e => ZIO.unless(e)(ZIO.fail(PipelineError.SnapshotMissing(name))).unit)
      view      <- catalog.readView(p.id).mapError(PipelineError.Failed(_))
      changeSet  = Diff.diff(view, scanned)
      now       <- Clock.instant
      existing  <- existingSessions(p.id)
      disc       = report.disc
      discType   = report.discType
      tombs      = report.tombstones
      sessOrd    = existing.getOrElse(disc.ordinal, 0) + 1
      manifest   = ManifestBuilder.build(name, sessOrd, disc, changeSet, tombs)
      label      = s"$name disc ${disc.ordinal}"
      update     = CatalogUpdate(p.id, Nil,
                     List(toDiscWrite(name, disc, discType, sessOrd, now, changeSet, tombs, manifest)))
      tmpCatalog = paths.workDir.resolve("burn-catalog.sqlite")
      _         <- ZIO.attemptBlockingIO {
                     Files.createDirectories(tmpCatalog.getParent); Files.deleteIfExists(tmpCatalog) }
                     .mapError(PipelineError.Failed(_))
      _         <- catalog.snapshotTo(tmpCatalog).mapError(PipelineError.Failed(_))
      // Materialize the post-burn state into the snapshot this disc embeds, so a
      // single disc fully describes the archive including its own session. Applies
      // to the copy only; the live catalog is committed after the disc is staged.
      _         <- CatalogStore.applyUpdateTo(tmpCatalog, update).mapError(PipelineError.Failed(_))
      pointers  <- catalog.archivePointers(p.id).mapError(PipelineError.Failed(_))
      identity   = DiscIdentity(name, disc.ordinal, sessOrd, now)
      meta       = DiscMetadata(identity, manifest, tmpCatalog, pointers)
      image      = DiscImage(
                     paths.workDir.resolve("staging").resolve(s"disc-${disc.ordinal}"),
                     snapRoot, disc.files, meta)
      burned    <- burner.burn(image).mapError(PipelineError.Failed(_))
      // Commit the single session only after the disc is staged — the crash-safe
      // commit point. The committed state equals the embedded snapshot.
      _         <- catalog.applyUpdate(update).mapError(PipelineError.Failed(_))
      _         <- snapshot.release(SnapshotHandle(snapRoot))
      _         <- staged.clear(paths.workDir)
      _         <- ZIO.attemptBlockingIO(Files.deleteIfExists(tmpCatalog)).mapError(PipelineError.Failed(_))
    yield BurnReport(
      DiscBurn(disc.ordinal, label, disc.willFinalize, burned.stagingDir, burned.imageBytes),
      report.remaining)

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
      states   <- catalog.readDiscStates(p.id).mapError(PipelineError.Failed(_))
      sessions <- catalog.sessionsByState(p.id, SessionState.BURNED).mapError(PipelineError.Failed(_))
      targets   = states.map(_.ordinal).filter(o => discOrdinal.forall(_ == o))
      reps     <- ZIO.foreach(targets)(o => verifyDisc(p.id, o, sessions))
    yield reps

  private def verifyDisc(
      pid: ProjectId, ordinal: Int, sessions: List[Session]
  ): IO[PipelineError, VerifyReport] =
    val dataDir = paths.workDir.resolve("staging").resolve(s"disc-$ordinal").resolve("data")
    for
      discMap <- catalog.placementsForDisc(pid, ordinal).mapError(PipelineError.Failed(_))
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
                        Option.when(!discMap.get(rel).contains(h))(rel)
                      }
                    }.map(_.flatten)
      _ <- ZIO.when(existed && mismatches.isEmpty)(
             sessions.filter(_.discOrdinal == ordinal).lastOption match
               case Some(s) => catalog.markVerified(pid, ordinal, s.ordinal).mapError(PipelineError.Failed(_))
               case None    => ZIO.unit)
    yield VerifyReport(ordinal, files.size, mismatches)
