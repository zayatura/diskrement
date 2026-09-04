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
          _     <- pipe.plan("p", None, finalize = false)
          _     <- pipe.burn("p")
          reps  <- pipe.verify("p", None)
          proj  <- ZIO.serviceWithZIO[CatalogStore](_.findProject("p"))
          ver   <- ZIO.serviceWithZIO[CatalogStore](_.sessionsByState(proj.get.id, SessionState.VERIFIED))
        yield assertTrue(
          reps.size == 1, reps.head.mismatches.isEmpty, reps.head.filesChecked == 1,
          ver.map(_.discOrdinal) == List(1))
      },
    test("verify of an explicitly named live disc checks just that disc"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.txt" -> "aaa"))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          _     <- pipe.scan("p", verify = false, force = false)
          _     <- pipe.plan("p", None, finalize = false)
          _     <- pipe.burn("p")
          reps  <- pipe.verify("p", Some(1))
          gone  <- pipe.verify("p", Some(99)).exit
        yield assertTrue(
          reps.map(_.discOrdinal) == List(1),
          reps.head.mismatches.isEmpty,
          !reps.head.skipped,
          // a named ordinal that does not exist is an error, not a silent exit 0
          gone.causeOption.flatMap(_.failureOption).contains(PipelineError.UnknownDisc(99)))
      },
    test("verify detects a corrupted staged file"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.txt" -> "aaa"))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          _     <- pipe.scan("p", verify = false, force = false)
          _     <- pipe.plan("p", None, finalize = false)
          rep   <- pipe.burn("p")
          _     <- ZIO.attempt(Files.writeString(rep.disc.stagingDir.resolve("data/a.txt"), "CORRUPT"))
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
          _     <- pipe.plan("p", None, finalize = false)
          _     <- pipe.burn("p")
          _     <- pipe.reset("p")   // removes the staging dirs (catalog session remains BURNED)
          reps  <- pipe.verify("p", None)
          proj  <- ZIO.serviceWithZIO[CatalogStore](_.findProject("p"))
          ver   <- ZIO.serviceWithZIO[CatalogStore](_.sessionsByState(proj.get.id, SessionState.VERIFIED))
        yield assertTrue(
          reps.size == 1, reps.head.filesChecked == 0, reps.head.mismatches.isEmpty,
          ver.isEmpty)
      },
    test("verify is clean across discs after modify/delete spreads versions over multiple discs"):
      ZIO.scoped {
        for
          _     <- TestSystem.putEnv("DISKREMENT_DISC_BYTES", "1000")
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.bin" -> ("a" * 800)))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          // cycle 1: a -> disc 1 (open, ~200 free)
          _     <- pipe.scan("p", verify = false, force = false)
          _     <- pipe.plan("p", None, finalize = false)
          _     <- pipe.burn("p")
          // cycle 2: add b -> disc 2 (800 can't fit disc 1's 200 free)
          _     <- ZIO.attempt(Files.writeString(src.resolve("b.bin"), "b" * 800))
          _     <- pipe.scan("p", verify = false, force = true)
          _     <- pipe.plan("p", None, finalize = false)
          _     <- pipe.burn("p")
          // cycle 3: modify a + delete b -> disc 3 (a now superseded on disc 1, b deleted on disc 2)
          _     <- ZIO.attempt {
                     Files.writeString(src.resolve("a.bin"), "A" * 800)
                     Files.deleteIfExists(src.resolve("b.bin"))
                   }
          _     <- pipe.scan("p", verify = false, force = true)
          _     <- pipe.plan("p", None, finalize = false)
          _     <- pipe.burn("p")
          reps  <- pipe.verify("p", None)
          proj  <- ZIO.serviceWithZIO[CatalogStore](_.findProject("p"))
          ver   <- ZIO.serviceWithZIO[CatalogStore](_.sessionsByState(proj.get.id, SessionState.VERIFIED))
        yield assertTrue(
          reps.size == 3,
          reps.forall(_.mismatches.isEmpty),
          ver.map(_.discOrdinal).sorted == List(1, 2, 3))
      }
  ).provideLayer(PipelineTestKit.env.orDie)
