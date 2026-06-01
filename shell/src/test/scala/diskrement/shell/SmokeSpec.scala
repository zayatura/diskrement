package diskrement.shell

import zio.test._

object SmokeSpec extends ZIOSpecDefault:
  def spec = suite("shell toolchain")(
    test("zio-test compiles and runs"):
      assertTrue(1 + 1 == 2)
  )
