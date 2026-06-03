package diskrement.shell.pipeline

import diskrement.core._
import zio._
import zio.test._
import java.nio.file.{Files, Path}
import java.time.Instant

object StagedStoreSpec extends ZIOSpecDefault:
  private val ts = Instant.parse("2026-06-01T00:00:00Z")
  private val scanned = List(
    ScannedFile(RelPath("a"), ContentHash("h1"), 10, ts),
    ScannedFile(RelPath("sub/b"), ContentHash("h2"), 20, ts))
  private val plan = BurnPlan(
    discs = List(PlannedDisc(1, isNew = true, willFinalize = false, scanned, 0L, 1_000L)),
    tombstones = List(RelPath("c")),
    oversized = Nil)

  private val makeWork: ZIO[Scope, Throwable, Path] =
    ZIO.acquireRelease(ZIO.attempt(Files.createTempDirectory("staged")))(
      d => ZIO.attempt {
        val s = Files.walk(d); try s.sorted(java.util.Comparator.reverseOrder())
          .forEach(p => Files.deleteIfExists(p)) finally s.close()
      }.orDie)

  def spec = suite("StagedStore")(
    test("round-trips a staged scan"):
      ZIO.scoped {
        for
          work <- makeWork
          ss    = StagedStoreLive()
          _    <- ss.writeScan(work, scanned)
          got  <- ss.readScan(work)
        yield assertTrue(got == Some(scanned))
      },
    test("round-trips a staged plan with its disc type"):
      ZIO.scoped {
        for
          work <- makeWork
          ss    = StagedStoreLive()
          _    <- ss.writePlan(work, plan, DiscType.BD_R_DL)
          got  <- ss.readPlan(work)
        yield assertTrue(got == Some((plan, DiscType.BD_R_DL)))
      },
    test("read returns None when nothing is staged; clear removes staged files"):
      ZIO.scoped {
        for
          work  <- makeWork
          ss     = StagedStoreLive()
          empty <- ss.readScan(work)
          _     <- ss.writeScan(work, scanned)
          _     <- ss.clear(work)
          after <- ss.readScan(work)
        yield assertTrue(empty.isEmpty, after.isEmpty)
      }
  )
