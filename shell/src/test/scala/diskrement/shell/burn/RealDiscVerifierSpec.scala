package diskrement.shell.burn

import diskrement.core.{ContentHash, RelPath, ScannedFile, SessionManifest}
import diskrement.shell.fs.HasherLive
import diskrement.shell.model.SupersededVersion
import zio._
import zio.test._
import java.nio.file.{Files, Path}
import java.time.Instant

object RealDiscVerifierSpec extends ZIOSpecDefault:

  private val hasXorriso: Boolean =
    try new ProcessBuilder("xorriso", "--version").start().waitFor() == 0
    catch case _: Exception => false

  private def sha(s: String) = ContentHash(java.security.MessageDigest.getInstance("SHA-256")
    .digest(s.getBytes("UTF-8")).map(b => f"${b & 0xff}%02x").mkString)

  private def burnSession(base: Path, iso: Path, n: Int, files: Map[String, String]) =
    val run = CmdRunnerLive()
    val dev = BurnDevice.ImageFile(iso)
    for
      ans <- Ref.make(List(true))
      log <- Ref.make(List.empty[String])
      b    = XorrisoBurnerLive(dev, run, XorrisoMediaInspectorLive(run),
               ScriptedOperator(ans, log), HasherLive(), SnapshotWriterLive())
      snap = base.resolve(s"snap$n")
      _    = files.foreach { (r, c) =>
               val p = snap.resolve(r); Files.createDirectories(p.getParent); Files.writeString(p, c) }
      cat  = base.resolve(s"cat$n")
      _    = Files.writeString(cat, "x")
      img  = DiscImage(base.resolve(s"stage$n"), snap,
               files.map((r, c) => ScannedFile(RelPath(r), sha(c), c.length.toLong,
                 Instant.parse("2026-08-30T10:00:00Z"))).toList,
               DiscMetadata(DiscIdentity("photos", 1, n, Instant.parse("2026-08-30T10:00:00Z")),
                 SessionManifest("photos", 1, n, Nil, Nil, Nil), cat, Nil), false)
      _   <- b.burn(img)
    yield ()

  private def mkVerifier(iso: Path, answers: List[Boolean]) =
    val run = CmdRunnerLive()
    val dev = BurnDevice.ImageFile(iso)
    for
      ans <- Ref.make(answers)
      log <- Ref.make(List.empty[String])
    yield RealDiscVerifierLive(dev, XorrisoMediaInspectorLive(run), ScriptedOperator(ans, log),
      DiscReaderLive(run), HasherLive())

  private def realSuite = suite("RealDiscVerifierSpec")(
    test("two-tier verify: clean disc passes, corrupted expectation mismatches, decline skips") {
      ZIO.attemptBlocking(Files.createTempDirectory("rdv")).orDie.flatMap { base =>
        val iso = base.resolve("disc.iso")
        val exp = DiscExpectation("photos", 1,
          latestSession = Map.empty,
          visible = Map(RelPath("a.txt") -> sha("v2"), RelPath("b.txt") -> sha("keep")),
          superseded = List(SupersededVersion(1, RelPath("a.txt"), sha("v1"))))
        for
          _     <- burnSession(base, iso, 1, Map("a.txt" -> "v1", "b.txt" -> "keep"))
          _     <- burnSession(base, iso, 2, Map("a.txt" -> "v2"))
          v     <- mkVerifier(iso, List(true))
          clean <- v.verify(exp)
          v2    <- mkVerifier(iso, List(true))
          bad   <- v2.verify(exp.copy(
                     visible = exp.visible.updated(RelPath("b.txt"), sha("TAMPERED")),
                     superseded = List(SupersededVersion(1, RelPath("a.txt"), sha("WRONG")))))
          v3    <- mkVerifier(iso, List(false))
          skip  <- v3.verify(exp)
        yield assertTrue(
          clean == VerifyOutcome(3, Nil, skipped = false), // 2 visible + 1 superseded
          bad.mismatches.toSet == Set(RelPath("b.txt"), RelPath("a.txt")),
          skip == VerifyOutcome(0, Nil, skipped = true))
      }
    },
    test("missing expected file is a mismatch") {
      ZIO.attemptBlocking(Files.createTempDirectory("rdv2")).orDie.flatMap { base =>
        val iso = base.resolve("disc.iso")
        for
          _   <- burnSession(base, iso, 1, Map("only.txt" -> "here"))
          v   <- mkVerifier(iso, List(true))
          out <- v.verify(DiscExpectation("photos", 1, Map.empty,
                   visible = Map(RelPath("only.txt") -> sha("here"),
                                 RelPath("ghost.txt") -> sha("missing")),
                   superseded = Nil))
        yield assertTrue(out.mismatches == List(RelPath("ghost.txt")))
      }
    }
  ) @@ TestAspect.sequential @@ TestAspect.withLiveClock

  def spec =
    if hasXorriso then realSuite
    else suite("RealDiscVerifierSpec")(test("SKIPPED: xorriso not installed")(assertCompletes))
