package diskrement.shell.pipeline

import diskrement.core.RelPath
import diskrement.shell.burn.BurnError
import java.nio.file.Path

/** Expected pipeline failures. Mirrors the other shell error ADTs. */
sealed abstract class PipelineError(msg: String, cause: Throwable = null)
    extends Exception(msg, cause)
object PipelineError:
  final case class ProjectNotFound(name: String)        extends PipelineError(s"project not found: $name")
  final case class NoStagedScan(name: String)           extends PipelineError(s"no staged scan for $name (run scan)")
  final case class NoStagedPlan(name: String)           extends PipelineError(s"no staged plan for $name (run plan)")
  final case class SnapshotMissing(name: String)        extends PipelineError(s"source snapshot missing for $name (run scan)")
  final case class NotSameFilesystem(src: Path, work: Path)
      extends PipelineError(s"work dir $work is not on the same filesystem as source $src")
  final case class NotCowCapable(workDir: Path, detail: String)
      extends PipelineError(s"work dir is not CoW-capable: $workDir [$detail]")
  final case class OversizedFiles(paths: List[RelPath])
      extends PipelineError(s"oversized files cannot be placed: ${paths.map(_.value).mkString(", ")}")
  final case class Burn(cause: BurnError) extends PipelineError(cause.getMessage, cause)
  final case class Failed(cause: Throwable) extends PipelineError("pipeline failure", cause)
