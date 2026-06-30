package diskrement.cli.it

import diskrement.shell.burn.{ManifestJson, PointersJson, ProjectJson}
import zio.json.*
import scala.jdk.StreamConverters.*
import java.nio.file.{Files, Path}

/** Reads a staged `disc-N/` directory and exposes facts for assertions. Uses the
  * production zio-json codecs so the on-disc format stays honest. */
object DiscOracle:
  /** Relative paths of all regular files under `disc-N/data`. */
  def dataFiles(discDir: Path): Set[String] =
    val data = discDir.resolve("data")
    if !Files.exists(data) then Set.empty
    else
      val stream = Files.walk(data)
      try stream.toScala(List)
        .filter(Files.isRegularFile(_))
        .map(p => data.relativize(p).toString)
        .toSet
      finally stream.close()

  def manifest(discDir: Path): ManifestJson =
    decode[ManifestJson](discDir.resolve(".diskrement/manifest.json"))

  def projectJson(discDir: Path): ProjectJson =
    decode[ProjectJson](discDir.resolve(".diskrement/project.json"))

  def pointers(discDir: Path): PointersJson =
    decode[PointersJson](discDir.resolve(".diskrement/pointers.json"))

  /** Files staged on this disc, per the manifest (added ++ modified). */
  def manifestFiles(m: ManifestJson): Set[String] =
    (m.added.map(_.relPath) ++ m.modified.map(_.relPath)).toSet

  /** True if the staged copy of `relPath` is byte-identical to the source. */
  def contentMatches(discDir: Path, source: Path, relPath: String): Boolean =
    val staged = discDir.resolve("data").resolve(relPath)
    val src    = source.resolve(relPath)
    Files.exists(staged) && Files.exists(src) && Files.mismatch(staged, src) == -1L

  /** Number of tables in the embedded catalog snapshot (proves it opens). */
  def catalogTableCount(discDir: Path): Int =
    val url  = s"jdbc:sqlite:${discDir.resolve(".diskrement/catalog.sqlite")}"
    val conn = java.sql.DriverManager.getConnection(url)
    try
      val rs = conn.createStatement()
        .executeQuery("select count(*) from sqlite_master where type='table'")
      rs.next()
      rs.getInt(1)
    finally conn.close()

  private def decode[A: JsonDecoder](path: Path): A =
    Files.readString(path).fromJson[A].fold(err => sys.error(s"$path: $err"), identity)
