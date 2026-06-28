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
    test("a cycle plans one disc, finalizes on spill, and the next cycle burns the rest"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.txt" -> ("a" * 40), "b.txt" -> ("b" * 40)))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          // cycle 1: only the first file fits; the disc finalizes on spill
          _     <- pipe.scan("p", verify = false, force = false)
          plan1 <- pipe.plan("p", None, finalize = false)
          rep1  <- pipe.burn("p")
          proj  <- ZIO.serviceWithZIO[CatalogStore](_.findProject("p"))
          st1   <- ZIO.serviceWithZIO[CatalogStore](_.readDiscStates(proj.get.id))
          // cycle 2: the remaining file lands on a fresh disc
          _     <- pipe.scan("p", verify = false, force = true)
          plan2 <- pipe.plan("p", None, finalize = false)
          rep2  <- pipe.burn("p")
          st2   <- ZIO.serviceWithZIO[CatalogStore](_.readDiscStates(proj.get.id))
          view  <- ZIO.serviceWithZIO[CatalogStore](_.readView(proj.get.id))
        yield assertTrue(
          plan1.disc.files.size == 1,
          plan1.disc.willFinalize,
          plan1.remaining.moreDiscs == 1,
          rep1.disc.ordinal == 1, rep1.disc.finalized,
          st1.size == 1, st1.head.status == DiscStatus.FINALIZED,
          plan2.disc.ordinal == 2,
          plan2.remaining.moreDiscs == 0,
          rep2.disc.ordinal == 2, rep2.disc.finalized == false,
          st2.size == 2,
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
          plan  <- pipe.plan("p", None, finalize = false)
          err   <- pipe.burn("p").flip
        yield assertTrue(
          plan.oversized.map(_.relPath.value) == List("big.txt"),
          err.isInstanceOf[PipelineError.OversizedFiles])
      },
    test("a new file that cannot top off the open disc opens a fresh disc this cycle (no empty finalization)"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.txt" -> ("a" * 40)))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          // cycle 1: a.txt (40) lands on disc 1, which stays OPEN (~10 bytes free)
          _     <- pipe.scan("p", verify = false, force = false)
          _     <- pipe.plan("p", None, finalize = false)
          rep1  <- pipe.burn("p")
          proj  <- ZIO.serviceWithZIO[CatalogStore](_.findProject("p"))
          st1   <- ZIO.serviceWithZIO[CatalogStore](_.readDiscStates(proj.get.id))
          // cycle 2: add b.txt (40); it cannot fit disc 1's ~10 free bytes
          _     <- ZIO.attempt(java.nio.file.Files.writeString(src.resolve("b.txt"), "b" * 40))
          _     <- pipe.scan("p", verify = false, force = true)
          plan2 <- pipe.plan("p", None, finalize = false)
          rep2  <- pipe.burn("p")
          st2   <- ZIO.serviceWithZIO[CatalogStore](_.readDiscStates(proj.get.id))
          view  <- ZIO.serviceWithZIO[CatalogStore](_.readView(proj.get.id))
        yield assertTrue(
          rep1.disc.ordinal == 1,
          // cycle-1 precondition: one disc, left OPEN with free space
          st1.size == 1,
          st1.head.status == DiscStatus.OPEN,
          // the fix: cycle 2 burns a NEW disc 2 with b.txt, not an empty disc 1
          plan2.disc.ordinal == 2,
          plan2.disc.files.map(_.relPath.value) == List("b.txt"),
          plan2.disc.isNew,
          plan2.remaining.moreDiscs == 0,
          rep2.disc.ordinal == 2,
          // disc 1 was NOT empty-finalized: it stays OPEN
          st2.find(_.ordinal == 1).map(_.status).contains(DiscStatus.OPEN),
          // disc 2 also stays OPEN — it has free space
          st2.find(_.ordinal == 2).map(_.status).contains(DiscStatus.OPEN),
          st2.size == 2,
          // both files are committed (b.txt was not lost/deferred)
          view.current.keySet.map(_.value) == Set("a.txt", "b.txt"))
      }
  ).provideLayer(PipelineTestKit.env.orDie)
