package diskrement.cli.it

import java.nio.charset.StandardCharsets.UTF_8
import java.nio.file.Files

final case class CliResult(exitCode: Int, stdout: String, stderr: String)

/** Execs the staged launcher with an isolated environment. The catalog is always
  * forced via --catalog; HOME/XDG_DATA_HOME/STATE_DIRECTORY are redirected into
  * the scratch dir so even a path-resolution bug cannot touch real locations. */
final class Cli(scratch: Scratch, discBytes: Option[Long]):
  private val bin: String =
    sys.props.getOrElse("diskrement.bin",
      sys.error("diskrement.bin not set — run via `sbt cli/IntegrationTest/test`"))

  def run(args: String*): CliResult =
    val cmd = (Seq(bin, "--catalog", scratch.catalog.toString) ++ args)
    val pb  = new ProcessBuilder(cmd*)
    val env = pb.environment()
    env.put("HOME", scratch.home.toString)
    env.put("XDG_DATA_HOME", scratch.xdg.toString)
    env.put("STATE_DIRECTORY", scratch.state.toString)
    discBytes.foreach(b => env.put("DISKREMENT_DISC_BYTES", b.toString))

    val outF = Files.createTempFile(scratch.dir, "out", ".txt")
    val errF = Files.createTempFile(scratch.dir, "err", ".txt")
    pb.redirectOutput(outF.toFile)
    pb.redirectError(errF.toFile)
    val code = pb.start().waitFor()
    val out  = new String(Files.readAllBytes(outF), UTF_8)
    val err  = new String(Files.readAllBytes(errF), UTF_8)
    Files.deleteIfExists(outF)
    Files.deleteIfExists(errF)
    CliResult(code, out, err)
