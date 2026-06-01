package diskrement.shell.catalog

import diskrement.core.DiscType
import diskrement.shell.model._
import diskrement.shell.model.CatalogError._
import zio._
import zio.jdbc._
import java.time.Instant

private[catalog] final case class CatalogStoreLive(pool: ZConnectionPool) extends CatalogStore:

  /** Run a connection program as one transaction, mapping expected faults to
    * CatalogError and dying on anything unexpected. */
  private def tx[A](z: ZIO[ZConnection, Throwable, A]): IO[CatalogError, A] =
    transaction(z)
      .provideEnvironment(ZEnvironment(pool))
      .refineOrDie {
        case e: CatalogError                              => e
        case e: java.sql.SQLException if isConstraint(e)  => ConstraintViolation(e)
      }

  private def isConstraint(e: java.sql.SQLException): Boolean =
    Option(e.getMessage).exists(_.toLowerCase.contains("constraint"))

  private def isUnique(cause: Throwable): Boolean =
    Option(cause.getMessage).exists(_.toLowerCase.contains("unique"))

  /** Last auto-increment id on the current connection (valid within a transaction). */
  private val lastId: ZIO[ZConnection, Throwable, Long] =
    sql"select last_insert_rowid()".query[Long].selectOne
      .someOrFail(new IllegalStateException("last_insert_rowid() returned no row"))

  def createProject(p: ProjectInit): IO[CatalogError, ProjectId] =
    tx {
      sql"""insert into project
              (name, source_root, work_dir, default_disc_type, margin_percent, copies, created_at)
            values
              (${p.name}, ${p.sourceRoot}, ${p.workDir}, ${p.defaultDiscType.toString},
               ${p.marginPercent}, ${p.copies}, ${p.createdAt.toString})""".insert
        *> lastId.map(ProjectId(_))
    }.catchSome {
      case ConstraintViolation(c) if isUnique(c) => ZIO.fail(DuplicateProject(p.name))
    }

  def findProject(name: String): IO[CatalogError, Option[Project]] =
    tx {
      sql"""select id, name, source_root, work_dir, default_disc_type,
                   margin_percent, copies, created_at
            from project where name = $name"""
        .query[(Long, String, String, String, String, Double, Int, String)].selectOne
    }.map(_.map { case (id, n, sr, wd, dt, mp, cp, ca) =>
      Project(ProjectId(id), n, sr, wd, DiscType.valueOf(dt), mp, cp, Instant.parse(ca))
    })
