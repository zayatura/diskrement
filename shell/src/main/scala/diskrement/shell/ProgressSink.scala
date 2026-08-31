package diskrement.shell

import zio._

/** Operator-facing progress for long byte-crunching phases (scan hashing, burn
  * staging). The pipeline reports through this port so it stays free of
  * console I/O; the CLI provides a stderr renderer, tests use the no-op.
  * `advance` is called from parallel fibers — implementations must be
  * thread-safe and cheap. */
trait ProgressSink:
  def start(label: String, totalBytes: Long, totalFiles: Int): UIO[Unit]
  def advance(deltaBytes: Long, deltaFiles: Int): UIO[Unit]
  def done: UIO[Unit]

object ProgressSink:
  object Noop extends ProgressSink:
    def start(label: String, totalBytes: Long, totalFiles: Int): UIO[Unit] = ZIO.unit
    def advance(deltaBytes: Long, deltaFiles: Int): UIO[Unit]              = ZIO.unit
    def done: UIO[Unit]                                                    = ZIO.unit

  val noop: ULayer[ProgressSink] = ZLayer.succeed(Noop)
