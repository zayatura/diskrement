package diskrement.shell.pipeline

import diskrement.core.*
import diskrement.shell.model.*
import zio.*
import zio.test.*
import java.nio.file.Files
import java.time.Instant

object PipelineMarkDeadSpec extends ZIOSpecDefault:
  private def initOf(name: String, src: java.nio.file.Path, work: java.nio.file.Path) =
    ProjectInit(name, src.toString, work.toString, DiscType.BD_R, 2.0, 1,
      Instant.parse("2026-06-01T00:00:00Z"))

  def spec = suite("Pipeline.markDead")(
    test("mark-dead re-plans the dead disc's live files onto a new disc; verify skips it"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.txt" -> "aaa", "b.txt" -> "bbb"))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          _     <- pipe.scan("p", verify = false, force = false)
          _     <- pipe.plan("p", None, finalize = false)
          _     <- pipe.burn("p")
          // second session on the same disc supersedes b.txt in place
          _     <- ZIO.attempt(Files.writeString(src.resolve("b.txt"), "bbbb"))
          _     <- pipe.scan("p", verify = false, force = true)
          _     <- pipe.plan("p", None, finalize = false)
          _     <- pipe.burn("p")
          rep   <- pipe.markDead("p", 1)
          st    <- pipe.status("p")
          // recovery cycle: everything current re-plans onto a fresh disc 2
          s3    <- pipe.scan("p", verify = false, force = true)
          plan3 <- pipe.plan("p", None, finalize = false)
          _     <- pipe.burn("p")
          vreps <- pipe.verify("p", None)
          hb    <- pipe.history("p", RelPath("b.txt"))
        yield assertTrue(
          rep.discOrdinal == 1,
          rep.lost.map(_.relPath.value) == List("b.txt"),
          rep.reburn.map(_.relPath.value).toSet == Set("a.txt", "b.txt"),
          st.discs.find(_.ordinal == 1).exists(_.status == DiscStatus.DEAD),
          st.openDiscOrdinal.isEmpty,
          s3.added == 2, s3.modified == 0, s3.deleted == 0, s3.unchanged == 0,
          plan3.disc.isNew,
          plan3.disc.ordinal == 2,
          plan3.disc.files.map(_.relPath.value) == List("a.txt", "b.txt"),
          vreps.map(_.discOrdinal) == List(2),
          hb.map(_.status) == List(VersionStatus.LOST, VersionStatus.LOST, VersionStatus.CURRENT))
      }
    ,
    test("unknown ordinal and double mark fail with dedicated errors"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.txt" -> "aaa"))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          _     <- pipe.scan("p", verify = false, force = false)
          _     <- pipe.plan("p", None, finalize = false)
          _     <- pipe.burn("p")
          bad   <- pipe.markDead("p", 9).exit
          _     <- pipe.markDead("p", 1)
          dup   <- pipe.markDead("p", 1).exit
        yield assertTrue(
          bad.causeOption.flatMap(_.failureOption).contains(PipelineError.UnknownDisc(9)),
          dup.causeOption.flatMap(_.failureOption).contains(PipelineError.DiscAlreadyDead(1)))
      }
    ,
    test("burn refuses a staged plan whose target disc was marked dead after planning"):
      ZIO.scoped {
        for
          paths <- ZIO.service[ProjectPaths]
          src   <- PipelineTestKit.makeSource(Map("a.txt" -> "aaa"))
          pipe  <- PipelineTestKit.mkPipeline(FakeSnapshot())
          _     <- pipe.init(initOf("p", src, paths.workDir))
          _     <- pipe.scan("p", verify = false, force = false)
          _     <- pipe.plan("p", None, finalize = false)
          _     <- pipe.burn("p")
          // stage a top-off of disc 1, then kill disc 1 before burning
          _     <- ZIO.attempt(Files.writeString(src.resolve("b.txt"), "bb"))
          _     <- pipe.scan("p", verify = false, force = true)
          _     <- pipe.plan("p", None, finalize = false)
          _     <- pipe.markDead("p", 1)
          res   <- pipe.burn("p").exit
        yield assertTrue(
          res.causeOption.flatMap(_.failureOption)
            .exists(_.isInstanceOf[PipelineError.StalePlan]))
      }
  ).provideLayer(PipelineTestKit.env.orDie)
