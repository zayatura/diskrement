package diskrement.shell.fs

import zio._

/** Bounded fiber count for hashing phases (scan + staging re-hash).
  * `DISKREMENT_HASH_PARALLELISM` overrides the default of 6; non-numeric or
  * non-positive values are ignored. Read via zio.System so tests can inject.
  * The value is read once at layer construction and fixed for the process;
  * values are trimmed and clamped to 64. */
final case class HashParallelism(value: Int)

object HashParallelism:
  val default: HashParallelism = HashParallelism(6)

  /** Values above this are almost certainly a misunderstanding of the knob
    * (each hashing fiber holds an open stream and a 64 KiB buffer). */
  private val MaxValue = 64

  val fromEnv: ZLayer[Any, Nothing, HashParallelism] =
    ZLayer {
      zio.System.env("DISKREMENT_HASH_PARALLELISM").orElseSucceed(None).flatMap {
        case None => ZIO.succeed(default)
        case Some(raw) =>
          raw.trim.toIntOption.filter(_ > 0) match
            case Some(n) => ZIO.succeed(HashParallelism(n min MaxValue))
            case None =>
              ZIO.logWarning(
                s"ignoring unusable DISKREMENT_HASH_PARALLELISM '$raw' (want a positive int); using ${default.value}")
                .as(default)
      }
    }
