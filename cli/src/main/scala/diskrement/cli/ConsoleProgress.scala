package diskrement.cli

import diskrement.shell.ProgressSink
import zio._

/** Stderr progress renderer. On a TTY: one carriage-return-refreshed line,
  * throttled to ~1/s — per completed file, so a single huge file advances
  * only when it finishes — cleared to end-of-line so a shrinking line leaves
  * no stale characters. Not a TTY: a plain line every ~10 s (so logs stay
  * small and IT stdout parsing is never touched). Writes ONLY to stderr. */
object ConsoleProgress:

  /** Renders one progress line.
    *
    * @param label phase label shown at the start of the line
    * @param doneBytes bytes completed so far
    * @param totalBytes total bytes for the phase
    * @param doneFiles files completed so far
    * @param totalFiles total files for the phase
    * @param rateBytesPerSec throughput in bytes/sec (rendered as MB/s)
    * @param etaSeconds estimated seconds remaining; negative renders "ETA --"
    */
  def format(label: String, doneBytes: Long, totalBytes: Long,
             doneFiles: Int, totalFiles: Int,
             rateBytesPerSec: Double, etaSeconds: Long): String =
    val etaPart =
      if etaSeconds < 0 then "ETA --"
      else f"ETA ${etaSeconds / 60}%dm${etaSeconds % 60}%02ds"
    f"$label ${doneBytes / 1e9}%.1f/${totalBytes / 1e9}%.1f GB · " +
      f"$doneFiles%d/$totalFiles%d files · ${rateBytesPerSec / 1e6}%.0f MB/s · " + etaPart

  /** Best-effort "is stderr a terminal": JDK 22+'s Console.isTerminal() is
    * authoritative (pre-22, a non-null Console already implies a terminal);
    * TERM=dumb opts out explicitly (also the escape hatch for `2> log`, which
    * the JDK cannot detect — Console reflects stdin/stdout, not stderr).
    *
    * @param env environment lookup, injectable for testing (default: real process env)
    * @param console Console supplier, injectable for testing (default: `System.console()`)
    */
  private[cli] def stderrIsTty(
      env: Map[String, String] = sys.env,
      console: () => java.io.Console = () => java.lang.System.console()
  ): Boolean =
    env.get("TERM").forall(_ != "dumb") && (
      console() match
        case null => false
        case c =>
          // Look isTerminal up on the PUBLIC Console type: the runtime class
          // (ProxyingConsole) is package-private and direct reflection on it
          // throws IllegalAccessException on JDK 22+. TTY detection must never
          // be fatal — any reflection failure falls back to "assume terminal"
          // (correct pre-22, where a non-null Console implies one).
          try classOf[java.io.Console].getMethod("isTerminal").invoke(c).asInstanceOf[Boolean]
          catch case _: Throwable => true
    )

  private final case class State(
      label: String, totalBytes: Long, totalFiles: Int,
      doneBytes: Long, doneFiles: Int,
      startedNanos: Long, lastEmitNanos: Long, emitted: Boolean)

  private[cli] final class Live private[ConsoleProgress] (
      ref: Ref[Option[State]], tty: Boolean, write: String => Unit)
      extends ProgressSink:
    private val intervalNanos = if tty then 1_000_000_000L else 10_000_000_000L

    def start(label: String, totalBytes: Long, totalFiles: Int): UIO[Unit] =
      Clock.nanoTime.flatMap(now =>
        ref.set(Some(State(label, totalBytes, totalFiles, 0L, 0, now, now, false))))

    def advance(deltaBytes: Long, deltaFiles: Int): UIO[Unit] =
      Clock.nanoTime.flatMap { now =>
        ref.modify[Option[State]] {
          case Some(s) =>
            val s2 = s.copy(doneBytes = s.doneBytes + deltaBytes,
                            doneFiles = s.doneFiles + deltaFiles)
            if now - s.lastEmitNanos >= intervalNanos then
              (Some(s2), Some(s2.copy(lastEmitNanos = now, emitted = true)))
            else (None, Some(s2))
          case None => (None, None)
        }.flatMap {
          case Some(snap) => emit(snap, now)
          case None       => ZIO.unit
        }
      }

    // No late advance can land after this: FileScanner attaches `phase`
    // OUTSIDE `foreachPar`, so all child fibers are joined (and their
    // `advance` calls complete) before `done` ever runs.
    def done: UIO[Unit] =
      ref.getAndSet(None).flatMap {
        case Some(s) if s.emitted =>
          Clock.nanoTime.flatMap(emit(s, _)) *>
            ZIO.when(tty)(ZIO.succeed(write("\n"))).unit
        case _ => ZIO.unit
      }

    private def emit(s: State, now: Long): UIO[Unit] =
      ZIO.succeed {
        // unreachable in practice (first emit needs a full interval); kept for safety
        val elapsedSec = math.max((now - s.startedNanos) / 1e9, 1e-9)
        val rate       = s.doneBytes / elapsedSec
        val eta        = if rate > 0 then ((s.totalBytes - s.doneBytes) / rate).toLong else -1L
        val line       = format(s.label, s.doneBytes, s.totalBytes,
                                s.doneFiles, s.totalFiles, rate, eta)
        val chunk      = if tty then "\r" + line + "[K" else line + "\n"
        write(chunk)
      }

  private[cli] def unsafeMake(tty: Boolean, write: String => Unit): UIO[ProgressSink] =
    Ref.make(Option.empty[State]).map(ref => new Live(ref, tty, write))

  val live: ZLayer[Any, Nothing, ProgressSink] =
    ZLayer.fromZIO(unsafeMake(stderrIsTty(), s => { java.lang.System.err.print(s); () }))
