package diskrement.core

import java.time.Instant

class ModelTest extends munit.FunSuite:
  test("RelPath orders lexicographically by value"):
    val sorted = List(RelPath("b/a"), RelPath("a/z"), RelPath("a/a"))
      .sorted
      .map(_.value)
    assertEquals(sorted, List("a/a", "a/z", "b/a"))

  test("CatalogView.empty has no current entries"):
    assertEquals(CatalogView.empty.current.size, 0)

  test("CatalogView lookups are by RelPath"):
    val entry = CatalogEntry(RelPath("x/y"), ContentHash("ab"), 10L)
    val view = CatalogView(Map(entry.relPath -> entry))
    assertEquals(view.current.get(RelPath("x/y")), Some(entry))

  test("ScannedFile carries path, hash, size, mtime"):
    val f = ScannedFile(RelPath("p"), ContentHash("cd"), 42L, Instant.EPOCH)
    assertEquals(f.sizeBytes, 42L)
    assertEquals(f.contentHash, ContentHash("cd"))
