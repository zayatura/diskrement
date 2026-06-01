package diskrement.core

import org.scalacheck.Prop.forAll

class GeneratorsTest extends munit.ScalaCheckSuite:
  property("generated scanned-file lists have unique relPaths"):
    forAll(Generators.genScannedFiles): files =>
      val paths = files.map(_.relPath)
      paths.distinct.size == paths.size

  property("generated file sizes are positive"):
    forAll(Generators.genScannedFile): f =>
      f.sizeBytes > 0L
