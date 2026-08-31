package diskrement.shell.burn

import diskrement.shell.ProgressSink
import diskrement.shell.fs.{Hasher, HashParallelism}
import zio._

/** The stage-to-directory burner: the disc "image" is a work-dir tree. Kept as
  * the test/dev backend; select with --burner simulated / DISKREMENT_BURNER. */
final case class SimulatedBurnerLive(
    hasher: Hasher, writer: SnapshotWriter, sink: ProgressSink, parallelism: Int
) extends Burner:
  def burn(image: DiscImage): IO[BurnError, BurnResult] =
    Stager.stage(image, hasher, writer, sink, parallelism)

object SimulatedBurner:
  val live: ZLayer[Hasher & SnapshotWriter & ProgressSink & HashParallelism, Nothing, Burner] =
    ZLayer {
      for
        hasher <- ZIO.service[Hasher]
        writer <- ZIO.service[SnapshotWriter]
        sink   <- ZIO.service[ProgressSink]
        par    <- ZIO.service[HashParallelism]
      yield SimulatedBurnerLive(hasher, writer, sink, par.value)
    }
