package diskrement.cli.it

import java.nio.file.{Files, Path}

/** Probes whether `dir`'s filesystem supports reflink copies (btrfs/xfs/bcachefs).
  * diskrement's real SourceSnapshot requires CoW; without it the suite skips. */
object CoW:
  def capable(dir: Path): Boolean =
    Files.createDirectories(dir)
    val a = dir.resolve(".cow-probe-src")
    val b = dir.resolve(".cow-probe-dst")
    try
      Files.write(a, Array[Byte](1, 2, 3, 4))
      Files.deleteIfExists(b)
      val proc = new ProcessBuilder("cp", "--reflink=always", a.toString, b.toString)
        .redirectOutput(ProcessBuilder.Redirect.DISCARD)
        .redirectError(ProcessBuilder.Redirect.DISCARD)
        .start()
      proc.waitFor() == 0
    catch case _: Throwable => false
    finally
      Files.deleteIfExists(a)
      Files.deleteIfExists(b)
