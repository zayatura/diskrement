package diskrement.shell.fs

import diskrement.core.ContentHash
import zio._
import java.io.IOException
import java.nio.file.{Files, Path}

/** Content hashing port. SHA-256 in cycle 1; a port so the algorithm can change
  * later without touching callers (engine design §5.3). */
trait Hasher:
  def hash(path: Path): IO[IOException, ContentHash]

final case class HasherLive() extends Hasher:
  def hash(path: Path): IO[IOException, ContentHash] =
    ZIO.attemptBlockingIO {
      val md  = java.security.MessageDigest.getInstance("SHA-256")
      val buf = new Array[Byte](64 * 1024)
      val in  = Files.newInputStream(path)
      try
        var n = in.read(buf)
        while n >= 0 do
          md.update(buf, 0, n)
          n = in.read(buf)
      finally in.close()
      ContentHash(md.digest().map(b => f"${b & 0xff}%02x").mkString)
    }

object Hasher:
  val live: ZLayer[Any, Nothing, Hasher] = ZLayer.succeed(HasherLive())
