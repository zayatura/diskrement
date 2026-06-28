package diskrement.shell.pipeline

import diskrement.core._
import diskrement.shell.model._
import zio._
import zio.test._
import java.time.Instant

object PipelinePlanSpec extends ZIOSpecDefault:
  private def initOf(name: String, src: java.nio.file.Path, work: java.nio.file.Path) =
    ProjectInit(name, src.toString, work.toString, DiscType.BD_R, 2.0, 1,
      Instant.parse("2026-06-01T00:00:00Z"))

  def spec = suite("Pipeline plan")(
    test("plan from a staged scan places all files on one new disc and stages the plan"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.txt" -> "aaa", "b.txt" -> "bb"))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          _     <- pipe.scan("p", verify = false, force = false)
          plan  <- pipe.plan("p", discTypeOverride = None, finalize = false)
          staged<- ZIO.serviceWithZIO[StagedStore](_.readPlan(paths.workDir))
        yield assertTrue(
          plan.disc.isNew,
          plan.disc.files.map(_.relPath.value).sorted == List("a.txt", "b.txt"),
          plan.oversized.isEmpty,
          staged.map(_.discType) == Some(DiscType.BD_R))
      },
    test("plan fails NoStagedScan when scan has not run"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.txt" -> "aaa"))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          err   <- pipe.plan("p", None, finalize = false).flip
        yield assertTrue(err.isInstanceOf[PipelineError.NoStagedScan])
      }
  ).provideLayer(PipelineTestKit.env.orDie)
