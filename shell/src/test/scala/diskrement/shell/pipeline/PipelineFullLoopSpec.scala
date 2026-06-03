package diskrement.shell.pipeline

import diskrement.core._
import diskrement.shell.catalog.CatalogStore
import diskrement.shell.model._
import zio._
import zio.test._
import java.nio.file.Files
import java.time.Instant

object PipelineFullLoopSpec extends ZIOSpecDefault:
  private def initOf(name: String, src: java.nio.file.Path, work: java.nio.file.Path) =
    ProjectInit(name, src.toString, work.toString, DiscType.BD_R, 2.0, 1,
      Instant.parse("2026-06-01T00:00:00Z"))

  def spec = suite("Pipeline full loop")(
    test("init -> scan -> plan -> burn -> modify -> scan -> plan -> burn -> verify -> history"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.txt" -> "aaa", "b.txt" -> "bbb"))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          // first burn
          s1    <- pipe.scan("p", verify = false, force = false)
          _     <- pipe.plan("p", None)
          _     <- pipe.burn("p")
          // modify source: change a.txt, delete b.txt, add c.txt
          _     <- ZIO.attempt {
                     Files.writeString(src.resolve("a.txt"), "aaaa")
                     Files.deleteIfExists(src.resolve("b.txt"))
                     Files.writeString(src.resolve("c.txt"), "cc")
                   }
          // second burn (force a fresh scan since a staged scan exists)
          s2    <- pipe.scan("p", verify = false, force = true)
          _     <- pipe.plan("p", Some(DiscType.BD_R_DL))
          _     <- pipe.burn("p")
          reps  <- pipe.verify("p", None)
          hA    <- pipe.history("p", RelPath("a.txt"))
          hB    <- pipe.history("p", RelPath("b.txt"))
          proj  <- ZIO.serviceWithZIO[CatalogStore](_.findProject("p"))
          view  <- ZIO.serviceWithZIO[CatalogStore](_.readView(proj.get.id))
        yield assertTrue(
          s1 == ScanResult(2, 0, 0, 0),
          s2 == ScanResult(added = 1, modified = 1, deleted = 1, unchanged = 0),
          view.current.keySet.map(_.value) == Set("a.txt", "c.txt"),
          reps.forall(_.mismatches.isEmpty),
          hA.map(_.status) == List(VersionStatus.SUPERSEDED, VersionStatus.CURRENT),
          hB.map(_.status) == List(VersionStatus.DELETED))
      }
  ).provideLayer(PipelineTestKit.env.orDie)
