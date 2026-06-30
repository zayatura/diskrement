package diskrement.shell.pipeline

import diskrement.core.*
import diskrement.shell.model.*
import zio.*
import zio.test.*
import java.time.Instant

object PipelineCapacityOverrideSpec extends ZIOSpecDefault:
  private def initOf(name: String, src: java.nio.file.Path, work: java.nio.file.Path) =
    ProjectInit(name, src.toString, work.toString, DiscType.BD_R, 2.0, 1,
      Instant.parse("2026-06-01T00:00:00Z"))

  // Two 60-byte files: trivially fit a real BD-R together, but with a 100-byte
  // override the second spills to another disc.
  private val files = Map("a.bin" -> ("a" * 60), "b.bin" -> ("b" * 60))

  def spec = suite("DISKREMENT_DISC_BYTES override")(
    test("absent: everything fits one disc (no remaining)"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(files)
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          _     <- pipe.scan("p", verify = false, force = false)
          plan  <- pipe.plan("p", None, finalize = false)
        yield assertTrue(plan.remaining.moreDiscs == 0)
      }
    ,
    test("set to 100: the plan spills to more discs"):
      ZIO.scoped {
        for
          _     <- TestSystem.putEnv("DISKREMENT_DISC_BYTES", "100")
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(files)
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("q", src, paths.workDir))
          _     <- pipe.scan("q", verify = false, force = false)
          plan  <- pipe.plan("q", None, finalize = false)
        yield assertTrue(plan.remaining.moreDiscs >= 1)
      }
    ,
    test("a value >= normal usable capacity is ignored (override only shrinks)"):
      ZIO.scoped {
        for
          _     <- TestSystem.putEnv("DISKREMENT_DISC_BYTES", "999999999999") // ~1TB, far above BD_R usable
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(files)
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("r", src, paths.workDir))
          _     <- pipe.scan("r", verify = false, force = false)
          plan  <- pipe.plan("r", None, finalize = false)
        yield assertTrue(
          plan.remaining.moreDiscs == 0,
          plan.disc.usableBytes == Capacity.fromDiscType(DiscType.BD_R, 2.0).usableBytes)
      }
  ).provideLayer(PipelineTestKit.env.orDie)
