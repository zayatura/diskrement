package diskrement.shell.model

import java.time.Instant

/** Lifecycle state of a burn session. */
enum SessionState:
  case PLANNED, BURNED, VERIFIED

/** A persisted session row, joined to its disc's ordinal for convenience. */
final case class Session(
    discOrdinal: Int,
    ordinal: Int,
    state: SessionState,
    burnedAt: Option[Instant],
    manifestHash: Option[String]
)
