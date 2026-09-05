package diskrement.cli

import diskrement.core.DiscType
import java.nio.file.Path

enum BurnerKind { case Real, Simulated }

/** Global options accepted before any subcommand. */
final case class GlobalOpts(
    catalog: Option[Path],
    dataDir: Option[Path],
    device: Option[String] = None,
    burner: Option[BurnerKind] = None)

/** A fully-parsed command line, independent of zio-cli. The unit Render and
  * Runner operate on. `relPath` stays a String here; Runner wraps it in RelPath. */
enum DiskrementCommand:
  case Init(name: String, source: Path, workDir: Path, discType: DiscType, marginPercent: Double, copies: Int)
  case Scan(name: String, verify: Boolean, force: Boolean)
  case Plan(name: String, discType: Option[DiscType], willFinalize: Boolean)
  case Burn(name: String, assumeYes: Boolean)
  case Reset(name: String)
  case Status(name: String)
  case Verify(name: String, disc: Option[Int])
  case History(name: String, relPath: String)
  case MarkDead(name: String, disc: Int, assumeYes: Boolean)
