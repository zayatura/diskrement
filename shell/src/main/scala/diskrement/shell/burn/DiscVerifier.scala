package diskrement.shell.burn

import diskrement.core.{ContentHash, RelPath}
import diskrement.shell.model.SupersededVersion
import zio._

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
