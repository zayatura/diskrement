package diskrement.cli

import diskrement.shell.burn.BurnDevice

enum Backend:
  case Simulated
  case Real(device: BurnDevice)

/** Pure flag/env resolution of the burn backend. Precedence: flag > env >
  * default (real burner on /dev/sr0). The environment is injected, matching
  * PathResolver's style. */
object BackendResolver:
  def resolve(global: GlobalOpts, env: String => Option[String]): Backend =
    val kind = global.burner.orElse {
      env("DISKREMENT_BURNER").map(_.trim.toLowerCase).collect {
        case "simulated" => BurnerKind.Simulated
        case "real"      => BurnerKind.Real
      }
    }.getOrElse(BurnerKind.Real)
    kind match
      case BurnerKind.Simulated => Backend.Simulated
      case BurnerKind.Real =>
        val dev = global.device
          .orElse(env("DISKREMENT_DEVICE").filter(_.nonEmpty))
          .getOrElse("/dev/sr0")
        Backend.Real(BurnDevice.RealDevice(dev))
