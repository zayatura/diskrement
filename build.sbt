ThisBuild / scalaVersion := "3.3.4"
ThisBuild / organization := "io.github.zayatura.diskrement"
ThisBuild / scalacOptions ++= Seq("-deprecation", "-feature", "-Wunused:all")

val zioVersion     = "2.1.19"
val zioJdbcVersion = "0.1.2"
val sqliteVersion  = "3.49.1.0"
val zioJsonVersion = "0.7.3"
val zioCliVersion  = "0.5.0"

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
    testFrameworks += new TestFramework("zio.test.sbt.ZTestFramework")
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
    Defaults.itSettings,
    IntegrationTest / fork := true,
    // Build the launcher before IT runs, and hand its path to the tests.
    IntegrationTest / javaOptions += {
      val stagedDir = (Universal / stage).value
      s"-Ddiskrement.bin=${(stagedDir / "bin" / "diskrement").getAbsolutePath}"
    }
  )

lazy val root = (project in file("."))
  .aggregate(core, shell, cli)
  .settings(name := "diskrement")
