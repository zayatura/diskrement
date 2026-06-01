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

  /** A capacity comfortably larger than the largest single generated file,
    * so that "oversized" is a deliberate, separate concern. */
  val genNewDiscCap: Gen[Capacity] =
    Gen.choose(1_000_000_000L, 10_000_000_000L).map(Capacity.apply)

  /** Files guaranteed to fit on a disc of the given capacity. */
  def genFilesWithin(cap: Capacity): Gen[List[ScannedFile]] =
    for files <- genScannedFiles
    yield files.map { f =>
      val size = math.max(1L, f.sizeBytes % cap.usableBytes)
      f.copy(sizeBytes = size)
    }

  /** An optional pre-existing OPEN disc with the same usable capacity. */
  def genOptOpenDisc(cap: Capacity): Gen[List[DiscState]] =
    Gen.option(
      Gen.choose(0L, cap.usableBytes).map { used =>
        DiscState(ordinal = 1, status = DiscStatus.OPEN,
                  usableBytes = cap.usableBytes, usedBytes = used)
      }
    ).map(_.toList)

  /** A full planner input: capacity, optional open disc, and fitting changes. */
  val genPlannerInput: Gen[(ChangeSet, List[DiscState], Capacity)] =
    for
      cap   <- genNewDiscCap
      open  <- genOptOpenDisc(cap)
      files <- genFilesWithin(cap)
      // split files arbitrarily into added vs modified
      split <- Gen.choose(0, files.size)
    yield
      val (add, mod) = files.splitAt(split)
      (ChangeSet(add, mod, Nil, Nil), open, cap)
