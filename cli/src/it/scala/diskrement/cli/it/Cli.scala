package diskrement.cli.it

import java.nio.charset.StandardCharsets.UTF_8
import java.nio.file.Files

final case class CliResult(exitCode: Int, stdout: String, stderr: String)

/** Execs the staged launcher with an isolated environment. Isolation is primarily
  * guaranteed by always passing --catalog, which short-circuits PathResolver's
  * fallback chain. HOME/XDG_DATA_HOME/STATE_DIRECTORY are also redirected into the
  * scratch dir as defense-in-depth (note: on Linux the JVM derives user.home from
  * the password database, not $HOME, so the HOME override is belt-and-suspenders). */
final class Cli(scratch: Scratch, discBytes: Option[Long]):
  private val bin: String =
    sys.props.getOrElse("diskrement.bin",
      sys.error("diskrement.bin not set — run via `sbt cli/IntegrationTest/test`"))

  def run(args: String*): CliResult =
    val cmd = Seq(bin, "--catalog", scratch.catalog.toString) ++ args
    val pb  = new ProcessBuilder(cmd*)
    val env = pb.environment()
    env.put("HOME", scratch.home.toString)
    env.put("XDG_DATA_HOME", scratch.xdg.toString)
    env.put("STATE_DIRECTORY", scratch.state.toString)
    env.put("DISKREMENT_BURNER", "simulated")
    discBytes match
      case Some(b) => env.put("DISKREMENT_DISC_BYTES", b.toString)
      case None    => env.remove("DISKREMENT_DISC_BYTES")

    val outF = Files.createTempFile(scratch.dir, "out", ".txt")
    val errF = Files.createTempFile(scratch.dir, "err", ".txt")
    pb.redirectOutput(outF.toFile)
    pb.redirectError(errF.toFile)
    val process = pb.start()
    process.getOutputStream.close() // EOF on stdin: a missing --yes safely declines instead of hanging
    val timedOut = !process.waitFor(60, java.util.concurrent.TimeUnit.SECONDS)
    if timedOut then
      process.destroyForcibly()
      throw new RuntimeException(s"CLI timed out after 60s: ${cmd.mkString(" ")}")
    val code = process.exitValue()
    val out  = new String(Files.readAllBytes(outF), UTF_8)
    val err  = new String(Files.readAllBytes(errF), UTF_8)
    Files.deleteIfExists(outF)
    Files.deleteIfExists(errF)
    CliResult(code, out, err)
