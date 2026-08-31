package diskrement.shell.fs

import zio._

/** Bounded fiber count for hashing phases (scan + staging re-hash).
  * `DISKREMENT_HASH_PARALLELISM` overrides the default of 6; non-numeric or
  * non-positive values are ignored. Read via zio.System so tests can inject. */
final case class HashParallelism(value: Int)

object HashParallelism:
  val default = 6

  val fromEnv: ZLayer[Any, Nothing, HashParallelism] =
    ZLayer {
      zio.System.env("DISKREMENT_HASH_PARALLELISM").orDie
        .map(_.flatMap(_.toIntOption).filter(_ > 0)
          .fold(HashParallelism(default))(HashParallelism(_)))
    }
