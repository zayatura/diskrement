package diskrement.shell.burn

import zio._

/** Operator interaction during real burns and verifies: insert-disc
  * confirmations and live progress lines. The CLI supplies a console
  * implementation; tests script one. */
trait BurnOperator:
  /** Show `message`, wait for the operator. false = abort/skip. */
  def confirmInsert(message: String): UIO[Boolean]
  def progress(line: String): UIO[Unit]
