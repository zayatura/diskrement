package diskrement.cli

import diskrement.shell.burn.BurnDevice
import zio.test._

object BackendResolverSpec extends ZIOSpecDefault:
  private val none: String => Option[String] = _ => None
  private def env(pairs: (String, String)*): String => Option[String] = pairs.toMap.get

  def spec = suite("BackendResolverSpec")(
    test("defaults to real burner on /dev/sr0") {
      assertTrue(BackendResolver.resolve(GlobalOpts(None, None), none) ==
        Backend.Real(BurnDevice.RealDevice("/dev/sr0")))
    },
    test("--burner simulated wins over env") {
      assertTrue(BackendResolver.resolve(
        GlobalOpts(None, None, burner = Some(BurnerKind.Simulated)),
        env("DISKREMENT_BURNER" -> "real")) == Backend.Simulated)
    },
    test("DISKREMENT_BURNER=simulated works without flags") {
      assertTrue(BackendResolver.resolve(GlobalOpts(None, None),
        env("DISKREMENT_BURNER" -> "simulated")) == Backend.Simulated)
    },
    test("device precedence: flag > env > default") {
      assertTrue(
        BackendResolver.resolve(GlobalOpts(None, None, device = Some("/dev/sr1")),
          env("DISKREMENT_DEVICE" -> "/dev/sr9")) == Backend.Real(BurnDevice.RealDevice("/dev/sr1")),
        BackendResolver.resolve(GlobalOpts(None, None),
          env("DISKREMENT_DEVICE" -> "/dev/sr9")) == Backend.Real(BurnDevice.RealDevice("/dev/sr9")))
    },
    test("unknown env burner value falls back to real") {
      assertTrue(BackendResolver.resolve(GlobalOpts(None, None),
        env("DISKREMENT_BURNER" -> "banana")) == Backend.Real(BurnDevice.RealDevice("/dev/sr0")))
    }
  )
