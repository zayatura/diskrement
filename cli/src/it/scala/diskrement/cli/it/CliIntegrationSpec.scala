package diskrement.cli.it

import zio.*
import zio.test.*
import scala.jdk.StreamConverters.*
import java.nio.file.Files

/** Entry point: composes every scenario suite, runs them sequentially, and
  * skips cleanly when the working directory lacks CoW support. */
object CliIntegrationSpec extends ZIOSpecDefault:
  private val noLeftovers =
    test("leaves no scratch directories behind"):
      ZIO.attempt {
        if !Files.exists(Scratch.base) then Nil
        else
          val stream = Files.list(Scratch.base)
          try stream.toScala(List)
            .filter(p => Files.isDirectory(p) && p.getFileName.toString.startsWith("it-"))
          finally stream.close()
      }.map(leftover => assertTrue(leftover.isEmpty))

  def spec = suite("diskrement CLI integration")(
    scenarios.Scenario01Happy.suite,
    scenarios.Scenario02AppendOpen.suite,
    scenarios.Scenario03FinalizedNewDisc.suite,
    scenarios.Scenario04ThreeDisc.suite,
    scenarios.ScenarioExitCodes.suite,
    scenarios.Scenario05Spill.suite,
    scenarios.Scenario06Oversized.suite,
    scenarios.Scenario07Reset.suite,
    scenarios.Scenario08Idempotent.suite,
    scenarios.Scenario09MarkDead.suite,
    scenarios.Scenario10WarmRescan.suite,
    noLeftovers
  ) @@ Support.cowOrIgnore @@ TestAspect.sequential
