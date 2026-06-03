package diskrement.shell.pipeline

import diskrement.core._
import diskrement.shell.catalog.CatalogStore
import diskrement.shell.model._
import zio._
import zio.test._
import java.nio.file.Files
import java.time.Instant

object PipelineVerifySpec extends ZIOSpecDefault:
  private def initOf(name: String, src: java.nio.file.Path, work: java.nio.file.Path) =
    ProjectInit(name, src.toString, work.toString, DiscType.BD_R, 2.0, 1,
      Instant.parse("2026-06-01T00:00:00Z"))

  def spec = suite("Pipeline verify")(
    test("verify of a clean burned disc reports no mismatches and marks it VERIFIED"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.txt" -> "aaa"))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          _     <- pipe.scan("p", verify = false, force = false)
          _     <- pipe.plan("p", None)
          _     <- pipe.burn("p")
          reps  <- pipe.verify("p", None)
          proj  <- ZIO.serviceWithZIO[CatalogStore](_.findProject("p"))
          ver   <- ZIO.serviceWithZIO[CatalogStore](_.sessionsByState(proj.get.id, SessionState.VERIFIED))
        yield assertTrue(
          reps.size == 1, reps.head.mismatches.isEmpty, reps.head.filesChecked == 1,
          ver.map(_.discOrdinal) == List(1))
      },
    test("verify detects a corrupted staged file"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.txt" -> "aaa"))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          _     <- pipe.scan("p", verify = false, force = false)
          _     <- pipe.plan("p", None)
          rep   <- pipe.burn("p")
          _     <- ZIO.attempt(Files.writeString(rep.discs.head.stagingDir.resolve("data/a.txt"), "CORRUPT"))
          reps  <- pipe.verify("p", None)
        yield assertTrue(reps.head.mismatches.map(_.value) == List("a.txt"))
      },
    test("verify does NOT mark a disc VERIFIED when its staging dir is gone"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.txt" -> "aaa"))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          _     <- pipe.scan("p", verify = false, force = false)
          _     <- pipe.plan("p", None)
          _     <- pipe.burn("p")
          _     <- pipe.reset("p")   // removes the staging dirs (catalog session remains BURNED)
          reps  <- pipe.verify("p", None)
          proj  <- ZIO.serviceWithZIO[CatalogStore](_.findProject("p"))
          ver   <- ZIO.serviceWithZIO[CatalogStore](_.sessionsByState(proj.get.id, SessionState.VERIFIED))
        yield assertTrue(
          reps.size == 1, reps.head.filesChecked == 0, reps.head.mismatches.isEmpty,
          ver.isEmpty)
      }
  ).provideLayer(PipelineTestKit.env.orDie)
