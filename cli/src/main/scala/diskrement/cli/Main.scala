package diskrement.cli

import zio.*
import zio.cli.*
import zio.cli.HelpDoc.Span.text

object Main extends ZIOCliDefault:
  /** ZIO's default logger writes to stdout, which is the CLI's data channel
    * (the IT suite parses it). Operator-facing diagnostics belong on stderr. */
  override val bootstrap: ZLayer[Any, Nothing, Unit] =
    Runtime.removeDefaultLoggers >>> Runtime.addLogger(
      (_, _, level: LogLevel, message: () => String, _, _, _, _) =>
        java.lang.System.err.println(s"[${level.label.toLowerCase}] ${message()}"))

  val cliApp = CliApp.make(
    name = "diskrement",
    version = "0.1.0",
    summary = text("incremental backups to optical discs, with a memory"),
    command = Commands.root
  ) { case (global, cmd) => Runner.run(global, cmd) }

  // ZIOCli.run() runs cliApp and discards the ExitCode the handler returns, so the
  // process always exits 0 on success. Override to propagate Runner's ExitCode to
  // the process. Built-ins (--help/--version) yield Unit -> exit 0; zio-cli
  // validation errors still fail the effect -> exit 1.
  override def run =
    for
      args   <- getArgs
      result <- cliApp.run(args.toList)
      _      <- result match
                  case code: ExitCode => exit(code)
                  case _              => ZIO.unit
    yield ()
