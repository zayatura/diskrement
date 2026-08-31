package diskrement.cli

import zio.*
import zio.test.*

object MainLoggerSpec extends ZIOSpecDefault:

  def spec = suite("Main.logLine")(
    test("a WARN with an empty cause renders exactly the level and message, no detail") {
      assertTrue(
        Main.logLine(LogLevel.Warning, "fingerprint cache is corrupt", Cause.empty) ==
          Some("[warn] fingerprint cache is corrupt"))
    },
    test("an ERROR with a real cause keeps the cause payload in the rendered line") {
      val out = Main.logLine(LogLevel.Error, "", Cause.die(new RuntimeException("boom")))
      assertTrue(
        out.isDefined,
        out.get.startsWith("[error] "),
        out.get.contains("boom"))
    },
    test("a DEBUG entry is filtered out") {
      assertTrue(Main.logLine(LogLevel.Debug, "noise", Cause.empty) == None)
    }
  )
