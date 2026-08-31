package diskrement.cli

import zio._
import zio.test._

import java.util.concurrent.ConcurrentLinkedQueue
import scala.jdk.CollectionConverters._

object ConsoleProgressSpec extends ZIOSpecDefault:
  def spec = suite("ConsoleProgress")(
    suite("format")(
      test("renders bytes, files, rate and ETA"):
        val line = ConsoleProgress.format(
          label = "hashing", doneBytes = 12_300_000_000L, totalBytes = 413_600_000_000L,
          doneFiles = 3021, totalFiles = 75618, rateBytesPerSec = 840_000_000d, etaSeconds = 432L)
        assertTrue(line == "hashing 12.3/413.6 GB · 3021/75618 files · 840 MB/s · ETA 7m12s")
      ,
      test("zero totals do not divide by zero"):
        val line = ConsoleProgress.format("hashing", 0L, 0L, 0, 0, 0d, -1L)
        assertTrue(line == "hashing 0.0/0.0 GB · 0/0 files · 0 MB/s · ETA --")
    ),
    suite("Live")(
      test("throttles: nothing before the interval, one emit after"):
        val q = new ConcurrentLinkedQueue[String]()
        for
          sink <- ConsoleProgress.unsafeMake(tty = true, write = s => { q.add(s); () })
          _    <- sink.start("p", 100, 10)
          _    <- sink.advance(5, 1) // clock at 0 → below the 1s interval, no emit
          _    <- TestClock.adjust(2.seconds)
          _    <- sink.advance(5, 1) // now past the interval → exactly one emit
          afterFirstEmit = q.asScala.toVector
          _    <- sink.advance(1, 1) // immediately after → still throttled
          afterSecond = q.asScala.toVector
        yield assertTrue(
          afterFirstEmit.size == 1,
          afterFirstEmit.head.contains("2/10 files"),
          afterSecond.size == 1
        )
      ,
      test("done after an emit writes a final snapshot then a newline (TTY)"):
        val q = new ConcurrentLinkedQueue[String]()
        for
          sink <- ConsoleProgress.unsafeMake(tty = true, write = s => { q.add(s); () })
          _    <- sink.start("p", 100, 10)
          _    <- sink.advance(5, 1)
          _    <- TestClock.adjust(2.seconds)
          _    <- sink.advance(5, 1) // one emit, so `emitted` is true
          _    <- sink.advance(1, 1) // brings totals to 3/10 files, not yet emitted
          before = q.asScala.toVector
          _    <- sink.done
          after = q.asScala.toVector
        yield assertTrue(
          before.size == 1,
          after.size == before.size + 2,
          after(before.size).startsWith("\r"),
          after(before.size).contains("3/10 files"),
          after(before.size + 1) == "\n"
        )
      ,
      test("done with nothing emitted stays silent"):
        val q = new ConcurrentLinkedQueue[String]()
        for
          sink <- ConsoleProgress.unsafeMake(tty = true, write = s => { q.add(s); () })
          _    <- sink.start("p", 100, 10)
          _    <- sink.advance(5, 1) // clock never advanced → below interval, no emit
          _    <- sink.done
        yield assertTrue(q.isEmpty)
    )
  )
