package diskrement.shell.pipeline

import diskrement.core._
import diskrement.shell.model._
import zio._
import zio.test._
import java.time.Instant

object PipelineInitStatusHistorySpec extends ZIOSpecDefault:
  private def initOf(name: String, src: java.nio.file.Path, work: java.nio.file.Path) =
    ProjectInit(name, src.toString, work.toString, DiscType.BD_R, 2.0, 1,
      Instant.parse("2026-06-01T00:00:00Z"))

  def spec = suite("Pipeline init/status/history")(
    test("init creates a project; status reports zero discs"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.txt" -> "aaa"))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("photos", src, paths.workDir))
          st    <- pipe.status("photos")
        yield assertTrue(st.discs.isEmpty, st.totalCurrentFiles == 0, st.openDiscOrdinal.isEmpty)
      },
    test("init fails NotCowCapable when the work dir is not CoW-capable"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.txt" -> "aaa"))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot(cowOk = false))
          err   <- pipe.init(initOf("photos", src, paths.workDir)).flip
        yield assertTrue(err.isInstanceOf[PipelineError.NotCowCapable])
      },
    test("status on an unknown project fails ProjectNotFound"):
      ZIO.scoped {
        for
          pipe <- PipelineTestKit.mkPipeline(FakeSnapshot())
          err  <- pipe.status("nope").flip
        yield assertTrue(err.isInstanceOf[PipelineError.ProjectNotFound])
      }
  ).provideLayer(PipelineTestKit.env.orDie)
