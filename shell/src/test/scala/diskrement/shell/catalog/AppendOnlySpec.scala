package diskrement.shell.catalog

import diskrement.core._
import diskrement.shell.model._
import zio._
import zio.jdbc._
import zio.test._
import java.time.Instant

object AppendOnlySpec extends ZIOSpecDefault:
  private val init = ProjectInit("p", "/src", "/work", DiscType.BD_R, 2.0, 1,
    Instant.parse("2026-06-01T00:00:00Z"))
  private val ts = Instant.parse("2026-06-01T10:00:00Z")

  /** A scan = a set of paths each mapped to a content hash. */
  private val genScan: Gen[Any, Map[String, String]] =
    Gen.mapOf(Gen.elements("a", "b", "c", "d", "e"),
              Gen.elements("h1", "h2", "h3"))

  private val genScans: Gen[Any, List[Map[String, String]]] =
    Gen.listOfBounded(1, 6)(genScan)

  private def scannedOf(scan: Map[String, String]): List[ScannedFile] =
    scan.toList.map { case (p, h) => ScannedFile(RelPath(p), ContentHash(h), 1L, ts) }

  /** Turn a diff into a one-disc CatalogUpdate (test-local mini-orchestration). */
  private def updateOf(pid: ProjectId, ord: Int, cs: ChangeSet): CatalogUpdate =
    def fw(f: ScannedFile) =
      FileWrite(f.relPath, f.contentHash, f.sizeBytes, ts, "/data/" + f.relPath.value, 0)
    CatalogUpdate(pid, Nil, List(DiscWrite(
      ordinal = 1, isNew = ord == 1, discType = DiscType.BD_R, usableBytes = 1_000_000_000L,
      status = DiscStatus.OPEN, label = "d1",
      session = SessionWrite(ord, ts, "m" + ord,
        added = cs.added.map(fw), modified = cs.modified.map(fw), deleted = cs.deleted))))

  private def fvCount: URIO[ZConnectionPool, Int] =
    transaction(sql"select count(*) from file_version".query[Int].selectOne).map(_.getOrElse(0)).orDie

  def spec = suite("catalog append-only")(
    test("readView equals the latest scan; file_version only grows"):
      check(genScans) { scans =>
        for
          name <- Random.nextUUID.map("p-" + _.toString)
          pid  <- ZIO.serviceWithZIO[CatalogStore](_.createProject(init.copy(name = name)))
          finalView <- ZIO.foldLeft(scans.zipWithIndex)
                         ((CatalogView.empty, 0)) { case ((prevView, prevCount), (scan, idx)) =>
            val scanned = scannedOf(scan)
            val cs      = Diff.diff(prevView, scanned)
            for
              _        <- ZIO.serviceWithZIO[CatalogStore](_.applyUpdate(updateOf(pid, idx + 1, cs)))
              count    <- fvCount
              _        <- ZIO.when(count < prevCount)(
                            ZIO.die(new AssertionError("file_version count shrank")))
              nextView  = CatalogView.fromScanned(scanned)
            yield (nextView, count)
          }
          (expectedView, _) = finalView
          got <- ZIO.serviceWithZIO[CatalogStore](_.readView(pid))
        yield assertTrue(
          got.current.view.mapValues(_.contentHash).toMap ==
            expectedView.current.view.mapValues(_.contentHash).toMap)
      }
  ).provideLayer(TestDb.layer) @@ TestAspect.samples(40)
