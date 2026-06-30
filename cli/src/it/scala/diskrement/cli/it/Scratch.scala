package diskrement.cli.it

import java.nio.file.{Files, Path, Paths}
import scala.jdk.StreamConverters.*

/** Per-test scratch layout. Everything lives under one unique dir so cleanup is
  * a single recursive delete. The catalog is a fixed file we always pass via
  * --catalog, so the system-wide default location is never touched. */
final case class Scratch(dir: Path):
  val source: Path  = dir.resolve("source")
  val work: Path    = dir.resolve("work")
  val catalog: Path = dir.resolve("catalog/cat.sqlite")
  val home: Path    = dir.resolve("home")
  val xdg: Path     = dir.resolve("home/.local/share")
  val state: Path   = dir.resolve("home/state")
  def discDir(ordinal: Int): Path = work.resolve(s"staging/disc-$ordinal")

object Scratch:
  /** Base under the working directory, which the suite assumes is on btrfs. */
  val base: Path = Paths.get("target", "it-scratch")

  def make(): Scratch =
    Files.createDirectories(base)
    val s = Scratch(Files.createTempDirectory(base, "it-"))
    List(s.source, s.work, s.catalog.getParent, s.home, s.xdg, s.state)
      .foreach(Files.createDirectories(_))
    s

  def delete(s: Scratch): Unit = deleteRecursive(s.dir)

  def deleteRecursive(dir: Path): Unit =
    if Files.exists(dir) then
      val stream = Files.walk(dir)
      try stream.sorted(java.util.Comparator.reverseOrder()).forEach(p => Files.deleteIfExists(p))
      finally stream.close()
