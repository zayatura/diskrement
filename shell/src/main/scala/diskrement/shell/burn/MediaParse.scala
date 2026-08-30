package diskrement.shell.burn

enum RawStatus { case Blank, Appendable, Closed, Unsuitable, NotPresent }
final case class RawMediaInfo(status: RawStatus, sessions: Int, freeBytes: Long)

enum FailureKind { case NoMediaKind, PermissionKind, NotFoundKind, OtherKind }

/** Pure parsers for `xorriso -toc -tell_media_space` output and for classifying
  * xorriso failure output. Line shapes validated on xorriso 1.5.8; the "closed"
  * and empty-tray shapes are tolerant assumptions confirmed by the smoke test. */
object MediaParse:

  private val SessionsRe = """.*Media summary:\s*(\d+)\s+session.*""".r
  private val SpaceRe    = """.*Media space\s*:\s*(\d+)s.*""".r

  def parse(lines: List[String]): Either[String, RawMediaInfo] =
    val status = lines.collectFirst {
      case l if l.contains("Media status") && l.contains("is not present") => RawStatus.NotPresent
      case l if l.contains("Media status") && l.contains("is blank")      => RawStatus.Blank
      case l if l.contains("Media status") && l.contains("is unsuitable") => RawStatus.Unsuitable
      case l if l.contains("Media status") && l.contains("is appendable") => RawStatus.Appendable
      case l if l.contains("Media status") && l.contains("is written")    => RawStatus.Closed
    }
    val sessions = lines.collectFirst { case SessionsRe(n) => n.toInt }.getOrElse(0)
    val free     = lines.collectFirst { case SpaceRe(blocks) => blocks.toLong * 2048 }.getOrElse(0L)
    status.toRight(s"no Media status line in xorriso output: ${lines.take(5).mkString(" | ")}")
      .map(RawMediaInfo(_, sessions, free))

  /** Why did an inspection/burn invocation fail? Substrings chosen from observed
    * and documented libburn messages; unknown text falls through to OtherKind. */
  def classifyFailure(lines: List[String]): FailureKind =
    val text = lines.mkString("\n").toLowerCase
    if text.contains("permission denied") then FailureKind.PermissionKind
    else if text.contains("no such file") || text.contains("cannot find device") then FailureKind.NotFoundKind
    else if List("no disc", "not ready", "no medium", "not present").exists(text.contains) then
      FailureKind.NoMediaKind
    else FailureKind.OtherKind
