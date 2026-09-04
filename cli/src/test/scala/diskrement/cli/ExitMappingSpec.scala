package diskrement.cli

import diskrement.core.RelPath
import diskrement.shell.burn.BurnError
import diskrement.shell.pipeline.*
import zio.ExitCode
import zio.test.*
import java.nio.file.Paths

object ExitMappingSpec extends ZIOSpecDefault:
  def spec = suite("Render.handle / verifyExit")(
    test("ProjectNotFound -> exit 2 with its message") {
      val (msg, code) = Render.handle(PipelineError.ProjectNotFound("photos"))
      assertTrue(code == ExitCode(2), msg.contains("project not found: photos"))
    },
    test("NoStagedScan / NoStagedPlan / SnapshotMissing -> exit 2") {
      assertTrue(
        Render.handle(PipelineError.NoStagedScan("p"))._2 == ExitCode(2),
        Render.handle(PipelineError.NoStagedPlan("p"))._2 == ExitCode(2),
        Render.handle(PipelineError.SnapshotMissing("p"))._2 == ExitCode(2))
    },
    test("disc-state errors -> exit 2 with their messages") {
      val (staleMsg, staleCode) = Render.handle(PipelineError.StalePlan("disc 1 was marked dead"))
      assertTrue(
        Render.handle(PipelineError.UnknownDisc(9))._2 == ExitCode(2),
        Render.handle(PipelineError.DiscAlreadyDead(1))._2 == ExitCode(2),
        Render.handle(PipelineError.DiscIsDead(1))._2 == ExitCode(2),
        staleCode == ExitCode(2),
        staleMsg.contains("disc 1 was marked dead"),
        // the advertised recovery is `plan` alone; `reset` would wipe verify evidence
        staleMsg.contains("re-run plan"),
        !staleMsg.contains("reset"))
    },
    test("precondition failures -> exit 3") {
      assertTrue(
        Render.handle(PipelineError.NotSameFilesystem(Paths.get("/a"), Paths.get("/b")))._2 == ExitCode(3),
        Render.handle(PipelineError.NotCowCapable(Paths.get("/w"), "x"))._2 == ExitCode(3),
        Render.handle(PipelineError.OversizedFiles(List(RelPath("h"))))._2 == ExitCode(3))
    },
    test("Failed -> exit 5") {
      assertTrue(Render.handle(PipelineError.Failed(new RuntimeException("boom")))._2 == ExitCode(5))
    },
    test("Burn -> exit 6; an operator abort is a clean exit 0") {
      val (abortMsg, abortCode) = Render.handle(PipelineError.Burn(BurnError.Aborted))
      val (burnMsg, burnCode)   = Render.handle(PipelineError.Burn(BurnError.NoMedia("/dev/sr0")))
      assertTrue(
        abortCode == ExitCode(0), abortMsg == "Aborted.",
        burnCode == ExitCode(6), burnMsg.contains("no disc in /dev/sr0"))
    },
    test("verifyExit: clean reports -> 0, any mismatch -> 4") {
      val clean = List(VerifyReport(1, 10, Nil))
      val dirty = List(VerifyReport(1, 10, Nil), VerifyReport(2, 5, List(RelPath("bad"))))
      assertTrue(Render.verifyExit(clean) == ExitCode(0), Render.verifyExit(dirty) == ExitCode(4))
    }
  )
