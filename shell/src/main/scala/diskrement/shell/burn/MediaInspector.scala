package diskrement.shell.burn

import zio._
import zio.json._
import java.nio.file.{Files, Path}

enum MediaStatus:
  case NoMedia
  case Blank(freeBytes: Long)
  case Appendable(freeBytes: Long, sessions: Int, identity: Option[DiscIdentity])
  case Closed(identity: Option[DiscIdentity])
  /** Media libburn refuses to write sequentially, e.g. a POW-formatted BD-R. */
  case Unsuitable

/** Interrogates the medium in a device: state, free space, and — for written
  * diskrement discs — the identity embedded in the last session. */
trait MediaInspector:
  def inspect(device: BurnDevice): IO[BurnError, MediaStatus]
  /** The /.diskrement/project.json identity of the disc's merged view, if any. */
  def readIdentity(device: BurnDevice): IO[BurnError, Option[DiscIdentity]]

final case class XorrisoMediaInspectorLive(runner: CmdRunner) extends MediaInspector:

  def inspect(device: BurnDevice): IO[BurnError, MediaStatus] =
    runner.run(Xorriso.inspectArgs(device)).flatMap { res =>
      if res.exitCode != 0 then
        MediaParse.classifyFailure(res.output) match
          case FailureKind.NoMediaKind    => ZIO.succeed(MediaStatus.NoMedia)
          case FailureKind.PermissionKind => ZIO.fail(BurnError.DevicePermissionDenied(device.xorrisoAddress))
          case FailureKind.NotFoundKind   => ZIO.fail(BurnError.DeviceNotFound(device.xorrisoAddress))
          case FailureKind.OtherKind      => ZIO.fail(BurnError.BurnFailed(res.exitCode, res.tail()))
      else
        MediaParse.parse(res.output) match
          case Left(msg) => ZIO.fail(BurnError.BurnFailed(res.exitCode, List(msg)))
          case Right(RawMediaInfo(RawStatus.Blank, _, free)) =>
            ZIO.succeed(MediaStatus.Blank(free))
          case Right(RawMediaInfo(RawStatus.Appendable, sessions, free)) =>
            readIdentity(device).map(MediaStatus.Appendable(free, sessions, _))
          case Right(RawMediaInfo(RawStatus.Closed, _, _)) =>
            readIdentity(device).map(MediaStatus.Closed(_))
          case Right(RawMediaInfo(RawStatus.Unsuitable, _, _)) =>
            ZIO.succeed(MediaStatus.Unsuitable)
    }

  def readIdentity(device: BurnDevice): IO[BurnError, Option[DiscIdentity]] =
    ZIO.attemptBlockingIO(Files.createTempFile("diskrement-identity", ".json"))
      .mapError(BurnError.BurnIoError(_))
      .flatMap { tmp =>
        runner.run(Xorriso.extractArgs(device, "/.diskrement/project.json", tmp, None))
          .flatMap { res =>
            if res.exitCode != 0 then ZIO.succeed(None)
            else
              ZIO.attemptBlockingIO(Files.readString(tmp)).mapError(BurnError.BurnIoError(_))
                .map(_.fromJson[ProjectJson].flatMap(ProjectJson.toIdentity).toOption)
          }
          .ensuring(ZIO.attemptBlockingIO(Files.deleteIfExists(tmp)).ignore)
      }

object MediaInspector:
  val live: ZLayer[CmdRunner, Nothing, MediaInspector] =
    ZLayer(ZIO.service[CmdRunner].map(XorrisoMediaInspectorLive(_)))
