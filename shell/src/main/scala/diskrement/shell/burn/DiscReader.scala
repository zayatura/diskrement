package diskrement.shell.burn

import zio._
import java.nio.file.{Files, Path}
import java.util.Comparator

/** Read access to a burned disc's content for verification. Real devices are
  * mounted via udisks (polkit lets a desktop session mount removable media
  * without root); image files are extracted via xorriso's osirrox. */
trait DiscReader:
  /** Expose the disc's merged /data tree as a local directory for `use`. */
  def withDataTree[A](device: BurnDevice)(use: Path => IO[BurnError, A]): IO[BurnError, A]
  /** Extract one file as recorded by a specific session (Tier-2 verify). */
  def extractFromSession(device: BurnDevice, sessionOrdinal: Int,
                         discPath: String, dest: Path): IO[BurnError, Unit]

final case class DiscReaderLive(runner: CmdRunner) extends DiscReader:

  def withDataTree[A](device: BurnDevice)(use: Path => IO[BurnError, A]): IO[BurnError, A] =
    device match
      case BurnDevice.RealDevice(path) =>
        ZIO.acquireReleaseWith(mount(path))(_ => unmount(path)) { mountPoint =>
          use(mountPoint.resolve("data"))
        }
      case BurnDevice.ImageFile(_) =>
        ZIO.acquireReleaseWith(
          ZIO.attemptBlockingIO(Files.createTempDirectory("diskrement-verify"))
            .mapError(BurnError.BurnIoError(_)))(dir =>
          ZIO.attemptBlockingIO {
            val s = Files.walk(dir)
            try s.sorted(Comparator.reverseOrder()).forEach(p => Files.deleteIfExists(p))
            finally s.close()
          }.ignore) { dir =>
          val dataDir = dir.resolve("data")
          runner.run(Xorriso.extractArgs(device, "/data", dataDir, None)).flatMap { r =>
            if r.exitCode != 0 then ZIO.fail(BurnError.MountFailed(
              s"osirrox extraction failed: ${r.tail().mkString(" | ")}"))
            else use(dataDir)
          }
        }

  def extractFromSession(device: BurnDevice, sessionOrdinal: Int,
                         discPath: String, dest: Path): IO[BurnError, Unit] =
    runner.run(Xorriso.extractArgs(device, discPath, dest, Some(sessionOrdinal))).flatMap { r =>
      ZIO.unless(r.exitCode == 0)(ZIO.fail(BurnError.MountFailed(
        s"session $sessionOrdinal extraction of $discPath failed: ${r.tail().mkString(" | ")}"))).unit
    }

  private def mount(devicePath: String): IO[BurnError, Path] =
    runner.run(List("udisksctl", "mount", "-b", devicePath)).flatMap { r =>
      DiscReaderLive.parseMountPoint(r.output) match
        case Some(p) => ZIO.succeed(p)
        case None => ZIO.fail(BurnError.MountFailed(
          s"udisksctl gave no mount point: ${r.tail().mkString(" | ")}"))
    }.mapError {
      case BurnError.ToolMissing(d) => BurnError.MountFailed(s"udisksctl not available: $d")
      case other                    => other
    }

  private def unmount(devicePath: String): UIO[Unit] =
    runner.run(List("udisksctl", "unmount", "-b", devicePath)).ignore

object DiscReaderLive:
  private val MountedRe = """.*Mounted \S+ at (\S+?)\.?$""".r
  private val AlreadyRe = """.*already mounted at `([^']+)'.*""".r

  def parseMountPoint(lines: List[String]): Option[Path] =
    lines.collectFirst {
      case MountedRe(p) => Path.of(p)
      case AlreadyRe(p) => Path.of(p)
    }

object DiscReader:
  val live: ZLayer[CmdRunner, Nothing, DiscReader] =
    ZLayer(ZIO.service[CmdRunner].map(DiscReaderLive(_)))
