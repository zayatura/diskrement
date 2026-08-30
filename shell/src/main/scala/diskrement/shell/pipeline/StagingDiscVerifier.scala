package diskrement.shell.pipeline

import diskrement.core.RelPath
import diskrement.shell.burn.{BurnError, DiscExpectation, DiscVerifier, VerifyOutcome}
import diskrement.shell.fs.Hasher
import zio._
import java.nio.file.{Files, Path}
import scala.jdk.StreamConverters.*

/** Verifies a disc against the work-dir staging tree (simulated backend). The
  * staging dir holds only the disc's LATEST session, so the expectation's
  * `latestSession` map is the reference; an absent staging dir is a skip. */
final case class StagingDiscVerifier(stagingRoot: Path, hasher: Hasher) extends DiscVerifier:

  def verify(exp: DiscExpectation): IO[BurnError, VerifyOutcome] =
    val dataDir = stagingRoot.resolve(s"disc-${exp.discOrdinal}").resolve("data")
    ZIO.attemptBlockingIO(Files.exists(dataDir)).mapError(BurnError.BurnIoError(_)).flatMap {
      case false => ZIO.succeed(VerifyOutcome(0, Nil, skipped = true))
      case true =>
        for
          files <- ZIO.attemptBlockingIO {
                     val s = Files.walk(dataDir)
                     try s.toScala(List).filter(p => Files.isRegularFile(p))
                     finally s.close()
                   }.mapError(BurnError.BurnIoError(_))
          mismatches <- ZIO.foreach(files) { f =>
                          val rel = RelPath(dataDir.relativize(f).toString)
                          hasher.hash(f).mapError(BurnError.BurnIoError(_)).map { h =>
                            Option.when(!exp.latestSession.get(rel).contains(h))(rel)
                          }
                        }.map(_.flatten)
        yield VerifyOutcome(files.size, mismatches, skipped = false)
    }

object StagingDiscVerifier:
  val live: ZLayer[ProjectPaths & Hasher, Nothing, DiscVerifier] =
    ZLayer {
      for
        paths  <- ZIO.service[ProjectPaths]
        hasher <- ZIO.service[Hasher]
      yield StagingDiscVerifier(paths.workDir.resolve("staging"), hasher)
    }
