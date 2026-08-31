package diskrement.shell.fs

import zio._
import zio.test._

object HashParallelismSpec extends ZIOSpecDefault:
  // ZIOSpecDefault routes zio.System.env to TestSystem, so building the layer
  // inside the test effect picks up putEnv (same pattern as the
  // DISKREMENT_DISC_BYTES tests in PipelineCapacityOverrideSpec).
  private val resolved: UIO[Int] =
    ZIO.service[HashParallelism].map(_.value).provide(HashParallelism.fromEnv)

  def spec = suite("HashParallelism")(
    test("defaults to 6 when the env var is unset"):
      resolved.map(v => assertTrue(v == 6))
    ,
    test("reads a positive DISKREMENT_HASH_PARALLELISM"):
      for
        _ <- TestSystem.putEnv("DISKREMENT_HASH_PARALLELISM", "3")
        v <- resolved
      yield assertTrue(v == 3)
    ,
    test("ignores non-numeric and non-positive values"):
      for
        _ <- TestSystem.putEnv("DISKREMENT_HASH_PARALLELISM", "banana")
        a <- resolved
        _ <- TestSystem.putEnv("DISKREMENT_HASH_PARALLELISM", "0")
        b <- resolved
        _ <- TestSystem.putEnv("DISKREMENT_HASH_PARALLELISM", "-1")
        c <- resolved
        _ <- TestSystem.putEnv("DISKREMENT_HASH_PARALLELISM", "")
        d <- resolved
      yield assertTrue(a == 6, b == 6, c == 6, d == 6)
    ,
    test("trims whitespace and clamps huge values"):
      for
        _ <- TestSystem.putEnv("DISKREMENT_HASH_PARALLELISM", " 3")
        a <- resolved
        _ <- TestSystem.putEnv("DISKREMENT_HASH_PARALLELISM", "100000")
        b <- resolved
      yield assertTrue(a == 3, b == 64)
  )
