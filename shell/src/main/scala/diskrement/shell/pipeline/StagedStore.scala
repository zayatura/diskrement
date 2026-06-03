package diskrement.shell.pipeline

import diskrement.core._
import zio._
import zio.json._
import java.nio.file.{Files, Path}
import java.time.Instant

/** Persists the staged scan + plan in the work dir between pipeline stages. */
trait StagedStore:
  def writeScan(workDir: Path, scanned: List[ScannedFile]): IO[PipelineError, Unit]
  def readScan(workDir: Path): IO[PipelineError, Option[List[ScannedFile]]]
  def writePlan(workDir: Path, plan: BurnPlan, discType: DiscType): IO[PipelineError, Unit]
  def readPlan(workDir: Path): IO[PipelineError, Option[(BurnPlan, DiscType)]]
  def clear(workDir: Path): IO[PipelineError, Unit]

private[pipeline] final case class ScannedFileJson(
    relPath: String, contentHash: String, sizeBytes: Long, mtime: String)
private[pipeline] object ScannedFileJson:
  given JsonCodec[ScannedFileJson] = DeriveJsonCodec.gen[ScannedFileJson]
  def from(f: ScannedFile): ScannedFileJson =
    ScannedFileJson(f.relPath.value, f.contentHash.value, f.sizeBytes, f.mtime.toString)
  def to(j: ScannedFileJson): ScannedFile =
    ScannedFile(RelPath(j.relPath), ContentHash(j.contentHash), j.sizeBytes, Instant.parse(j.mtime))

private[pipeline] final case class PlannedDiscJson(
    ordinal: Int, isNew: Boolean, willFinalize: Boolean,
    files: List[ScannedFileJson], priorUsedBytes: Long, usableBytes: Long)
private[pipeline] object PlannedDiscJson:
  given JsonCodec[PlannedDiscJson] = DeriveJsonCodec.gen[PlannedDiscJson]
  def from(d: PlannedDisc): PlannedDiscJson =
    PlannedDiscJson(d.ordinal, d.isNew, d.willFinalize,
      d.files.map(ScannedFileJson.from), d.priorUsedBytes, d.usableBytes)
  def to(j: PlannedDiscJson): PlannedDisc =
    PlannedDisc(j.ordinal, j.isNew, j.willFinalize,
      j.files.map(ScannedFileJson.to), j.priorUsedBytes, j.usableBytes)

private[pipeline] final case class BurnPlanJson(
    discs: List[PlannedDiscJson], tombstones: List[String],
    oversized: List[ScannedFileJson], discType: String)
private[pipeline] object BurnPlanJson:
  given JsonCodec[BurnPlanJson] = DeriveJsonCodec.gen[BurnPlanJson]
  def from(p: BurnPlan, discType: DiscType): BurnPlanJson =
    BurnPlanJson(p.discs.map(PlannedDiscJson.from), p.tombstones.map(_.value),
      p.oversized.map(ScannedFileJson.from), discType.toString)
  def to(j: BurnPlanJson): (BurnPlan, DiscType) =
    (BurnPlan(j.discs.map(PlannedDiscJson.to), j.tombstones.map(RelPath(_)),
       j.oversized.map(ScannedFileJson.to)), DiscType.valueOf(j.discType))

final case class StagedStoreLive() extends StagedStore:
  private def stagedDir(workDir: Path): Path = workDir.resolve("staged")
  private def scanFile(workDir: Path): Path  = stagedDir(workDir).resolve("scan.json")
  private def planFile(workDir: Path): Path  = stagedDir(workDir).resolve("plan.json")

  def writeScan(workDir: Path, scanned: List[ScannedFile]): IO[PipelineError, Unit] =
    write(scanFile(workDir), scanned.map(ScannedFileJson.from).toJson)

  // A corrupt staged JSON file is disk corruption, not a normal condition, so a
  // decode failure is surfaced as a defect (die) rather than a typed PipelineError.
  def readScan(workDir: Path): IO[PipelineError, Option[List[ScannedFile]]] =
    read(scanFile(workDir)).map(_.map(s =>
      s.fromJson[List[ScannedFileJson]].fold(
        e => throw new RuntimeException(s"corrupt scan.json: $e"),
        _.map(ScannedFileJson.to))))

  def writePlan(workDir: Path, plan: BurnPlan, discType: DiscType): IO[PipelineError, Unit] =
    write(planFile(workDir), BurnPlanJson.from(plan, discType).toJson)

  def readPlan(workDir: Path): IO[PipelineError, Option[(BurnPlan, DiscType)]] =
    read(planFile(workDir)).map(_.map(s =>
      s.fromJson[BurnPlanJson].fold(
        e => throw new RuntimeException(s"corrupt plan.json: $e"),
        BurnPlanJson.to)))

  def clear(workDir: Path): IO[PipelineError, Unit] =
    ZIO.attemptBlockingIO {
      Files.deleteIfExists(scanFile(workDir))
      Files.deleteIfExists(planFile(workDir))
      Files.deleteIfExists(stagedDir(workDir))
      ()
    }.mapError(PipelineError.Failed(_))

  private def write(file: Path, json: String): IO[PipelineError, Unit] =
    ZIO.attemptBlockingIO {
      Files.createDirectories(file.getParent)
      Files.writeString(file, json)
      ()
    }.mapError(PipelineError.Failed(_))

  private def read(file: Path): IO[PipelineError, Option[String]] =
    ZIO.attemptBlockingIO(
      if Files.exists(file) then Some(Files.readString(file)) else None
    ).mapError(PipelineError.Failed(_))

object StagedStore:
  val live: ZLayer[Any, Nothing, StagedStore] = ZLayer.succeed(StagedStoreLive())
