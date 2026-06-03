package diskrement.shell.pipeline

import diskrement.core._
import diskrement.shell.model._
import zio._
import zio.test._
import java.nio.file.Files
import java.time.Instant

object PipelineScanResetSpec extends ZIOSpecDefault:
  private def initOf(name: String, src: java.nio.file.Path, work: java.nio.file.Path) =
    ProjectInit(name, src.toString, work.toString, DiscType.BD_R, 2.0, 1,
      Instant.parse("2026-06-01T00:00:00Z"))

  def spec = suite("Pipeline scan/reset")(
    test("scan on a fresh project reports all files as added and stages them"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.txt" -> "aaa", "sub/b.txt" -> "bbbb"))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          res   <- pipe.scan("p", verify = false, force = false)
          staged<- ZIO.serviceWithZIO[StagedStore](_.readScan(paths.workDir))
        yield assertTrue(
          res == ScanResult(added = 2, modified = 0, deleted = 0, unchanged = 0),
          staged.map(_.size) == Some(2))
      },
    test("reset clears staged scan and the staging dir"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.txt" -> "aaa"))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          _     <- pipe.scan("p", verify = false, force = false)
          _     <- pipe.reset("p")
          staged<- ZIO.serviceWithZIO[StagedStore](_.readScan(paths.workDir))
          snapGone <- ZIO.attempt(!Files.exists(paths.workDir.resolve("snapshot")))
        yield assertTrue(staged.isEmpty, snapGone)
      }
  ).provideLayer(PipelineTestKit.env.orDie)
