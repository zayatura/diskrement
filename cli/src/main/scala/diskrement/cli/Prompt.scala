package diskrement.cli

import zio.*

/** A [y/N] confirmation over the ZIO Console (testable with TestConsole). */
object Prompt:
  def confirm(message: String, assumeYes: Boolean): UIO[Boolean] =
    if assumeYes then ZIO.succeed(true)
    else
      (Console.print(s"$message [y/N] ") *> Console.readLine).either.map {
        case Right(line) =>
          val t = line.trim.toLowerCase
          t == "y" || t == "yes"
        case Left(_) => false // EOF or read error -> treat as "no"
      }
