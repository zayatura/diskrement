package diskrement.cli.it

import zio.*
import zio.test.*
import java.nio.file.{Files, Paths}

/** Proves the IntegrationTest config is wired: the staged binary path is passed
  * via -Ddiskrement.bin and the launcher runs. */
object BuildSmokeSpec extends ZIOSpecDefault:
  def spec = suite("build smoke")(
    test("diskrement.bin system property points at an existing executable"):
      ZIO.attempt {
        val bin = sys.props.getOrElse("diskrement.bin", "")
        (bin, Files.isRegularFile(Paths.get(bin)))
      }.map { case (bin, exists) =>
        assertTrue(bin.nonEmpty, exists)
      }
    ,
    test("`diskrement --help` exits 0"):
      ZIO.attemptBlocking {
        val p = new ProcessBuilder(sys.props("diskrement.bin"), "--help")
          .redirectOutput(ProcessBuilder.Redirect.DISCARD)
          .redirectError(ProcessBuilder.Redirect.DISCARD)
          .start()
        p.waitFor()
      }.map(code => assertTrue(code == 0))
  )
