package diskrement.shell.burn

import zio._
import zio.json._
import java.nio.file.{Files, Path, StandardCopyOption}

/** Writes the /.diskrement metadata for one disc. */
trait SnapshotWriter:
  def write(diskrementDir: Path, meta: DiscMetadata): IO[BurnError, Unit]

final case class SnapshotWriterLive() extends SnapshotWriter:
  def write(diskrementDir: Path, meta: DiscMetadata): IO[BurnError, Unit] =
    ZIO.attemptBlockingIO {
      Files.createDirectories(diskrementDir)
      Files.writeString(diskrementDir.resolve("project.json"),
        ProjectJson.from(meta.identity).toJsonPretty)
      Files.writeString(diskrementDir.resolve("manifest.json"),
        ManifestJson.from(meta.manifest).toJsonPretty)
      Files.writeString(diskrementDir.resolve("pointers.json"),
        PointersJson.from(meta.pointers).toJsonPretty)
      Files.copy(meta.catalogSnapshot, diskrementDir.resolve("catalog.sqlite"),
        StandardCopyOption.REPLACE_EXISTING)
      ()
    }.mapError(BurnError.BurnIoError(_))

object SnapshotWriter:
  val live: ZLayer[Any, Nothing, SnapshotWriter] = ZLayer.succeed(SnapshotWriterLive())
