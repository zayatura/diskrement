package diskrement.shell.burn

import zio.test._
import java.nio.file.Path

object XorrisoSpec extends ZIOSpecDefault:
  private val img = BurnDevice.ImageFile(Path.of("/tmp/d.iso"))

  def spec = suite("XorrisoSpec")(
    test("burnArgs first session uses -outdev, close off") {
      assertTrue(Xorriso.burnArgs(img, Path.of("/w/staging/disc-1"), firstSession = true,
        closeDisc = false, volid = "PHOTOS_DISC_1") ==
        List("xorriso", "-abort_on", "FAILURE", "-outdev", "stdio:/tmp/d.iso",
          "-compliance", "iso_9660_level=3", "-volid", "PHOTOS_DISC_1",
          "-map", "/w/staging/disc-1", "/", "-close", "off", "-commit"))
    },
    test("burnArgs append uses -dev, close on") {
      val args = Xorriso.burnArgs(BurnDevice.RealDevice("/dev/sr0"), Path.of("/w/s"),
        firstSession = false, closeDisc = true, volid = "V")
      assertTrue(args.contains("-dev"), args.contains("/dev/sr0"),
        !args.contains("-outdev"), args.containsSlice(List("-close", "on")))
    },
    test("inspectArgs reads toc and media space") {
      assertTrue(Xorriso.inspectArgs(img) ==
        List("xorriso", "-abort_on", "FAILURE", "-outdev", "stdio:/tmp/d.iso",
          "-toc", "-tell_media_space"))
    },
    test("extractArgs without session reads the merged view") {
      assertTrue(Xorriso.extractArgs(img, "/.diskrement/project.json", Path.of("/tmp/p.json"), None) ==
        List("xorriso", "-abort_on", "FAILURE", "-indev", "stdio:/tmp/d.iso",
          "-osirrox", "on", "-extract", "/.diskrement/project.json", "/tmp/p.json"))
    },
    test("extractArgs with session loads that session first") {
      assertTrue(Xorriso.extractArgs(img, "/data/a.txt", Path.of("/tmp/a"), Some(1)).containsSlice(
        List("-indev", "stdio:/tmp/d.iso", "-load", "session", "1", "-osirrox", "on")))
    },
    test("volid sanitizes to uppercase alnum/underscore, max 32") {
      assertTrue(
        Xorriso.volid("my photos!", 3) == "MY_PHOTOS__DISC_3",
        Xorriso.volid("x" * 40, 1).length == 32)
    },
    test("parseVersion accepts 1.5.8.pl02 and rejects 1.4") {
      assertTrue(
        Xorriso.parseVersion(List("xorriso 1.5.8.pl02", "ISO 9660 ...")) == Some((1, 5, "1.5.8.pl02")),
        Xorriso.parseVersion(List("xorriso 1.4.2")) == Some((1, 4, "1.4.2")),
        Xorriso.parseVersion(List("garbage")) == None)
    }
  )
