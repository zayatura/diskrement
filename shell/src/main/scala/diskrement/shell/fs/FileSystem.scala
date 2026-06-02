package diskrement.shell.fs

import zio._
import java.io.IOException
import java.nio.file.{Files, LinkOption, Path}
import java.nio.file.attribute.BasicFileAttributes
import java.time.Instant
import java.time.temporal.ChronoUnit
import scala.jdk.StreamConverters.*

/** Size and last-modified time of a regular file. mtime is truncated to
  * milliseconds so it matches the precision persisted in the catalog (this is
  * the single point where mtime precision is normalized). */
final case class FileStat(sizeBytes: Long, mtime: Instant)

/** Read-only filesystem port: enumerate regular files and stat them. */
trait FileSystem:
  def walk(root: Path): IO[IOException, List[Path]]
  def stat(path: Path): IO[IOException, FileStat]

final case class FileSystemLive() extends FileSystem:
  def walk(root: Path): IO[IOException, List[Path]] =
    ZIO.attemptBlockingIO {
      val stream = Files.walk(root)   // does NOT follow symlinks by default
      try
        stream.toScala(List)
          .filter(p => Files.isRegularFile(p, LinkOption.NOFOLLOW_LINKS))
          .sortBy(_.toString)
      finally stream.close()
    }

  def stat(path: Path): IO[IOException, FileStat] =
    ZIO.attemptBlockingIO {
      val attrs = Files.readAttributes(
        path, classOf[BasicFileAttributes], LinkOption.NOFOLLOW_LINKS)
      FileStat(attrs.size(), attrs.lastModifiedTime().toInstant.truncatedTo(ChronoUnit.MILLIS))
    }

object FileSystem:
  val live: ZLayer[Any, Nothing, FileSystem] = ZLayer.succeed(FileSystemLive())
