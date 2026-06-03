package diskrement.shell.pipeline

import diskrement.core._
import diskrement.shell.catalog.CatalogStore
import diskrement.shell.model._
import zio._
import zio.test._
import java.nio.file.Files
import java.time.Instant

object PipelineBurnSpec extends ZIOSpecDefault:
  private def initOf(name: String, src: java.nio.file.Path, work: java.nio.file.Path) =
    ProjectInit(name, src.toString, work.toString, DiscType.BD_R, 2.0, 1,
      Instant.parse("2026-06-01T00:00:00Z"))

  def spec = suite("Pipeline burn")(
    test("burn stages discs, writes /.diskrement, and commits the catalog"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.txt" -> "aaa", "sub/b.txt" -> "bbbb"))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          _     <- pipe.scan("p", verify = false, force = false)
          _     <- pipe.plan("p", None)
          rep   <- pipe.burn("p")
          d0     = rep.discs.head
          dataA <- ZIO.attempt(Files.readString(d0.stagingDir.resolve("data/a.txt")))
          hasManifest <- ZIO.attempt(Files.exists(d0.stagingDir.resolve(".diskrement/manifest.json")))
          hasCatalog  <- ZIO.attempt(Files.exists(d0.stagingDir.resolve(".diskrement/catalog.sqlite")))
          proj  <- ZIO.serviceWithZIO[CatalogStore](_.findProject("p"))
          view  <- ZIO.serviceWithZIO[CatalogStore](_.readView(proj.get.id))
          states<- ZIO.serviceWithZIO[CatalogStore](_.readDiscStates(proj.get.id))
        yield assertTrue(
          rep.discs.size == 1, rep.oversized.isEmpty,
          dataA == "aaa", hasManifest, hasCatalog,
          view.current.keySet.map(_.value) == Set("a.txt", "sub/b.txt"),
          states.size == 1, states.head.usedBytes == 7L)
      },
    test("burn fails NoStagedPlan when plan has not run"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.txt" -> "aaa"))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          _     <- pipe.scan("p", verify = false, force = false)
          err   <- pipe.burn("p").flip
        yield assertTrue(err.isInstanceOf[PipelineError.NoStagedPlan])
      }
  ).provideLayer(PipelineTestKit.env.orDie)
