package diskrement.shell.fs

import diskrement.core.{ContentHash, RelPath}
import diskrement.shell.model.FileFingerprint
import zio._
import zio.test._
import java.nio.file.{Files, Path}
import java.time.Instant

object FingerprintCacheSpec extends ZIOSpecDefault:
  private val mt = Instant.parse("2026-08-31T00:00:00Z")
  private def fp(sz: Long, h: String) = FileFingerprint(sz, mt, ContentHash(h))

  /** Fresh cache path in a per-test temp dir, deleted (recursively) on scope
    * release — nothing else cleans these up. */
  private val tmpCache: ZIO[Scope, Nothing, Path] =
    ZIO.acquireRelease(
      ZIO.attemptBlocking {
        val d = Files.createTempDirectory("fpcache-test")
        d.resolve("proj.scancache.sqlite")
      }.orDie
    )(p =>
      ZIO.attemptBlocking {
        val dir = p.getParent
        if Files.exists(dir) then
          val s = Files.walk(dir)
          try s.sorted(java.util.Comparator.reverseOrder()).forEach(x => Files.deleteIfExists(x))
          finally s.close()
      }.orDie)

  def spec = suite("FingerprintCache")(
    test("readAll on a missing file returns empty"):
      ZIO.scoped:
        for
          path <- tmpCache
          m    <- FingerprintCacheLive(path).readAll
        yield assertTrue(m.isEmpty)
    ,
    test("putAll then readAll round-trips; putAll upserts"):
      ZIO.scoped:
        for
          path <- tmpCache
          c     = FingerprintCacheLive(path)
          _    <- c.putAll(Map(RelPath("a") -> fp(1, "h1"), RelPath("b") -> fp(2, "h2")))
          _    <- c.putAll(Map(RelPath("a") -> fp(9, "h9")))
          m    <- c.readAll
        yield assertTrue(
          m == Map(RelPath("a") -> fp(9, "h9"), RelPath("b") -> fp(2, "h2")))
    ,
    test("the schema version stamp is 1"):
      ZIO.scoped:
        for
          path <- tmpCache
          c     = FingerprintCacheLive(path)
          _    <- c.putAll(Map(RelPath("a") -> fp(1, "h1")))
          ver  <- ZIO.attemptBlocking {
                    val conn = java.sql.DriverManager.getConnection(s"jdbc:sqlite:$path")
                    try
                      val st = conn.createStatement()
                      try
                        val rs = st.executeQuery("PRAGMA user_version")
                        rs.next(); rs.getInt(1)
                      finally st.close()
                    finally conn.close()
                  }.orDie
        yield assertTrue(ver == 1)
    ,
    test("a corrupt cache file is recreated empty and stays usable (readAll-first)"):
      ZIO.scoped:
        for
          path <- tmpCache
          _    <- ZIO.attemptBlocking(Files.write(path, "not a database at all".getBytes)).orDie
          c     = FingerprintCacheLive(path)
          m0   <- c.readAll
          _    <- c.putAll(Map(RelPath("a") -> fp(1, "h1")))
          m1   <- c.readAll
        yield assertTrue(m0.isEmpty, m1 == Map(RelPath("a") -> fp(1, "h1")))
    ,
    test("a corrupt cache file is recreated empty and stays usable (putAll-first)"):
      ZIO.scoped:
        for
          path <- tmpCache
          _    <- ZIO.attemptBlocking(Files.write(path, "not a database at all".getBytes)).orDie
          c     = FingerprintCacheLive(path)
          _    <- c.putAll(Map(RelPath("a") -> fp(1, "h1")))
          m    <- c.readAll
        yield assertTrue(m == Map(RelPath("a") -> fp(1, "h1")))
    ,
    test("a future schema version is recreated empty (disposable cache)"):
      ZIO.scoped:
        for
          path <- tmpCache
          c     = FingerprintCacheLive(path)
          _    <- c.putAll(Map(RelPath("a") -> fp(1, "h1")))
          _    <- ZIO.attemptBlocking {
                    val conn = java.sql.DriverManager.getConnection(s"jdbc:sqlite:$path")
                    try conn.createStatement().execute("PRAGMA user_version = 99")
                    finally conn.close()
                  }.orDie
          m    <- c.readAll
        yield assertTrue(m.isEmpty)
    ,
    test("a non-corrupt failure (locked file) does NOT wipe a healthy cache"):
      ZIO.scoped:
        for
          path <- tmpCache
          c     = FingerprintCacheLive(path)
          _    <- c.putAll(Map(RelPath("a") -> fp(1, "h1"), RelPath("b") -> fp(2, "h2")))
          lockConn <- ZIO.attemptBlocking {
                        val conn = java.sql.DriverManager.getConnection(s"jdbc:sqlite:$path")
                        conn.createStatement().executeUpdate("begin immediate")
                        conn
                      }.orDie
          result   <- c.putAll(Map(RelPath("c") -> fp(3, "h3"))).flip
          _        <- ZIO.attemptBlocking(lockConn.close()).orDie
          m        <- c.readAll
        yield assertTrue(
          result.isInstanceOf[CacheError.CacheIoError],
          m == Map(RelPath("a") -> fp(1, "h1"), RelPath("b") -> fp(2, "h2")))
  )
