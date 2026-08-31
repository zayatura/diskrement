package diskrement.shell

import zio._

/** Operator-facing progress for long byte-crunching phases (scan hashing, burn
  * staging). The pipeline reports through this port so it stays free of
  * console I/O; the CLI provides a stderr renderer, tests use the no-op.
  *
  * Lifecycle: wrap each phase in `phase(...)` — it calls `start` before the
  * work and guarantees `done` on success, failure, AND interrupt (so a TTY
  * renderer can always terminate its in-place line). One sink instance may run
  * several phases in sequence (e.g. scan hashing, then burn staging); `start`
  * resets any accumulated state. `advance` is called from parallel fibers —
  * implementations must be thread-safe and cheap. */
trait ProgressSink:
  def start(label: String, totalBytes: Long, totalFiles: Int): UIO[Unit]
  def advance(deltaBytes: Long, deltaFiles: Int): UIO[Unit]
  def done: UIO[Unit]

  /** Bracket one phase: `start` before `work`, `done` on any exit. */
  final def phase[R, E, A](label: String, totalBytes: Long, totalFiles: Int)(
      work: ZIO[R, E, A]): ZIO[R, E, A] =
    start(label, totalBytes, totalFiles) *> work.ensuring(done)

object ProgressSink:
  object Noop extends ProgressSink:
    def start(label: String, totalBytes: Long, totalFiles: Int): UIO[Unit] = ZIO.unit
    def advance(deltaBytes: Long, deltaFiles: Int): UIO[Unit]              = ZIO.unit
    def done: UIO[Unit]                                                    = ZIO.unit

  val noop: ZLayer[Any, Nothing, ProgressSink] = ZLayer.succeed(Noop)
