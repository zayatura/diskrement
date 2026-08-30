package diskrement.shell.burn

import diskrement.core.{ContentHash, RelPath}
import diskrement.shell.fs.Hasher
import diskrement.shell.model.SupersededVersion
import zio._

import java.nio.file.Files
import scala.jdk.StreamConverters.*

/** What the catalog says one disc should hold. `latestSession` is what the
  * simulated backend's staging dir contains; `visible`/`superseded` describe the
  * physical disc (merged mount view + versions hidden by later sessions). Each
  * verifier consumes the fields that match its substrate. */
final case class DiscExpectation(
    projectName: String,
    discOrdinal: Int,
    latestSession: Map[RelPath, ContentHash],
    visible: Map[RelPath, ContentHash],
    superseded: List[SupersededVersion])

/** skipped = nothing was checked (no staged data / operator declined the disc);
  * a skipped disc is never marked verified. */
final case class VerifyOutcome(filesChecked: Int, mismatches: List[RelPath], skipped: Boolean)

trait DiscVerifier:
  def verify(expectation: DiscExpectation): IO[BurnError, VerifyOutcome]

/** Read-back verification of physical (or image) media. Tier 1: the merged
  * /data tree via DiscReader, every file hashed against `visible`, expected
  * files missing from the disc are mismatches. Tier 2: versions superseded
  * within the disc, extracted from their recording session. An operator
  * decline skips the disc (it stays unverified). */
final case class RealDiscVerifierLive(
    device: BurnDevice,
    inspector: MediaInspector,
    operator: BurnOperator,
    reader: DiscReader,
    hasher: Hasher
) extends DiscVerifier:

  def verify(exp: DiscExpectation): IO[BurnError, VerifyOutcome] =
    awaitDisc(exp).flatMap {
      case false => ZIO.succeed(VerifyOutcome(0, Nil, skipped = true))
      case true =>
        for
          t1 <- tier1(exp)
          t2 <- tier2(exp)
        yield VerifyOutcome(t1._1 + t2._1, t1._2 ++ t2._2, skipped = false)
    }

  /** Insert/identity loop like the burner's, but any session ordinal of the
    * right disc is acceptable, and a decline is a skip, not an abort. */
  private def awaitDisc(exp: DiscExpectation): IO[BurnError, Boolean] =
    val prompt =
      s"Insert disc ${exp.discOrdinal} of '${exp.projectName}' into ${device.xorrisoAddress} " +
        "to verify it (or decline to skip this disc)."
    def matches(id: DiscIdentity) =
      id.projectName == exp.projectName && id.discOrdinal == exp.discOrdinal
    def loop: IO[BurnError, Boolean] =
      operator.confirmInsert(prompt).flatMap {
        case false => ZIO.succeed(false)
        case true =>
          inspector.inspect(device).flatMap {
            case MediaStatus.Appendable(_, _, Some(id)) if matches(id) => ZIO.succeed(true)
            case MediaStatus.Closed(Some(id)) if matches(id)           => ZIO.succeed(true)
            case found =>
              operator.progress(
                s"Not the expected disc (found: $found). Swap discs and confirm again.") *> loop
          }
      }
    loop

  private def tier1(exp: DiscExpectation): IO[BurnError, (Int, List[RelPath])] =
    reader.withDataTree(device) { dataDir =>
      for
        files <- ZIO.attemptBlockingIO {
                   val s = Files.walk(dataDir)
                   try s.toScala(List).filter(p => Files.isRegularFile(p))
                   finally s.close()
                 }.mapError(BurnError.BurnIoError(_))
        found <- ZIO.foreach(files) { f =>
                   val rel = RelPath(dataDir.relativize(f).toString)
                   hasher.hash(f).mapError(BurnError.BurnIoError(_)).map(h => rel -> h)
                 }
        foundMap   = found.toMap
        mismatched = found.collect {
                       case (rel, h) if !exp.visible.get(rel).contains(h) => rel }
        missing    = exp.visible.keys.filterNot(foundMap.contains).toList
      yield (files.size + missing.size, mismatched ++ missing)
    }

  private def tier2(exp: DiscExpectation): IO[BurnError, (Int, List[RelPath])] =
    ZIO.foreach(exp.superseded) { sv =>
      ZIO.attemptBlockingIO(Files.createTempFile("diskrement-tier2", ".bin"))
        .mapError(BurnError.BurnIoError(_))
        .flatMap { tmp =>
          val checkOne =
            ZIO.attemptBlockingIO { Files.deleteIfExists(tmp); () }
              .mapError(BurnError.BurnIoError(_)) *>
              reader.extractFromSession(
                device, sv.sessionOrdinal, "/data/" + sv.relPath.value, tmp) *>
              hasher.hash(tmp).mapError(BurnError.BurnIoError(_))
                .map(h => Option.when(h != sv.contentHash)(sv.relPath))
          checkOne
            .catchAll(_ => ZIO.succeed(Some(sv.relPath))) // unreadable = mismatch
            .ensuring(ZIO.attemptBlockingIO(Files.deleteIfExists(tmp)).ignore)
        }
    }.map(res => (res.size, res.flatten))

object RealDiscVerifier:
  val live: ZLayer[BurnDevice & MediaInspector & BurnOperator & DiscReader & Hasher,
                   Nothing, DiscVerifier] =
    ZLayer {
      for
        dev  <- ZIO.service[BurnDevice]
        insp <- ZIO.service[MediaInspector]
        op   <- ZIO.service[BurnOperator]
        rd   <- ZIO.service[DiscReader]
        h    <- ZIO.service[Hasher]
      yield RealDiscVerifierLive(dev, insp, op, rd, h)
    }
