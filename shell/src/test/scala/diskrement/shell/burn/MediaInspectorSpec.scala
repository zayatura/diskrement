package diskrement.shell.burn

import zio._
import zio.test._
import java.nio.file.{Files, Path}

object MediaInspectorSpec extends ZIOSpecDefault:
  private val dev = BurnDevice.RealDevice("/dev/sr0")

  private val blankToc = CmdResult(0, List(
    "Media status : is blank",
    "Media summary: 0 sessions, 0 data blocks, 0 data, 23g free",
    "Media space  : 11826176s"))

  private val appendableToc = CmdResult(0, List(
    "Media status : is written , is appendable",
    "Media summary: 2 sessions, 57 data blocks, 114k data, 23g free",
    "Media space  : 11826000s"))

  private def identityJson = """{"projectName":"photos","discOrdinal":1,"sessionOrdinal":2,"createdAt":"2026-08-30T10:00:00Z"}"""

  def spec = suite("MediaInspectorSpec")(
    test("blank media") {
      val insp = XorrisoMediaInspectorLive(FakeCmdRunner(_ => ZIO.succeed(blankToc)))
      insp.inspect(dev).map(s => assertTrue(s == MediaStatus.Blank(11826176L * 2048)))
    },
    test("appendable diskrement disc yields identity") {
      val runner = FakeCmdRunner { cmd =>
        if cmd.contains("-toc") then ZIO.succeed(appendableToc)
        else if cmd.contains("-extract") then
          ZIO.succeed { Files.writeString(Path.of(cmd.last), identityJson); CmdResult(0, Nil) }
        else ZIO.succeed(CmdResult(1, List("unexpected")))
      }
      XorrisoMediaInspectorLive(runner).inspect(dev).map {
        case MediaStatus.Appendable(free, sessions, Some(id)) =>
          assertTrue(free == 11826000L * 2048, sessions == 2,
            id.projectName == "photos", id.discOrdinal == 1, id.sessionOrdinal == 2)
        case other => assertNever(s"unexpected: $other")
      }
    },
    test("appendable non-diskrement disc yields None identity") {
      val runner = FakeCmdRunner { cmd =>
        if cmd.contains("-toc") then ZIO.succeed(appendableToc)
        else ZIO.succeed(CmdResult(5, List("FAILURE : cannot find path")))
      }
      XorrisoMediaInspectorLive(runner).inspect(dev).map {
        case MediaStatus.Appendable(_, _, None) => assertCompletes
        case other                              => assertNever(s"unexpected: $other")
      }
    },
    test("empty tray with exit 0 and 'is not present' maps to NoMedia") {
      val runner = FakeCmdRunner(_ => ZIO.succeed(CmdResult(0, List(
        "Media current: is not recognizable", "Media status : is not present", "Media space  : 0s"))))
      XorrisoMediaInspectorLive(runner).inspect(dev).map(s => assertTrue(s == MediaStatus.NoMedia))
    },
    test("empty tray maps to NoMedia") {
      val runner = FakeCmdRunner(_ => ZIO.succeed(CmdResult(5, List("libburn : SORRY : No disc loaded, not ready"))))
      XorrisoMediaInspectorLive(runner).inspect(dev).map(s => assertTrue(s == MediaStatus.NoMedia))
    },
    test("permission denied fails with DevicePermissionDenied") {
      val runner = FakeCmdRunner(_ => ZIO.succeed(CmdResult(5, List("Failed to open device '/dev/sr0' : Permission denied"))))
      XorrisoMediaInspectorLive(runner).inspect(dev).exit.map(e =>
        assertTrue(e.isFailure) && assertTrue(e.causeOption.flatMap(_.failureOption).exists(_.isInstanceOf[BurnError.DevicePermissionDenied]))
      )
    }
  )
