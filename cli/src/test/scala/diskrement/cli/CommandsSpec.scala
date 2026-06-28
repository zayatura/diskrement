package diskrement.cli

import diskrement.core.DiscType
import zio.cli.*
import zio.test.*
import java.nio.file.Paths

object CommandsSpec extends ZIOSpecDefault:

  /** Parse argv and pull out the (GlobalOpts, DiskrementCommand) value. zio-cli's
    * parse requires the root command name first, so we prepend "diskrement". */
  private def parse(args: String*) =
    Commands.root.parse("diskrement" :: args.toList, CliConfig.default).map {
      case CommandDirective.UserDefined(_, value) => value
      case other => throw new AssertionError(s"expected UserDefined, got $other")
    }

  def spec = suite("Commands")(
    test("init with defaults") {
      for v <- parse("init", "photos", "--source", "/srv/p", "--work-dir", "/srv/w")
      yield assertTrue(v == (
        GlobalOpts(None, None),
        DiskrementCommand.Init("photos", Paths.get("/srv/p"), Paths.get("/srv/w"), DiscType.BD_R, 2.0, 1)))
    },
    test("init with explicit disc-type, margin, copies") {
      for v <- parse("init", "photos", "--source", "/s", "--work-dir", "/w",
                     "--disc-type", "BD_R_DL", "--margin", "5.0", "--copies", "2")
      yield assertTrue(v._2 ==
        DiskrementCommand.Init("photos", Paths.get("/s"), Paths.get("/w"), DiscType.BD_R_DL, 5.0, 2))
    },
    test("scan flags") {
      for v <- parse("scan", "photos", "--verify", "--force")
      yield assertTrue(v._2 == DiskrementCommand.Scan("photos", verify = true, force = true))
    },
    test("scan defaults (flags off)") {
      for v <- parse("scan", "photos")
      yield assertTrue(v._2 == DiskrementCommand.Scan("photos", verify = false, force = false))
    },
    test("plan with optional disc-type override and finalize flag") {
      for a <- parse("plan", "photos")
          b <- parse("plan", "photos", "--disc-type", "DVD")
          c <- parse("plan", "photos", "--finalize")
      yield assertTrue(
        a._2 == DiskrementCommand.Plan("photos", None, willFinalize = false),
        b._2 == DiskrementCommand.Plan("photos", Some(DiscType.DVD), willFinalize = false),
        c._2 == DiskrementCommand.Plan("photos", None, willFinalize = true))
    },
    test("burn with --yes") {
      for v <- parse("burn", "photos", "--yes")
      yield assertTrue(v._2 == DiskrementCommand.Burn("photos", assumeYes = true))
    },
    test("reset / status") {
      for r <- parse("reset", "photos"); s <- parse("status", "photos")
      yield assertTrue(r._2 == DiskrementCommand.Reset("photos"), s._2 == DiskrementCommand.Status("photos"))
    },
    test("verify with optional --disc") {
      for a <- parse("verify", "photos"); b <- parse("verify", "photos", "--disc", "2")
      yield assertTrue(a._2 == DiskrementCommand.Verify("photos", None), b._2 == DiskrementCommand.Verify("photos", Some(2)))
    },
    test("history with relPath arg") {
      for v <- parse("history", "photos", "vacation/img.jpg")
      yield assertTrue(v._2 == DiskrementCommand.History("photos", "vacation/img.jpg"))
    },
    test("global --catalog before the subcommand is captured") {
      for v <- parse("--catalog", "/tmp/c.sqlite", "status", "photos")
      yield assertTrue(v._1 == GlobalOpts(Some(Paths.get("/tmp/c.sqlite")), None))
    },
    test("global --catalog after the subcommand is also captured") {
      for v <- parse("status", "--catalog", "/tmp/c.sqlite", "photos")
      yield assertTrue(v._1 == GlobalOpts(Some(Paths.get("/tmp/c.sqlite")), None))
    },
    test("an unknown disc-type is rejected") {
      parse("init", "photos", "--source", "/s", "--work-dir", "/w", "--disc-type", "ZIP")
        .exit.map(e => assertTrue(e.isFailure))
    }
  )
