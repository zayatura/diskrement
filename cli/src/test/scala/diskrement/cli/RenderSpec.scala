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
    test("plan: discs with top-off/new and an oversized block") {
      val plan = BurnPlan(
        discs = List(
          PlannedDisc(1, isNew = false, willFinalize = true, files = List(sf("a", 24_100_000_000L)),
            priorUsedBytes = 0L, usableBytes = 25_000_000_000L),
          PlannedDisc(2, isNew = true, willFinalize = false, files = List(sf("b", 3_200_000_000L)),
            priorUsedBytes = 0L, usableBytes = 25_000_000_000L)),
        tombstones = Nil,
        oversized = List(sf("huge.bin", 99_000_000_000L)))
      val out = Render.plan(plan, DiscType.BD_R)
      assertTrue(
        out.contains("Plan: 2 discs (BD_R)"),
        out.contains("disc 1"), out.contains("(top-off)"),
        out.contains("disc 2"), out.contains("(new)"),
        out.contains("Oversized (cannot be placed):"),
        out.contains("huge.bin"))
    },
    test("plan: empty plan says nothing to burn") {
      assertTrue(Render.plan(BurnPlan(Nil, Nil, Nil), DiscType.BD_R) == "Nothing to burn: no changes to plan.")
    },
    test("burn report") {
      val report = BurnReport(
        discs = List(DiscBurn(1, Paths.get("/w/disc-1"), 24_100_000_000L), DiscBurn(2, Paths.get("/w/disc-2"), 3_200_000_000L)),
        oversized = Nil)
      val out = Render.burn(report)
      assertTrue(out.contains("Burned disc 1"), out.contains("Burned disc 2"), out.contains("✓"))
    },
    test("burn report: oversized files are listed as skipped") {
      val report = BurnReport(
        discs = List(DiscBurn(1, Paths.get("/w/disc-1"), 24_100_000_000L)),
        oversized = List(RelPath("huge.bin")))
      val out = Render.burn(report)
      assertTrue(out.contains("Burned disc 1"), out.contains("Oversized (skipped):"), out.contains("huge.bin"))
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
