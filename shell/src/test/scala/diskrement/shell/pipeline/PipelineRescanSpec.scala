package diskrement.shell.pipeline

import diskrement.core._
import diskrement.shell.model._
import zio._
import zio.test._
import java.time.Instant

object PipelineRescanSpec extends ZIOSpecDefault:
  private def initOf(name: String, src: java.nio.file.Path, work: java.nio.file.Path) =
    ProjectInit(name, src.toString, work.toString, DiscType.BD_R, 2.0, 1,
      Instant.parse("2026-08-31T00:00:00Z"))

  def spec = suite("Pipeline rescan with fingerprint cache")(
    test("a forced rescan after a first scan hashes nothing (cache primed)"):
      ZIO.scoped {
        for
          paths  <- ZIO.service[ProjectPaths]
          src    <- PipelineTestKit.makeSource(
                      Map("a.txt" -> "aaa", "b.txt" -> "bb", "sub/c.txt" -> "c"))
          pipe   <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _      <- pipe.init(initOf("p", src, paths.workDir))
          first  <- pipe.scan("p", verify = false, force = false)
          second <- pipe.scan("p", verify = false, force = true)
        yield assertTrue(first.hashed == Some(3), second.hashed == Some(0),
          second.added == 3, second.unchanged == 0)
      }
    ,
    test("a forced rescan re-hashes only a modified file"):
      ZIO.scoped {
        for
          paths  <- ZIO.service[ProjectPaths]
          src    <- PipelineTestKit.makeSource(
                      Map("a.txt" -> "aaa", "b.txt" -> "bb", "sub/c.txt" -> "c"))
          pipe   <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _      <- pipe.init(initOf("p", src, paths.workDir))
          _      <- pipe.scan("p", verify = false, force = false)
          _      <- ZIO.attemptBlocking {
                      java.nio.file.Files.writeString(src.resolve("b.txt"), "CHANGED-CONTENT")
                    }.orDie
          second <- pipe.scan("p", verify = false, force = true)
        yield assertTrue(second.hashed == Some(1))
      }
    ,
    test("verify forces a full re-hash even with a primed cache"):
      ZIO.scoped {
        for
          paths  <- ZIO.service[ProjectPaths]
          src    <- PipelineTestKit.makeSource(Map("a.txt" -> "aaa", "b.txt" -> "bb"))
          pipe   <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _      <- pipe.init(initOf("p", src, paths.workDir))
          _      <- pipe.scan("p", verify = false, force = false)
          again  <- pipe.scan("p", verify = true, force = true)
        yield assertTrue(again.hashed == Some(2))
      }
    ,
    test("a scan with a failing cache read still succeeds and hashes everything"):
      ZIO.scoped {
        for
          paths  <- ZIO.service[ProjectPaths]
          src    <- PipelineTestKit.makeSource(
                      Map("a.txt" -> "aaa", "b.txt" -> "bb", "sub/c.txt" -> "c"))
          pipe   <- PipelineTestKit.mkPipeline(FakeSnapshot(), failRead = true)
          _      <- pipe.init(initOf("p", src, paths.workDir))
          result <- pipe.scan("p", verify = false, force = false)
        yield assertTrue(result.hashed == Some(3), result.added == 3)
      }
    ,
    test("a scan with a failing cache write still succeeds, and nothing was cached"):
      ZIO.scoped {
        for
          paths  <- ZIO.service[ProjectPaths]
          src    <- PipelineTestKit.makeSource(
                      Map("a.txt" -> "aaa", "b.txt" -> "bb", "sub/c.txt" -> "c"))
          pipe   <- PipelineTestKit.mkPipeline(FakeSnapshot(), failWrite = true)
          _      <- pipe.init(initOf("p", src, paths.workDir))
          first  <- pipe.scan("p", verify = false, force = false)
          second <- pipe.scan("p", verify = false, force = true)
        yield assertTrue(first.hashed == Some(3), second.hashed == Some(3))
      }
  ).provideLayer(PipelineTestKit.env.orDie)
