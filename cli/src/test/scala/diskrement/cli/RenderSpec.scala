package diskrement.cli

import diskrement.core.*
import diskrement.shell.model.*
import diskrement.shell.pipeline.*
import zio.test.*
import java.nio.file.Paths
import java.time.Instant

object RenderSpec extends ZIOSpecDefault:

  private val project = Project(
    ProjectId(1), "photos", "/srv/photos", "/srv/work",
    DiscType.BD_R, 2.0, 1, Instant.parse("2026-06-21T00:00:00Z"))

  private def sf(path: String, size: Long): ScannedFile =
    ScannedFile(RelPath(path), ContentHash("h"), size, Instant.parse("2026-06-21T00:00:00Z"))

  def spec = suite("Render")(
    test("scan summary") {
      assertTrue(Render.scan(ScanResult(12, 3, 1, 540)) ==
        "Scanned: 12 added, 3 modified, 1 deleted, 540 unchanged.")
    },
    test("plan: single disc with remaining estimate") {
      val rep = PlanReport(
        disc = PlannedDisc(1, isNew = true, willFinalize = true, files = List(sf("a", 24_100_000_000L)),
          priorUsedBytes = 0L, usableBytes = 25_000_000_000L),
        discType = DiscType.BD_R, tombstones = Nil,
        remaining = RemainingEstimate(moreDiscs = 1, moreFiles = 1, moreBytes = 3_200_000_000L),
        oversized = Nil)
      val out = Render.plan(rep)
      assertTrue(
        out.contains("disc 1"), out.contains("BD_R"),
        out.contains("new disc"),
        out.contains("FINALIZE"), out.contains("more discs"))
    },
    test("plan: appending to an open disc, left open") {
      val rep = PlanReport(
        disc = PlannedDisc(2, isNew = false, willFinalize = false, files = List(sf("a", 3_200_000_000L)),
          priorUsedBytes = 10_000_000_000L, usableBytes = 25_000_000_000L),
        discType = DiscType.BD_R, tombstones = Nil,
        remaining = RemainingEstimate(moreDiscs = 0, moreFiles = 0, moreBytes = 0L),
        oversized = Nil)
      val out = Render.plan(rep)
      assertTrue(
        out.contains("appending to open disc 2"), out.contains("left OPEN"))
    },
    test("plan: empty plan says nothing to burn") {
      val rep = PlanReport(
        PlannedDisc(1, isNew = false, willFinalize = false, Nil, 0L, 25_000_000_000L),
        DiscType.BD_R, Nil, RemainingEstimate(0, 0, 0), Nil)
      assertTrue(Render.plan(rep) == "Nothing to burn: no changes to plan.")
    },
    test("plan: oversized files are listed") {
      val rep = PlanReport(
        PlannedDisc(1, isNew = true, willFinalize = false, List(sf("a", 10L)), 0L, 25_000_000_000L),
        DiscType.BD_R, Nil, RemainingEstimate(0, 0, 0), List(sf("huge.bin", 99_000_000_000L)))
      val out = Render.plan(rep)
      assertTrue(out.contains("Oversized (cannot be placed):"), out.contains("huge.bin"))
    },
    test("burn: finalized disc, with more discs remaining") {
      val rep = BurnReport(
        DiscBurn(1, "photos disc 1", finalized = true, Paths.get("/w/disc-1"), 24_100_000_000L),
        RemainingEstimate(moreDiscs = 2, moreFiles = 50, moreBytes = 40_000_000_000L))
      val out = Render.burn(rep)
      assertTrue(
        out.contains("Wrote disc 1"), out.contains("photos disc 1"),
        out.contains("FINALIZED"), out.contains("more discs"))
    },
    test("burn: open disc, set up to date") {
      val rep = BurnReport(
        DiscBurn(1, "photos disc 1", finalized = false, Paths.get("/w/disc-1"), 1_000L),
        RemainingEstimate(0, 0, 0))
      val out = Render.burn(rep)
      assertTrue(out.contains("Wrote disc 1"), out.contains("OPEN"), out.contains("up to date"))
    },
    test("status table") {
      val report = StatusReport(
        discs = List(DiscState(1, DiscStatus.FINALIZED, 25_000_000_000L, 24_800_000_000L),
                     DiscState(2, DiscStatus.OPEN, 25_000_000_000L, 3_200_000_000L)),
        openDiscOrdinal = Some(2), totalCurrentFiles = 556,
        lastBurnAt = Some(Instant.parse("2026-06-21T14:03:00Z")))
      val out = Render.status(project, report)
      assertTrue(
        out.contains("Project photos"),
        out.contains("BD_R"), out.contains("margin 2.0%"),
        out.contains("#1"), out.contains("FINALIZED"),
        out.contains("#2"), out.contains("OPEN"), out.contains("<- open"),
        out.contains("Current files: 556"), out.contains("Last burn:"))
    },
    test("verify: empty -> nothing to verify") {
      assertTrue(Render.verify(Nil) == "Nothing to verify: no burned discs.")
    },
    test("verify: a clean disc reports OK") {
      val out = Render.verify(List(VerifyReport(1, 10, Nil)))
      assertTrue(out.contains("disc 1"), out.contains("10 files OK"), out.contains("✓"))
    },
    test("verify: mismatches are listed") {
      val out = Render.verify(List(VerifyReport(2, 5, List(RelPath("bad/file.jpg")))))
      assertTrue(out.contains("disc 2"), out.contains("MISMATCH"), out.contains("bad/file.jpg"))
    },
    test("history list") {
      val entries = List(
        FileHistoryEntry(ContentHash("a1b2c3"), 3_400_000L, 1, 1, VersionStatus.SUPERSEDED),
        FileHistoryEntry(ContentHash("d4e5f6"), 3_500_000L, 2, 2, VersionStatus.CURRENT))
      val out = Render.history(RelPath("vacation/img.jpg"), entries)
      assertTrue(
        out.contains("vacation/img.jpg"),
        out.contains("disc 1 / session 1"), out.contains("SUPERSEDED"),
        out.contains("disc 2 / session 2"), out.contains("CURRENT"))
    },
    test("history: empty") {
      assertTrue(Render.history(RelPath("nope"), Nil) == "No history for nope.")
    },
    test("init confirmation line") {
      assertTrue(Render.init("photos", ProjectId(7)) == "Created project photos (id 7).")
    }
  )
