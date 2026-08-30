package diskrement.shell.burn

import diskrement.shell.fs.Hasher
import zio._

/** The stage-to-directory burner: the disc "image" is a work-dir tree. Kept as
  * the test/dev backend; select with --burner simulated / DISKREMENT_BURNER. */
final case class SimulatedBurnerLive(hasher: Hasher, writer: SnapshotWriter) extends Burner:
  def burn(image: DiscImage): IO[BurnError, BurnResult] =
    Stager.stage(image, hasher, writer)

object SimulatedBurner:
  val live: ZLayer[Hasher & SnapshotWriter, Nothing, Burner] =
    ZLayer {
      for
        hasher <- ZIO.service[Hasher]
        writer <- ZIO.service[SnapshotWriter]
      yield SimulatedBurnerLive(hasher, writer)
    }
