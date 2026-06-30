package diskrement.cli.it

import zio.*
import zio.test.*

/** Shared ZIO helpers for the integration scenarios. */
object Support:
  /** A scratch dir scoped to the test: created on acquire, deleted on release
    * (runs even if the test fails). */
  val scratch: ZIO[Scope, Throwable, Scratch] =
    ZIO.acquireRelease(ZIO.attempt(Scratch.make()))(s => ZIO.attempt(Scratch.delete(s)).orDie)

  /** Run one CLI command on the blocking pool. */
  def run(cli: Cli, args: String*): Task[CliResult] =
    ZIO.attemptBlocking(cli.run(args*))

  /** Skip the whole suite cleanly when the working directory is not CoW-capable. */
  val cowOrIgnore: TestAspectPoly =
    if CoW.capable(Scratch.base.resolve(".cow-check")) then TestAspect.identity
    else TestAspect.ignore
