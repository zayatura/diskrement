package diskrement.shell.fs

import diskrement.core.{ContentHash, RelPath}
import diskrement.shell.model.FileFingerprint
import zio._
import java.nio.file.{Files, Path}
import java.sql.{Connection, DriverManager}
import java.time.Instant

/** Filesystem location of a project's sidecar scan-fingerprint cache
  * (`<catalog-stem>.scancache.sqlite`, resolved by the CLI). */
final case class CachePath(value: Path)

/** Expected cache failures. Callers treat them as warnings, never fatal. */
sealed abstract class CacheError(msg: String, cause: Throwable = null)
    extends Exception(msg, cause)
object CacheError:
  final case class CacheIoError(cause: Throwable)
      extends CacheError("fingerprint cache I/O error", cause)

/** Sidecar cache of size+mtime→hash observations for *scanned* files, so a
  * rescan hashes only new/changed content. Deliberately outside the append-only
  * catalog: it is disposable — deleting it costs one re-hash, nothing else. A
  * corrupt or version-incompatible file is deleted and recreated empty. */
trait FingerprintCache:
  def readAll: IO[CacheError, Map[RelPath, FileFingerprint]]
  def putAll(entries: Map[RelPath, FileFingerprint]): IO[CacheError, Unit]

final case class FingerprintCacheLive(path: Path) extends FingerprintCache:
  import FingerprintCacheLive.SchemaVersion

  def readAll: IO[CacheError, Map[RelPath, FileFingerprint]] =
    guarded { conn =>
      val st = conn.createStatement()
      try
        val rs = st.executeQuery(
          "select rel_path, size_bytes, mtime, content_hash from fingerprint")
        val b = Map.newBuilder[RelPath, FileFingerprint]
        while rs.next() do
          b += RelPath(rs.getString(1)) -> FileFingerprint(
            rs.getLong(2), Instant.parse(rs.getString(3)), ContentHash(rs.getString(4)))
        b.result()
      finally st.close()
    }

  def putAll(entries: Map[RelPath, FileFingerprint]): IO[CacheError, Unit] =
    guarded { conn =>
      conn.setAutoCommit(false)
      val ps = conn.prepareStatement(
        """insert into fingerprint (rel_path, size_bytes, mtime, content_hash, updated_at)
           values (?, ?, ?, ?, ?)
           on conflict(rel_path) do update set
             size_bytes = excluded.size_bytes, mtime = excluded.mtime,
             content_hash = excluded.content_hash, updated_at = excluded.updated_at""")
      try
        val now = Instant.now().toString
        entries.foreach { case (rel, fp) =>
          ps.setString(1, rel.value)
          ps.setLong(2, fp.sizeBytes)
          ps.setString(3, fp.mtime.toString)
          ps.setString(4, fp.contentHash.value)
          ps.setString(5, now)
          ps.addBatch()
        }
        ps.executeBatch()
        conn.commit()
      finally ps.close()
    }.unit

  /** Run `use` on a fresh connection; on ANY failure assume the cache file is
    * unusable, warn, wipe it, and retry once against a fresh empty cache. */
  private def guarded[A](use: Connection => A): IO[CacheError, A] =
    attempt(use).catchAll { e =>
      ZIO.logWarning(
        s"fingerprint cache at $path unreadable (${e.getMessage}); recreating empty") *>
        wipe *> attempt(use).mapError(CacheError.CacheIoError(_))
    }

  private def attempt[A](use: Connection => A): Task[A] =
    ZIO.attemptBlocking {
      val conn = openConn()
      try use(conn)
      finally conn.close()
    }

  /** Open + create-on-demand. A version-0 (fresh) file gets the schema; any
    * other version than SchemaVersion throws (caught by `guarded` → recreate). */
  private def openConn(): Connection =
    Option(path.getParent).foreach(Files.createDirectories(_))
    val conn = DriverManager.getConnection(s"jdbc:sqlite:$path")
    try
      val st = conn.createStatement()
      try
        st.execute("PRAGMA busy_timeout=5000")
        val rs  = st.executeQuery("PRAGMA user_version")
        val ver = { rs.next(); rs.getInt(1) }
        if ver == 0 then
          st.execute(
            """create table if not exists fingerprint (
                 rel_path     text primary key,
                 size_bytes   integer not null,
                 mtime        text not null,
                 content_hash text not null,
                 updated_at   text not null)""")
          st.execute(s"PRAGMA user_version = $SchemaVersion")
        else if ver != SchemaVersion then
          throw new IllegalStateException(s"cache schema v$ver, expected v$SchemaVersion")
      finally st.close()
      conn
    catch
      case e: Throwable =>
        conn.close()
        throw e

  private val wipe: UIO[Unit] =
    ZIO.attemptBlocking {
      Files.deleteIfExists(path)
      Files.deleteIfExists(Path.of(path.toString + "-wal"))
      Files.deleteIfExists(Path.of(path.toString + "-shm"))
    }.ignoreLogged

object FingerprintCacheLive:
  private val SchemaVersion = 1

object FingerprintCache:
  val live: ZLayer[CachePath, Nothing, FingerprintCache] =
    ZLayer.fromFunction((p: CachePath) => FingerprintCacheLive(p.value))
