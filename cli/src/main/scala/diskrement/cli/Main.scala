package diskrement.cli

import zio.cli.*
import zio.cli.HelpDoc.Span.text

object Main extends ZIOCliDefault:
  val cliApp = CliApp.make(
    name = "diskrement",
    version = "0.1.0",
    summary = text("incremental backups to optical discs, with a memory"),
    command = Commands.root
  ) { case (global, cmd) => Runner.run(global, cmd) }
