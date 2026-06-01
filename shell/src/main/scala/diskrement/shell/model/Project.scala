package diskrement.shell.model

import diskrement.core.DiscType
import java.time.Instant

/** Primary key of a project row. */
final case class ProjectId(value: Long)

/** Fields needed to create a project (no id yet). */
final case class ProjectInit(
    name: String,
    sourceRoot: String,
    workDir: String,
    defaultDiscType: DiscType,
    marginPercent: Double,
    copies: Int,
    createdAt: Instant
)

/** A persisted project row. */
final case class Project(
    id: ProjectId,
    name: String,
    sourceRoot: String,
    workDir: String,
    defaultDiscType: DiscType,
    marginPercent: Double,
    copies: Int,
    createdAt: Instant
)
