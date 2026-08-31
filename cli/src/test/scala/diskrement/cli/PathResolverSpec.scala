package diskrement.cli

import zio.test.*
import java.nio.file.Paths

object PathResolverSpec extends ZIOSpecDefault:
  private val home = Paths.get("/home/u")
  private def env(m: Map[String, String]): String => Option[String] = m.get

  def spec = suite("PathResolver")(
    test("defaults to ~/.local/share/diskrement when nothing is set") {
      val r = PathResolver.resolve("photos", GlobalOpts(None, None), env(Map.empty), home)
      assertTrue(
        r.dataDir == Paths.get("/home/u/.local/share/diskrement"),
        r.catalogPath == Paths.get("/home/u/.local/share/diskrement/photos.sqlite"))
    },
    test("uses XDG_DATA_HOME when set") {
      val r = PathResolver.resolve("photos", GlobalOpts(None, None), env(Map("XDG_DATA_HOME" -> "/xdg")), home)
      assertTrue(r.catalogPath == Paths.get("/xdg/diskrement/photos.sqlite"))
    },
    test("STATE_DIRECTORY (systemd) wins over XDG, first entry of a colon list") {
      val r = PathResolver.resolve("photos", GlobalOpts(None, None),
        env(Map("STATE_DIRECTORY" -> "/var/lib/diskrement:/other", "XDG_DATA_HOME" -> "/xdg")), home)
      assertTrue(r.catalogPath == Paths.get("/var/lib/diskrement/photos.sqlite"))
    },
    test("--data-dir overrides env") {
      val r = PathResolver.resolve("photos", GlobalOpts(None, Some(Paths.get("/d"))),
        env(Map("STATE_DIRECTORY" -> "/var/lib/diskrement")), home)
      assertTrue(r.catalogPath == Paths.get("/d/photos.sqlite"), r.dataDir == Paths.get("/d"))
    },
    test("an empty STATE_DIRECTORY/XDG_DATA_HOME is treated as unset") {
      val r = PathResolver.resolve("photos", GlobalOpts(None, None),
        env(Map("STATE_DIRECTORY" -> "", "XDG_DATA_HOME" -> "")), home)
      assertTrue(r.catalogPath == Paths.get("/home/u/.local/share/diskrement/photos.sqlite"))
    },
    test("--catalog is used directly; its parent is the data dir") {
      val r = PathResolver.resolve("photos", GlobalOpts(Some(Paths.get("/tmp/c/my.sqlite")), None), env(Map.empty), home)
      assertTrue(
        r.catalogPath == Paths.get("/tmp/c/my.sqlite"),
        r.dataDir == Paths.get("/tmp/c"))
    },
    test("--catalog with a relative path is normalized to absolute") {
      val r = PathResolver.resolve("photos", GlobalOpts(Some(Paths.get("rel/my.sqlite")), None), env(Map.empty), home)
      assertTrue(r.catalogPath.isAbsolute, r.dataDir.isAbsolute, r.catalogPath.endsWith(Paths.get("rel/my.sqlite")))
    },
    test("scanCachePath is derived next to the catalog file, namespaced by project") {
      val r = PathResolver.resolve("p", GlobalOpts(Some(Paths.get("/data/cat/proj.sqlite")), None), env(Map.empty), home)
      assertTrue(r.scanCachePath == Paths.get("/data/cat/proj.p.scancache.sqlite"))
    },
    test("scanCachePath defaults next to the default catalog, namespaced by project") {
      val r = PathResolver.resolve("p", GlobalOpts(None, None), env(Map.empty), home)
      assertTrue(r.scanCachePath == Paths.get("/home/u/.local/share/diskrement/p.p.scancache.sqlite"))
    },
    test("scanCachePath differs across two projects sharing one catalog file") {
      val r1 = PathResolver.resolve("proj1", GlobalOpts(Some(Paths.get("/data/cat/shared.sqlite")), None), env(Map.empty), home)
      val r2 = PathResolver.resolve("proj2", GlobalOpts(Some(Paths.get("/data/cat/shared.sqlite")), None), env(Map.empty), home)
      assertTrue(r1.scanCachePath != r2.scanCachePath)
    }
  )
