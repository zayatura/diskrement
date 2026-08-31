package diskrement.cli

import zio.test._

object ConsoleProgressSpec extends ZIOSpecDefault:
  def spec = suite("ConsoleProgress.format")(
    test("renders bytes, files, rate and ETA"):
      val line = ConsoleProgress.format(
        label = "hashing", doneBytes = 12_300_000_000L, totalBytes = 413_600_000_000L,
        doneFiles = 3021, totalFiles = 75618, rateBytesPerSec = 840_000_000d, etaSeconds = 432L)
      assertTrue(line == "hashing 12.3/413.6 GB · 3021/75618 files · 840 MB/s · ETA 7m12s")
    ,
    test("zero totals do not divide by zero"):
      val line = ConsoleProgress.format("hashing", 0L, 0L, 0, 0, 0d, 0L)
      assertTrue(line == "hashing 0.0/0.0 GB · 0/0 files · 0 MB/s · ETA 0m00s")
  )
