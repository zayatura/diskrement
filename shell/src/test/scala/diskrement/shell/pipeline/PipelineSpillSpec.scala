package diskrement.shell.pipeline

import diskrement.core._
import diskrement.shell.catalog.CatalogStore
import diskrement.shell.model._
import zio._
import zio.test._
import java.time.Instant

object PipelineSpillSpec extends ZIOSpecDefault:
  // Extreme margin -> tiny usable capacity (~50 bytes) so small files spill / go oversized.
  // BD_R nominal = 25_000_000_000; margin 99.9999998 -> usable = floor(25e9 * 2e-9) = 50 bytes.
  private def initOf(name: String, src: java.nio.file.Path, work: java.nio.file.Path) =
    ProjectInit(name, src.toString, work.toString, DiscType.BD_R, 99.9999998, 1,
      Instant.parse("2026-06-01T00:00:00Z"))

  def spec = suite("Pipeline spill / oversized")(
    test("two files larger than half capacity spill across two discs and both commit"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.txt" -> ("a" * 40), "b.txt" -> ("b" * 40)))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          _     <- pipe.scan("p", verify = false, force = false)
          plan  <- pipe.plan("p", None)
          rep   <- pipe.burn("p")
          proj  <- ZIO.serviceWithZIO[CatalogStore](_.findProject("p"))
          states<- ZIO.serviceWithZIO[CatalogStore](_.readDiscStates(proj.get.id))
          view  <- ZIO.serviceWithZIO[CatalogStore](_.readView(proj.get.id))
        yield assertTrue(
          plan.discs.size == 2,
          plan.oversized.isEmpty,
          rep.discs.map(_.ordinal).sorted == List(1, 2),
          states.size == 2,
          view.current.keySet.map(_.value) == Set("a.txt", "b.txt"))
      },
    test("a file larger than any disc is surfaced by plan and rejected by burn"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("big.txt" -> ("x" * 100)))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          _     <- pipe.scan("p", verify = false, force = false)
          plan  <- pipe.plan("p", None)
          err   <- pipe.burn("p").flip
        yield assertTrue(
          plan.oversized.map(_.relPath.value) == List("big.txt"),
          err.isInstanceOf[PipelineError.OversizedFiles])
      }
  ).provideLayer(PipelineTestKit.env.orDie)
