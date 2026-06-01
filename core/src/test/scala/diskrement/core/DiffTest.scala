package diskrement.core

import java.time.Instant

class DiffTest extends munit.FunSuite:
  private def sf(path: String, hash: String, size: Long = 1L): ScannedFile =
    ScannedFile(RelPath(path), ContentHash(hash), size, Instant.EPOCH)

  private def entry(path: String, hash: String, size: Long = 1L): CatalogEntry =
    CatalogEntry(RelPath(path), ContentHash(hash), size)

  private def viewOf(es: CatalogEntry*): CatalogView =
    CatalogView(es.map(e => e.relPath -> e).toMap)

  test("new path is classified as added"):
    val cs = Diff.diff(CatalogView.empty, List(sf("a", "h1")))
    assertEquals(cs.added.map(_.relPath), List(RelPath("a")))
    assert(cs.modified.isEmpty && cs.deleted.isEmpty && cs.unchanged.isEmpty)

  test("same path with different hash is modified"):
    val cs = Diff.diff(viewOf(entry("a", "old")), List(sf("a", "new")))
    assertEquals(cs.modified.map(_.contentHash), List(ContentHash("new")))
    assert(cs.added.isEmpty)

  test("same path and hash is unchanged"):
    val cs = Diff.diff(viewOf(entry("a", "h")), List(sf("a", "h")))
    assertEquals(cs.unchanged.map(_.relPath), List(RelPath("a")))
    assert(cs.added.isEmpty && cs.modified.isEmpty)

  test("catalog path absent from scan is deleted"):
    val cs = Diff.diff(viewOf(entry("a", "h")), List.empty)
    assertEquals(cs.deleted, List(RelPath("a")))

  test("outputs are sorted by relPath for determinism"):
    val cs = Diff.diff(CatalogView.empty, List(sf("b", "h"), sf("a", "h")))
    assertEquals(cs.added.map(_.relPath.value), List("a", "b"))

  test("mixed scenario classifies every file exactly once"):
    val catalog = viewOf(entry("keep", "h"), entry("edit", "old"), entry("rm", "h"))
    val scan = List(sf("keep", "h"), sf("edit", "new"), sf("add", "h"))
    val cs = Diff.diff(catalog, scan)
    assertEquals(cs.added.map(_.relPath.value), List("add"))
    assertEquals(cs.modified.map(_.relPath.value), List("edit"))
    assertEquals(cs.unchanged.map(_.relPath.value), List("keep"))
    assertEquals(cs.deleted.map(_.value), List("rm"))
