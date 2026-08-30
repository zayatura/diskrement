package diskrement.shell.burn

import java.nio.file.Path

/** Pure xorriso command construction and version parsing. Every list starts with
  * the binary name; CmdRunner executes it. Shapes validated against xorriso 1.5.8
  * (see the plan's "Validated xorriso facts"). */
object Xorriso:

  def burnArgs(device: BurnDevice, stagedDir: Path, firstSession: Boolean,
               closeDisc: Boolean, volid: String): List[String] =
    List("xorriso", "-abort_on", "FAILURE",
      if firstSession then "-outdev" else "-dev", device.xorrisoAddress,
      "-compliance", "iso_9660_level=3",
      "-volid", volid,
      "-map", stagedDir.toString, "/",
      "-close", if closeDisc then "on" else "off",
      "-commit")

  def inspectArgs(device: BurnDevice): List[String] =
    List("xorriso", "-abort_on", "FAILURE", "-outdev", device.xorrisoAddress,
      "-toc", "-tell_media_space")

  def extractArgs(device: BurnDevice, discPath: String, dest: Path,
                  session: Option[Int]): List[String] =
    List("xorriso", "-abort_on", "FAILURE", "-indev", device.xorrisoAddress) ++
      session.toList.flatMap(n => List("-load", "session", n.toString)) ++
      List("-osirrox", "on", "-extract", discPath, dest.toString)

  def ejectArgs(device: BurnDevice): List[String] =
    List("xorriso", "-outdev", device.xorrisoAddress, "-eject", "all")

  val versionArgs: List[String] = List("xorriso", "--version")

  /** ISO volume id: uppercase, [A-Z0-9_], at most 32 chars. */
  def volid(projectName: String, discOrdinal: Int): String =
    s"${projectName}_DISC_$discOrdinal".toUpperCase
      .map(c => if c.isLetterOrDigit then c else '_')
      .take(32)

  private val VersionRe = """xorriso\s+((\d+)\.(\d+)\S*).*""".r

  /** (major, minor, raw version string) from `xorriso --version` output. */
  def parseVersion(lines: List[String]): Option[(Int, Int, String)] =
    lines.collectFirst { case VersionRe(raw, maj, min) => (maj.toInt, min.toInt, raw) }
