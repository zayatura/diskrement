package diskrement.shell.burn

import zio._

/** Scripted CmdRunner: matches each command by a predicate on its args and
  * replies with a canned result; `-extract` commands may also write the dest file. */
final case class FakeCmdRunner(script: List[String] => UIO[CmdResult]) extends CmdRunner:
  def run(command: List[String], onLine: String => UIO[Unit]): IO[BurnError, CmdResult] =
    script(command)
