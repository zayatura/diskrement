package diskrement.cli

import diskrement.core.*
import diskrement.shell.burn.BurnError
import diskrement.shell.model.*
import diskrement.shell.pipeline.*
import zio.ExitCode

/** Pure formatters: Pipeline result types -> terminal text. No I/O. */
object Render:

  /** Bytes as a fixed-precision GB string for deterministic golden tests. */
  private def gb(bytes: Long): String = f"${bytes / 1.0e9}%.1f GB"

  def scan(r: ScanResult): String =
    val first = s"Scanned: ${r.added} added, ${r.modified} modified, ${r.deleted} deleted, ${r.unchanged} unchanged."
    r.hashed match
      case Some(h) => first + s"\nHashed $h of ${r.scannedFiles} files (${r.scannedFiles - h} fingerprints reused)."
      case None    => first

  def plan(p: PlanReport): String =
    val d = p.disc
    if d.files.isEmpty && p.tombstones.isEmpty && p.oversized.isEmpty then
      if d.willFinalize then s"Plan: finalize disc ${d.ordinal} (seal; no new files)."
      else "Nothing to burn: no changes to plan."
    else
      val kind = if d.isNew then "new disc" else s"appending to open disc ${d.ordinal}"
      val fin  = if d.willFinalize then "will FINALIZE" else "left OPEN"
      val header =
        f"Plan: disc ${d.ordinal} ($kind, ${p.discType}) — ${d.files.size} files, ${gb(d.totalUsedBytes)}, $fin"
      val rem =
        if p.remaining.moreDiscs == 0 then "This disc completes the set."
        else s"~${p.remaining.moreFiles} more files (~${p.remaining.moreDiscs} more discs) remain after this one."
      val tomb = if p.tombstones.isEmpty then Nil else List(s"Deletions: ${p.tombstones.size}")
      val oversized =
        if p.oversized.isEmpty then Nil
        else "Oversized (cannot be placed):" :: p.oversized.map(f => s"  ${f.relPath.value}  ${gb(f.sizeBytes)}")
      (header :: rem :: tomb ::: oversized).mkString("\n")

  def burn(r: BurnReport): String =
    val d = r.disc
    val status =
      if d.finalized then "Status: FINALIZED — set this disc aside, it is sealed."
      else "Status: left OPEN — may receive more sessions later."
    val next =
      if r.remaining.moreDiscs == 0 then "No more changes — the set is up to date."
      else s"~${r.remaining.moreDiscs} more discs of changes remain. Run: scan → plan → burn"
    s"""✔ Wrote disc ${d.ordinal}  —  label: "${d.label}"
       |$status
       |$next""".stripMargin

  def status(project: Project, r: StatusReport): String =
    val header = f"Project ${project.name}  (source ${project.sourceRoot}, ${project.defaultDiscType}, margin ${project.marginPercent}%.1f%%)"
    val discLines = r.discs.map { d =>
      val open = if r.openDiscOrdinal.contains(d.ordinal) then "  <- open" else ""
      f"  #${d.ordinal}%-3d ${d.status}%-10s ${gb(d.usedBytes)} / ${gb(d.usableBytes)}$open"
    }
    val last = r.lastBurnAt.map(_.toString).getOrElse("never")
    (s"$header\nDiscs:" :: discLines ::: List(s"Current files: ${r.totalCurrentFiles}      Last burn: $last")).mkString("\n")

  def verify(reports: List[VerifyReport]): String =
    if reports.isEmpty then "Nothing to verify: no burned discs."
    else reports.map { v =>
      if v.skipped then s"disc ${v.discOrdinal}: skipped (not checked)"
      else if v.mismatches.isEmpty then s"disc ${v.discOrdinal}: ${v.filesChecked} files OK ✓"
      else s"disc ${v.discOrdinal}: ${v.filesChecked} checked, ${v.mismatches.size} MISMATCH:\n" +
        v.mismatches.map(p => s"  ${p.value}").mkString("\n")
    }.mkString("\n")

  def history(relPath: RelPath, entries: List[FileHistoryEntry]): String =
    if entries.isEmpty then s"No history for ${relPath.value}."
    else
      val lines = entries.zipWithIndex.map { case (e, i) =>
        f"  v${i + 1}  disc ${e.discOrdinal} / session ${e.sessionOrdinal}   ${gb(e.sizeBytes)}  ${e.contentHash.value}  ${e.status}"
      }
      (relPath.value :: lines).mkString("\n")

  def init(name: String, id: ProjectId): String =
    s"Created project $name (id ${id.value})."

  /** Known errors -> a stderr message and a stable exit code. */
  def handle(e: PipelineError): (String, ExitCode) =
    val code = e match
      case _: PipelineError.ProjectNotFound   => ExitCode(2)
      case _: PipelineError.NoStagedScan      => ExitCode(2)
      case _: PipelineError.NoStagedPlan      => ExitCode(2)
      case _: PipelineError.SnapshotMissing   => ExitCode(2)
      case _: PipelineError.UnknownDisc       => ExitCode(2)
      case _: PipelineError.DiscAlreadyDead   => ExitCode(2)
      case _: PipelineError.StalePlan         => ExitCode(2)
      case _: PipelineError.NotSameFilesystem => ExitCode(3)
      case _: PipelineError.NotCowCapable     => ExitCode(3)
      case _: PipelineError.OversizedFiles    => ExitCode(3)
      case PipelineError.Burn(BurnError.Aborted) => ExitCode(0)
      case _: PipelineError.Burn              => ExitCode(6)
      case _: PipelineError.Failed            => ExitCode(5)
    val message = e match
      case PipelineError.Burn(BurnError.Aborted) => "Aborted."
      case PipelineError.Failed(cause) =>
        val detail = Option(cause.getMessage).getOrElse(cause.getClass.getSimpleName)
        s"internal error: $detail (this is a bug)"
      case other => other.getMessage
    (message, code)

  /** Result-driven exit: a verify that finds corruption is a successful run with a bad outcome. */
  def verifyExit(reports: List[VerifyReport]): ExitCode =
    if reports.exists(_.mismatches.nonEmpty) then ExitCode(4) else ExitCode(0)
