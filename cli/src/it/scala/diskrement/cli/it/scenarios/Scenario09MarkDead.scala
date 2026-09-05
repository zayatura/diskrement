package diskrement.cli.it
package scenarios

import zio.ZIO
import zio.test.*

/** The coaster recovery flow: a top-off disc dies; its live content re-plans
  * onto a fresh disc, the superseded version is reported permanently lost, and
  * verify no longer looks at the dead disc. */
object Scenario09MarkDead:
  /** True if `verify` reported on disc `n` — matched on the line prefix, since a
    * bare "disc 1" substring would also match "disc 10". */
  private def verified(out: String, n: Int): Boolean =
    out.linesIterator.exists(_.startsWith(s"disc $n:"))

  val suite = zio.test.suite("scenario 9: mark-dead -> re-plan")(
    test("dead disc's live content re-plans; superseded version reported lost"):
      ZIO.scoped {
        for
          s    <- Support.scratch
          cli   = new Cli(s, discBytes = None)
          _    <- ZIO.attempt {
                    Tree.write(s.source, "a.txt", 128)
                    Tree.write(s.source, "b.txt", 128)
                  }
          _    <- Support.run(cli, "init", "p",
                    "--source", s.source.toString, "--work-dir", s.work.toString)
          // cycle 1: burn a+b to disc 1 (left OPEN)
          _    <- Support.run(cli, "scan", "p")
          _    <- Support.run(cli, "plan", "p")
          _    <- Support.run(cli, "burn", "p", "--yes")
          // cycle 2: modify b -> session 2 tops off disc 1, superseding b@v0 in place
          _    <- ZIO.attempt(Tree.modify(s.source, "b.txt", 128))
          _    <- Support.run(cli, "scan", "p")
          _    <- Support.run(cli, "plan", "p")
          _    <- Support.run(cli, "burn", "p", "--yes")
          // the coaster event
          dead <- Support.run(cli, "disc", "mark-dead", "p", "1", "--yes")
          st   <- Support.run(cli, "status", "p")
          // recovery cycle: everything live re-plans onto a fresh disc 2
          sc   <- Support.run(cli, "scan", "p")
          pl   <- Support.run(cli, "plan", "p")
          b2   <- Support.run(cli, "burn", "p", "--yes")
          d2   <- ZIO.attemptBlocking(DiscOracle.dataFiles(s.discDir(2)))
          ver  <- Support.run(cli, "verify", "p")
          hb   <- Support.run(cli, "history", "p", "b.txt")
        yield assertTrue(
          // the mark-dead report: one superseded version lost, two files to re-burn
          dead.exitCode == 0,
          dead.stdout.contains("Disc 1 marked DEAD."),
          dead.stdout.contains("Permanently lost (1):"),
          // the lost entry is b.txt's superseded v1, not just "b.txt appears
          // somewhere" -- it also appears in the re-burn list below
          dead.stdout.linesIterator.exists(l =>
            l.trim.startsWith("b.txt") && l.contains("(was session 1)")),
          dead.stdout.contains("Re-burn on next scan/plan/burn (2):"),
          st.stdout.contains("DEAD"),
          // recovery cycle: the dead disc's live content lands on a fresh disc 2
          sc.exitCode == 0,
          // the dead disc's content is gone from the burned view, so both files
          // read as new work rather than as unchanged
          sc.stdout.contains("Scanned: 2 added, 0 modified, 0 deleted, 0 unchanged."),
          pl.exitCode == 0,
          pl.stdout.contains("new disc"),
          b2.exitCode == 0,
          b2.stdout.contains("Wrote disc 2"),
          d2 == Set("a.txt", "b.txt"),
          // verify skips the dead disc; history records the loss
          ver.exitCode == 0,
          !verified(ver.stdout, 1),
          verified(ver.stdout, 2),
          ver.stdout.contains("files OK"),
          hb.exitCode == 0,
          hb.stdout.contains("LOST"))
      }
    ,
    test("declined confirmation aborts cleanly and marks nothing"):
      ZIO.scoped {
        for
          s    <- Support.scratch
          cli   = new Cli(s, discBytes = None)
          _    <- ZIO.attempt(Tree.write(s.source, "a.txt", 128))
          _    <- Support.run(cli, "init", "p",
                    "--source", s.source.toString, "--work-dir", s.work.toString)
          _    <- Support.run(cli, "scan", "p")
          _    <- Support.run(cli, "plan", "p")
          _    <- Support.run(cli, "burn", "p", "--yes")
          // no --yes: the harness closes stdin, so the prompt reads EOF -> decline
          dead <- Support.run(cli, "disc", "mark-dead", "p", "1")
          st   <- Support.run(cli, "status", "p")
        yield assertTrue(
          dead.exitCode == 0,
          dead.stdout.contains("Aborted."),
          !st.stdout.contains("DEAD"))
      }
  )
