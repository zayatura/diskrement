package diskrement.shell.fs

/** Shared detection of "this filesystem cannot reflink" from cp's stderr, used
  * by the snapshot cloner (hard failure: CoW is required there) and the stager
  * (soft failure: falls back to plain copy). This matches English cp text, so
  * callers MUST run cp with LC_ALL=C in the child's environment. */
object Reflink:
  def cloneUnsupported(stderr: String): Boolean =
    val s = stderr.toLowerCase
    s.contains("failed to clone") || s.contains("operation not supported")
