package diskrement.cli.it

import java.nio.file.{Files, Path}

/** Generates files with deterministic, per-(path,version) unique random content,
  * so two same-size files never collide and runs are reproducible. */
object Tree:
  private def bytes(relPath: String, version: Int, size: Int): Array[Byte] =
    val rnd = new java.util.Random(s"$relPath@$version".hashCode.toLong)
    val buf = new Array[Byte](size)
    rnd.nextBytes(buf)
    buf

  /** Create (or overwrite) `relPath` under `root` with version-0 content. */
  def write(root: Path, relPath: String, size: Int): Unit =
    val p = root.resolve(relPath)
    Files.createDirectories(p.getParent)
    Files.write(p, bytes(relPath, 0, size))

  /** Overwrite `relPath` with version-1 content (distinct from `write`). */
  def modify(root: Path, relPath: String, size: Int): Unit =
    val p = root.resolve(relPath)
    Files.createDirectories(p.getParent)
    Files.write(p, bytes(relPath, 1, size))

  def delete(root: Path, relPath: String): Unit =
    Files.deleteIfExists(root.resolve(relPath))
