package diskrement.core

import org.scalacheck.Prop.forAll

class DiffPropertyTest extends munit.ScalaCheckSuite:

  property("every scanned file is classified exactly once"):
    forAll(Generators.genScannedFiles): files =>
      val cs = Diff.diff(CatalogView.empty, files)
      val classified =
        cs.added.size + cs.modified.size + cs.unchanged.size
      // against an empty catalog, nothing is deleted, all are added
      classified == files.size && cs.deleted.isEmpty

  property("diff is idempotent: recording a scan yields an empty diff next time"):
    forAll(Generators.genScannedFiles): files =>
      val catalog = CatalogView.fromScanned(files)
      val cs = Diff.diff(catalog, files)
      cs.isEmpty && cs.unchanged.size == files.size
