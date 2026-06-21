package diskrement.cli

import zio.*
import zio.test.*

object PromptSpec extends ZIOSpecDefault:
  def spec = suite("Prompt.confirm")(
    test("--yes bypasses the prompt and returns true") {
      for ok <- Prompt.confirm("Proceed?", assumeYes = true)
      yield assertTrue(ok)
    },
    test("'y' is yes") {
      for _  <- TestConsole.feedLines("y")
          ok <- Prompt.confirm("Proceed?", assumeYes = false)
      yield assertTrue(ok)
    },
    test("'yes' is yes") {
      for _  <- TestConsole.feedLines("yes")
          ok <- Prompt.confirm("Proceed?", assumeYes = false)
      yield assertTrue(ok)
    },
    test("empty line is no") {
      for _  <- TestConsole.feedLines("")
          ok <- Prompt.confirm("Proceed?", assumeYes = false)
      yield assertTrue(!ok)
    },
    test("EOF (no input) is no") {
      for ok <- Prompt.confirm("Proceed?", assumeYes = false)
      yield assertTrue(!ok)
    }
  )
