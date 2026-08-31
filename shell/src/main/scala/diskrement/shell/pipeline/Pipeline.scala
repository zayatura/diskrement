package diskrement.shell.pipeline

import diskrement.core.*
import diskrement.shell.catalog.CatalogStore
import diskrement.shell.burn.{Burner, DiscVerifier}
import diskrement.shell.fs.{FileScanner, FingerprintCache, Hasher, SourceSnapshot}
import diskrement.shell.model.*
import zio.*
import java.nio.file.Path
import java.time.Instant

// Imports use package wildcards (codebase style) so the trait can grow new
// methods across tasks without churning a selective import list. The fs import
// is selective on purpose: only these four adapters are direct dependencies.

/** Where a project's catalog DB and work dir live (resolved by the CLI in 3b). */
final case class ProjectPaths(catalogPath: Path, workDir: Path)

final case class ScanResult(added: Int, modified: Int, deleted: Int, unchanged: Int, hashed: Int)

/** How much outstanding change did NOT fit on the disc planned this cycle. */
final case class RemainingEstimate(moreDiscs: Int, moreFiles: Int, moreBytes: Long)

/** The single disc this cycle will write, plus what spills to later cycles. */
final case class PlanReport(
    disc: PlannedDisc,
    discType: DiscType,
    tombstones: List[RelPath],
    remaining: RemainingEstimate,
    oversized: List[ScannedFile])

final case class DiscBurn(ordinal: Int, label: String, finalized: Boolean, stagingDir: Path, imageBytes: Long)
final case class BurnReport(disc: DiscBurn, remaining: RemainingEstimate)

final case class StatusReport(
    discs: List[DiscState], openDiscOrdinal: Option[Int],
    totalCurrentFiles: Int, lastBurnAt: Option[Instant])
final case class VerifyReport(
    discOrdinal: Int, filesChecked: Int, mismatches: List[RelPath], skipped: Boolean = false)

/** Staged orchestration of the whole workflow. Methods return data; the CLI (3b)
  * renders it. Grows across Plan 3a tasks. */
trait Pipeline:
  def init(p: ProjectInit): IO[PipelineError, ProjectId]
  def status(name: String): IO[PipelineError, StatusReport]
  def history(name: String, relPath: RelPath): IO[PipelineError, List[FileHistoryEntry]]
  def scan(name: String, verify: Boolean, force: Boolean): IO[PipelineError, ScanResult]
  def reset(name: String): IO[PipelineError, Unit]
  def plan(name: String, discTypeOverride: Option[DiscType], finalize: Boolean): IO[PipelineError, PlanReport]
  def burn(name: String): IO[PipelineError, BurnReport]
  def verify(name: String, discOrdinal: Option[Int]): IO[PipelineError, List[VerifyReport]]

object Pipeline:
  val live: ZLayer[
    ProjectPaths & CatalogStore & FileScanner & SourceSnapshot & Burner & DiscVerifier &
      StagedStore & Hasher & FingerprintCache,
    Nothing, Pipeline] =
    ZLayer {
      for
        paths    <- ZIO.service[ProjectPaths]
        catalog  <- ZIO.service[CatalogStore]
        scanner  <- ZIO.service[FileScanner]
        snapshot <- ZIO.service[SourceSnapshot]
        burner   <- ZIO.service[Burner]
        verifier <- ZIO.service[DiscVerifier]
        staged   <- ZIO.service[StagedStore]
        hasher   <- ZIO.service[Hasher]
        cache    <- ZIO.service[FingerprintCache]
      yield PipelineLive(catalog, scanner, snapshot, burner, verifier, staged, hasher, cache, paths)
    }
