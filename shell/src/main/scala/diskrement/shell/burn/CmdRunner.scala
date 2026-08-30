package diskrement.shell.burn

import zio._
import java.io.{BufferedReader, InputStreamReader}

final case class CmdResult(exitCode: Int, output: List[String]):
  def tail(n: Int = 10): List[String] = output.takeRight(n)

/** Runs one external command, streaming each combined stdout/stderr line through
  * `onLine` as it appears (xorriso progress reaches the operator live). Fails only
  * when the process cannot be launched; a nonzero exit is a normal CmdResult. */
trait CmdRunner:
  def run(command: List[String], onLine: String => UIO[Unit] = _ => ZIO.unit): IO[BurnError, CmdResult]

final case class CmdRunnerLive() extends CmdRunner:
  def run(command: List[String], onLine: String => UIO[Unit]): IO[BurnError, CmdResult] =
    ZIO.attemptBlocking {
      val pb = new ProcessBuilder(command*)
      pb.redirectErrorStream(true)
      pb.start()
    }.mapError {
      case e: java.io.IOException
          if Option(e.getMessage).exists(_.toLowerCase.contains("no such file")) =>
        BurnError.ToolMissing(s"cannot launch ${command.headOption.getOrElse("?")}: ${e.getMessage}")
      case t => BurnError.BurnIoError(t)
    }.flatMap { proc =>
      val reader = new BufferedReader(new InputStreamReader(proc.getInputStream))
      def loop(acc: List[String]): IO[BurnError, List[String]] =
        ZIO.attemptBlockingIO(Option(reader.readLine())).mapError(BurnError.BurnIoError(_)).flatMap {
          case None       => ZIO.succeed(acc.reverse)
          case Some(line) => onLine(line) *> loop(line :: acc)
        }
      for
        lines <- loop(Nil)
        code  <- ZIO.attemptBlocking(proc.waitFor()).mapError(BurnError.BurnIoError(_))
      yield CmdResult(code, lines)
    }

object CmdRunner:
  val live: ZLayer[Any, Nothing, CmdRunner] = ZLayer.succeed(CmdRunnerLive())
