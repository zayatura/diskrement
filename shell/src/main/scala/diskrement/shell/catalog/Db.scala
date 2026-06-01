package diskrement.shell.catalog

import zio._
import zio.jdbc._
import java.nio.file.Path

/** Filesystem location of a project's SQLite catalog. */
final case class DbPath(value: Path)

object Db:
  /** Wrap a connection so that getClientInfo() never returns null.
    * zio-jdbc 0.1.2's ZConnection$Restorable calls getClientInfo() on construction
    * and passes the result to Properties.forEach — which NPEs if the result is null.
    * SQLite JDBC returns null from getClientInfo(), so we intercept that one method. */
  private def wrapConn(conn: java.sql.Connection): java.sql.Connection =
    java.lang.reflect.Proxy
      .newProxyInstance(
        classOf[java.sql.Connection].getClassLoader,
        Array(classOf[java.sql.Connection]),
        (_, method, args) =>
          if method.getName == "getClientInfo" && (args == null || args.isEmpty) then new java.util.Properties()
          else if args == null then method.invoke(conn)
          else method.invoke(conn, args*)
      )
      .asInstanceOf[java.sql.Connection]

  /** Acquire one SQLite connection with the project's pragmas applied.
    * `foreign_keys` is per-connection in SQLite, so it is set on every connection. */
  private def acquireConn(path: Path): Task[java.sql.Connection] =
    ZIO.attempt {
      val raw = java.sql.DriverManager.getConnection(s"jdbc:sqlite:${path.toString}")
      try
        val st = raw.createStatement()
        try
          st.execute("PRAGMA journal_mode=WAL")
          st.execute("PRAGMA foreign_keys=ON")
          st.execute("PRAGMA synchronous=FULL")
        finally st.close()
        wrapConn(raw)
      catch
        case e: Throwable =>
          raw.close()
          throw e
    }

  /** A single-connection pool: SQLite has one writer; serializing avoids lock churn. */
  val poolLayer: ZLayer[DbPath, Throwable, ZConnectionPool] =
    ZLayer.scoped {
      for
        dbPath <- ZIO.service[DbPath]
        cfg     = ZConnectionPoolConfig.default.copy(minConnections = 1, maxConnections = 1)
        zenv   <- ZConnectionPool.make(acquireConn(dbPath.value)).build
                    .provideSome[Scope](ZLayer.succeed(cfg))
      yield zenv.get[ZConnectionPool]
    }
