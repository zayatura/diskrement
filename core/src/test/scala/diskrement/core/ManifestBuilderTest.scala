package diskrement.core

import java.time.Instant

class ManifestBuilderTest extends munit.FunSuite:
  private def sf(path: String, size: Long): ScannedFile =
    ScannedFile(RelPath(path), ContentHash("h-" + path), size, Instant.EPOCH)

  test("manifest splits a disc's files into added vs modified using the changeset"):
    val added = List(sf("new1", 10))
    val modified = List(sf("edit1", 10))
    val cs = ChangeSet(added = added, modified = modified, deleted = Nil, unchanged = Nil)
    val disc = PlannedDisc(ordinal = 1, isNew = true, finalize = false,
                           files = added ++ modified, priorUsedBytes = 0L, usableBytes = 100L)
    val m = ManifestBuilder.build("photos", sessionOrdinal = 1, disc = disc,
                                  changes = cs, tombstones = List(RelPath("gone")))
    assertEquals(m.projectName, "photos")
    assertEquals(m.discOrdinal, 1)
    assertEquals(m.sessionOrdinal, 1)
    assertEquals(m.added.map(_.relPath.value), List("new1"))
    assertEquals(m.modified.map(_.relPath.value), List("edit1"))
    assertEquals(m.deleted, List(RelPath("gone")))

  test("manifest entry carries hash and size"):
    val f = sf("a", 42)
    val cs = ChangeSet(added = List(f), modified = Nil, deleted = Nil, unchanged = Nil)
    val disc = PlannedDisc(1, true, false, List(f), 0L, 100L)
    val m = ManifestBuilder.build("p", 1, disc, cs, Nil)
    assertEquals(m.added.head, ManifestEntry(RelPath("a"), ContentHash("h-a"), 42L))
