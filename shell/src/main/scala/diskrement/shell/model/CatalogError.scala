package diskrement.shell.model

/** Expected, recoverable persistence failures. Extends Exception so it can flow
  * through zio-jdbc's `Throwable` error channel and be kept by `refineOrDie`. */
sealed abstract class CatalogError(message: String, cause: Throwable = null)
    extends Exception(message, cause)

object CatalogError:
  final case class DuplicateProject(name: String)
      extends CatalogError(s"project already exists: $name")
  final case class ProjectNotFound(name: String)
      extends CatalogError(s"project not found: $name")
  final case class IllegalStatusTransition(detail: String)
      extends CatalogError(detail)
  final case class MigrationFailed(cause: Throwable)
      extends CatalogError("migration failed", cause)
  final case class ConstraintViolation(cause: Throwable)
      extends CatalogError("constraint violation", cause)
