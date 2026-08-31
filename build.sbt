ThisBuild / scalaVersion := "3.3.4"
ThisBuild / organization := "io.github.zayatura.diskrement"
ThisBuild / scalacOptions ++= Seq("-deprecation", "-feature", "-Wunused:all")

val zioVersion     = "2.1.19"
val zioJdbcVersion = "0.1.2"
val sqliteVersion  = "3.49.1.0"
val zioJsonVersion = "0.7.3"
val zioCliVersion  = "0.5.0"

/** Run SQLite-touching tests in their own JVM.
  *
  * `java.sql.DriverManager`'s registry is JVM-global and its ServiceLoader
  * auto-discovery runs exactly once per JVM, but sbt hands every unforked test
  * task a brand-new `LayeredClassLoader`. From the second test task onward in
  * one sbt session the registry therefore holds the *previous* run's
  * `org.sqlite.JDBC`, which `getConnection` rejects (it compares class identity
  * under the caller's loader). The rejected lookup does load — and so
  * self-register — this run's driver, but the in-flight calls iterate a
  * copy-on-write snapshot taken before that, so every concurrent first call
  * fails with "No suitable driver found" while later ones succeed. That is what
  * made FingerprintCacheSpec fail 4-of-7 in a warm sbt session and pass on a
  * fresh one. A forked JVM per test run makes the global registry match the
  * classloader again. Remove this and the flake returns. */
val forkedTests = Seq(Test / fork := true)

lazy val core = (project in file("core"))
  .settings(
    name := "diskrement-core",
    libraryDependencies ++= Seq(
      "org.scalameta"  %% "munit"            % "1.0.0"  % Test,
      "org.scalameta"  %% "munit-scalacheck" % "1.0.0"  % Test,
      "org.scalacheck" %% "scalacheck"       % "1.18.0" % Test
    )
  )

lazy val shell = (project in file("shell"))
  .dependsOn(core)
  .settings(
    name := "diskrement-shell",
    libraryDependencies ++= Seq(
      "dev.zio"    %% "zio"          % zioVersion,
      "dev.zio"    %% "zio-jdbc"     % zioJdbcVersion,
      "dev.zio"    %% "zio-json"     % zioJsonVersion,
      "org.xerial"  % "sqlite-jdbc"  % sqliteVersion,
      "dev.zio"    %% "zio-test"     % zioVersion % Test,
      "dev.zio"    %% "zio-test-sbt" % zioVersion % Test
    ),
    testFrameworks += new TestFramework("zio.test.sbt.ZTestFramework"),
    forkedTests
  )

lazy val cli = (project in file("cli"))
  .dependsOn(shell)
  .enablePlugins(JavaAppPackaging)
  .configs(IntegrationTest)
  .settings(
    name := "diskrement",
    executableScriptName := "diskrement",
    Compile / mainClass := Some("diskrement.cli.Main"),
    libraryDependencies ++= Seq(
      "dev.zio" %% "zio-cli"      % zioCliVersion,
      "dev.zio" %% "zio-test"     % zioVersion % Test,
      "dev.zio" %% "zio-test-sbt" % zioVersion % Test,
      "dev.zio" %% "zio-test"     % zioVersion % IntegrationTest,
      "dev.zio" %% "zio-test-sbt" % zioVersion % IntegrationTest
    ),
    testFrameworks += new TestFramework("zio.test.sbt.ZTestFramework"),
    forkedTests,
    Defaults.itSettings,
    IntegrationTest / fork := true,
    IntegrationTest / parallelExecution := false,
    // Build the launcher before IT runs, and hand its path to the tests.
    IntegrationTest / javaOptions += {
      val stagedDir = (Universal / stage).value
      s"-Ddiskrement.bin=${(stagedDir / "bin" / "diskrement").getAbsolutePath}"
    }
  )

lazy val root = (project in file("."))
  .aggregate(core, shell, cli)
  .settings(name := "diskrement")
