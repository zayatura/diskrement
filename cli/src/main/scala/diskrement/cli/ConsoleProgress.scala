package diskrement.cli

import diskrement.shell.ProgressSink
import zio._

/** Stderr progress renderer. On a TTY: one carriage-return-refreshed line,
  * throttled to ~1/s, cleared to end-of-line so a shrinking line leaves no
  * stale characters. Not a TTY: a plain line every ~10 s (so logs stay small
  * and IT stdout parsing is never touched). Writes ONLY to stderr. */
object ConsoleProgress:

  def format(label: String, doneBytes: Long, totalBytes: Long,
             doneFiles: Int, totalFiles: Int,
             rateBytesPerSec: Double, etaSeconds: Long): String =
    f"$label ${doneBytes / 1e9}%.1f/${totalBytes / 1e9}%.1f GB · " +
      f"$doneFiles%d/$totalFiles%d files · ${rateBytesPerSec / 1e6}%.0f MB/s · " +
      f"ETA ${etaSeconds / 60}%dm${etaSeconds % 60}%02ds"

  private final case class State(
      label: String, totalBytes: Long, totalFiles: Int,
      doneBytes: Long, doneFiles: Int,
      startedNanos: Long, lastEmitNanos: Long, emitted: Boolean)

  private final class Live(ref: Ref[Option[State]], tty: Boolean) extends ProgressSink:
    private val intervalNanos = if tty then 1_000_000_000L else 10_000_000_000L

    def start(label: String, totalBytes: Long, totalFiles: Int): UIO[Unit] =
      Clock.nanoTime.flatMap(now =>
        ref.set(Some(State(label, totalBytes, totalFiles, 0L, 0, now, now, false))))

    def advance(deltaBytes: Long, deltaFiles: Int): UIO[Unit] =
      Clock.nanoTime.flatMap { now =>
        ref.modify {
          case Some(s) =>
            val s2 = s.copy(doneBytes = s.doneBytes + deltaBytes,
                            doneFiles = s.doneFiles + deltaFiles)
            if now - s.lastEmitNanos >= intervalNanos then
              (Some(s2), Some(s2.copy(lastEmitNanos = now, emitted = true))): (Option[State], Option[State])
            else (None, Some(s2)): (Option[State], Option[State])
          case None => (None, None): (Option[State], Option[State])
        }.flatMap {
          case Some(snap) => emit(snap, now)
          case None       => ZIO.unit
        }
      }

    def done: UIO[Unit] =
      ref.getAndSet(None).flatMap {
        case Some(s) if s.emitted && tty =>
          ZIO.succeed(java.lang.System.err.println())
        case _ => ZIO.unit
      }

    private def emit(s: State, now: Long): UIO[Unit] =
      ZIO.succeed {
        val elapsedSec = math.max((now - s.startedNanos) / 1e9, 1e-9)
        val rate       = s.doneBytes / elapsedSec
        val eta        = if rate > 0 then ((s.totalBytes - s.doneBytes) / rate).toLong else 0L
        val line       = format(s.label, s.doneBytes, s.totalBytes,
                                s.doneFiles, s.totalFiles, rate, eta)
        if tty then java.lang.System.err.print("\r" + line + "[K")
        else java.lang.System.err.println(line)
      }

  val live: ZLayer[Any, Nothing, ProgressSink] =
    ZLayer {
      Ref.make(Option.empty[State])
        .map(ref => Live(ref, java.lang.System.console() != null))
    }
