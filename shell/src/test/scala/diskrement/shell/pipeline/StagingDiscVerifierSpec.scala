package diskrement.shell.pipeline

import diskrement.core.{ContentHash, RelPath}
import diskrement.shell.burn.{DiscExpectation, VerifyOutcome}
import diskrement.shell.fs.HasherLive
import zio._
import zio.test._
import java.nio.file.Files

object StagingDiscVerifierSpec extends ZIOSpecDefault:

  private def sha(s: String) = ContentHash(java.security.MessageDigest.getInstance("SHA-256")
    .digest(s.getBytes("UTF-8")).map(b => f"${b & 0xff}%02x").mkString)

  def spec = suite("StagingDiscVerifierSpec")(
    test("clean staging dir verifies; corrupt file mismatches; missing dir skips") {
      ZIO.attemptBlocking(Files.createTempDirectory("sdv")).orDie.flatMap { work =>
        val data = work.resolve("staging/disc-1/data")
        Files.createDirectories(data)
        Files.writeString(data.resolve("a.txt"), "hello")
        Files.writeString(data.resolve("bad.txt"), "corrupted")
        val v = StagingDiscVerifier(work.resolve("staging"), HasherLive())
        val exp = DiscExpectation("p", 1,
          latestSession = Map(RelPath("a.txt") -> sha("hello"), RelPath("bad.txt") -> sha("original")),
          visible = Map.empty, superseded = Nil)
        for
          out  <- v.verify(exp)
          out2 <- v.verify(exp.copy(discOrdinal = 9))
        yield assertTrue(
          out == VerifyOutcome(2, List(RelPath("bad.txt")), skipped = false),
          out2 == VerifyOutcome(0, Nil, skipped = true))
      }
    }
  )
