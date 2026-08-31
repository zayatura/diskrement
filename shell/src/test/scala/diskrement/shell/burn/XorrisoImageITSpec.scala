package diskrement.shell.burn

import diskrement.core.{ContentHash, RelPath, ScannedFile, SessionManifest}
import diskrement.shell.fs.HasherLive
import zio._
import zio.test._
import java.nio.file.{Files, Path}
import java.time.Instant

/** Runs the REAL xorriso against stdio: image files — the production code path
  * end to end, no hardware. Skipped (with a visible marker test) when xorriso
  * is not installed. */
object XorrisoImageITSpec extends ZIOSpecDefault:

  private val hasXorriso: Boolean =
    try new ProcessBuilder("xorriso", "--version").start().waitFor() == 0
    catch case _: Exception => false

  private val hasher = HasherLive()

  private def sha256(s: String): ContentHash =
    ContentHash(java.security.MessageDigest.getInstance("SHA-256")
      .digest(s.getBytes("UTF-8")).map(b => f"${b & 0xff}%02x").mkString)

  private def scanned(rel: String, content: String): ScannedFile =
    ScannedFile(RelPath(rel), sha256(content), content.getBytes("UTF-8").length.toLong,
      Instant.parse("2026-08-30T10:00:00Z"))

  /** Build a real snapshot dir + DiscImage for one session. */
  private def mkImage(base: Path, sessionOrdinal: Int, files: Map[String, String],
                      closeDisc: Boolean): DiscImage =
    val snap = base.resolve(s"snapshot-$sessionOrdinal")
    files.foreach { case (rel, content) =>
      val p = snap.resolve(rel); Files.createDirectories(p.getParent); Files.writeString(p, content)
    }
    val catSnap = base.resolve(s"cat-$sessionOrdinal.sqlite")
    Files.writeString(catSnap, s"fake-catalog-session-$sessionOrdinal")
    val id = DiscIdentity("photos", 1, sessionOrdinal, Instant.parse("2026-08-30T10:00:00Z"))
    DiscImage(base.resolve(s"staging-$sessionOrdinal"), snap,
      files.map((r, c) => scanned(r, c)).toList,
      DiscMetadata(id, SessionManifest("photos", 1, sessionOrdinal, Nil, Nil, Nil), catSnap, Nil),
      closeDisc)

  private def mkBurner(iso: Path, answers: List[Boolean]) =
    for
      ans <- Ref.make(answers)
      log <- Ref.make(List.empty[String])
      dev  = BurnDevice.ImageFile(iso)
      run  = CmdRunnerLive()
      insp = XorrisoMediaInspectorLive(run)
      b    = XorrisoBurnerLive(dev, run, insp, ScriptedOperator(ans, log), hasher, SnapshotWriterLive(),
               diskrement.shell.ProgressSink.Noop, 2)
    yield (b, dev, run, insp)

  private def realSuite = suite("XorrisoImageITSpec")(
    test("burn session 1, append session 2; merged view hides v1; session 1 still readable") {
      ZIO.attemptBlocking(Files.createTempDirectory("xit")).orDie.flatMap { base =>
        val iso = base.resolve("disc.iso")
        for
          made1 <- mkBurner(iso, List(true))
          (b1, dev, run, insp) = made1
          // session 1: a.txt v1 + b.txt
          r1    <- b1.burn(mkImage(base, 1, Map("a.txt" -> "hello v1", "b.txt" -> "stable"),
                     closeDisc = false))
          s1    <- insp.inspect(dev)
          // session 2: a.txt v2 (hides v1)
          made2 <- mkBurner(iso, List(true))
          (b2, _, _, _) = made2
          r2    <- b2.burn(mkImage(base, 2, Map("a.txt" -> "hello v2"), closeDisc = false))
          s2    <- insp.inspect(dev)
          // merged view reads v2
          mergedDest = base.resolve("merged-a.txt")
          _     <- run.run(Xorriso.extractArgs(dev, "/data/a.txt", mergedDest, None))
          merged <- ZIO.attemptBlocking(Files.readString(mergedDest)).orDie
          // session 1 still holds v1 (Tier-2 mechanics)
          oldDest = base.resolve("old-a.txt")
          _     <- run.run(Xorriso.extractArgs(dev, "/data/a.txt", oldDest, Some(1)))
          old   <- ZIO.attemptBlocking(Files.readString(oldDest)).orDie
        yield assertTrue(
          r1.imageBytes > 0L, r2.imageBytes > 0L,
          s1 match { case MediaStatus.Appendable(_, 1, Some(id)) => id.sessionOrdinal == 1; case _ => false },
          s2 match { case MediaStatus.Appendable(_, 2, Some(id)) => id.sessionOrdinal == 2; case _ => false },
          merged == "hello v2", old == "hello v1")
      }
    },
    test("top-off with wrong expected session is refused until abort") {
      ZIO.attemptBlocking(Files.createTempDirectory("xit")).orDie.flatMap { base =>
        val iso = base.resolve("foreign.iso")
        for
          made1 <- mkBurner(iso, List(true))
          (b1, _, _, _) = made1
          _     <- b1.burn(mkImage(base, 1, Map("x.txt" -> "other"), closeDisc = false))
          // the image identifies as photos/disc 1/session 1; burning session 3 expects
          // previous ordinal 2 -> identity mismatch -> refusal loop -> scripted abort
          made2 <- mkBurner(iso, List(true, false)) // one retry then give up
          (b2, _, _, _) = made2
          img    = mkImage(base, 3, Map("y.txt" -> "new"), closeDisc = false)
          exit  <- b2.burn(img).exit
        yield assertTrue(exit == Exit.fail(BurnError.Aborted))
      }
    }
  ) @@ TestAspect.sequential @@ TestAspect.withLiveClock

  def spec =
    if hasXorriso then realSuite
    else suite("XorrisoImageITSpec")(
      test("SKIPPED: xorriso not installed")(assertCompletes))
