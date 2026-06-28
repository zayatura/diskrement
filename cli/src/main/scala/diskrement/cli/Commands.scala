package diskrement.cli

import diskrement.core.DiscType
import zio.cli.*
import java.nio.file.{Path, Paths}

/** zio-cli command definitions. Each subcommand parses into a DiskrementCommand;
  * the root pairs the global options (parsed by the parent) in front, yielding
  * `(GlobalOpts, DiskrementCommand)`. zio-cli accepts the global options either
  * before or after the subcommand name. */
object Commands:

  private val discTypeChoices: Seq[(String, DiscType)] = Seq(
    "BD_R"    -> DiscType.BD_R,
    "BD_R_DL" -> DiscType.BD_R_DL,
    "DVD"     -> DiscType.DVD,
    "DVD_DL"  -> DiscType.DVD_DL)

  private val nameArg: Args[String] = Args.text("name")
  private def path(s: String): Path = Paths.get(s)

  // --- global options, parsed by the parent command ---
  private val catalogOpt: Options[Option[Path]] = Options.text("catalog").map(path).optional
  private val dataDirOpt: Options[Option[Path]] = Options.text("data-dir").map(path).optional
  private val globalOpts: Options[GlobalOpts] =
    (catalogOpt ++ dataDirOpt).map { case (c, d) => GlobalOpts(c, d) }

  // --- init ---
  private val init: Command[DiskrementCommand] =
    val source   = Options.text("source").map(path)
    val workDir  = Options.text("work-dir").map(path)
    val discType = Options.enumeration("disc-type")(discTypeChoices*).withDefault(DiscType.BD_R)
    val margin   = Options.decimal("margin").withDefault(BigDecimal(2.0))
    val copies   = Options.integer("copies").withDefault(BigInt(1))
    // The options flatten via Zippable to a 5-tuple; the positional name arg is paired alongside.
    Command("init", source ++ workDir ++ discType ++ margin ++ copies, nameArg)
      .withHelp(HelpDoc.p("Create a project and run filesystem checks."))
      .map { case ((src, wd, dt, m, c), name) =>
        DiskrementCommand.Init(name, src, wd, dt, m.toDouble, c.toInt)
      }

  // --- scan ---
  private val scan: Command[DiskrementCommand] =
    val verify = Options.boolean("verify")
    val force  = Options.boolean("force")
    Command("scan", verify ++ force, nameArg)
      .withHelp(HelpDoc.p("Snapshot the source and stage a diff."))
      .map { case ((v, f), name) => DiskrementCommand.Scan(name, v, f) }

  // --- plan ---
  private val plan: Command[DiskrementCommand] =
    val discType = Options.enumeration("disc-type")(discTypeChoices*).optional
    val finalize = Options.boolean("finalize")
    Command("plan", discType ++ finalize, nameArg)
      .withHelp(HelpDoc.p("Plan the next disc from the staged scan."))
      .map { case ((dt, fin), name) => DiskrementCommand.Plan(name, dt, fin) }

  // --- burn ---
  private val burn: Command[DiskrementCommand] =
    val yes = Options.boolean("yes").alias("y")
    Command("burn", yes, nameArg)
      .withHelp(HelpDoc.p("Burn the staged plan (with confirmation)."))
      .map { case (y, name) => DiskrementCommand.Burn(name, y) }

  // --- reset / status (no options; name only) ---
  private val reset: Command[DiskrementCommand] =
    Command("reset", Options.none, nameArg)
      .withHelp(HelpDoc.p("Discard staged scan/plan state."))
      .map(name => DiskrementCommand.Reset(name))

  private val status: Command[DiskrementCommand] =
    Command("status", Options.none, nameArg)
      .withHelp(HelpDoc.p("Show discs, totals, and last burn."))
      .map(name => DiskrementCommand.Status(name))

  // --- verify ---
  private val verify: Command[DiskrementCommand] =
    val disc = Options.integer("disc").optional.map(_.map(_.toInt))
    Command("verify", disc, nameArg)
      .withHelp(HelpDoc.p("Re-hash burned disc(s) against the catalog."))
      .map { case (d, name) => DiskrementCommand.Verify(name, d) }

  // --- history ---
  private val history: Command[DiskrementCommand] =
    Command("history", Options.none, nameArg ++ Args.text("relPath"))
      .withHelp(HelpDoc.p("Show the full version history of one file."))
      .map { case (name, rel) => DiskrementCommand.History(name, rel) }

  /** The root command. zio-cli pairs the parent's GlobalOpts with the chosen
    * subcommand's value, giving `(GlobalOpts, DiskrementCommand)`. */
  val root: Command[(GlobalOpts, DiskrementCommand)] =
    Command("diskrement", globalOpts)
      .subcommands(init, scan, plan, burn, reset, status, verify, history)
