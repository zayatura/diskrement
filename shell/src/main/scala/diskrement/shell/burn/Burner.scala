package diskrement.shell.burn

import zio._

/** Writes one disc image (one session) to its destination. Implementations:
  * SimulatedBurnerLive (stage to a work directory) and XorrisoBurnerLive
  * (write real or image-file media via xorriso). */
trait Burner:
  def burn(image: DiscImage): IO[BurnError, BurnResult]
