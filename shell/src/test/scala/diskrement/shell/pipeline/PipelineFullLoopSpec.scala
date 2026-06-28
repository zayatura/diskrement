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
          _     <- pipe.plan("p", None, finalize = false)
          _     <- pipe.burn("p")
          // modify source: change a.txt, delete b.txt, add c.txt
          _     <- ZIO.attempt {
                     Files.writeString(src.resolve("a.txt"), "aaaa")
                     Files.deleteIfExists(src.resolve("b.txt"))
                     Files.writeString(src.resolve("c.txt"), "cc")
                   }
          // second burn (force a fresh scan since a staged scan exists)
          s2    <- pipe.scan("p", verify = false, force = true)
          _     <- pipe.plan("p", Some(DiscType.BD_R_DL), finalize = false)
          _     <- pipe.burn("p")
          reps  <- pipe.verify("p", None)
          hA    <- pipe.history("p", RelPath("a.txt"))
          hB    <- pipe.history("p", RelPath("b.txt"))
          proj  <- ZIO.serviceWithZIO[CatalogStore](_.findProject("p"))
          view  <- ZIO.serviceWithZIO[CatalogStore](_.readView(proj.get.id))
          ver   <- ZIO.serviceWithZIO[CatalogStore](_.sessionsByState(proj.get.id, SessionState.VERIFIED))
        yield assertTrue(
          s1 == ScanResult(2, 0, 0, 0),
          s2 == ScanResult(added = 1, modified = 1, deleted = 1, unchanged = 0),
          view.current.keySet.map(_.value) == Set("a.txt", "c.txt"),
          reps.forall(_.mismatches.isEmpty),
          hA.map(_.status) == List(VersionStatus.SUPERSEDED, VersionStatus.CURRENT),
          hB.map(_.status) == List(VersionStatus.DELETED),
          ver.map(s => (s.discOrdinal, s.ordinal)) == List((1, 2)))
      }
    ,
    test("plan --finalize seals a non-full disc; the next cycle starts a fresh disc"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.txt" -> "aaa"))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          // cycle 1: everything fits, but --finalize seals disc 1 anyway
          _     <- pipe.scan("p", verify = false, force = false)
          plan1 <- pipe.plan("p", None, finalize = true)
          rep1  <- pipe.burn("p")
          // cycle 2: a new file must start a fresh disc, since disc 1 is sealed
          _     <- ZIO.attempt(Files.writeString(src.resolve("b.txt"), "bb"))
          _     <- pipe.scan("p", verify = false, force = true)
          plan2 <- pipe.plan("p", None, finalize = false)
          rep2  <- pipe.burn("p")
          proj  <- ZIO.serviceWithZIO[CatalogStore](_.findProject("p"))
          st    <- ZIO.serviceWithZIO[CatalogStore](_.readDiscStates(proj.get.id))
        yield assertTrue(
          plan1.disc.willFinalize,
          rep1.disc.finalized,
          plan2.disc.ordinal == 2,
          rep2.disc.ordinal == 2,
          st.map(_.ordinal).sorted == List(1, 2),
          st.find(_.ordinal == 1).get.status == DiscStatus.FINALIZED)
      }
  ).provideLayer(PipelineTestKit.env.orDie)
