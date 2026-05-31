ThisBuild / scalaVersion := "3.3.4"
ThisBuild / organization := "io.github.zayatura.diskrement"
ThisBuild / scalacOptions ++= Seq("-deprecation", "-feature", "-Wunused:all")

lazy val core = (project in file("core"))
  .settings(
    name := "diskrement-core",
    libraryDependencies ++= Seq(
      "org.scalameta"  %% "munit"            % "1.0.0"  % Test,
      "org.scalameta"  %% "munit-scalacheck" % "1.0.0"  % Test,
      "org.scalacheck" %% "scalacheck"       % "1.18.0" % Test
    )
  )

lazy val root = (project in file("."))
  .aggregate(core)
  .settings(name := "diskrement")
