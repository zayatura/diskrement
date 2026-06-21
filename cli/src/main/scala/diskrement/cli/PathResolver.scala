package diskrement.cli

import java.nio.file.{Path, Paths}

final case class ResolvedPaths(catalogPath: Path, dataDir: Path)

/** Pure resolution of the catalog file location. Precedence:
  *   --catalog  >  --data-dir  >  $STATE_DIRECTORY  >  $XDG_DATA_HOME/diskrement
  *   >  <home>/.local/share/diskrement
  * The environment is injected (a `String => Option[String]` lookup) so this is a
  * pure function of its inputs. */
object PathResolver:
  def resolve(
      projectName: String,
      global: GlobalOpts,
      env: String => Option[String],
      homeDir: Path
  ): ResolvedPaths =
    global.catalog match
      case Some(catalogPath) =>
        val parent = Option(catalogPath.toAbsolutePath.getParent).getOrElse(Paths.get("."))
        ResolvedPaths(catalogPath, parent)
      case None =>
        // A set-but-empty env var (e.g. STATE_DIRECTORY="") would otherwise resolve
        // to the JVM working dir via Paths.get(""); treat empty as unset.
        val dataDir = global.dataDir
          .orElse(env("STATE_DIRECTORY").map(_.split(':').head).filter(_.nonEmpty).map(Paths.get(_)))
          .orElse(env("XDG_DATA_HOME").filter(_.nonEmpty).map(d => Paths.get(d).resolve("diskrement")))
          .getOrElse(homeDir.resolve(".local/share/diskrement"))
        ResolvedPaths(dataDir.resolve(s"$projectName.sqlite"), dataDir)
