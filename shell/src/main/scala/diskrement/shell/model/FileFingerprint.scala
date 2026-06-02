package diskrement.shell.model

import diskrement.core.ContentHash
import java.time.Instant

/** The prior-version identity the scanner needs to decide whether a file is
  * unchanged (size + mtime match -> reuse the stored content hash). */
final case class FileFingerprint(
    sizeBytes: Long,
    mtime: Instant,
    contentHash: ContentHash
)
