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

  /** A pre-existing disc chain: zero or more discs with ascending ordinals,
    * each FINALIZED except possibly the last, which may be OPEN. This mirrors
    * the catalog's auto-finalize invariant (only the newest disc stays open)
    * and exercises multi-disc catalogs and open discs at ordinals other than 1. */
  def genDiscStates(cap: Capacity): Gen[List[DiscState]] =
    for
      n        <- Gen.choose(0, 4)
      lastOpen <- Gen.oneOf(true, false)
      useds    <- Gen.listOfN(n, Gen.choose(0L, cap.usableBytes))
    yield useds.zipWithIndex.map { (used, idx) =>
      val isLast = idx == n - 1
      val status = if isLast && lastOpen then DiscStatus.OPEN else DiscStatus.FINALIZED
      DiscState(ordinal = idx + 1, status = status,
                usableBytes = cap.usableBytes, usedBytes = used)
    }

  /** A full planner input: capacity, a pre-existing disc chain, and fitting changes. */
  val genPlannerInput: Gen[(ChangeSet, List[DiscState], Capacity)] =
    for
      cap   <- genNewDiscCap
      discs <- genDiscStates(cap)
      files <- genFilesWithin(cap)
      // split files arbitrarily into added vs modified
      split <- Gen.choose(0, files.size)
    yield
      val (add, mod) = files.splitAt(split)
      (ChangeSet(add, mod, Nil, Nil), discs, cap)
