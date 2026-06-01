package diskrement.core

import java.time.Instant
import org.scalacheck.Gen

/** Shared generators for property tests over the functional core. */
object Generators:

  val genRelPath: Gen[RelPath] =
    for
      depth <- Gen.choose(1, 4)
      segs  <- Gen.listOfN(depth, Gen.identifier.suchThat(_.nonEmpty))
    yield RelPath(segs.mkString("/"))

  val genHash: Gen[ContentHash] =
    Gen.listOfN(8, Gen.hexChar).map(cs => ContentHash(cs.mkString))

  val genScannedFile: Gen[ScannedFile] =
    for
      p    <- genRelPath
      h    <- genHash
      size <- Gen.choose(1L, 5_000_000_000L)
    yield ScannedFile(p, h, size, Instant.EPOCH)

  /** A list of scanned files with unique relPaths (deduped, order arbitrary). */
  val genScannedFiles: Gen[List[ScannedFile]] =
    Gen.listOf(genScannedFile).map { fs =>
      fs.groupBy(_.relPath).values.map(_.head).toList
    }
