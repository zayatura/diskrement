package diskrement.shell.pipeline

import diskrement.core.*
import diskrement.shell.catalog.CatalogStore
import diskrement.shell.burn.SimulatedBurner
import diskrement.shell.fs.{FileScanner, Hasher, SourceSnapshot}
import diskrement.shell.model.*
import zio.*
import java.nio.file.{Files, Path}

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
      _  <- sameFilesystem(Path.of(p.sourceRoot), paths.workDir)
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
