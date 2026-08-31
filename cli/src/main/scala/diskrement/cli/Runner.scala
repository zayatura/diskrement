package diskrement.cli

import diskrement.core.RelPath
import diskrement.shell.burn.{
  BurnDevice, BurnOperator, CmdRunner, DiscReader, MediaInspector,
  RealDiscVerifier, SimulatedBurner, SnapshotWriter, XorrisoBurner
}
import diskrement.shell.catalog.{CatalogStore, Db, DbPath}
import diskrement.shell.fs.*
import diskrement.shell.model.{Project, ProjectInit}
import diskrement.shell.pipeline.*
import diskrement.shell.ProgressSink
import zio.*
import java.nio.file.{Files, Path, Paths}

/** Console implementation of the operator port: insert prompts + progress lines. */
private object ConsoleBurnOperator:
  val live: ZLayer[Any, Nothing, BurnOperator] = ZLayer.succeed(new BurnOperator:
    def confirmInsert(message: String): UIO[Boolean] = Prompt.confirm(message, assumeYes = false)
    def progress(line: String): UIO[Unit]            = Console.printLine(line).orDie)

/** Resolves paths, wires the Pipeline layer stack, runs one operation, and
  * renders the result. Returns a process ExitCode; never fails. */
object Runner:

  def run(global: GlobalOpts, cmd: DiskrementCommand): UIO[ExitCode] =
    program(global, cmd)
      .catchAll { e =>
        val (msg, code) = Render.handle(e)
        Console.printLineError(msg).orDie.as(code)
      }
      .catchAllDefect { t =>
        val detail = Option(t.getMessage).getOrElse(t.getClass.getSimpleName)
        Console.printLineError(s"internal error: $detail (this is a bug)").orDie.as(ExitCode(5))
      }

  // --- the happy path; PipelineError is handled by `run` ---
  private def program(global: GlobalOpts, cmd: DiskrementCommand): IO[PipelineError, ExitCode] =
    val name = projectName(cmd)
    for
      env      <- ZIO.attempt(java.lang.System.getenv()).map(m => (k: String) => Option(m.get(k))).mapError(PipelineError.Failed(_))
      home     <- ZIO.attempt(Paths.get(java.lang.System.getProperty("user.home"))).mapError(PipelineError.Failed(_))
      resolved  = PathResolver.resolve(name, global, env, home)
      backend   = BackendResolver.resolve(global, env)
      _        <- ZIO.attempt(Files.createDirectories(resolved.dataDir)).mapError(PipelineError.Failed(_))
      code     <- dispatch(cmd, resolved.catalogPath, backend)
    yield code

  private def projectName(cmd: DiskrementCommand): String = cmd match
    case c: DiskrementCommand.Init    => c.name
    case c: DiskrementCommand.Scan    => c.name
    case c: DiskrementCommand.Plan    => c.name
    case c: DiskrementCommand.Burn    => c.name
    case c: DiskrementCommand.Reset   => c.name
    case c: DiskrementCommand.Status  => c.name
    case c: DiskrementCommand.Verify  => c.name
    case c: DiskrementCommand.History => c.name

  private def dispatch(cmd: DiskrementCommand, catalogPath: Path, backend: Backend): IO[PipelineError, ExitCode] =
    cmd match
      case c: DiskrementCommand.Init =>
        ZIO.attempt(Files.createDirectories(c.workDir)).mapError(PipelineError.Failed(_)) *>
          withPipeline(catalogPath, c.workDir, backend) { pipeline =>
            val pi = ProjectInit(c.name, c.source.toString, c.workDir.toString, c.discType, c.marginPercent, c.copies, java.time.Instant.now())
            pipeline.init(pi).flatMap(id => printOut(Render.init(c.name, id))).as(ExitCode.success)
          }

      case c: DiskrementCommand.Scan =>
        withProject(catalogPath, c.name, backend) { (_, pipeline) =>
          pipeline.scan(c.name, c.verify, c.force).flatMap(r => printOut(Render.scan(r))).as(ExitCode.success)
        }

      case c: DiskrementCommand.Plan =>
        withProject(catalogPath, c.name, backend) { (_, pipeline) =>
          pipeline.plan(c.name, c.discType, c.willFinalize).flatMap(r => printOut(Render.plan(r))).as(ExitCode.success)
        }

      case c: DiskrementCommand.Burn =>
        // Burn the plan exactly as staged (it carries the disc type chosen at `plan`
        // time). We intentionally do NOT re-plan here: re-planning with `None` would
        // overwrite the staged plan with the project's default disc type, silently
        // discarding a `plan --disc-type ...` choice. The user saw the plan when they
        // ran `plan`; `Render.burn` then reports what was actually written.
        withProject(catalogPath, c.name, backend) { (_, pipeline) =>
          for
            ok   <- Prompt.confirm(s"Burn the staged plan for ${c.name}?", c.assumeYes)
            code <- if !ok then printOut("Aborted.").as(ExitCode.success)
                    else pipeline.burn(c.name).flatMap(r => printOut(Render.burn(r))).as(ExitCode.success)
          yield code
        }

      case c: DiskrementCommand.Reset =>
        withProject(catalogPath, c.name, backend) { (_, pipeline) =>
          pipeline.reset(c.name) *> printOut(s"Reset ${c.name}.").as(ExitCode.success)
        }

      case c: DiskrementCommand.Status =>
        withProject(catalogPath, c.name, backend) { (project, pipeline) =>
          pipeline.status(c.name).flatMap(r => printOut(Render.status(project, r))).as(ExitCode.success)
        }

      case c: DiskrementCommand.Verify =>
        withProject(catalogPath, c.name, backend) { (_, pipeline) =>
          pipeline.verify(c.name, c.disc).flatMap(rs => printOut(Render.verify(rs)).as(Render.verifyExit(rs)))
        }

      case c: DiskrementCommand.History =>
        withProject(catalogPath, c.name, backend) { (_, pipeline) =>
          val rel = RelPath(c.relPath)
          pipeline.history(c.name, rel).flatMap(es => printOut(Render.history(rel, es))).as(ExitCode.success)
        }

  private def printOut(s: String): UIO[Unit] = Console.printLine(s).orDie

  /** Load the project row (for workDir + metadata), then run with a wired Pipeline. */
  private def withProject[A](catalogPath: Path, name: String, backend: Backend)(
      use: (Project, Pipeline) => IO[PipelineError, A]): IO[PipelineError, A] =
    loadProject(catalogPath, name).flatMap { project =>
      withPipeline(catalogPath, Paths.get(project.workDir), backend)(pipeline => use(project, pipeline))
    }

  private def loadProject(catalogPath: Path, name: String): IO[PipelineError, Project] =
    ZIO.serviceWithZIO[CatalogStore](_.findProject(name))
      .provideLayer(ZLayer.succeed(DbPath(catalogPath)) >>> Db.poolLayer >>> CatalogStore.live)
      .mapError(PipelineError.Failed(_))
      .flatMap {
        case Some(p) => ZIO.succeed(p)
        case None    => ZIO.fail(PipelineError.ProjectNotFound(name))
      }

  /** Build the full Pipeline layer stack for one (catalog, workDir) pair and run `use`. */
  private def withPipeline[A](catalogPath: Path, workDir: Path, backend: Backend)(
      use: Pipeline => IO[PipelineError, A]): IO[PipelineError, A] =
    val layer = backend match
      case Backend.Simulated =>
        ZLayer.make[Pipeline](
          ZLayer.succeed(ProjectPaths(catalogPath, workDir)),
          ZLayer.fromFunction((p: ProjectPaths) => DbPath(p.catalogPath)),
          Db.poolLayer,
          FileSystem.live, Hasher.live, FileScanner.live,
          ProgressSink.noop, HashParallelism.fromEnv,
          SnapshotWriter.live, SimulatedBurner.live, SourceSnapshot.live,
          StagedStore.live, CatalogStore.live, StagingDiscVerifier.live,
          Pipeline.live)
      case Backend.Real(device) =>
        ZLayer.make[Pipeline](
          ZLayer.succeed(ProjectPaths(catalogPath, workDir)),
          ZLayer.fromFunction((p: ProjectPaths) => DbPath(p.catalogPath)),
          Db.poolLayer,
          FileSystem.live, Hasher.live, FileScanner.live,
          ProgressSink.noop, HashParallelism.fromEnv,
          SnapshotWriter.live, SourceSnapshot.live,
          StagedStore.live, CatalogStore.live,
          ZLayer.succeed(device: BurnDevice),
          CmdRunner.live, MediaInspector.live, DiscReader.live,
          ConsoleBurnOperator.live,
          XorrisoBurner.live, RealDiscVerifier.live,
          Pipeline.live)
    ZIO.serviceWithZIO[Pipeline](use)
      .provideLayer(layer)
      .mapError {
        case pe: PipelineError => pe
        case t: Throwable      => PipelineError.Failed(t)
      }
