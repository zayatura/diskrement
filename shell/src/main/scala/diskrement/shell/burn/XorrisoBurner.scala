package diskrement.shell.burn

import diskrement.shell.fs.Hasher
import zio._

/** The real burner: prompts for the disc, inspects it (strict identity check),
  * stages with re-hash, checks capacity, burns via xorriso, sanity-checks the
  * result, ejects. Spec §4. Any failure leaves the catalog uncommitted (the
  * pipeline commits only after this returns). */
final case class XorrisoBurnerLive(
    device: BurnDevice,
    runner: CmdRunner,
    inspector: MediaInspector,
    operator: BurnOperator,
    hasher: Hasher,
    writer: SnapshotWriter
) extends Burner:

  private val fixedOverheadBytes = 32L * 1024 * 1024

  def burn(image: DiscImage): IO[BurnError, BurnResult] =
    val id = image.metadata.identity
    val firstSession = id.sessionOrdinal == 1
    for
      _      <- preflight
      free   <- awaitCorrectMedia(image)
      staged <- Stager.stage(image, hasher, writer)
      _      <- checkCapacity(staged.imageBytes, free)
      _      <- doBurn(image, firstSession)
      _      <- postBurnCheck(id)
      _      <- eject
    yield staged

  private val preflight: IO[BurnError, Unit] =
    runner.run(Xorriso.versionArgs).flatMap { r =>
      Xorriso.parseVersion(r.output) match
        case None => ZIO.fail(BurnError.ToolMissing("cannot parse `xorriso --version` output"))
        case Some((maj, min, raw)) =>
          ZIO.unless(maj > 1 || (maj == 1 && min >= 5))(
            ZIO.fail(BurnError.ToolTooOld(raw))).unit
    }

  /** Prompt-inspect loop: returns the media's free bytes once the right disc is
    * in, fails Aborted when the operator declines. Wrong media re-prompts. */
  private def awaitCorrectMedia(image: DiscImage): IO[BurnError, Long] =
    val id = image.metadata.identity
    val expected =
      if id.sessionOrdinal == 1 then s"a blank disc for '${id.projectName}' disc ${id.discOrdinal}"
      else s"disc ${id.discOrdinal} of '${id.projectName}' (last burned session ${id.sessionOrdinal - 1})"
    val sealNote = if image.closeDisc then " This burn will FINALIZE the disc." else ""
    val prompt = s"Insert $expected into ${device.xorrisoAddress} and confirm.$sealNote"

    def check: IO[BurnError, Either[String, Long]] =
      inspector.inspect(device).map { status =>
        (status, id.sessionOrdinal) match
          case (MediaStatus.NoMedia, _)             => Left("no disc in the drive")
          case (MediaStatus.Blank(free), 1)         => Right(free)
          case (MediaStatus.Blank(_), _)            => Left("a blank disc (expected written media to append to)")
          case (MediaStatus.Appendable(_, _, _), 1) => Left("written media (expected a blank disc)")
          case (MediaStatus.Appendable(free, _, Some(found)), n)
              if found.projectName == id.projectName
                && found.discOrdinal == id.discOrdinal
                && found.sessionOrdinal == n - 1    => Right(free)
          case (MediaStatus.Appendable(_, _, Some(found)), _) =>
            Left(s"disc '${found.projectName}' #${found.discOrdinal} at session ${found.sessionOrdinal}")
          case (MediaStatus.Appendable(_, _, None), _) => Left("a written non-diskrement disc")
          case (MediaStatus.Closed(_), _)              => Left("a finalized (closed) disc")
          case (MediaStatus.Unsuitable, _) =>
            Left("unsuitable media (e.g. a POW-formatted BD-R) — use a plain unformatted BD-R")
      }

    def loop: IO[BurnError, Long] =
      operator.confirmInsert(prompt).flatMap {
        case false => ZIO.fail(BurnError.Aborted)
        case true =>
          check.flatMap {
            case Right(free) => ZIO.succeed(free)
            case Left(found) =>
              operator.progress(s"Not the expected disc: found $found. Swap discs and confirm again.") *> loop
          }
      }
    loop

  private def checkCapacity(stagedBytes: Long, freeBytes: Long): IO[BurnError, Unit] =
    val needed = stagedBytes + stagedBytes / 100 + fixedOverheadBytes
    ZIO.unless(needed <= freeBytes)(
      ZIO.fail(BurnError.InsufficientCapacity(needed, freeBytes))).unit

  private def doBurn(image: DiscImage, firstSession: Boolean): IO[BurnError, Unit] =
    val id    = image.metadata.identity
    val volid = Xorriso.volid(id.projectName, id.discOrdinal)
    runner.run(
      Xorriso.burnArgs(device, image.stagingDir, firstSession, image.closeDisc, volid),
      line => ZIO.when(line.contains("UPDATE"))(operator.progress(line)).unit
    ).flatMap { r =>
      ZIO.unless(r.exitCode == 0)(
        ZIO.fail(BurnError.BurnFailed(r.exitCode, r.tail()))).unit
    }

  private def postBurnCheck(id: DiscIdentity): IO[BurnError, Unit] =
    inspector.readIdentity(device).flatMap {
      case Some(found)
          if found.projectName == id.projectName
            && found.discOrdinal == id.discOrdinal
            && found.sessionOrdinal == id.sessionOrdinal => ZIO.unit
      case other =>
        ZIO.fail(BurnError.PostBurnCheckFailed(
          s"expected identity (${id.projectName}, disc ${id.discOrdinal}, session ${id.sessionOrdinal}) " +
            s"readable from the disc, found: $other"))
    }

  /** Best-effort: the burn already succeeded, an eject hiccup must not fail it. */
  private val eject: IO[BurnError, Unit] = device match
    case BurnDevice.RealDevice(_) =>
      runner.run(Xorriso.ejectArgs(device))
        .flatMap(r => ZIO.when(r.exitCode != 0)(operator.progress("note: eject failed"))).unit
        .catchAll(_ => operator.progress("note: eject failed"))
    case BurnDevice.ImageFile(_) => ZIO.unit

object XorrisoBurner:
  val live: ZLayer[
    BurnDevice & CmdRunner & MediaInspector & BurnOperator & Hasher & SnapshotWriter,
    Nothing, Burner] =
    ZLayer {
      for
        dev  <- ZIO.service[BurnDevice]
        run  <- ZIO.service[CmdRunner]
        insp <- ZIO.service[MediaInspector]
        op   <- ZIO.service[BurnOperator]
        h    <- ZIO.service[Hasher]
        w    <- ZIO.service[SnapshotWriter]
      yield XorrisoBurnerLive(dev, run, insp, op, h, w)
    }
