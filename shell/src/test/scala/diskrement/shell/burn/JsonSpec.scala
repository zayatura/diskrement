package diskrement.shell.burn

import diskrement.core._
import diskrement.shell.model.ArchivePointer
import zio.json._
import zio.test._
import java.time.Instant

object JsonSpec extends ZIOSpecDefault:
  private val ts = Instant.parse("2026-06-01T00:00:00Z")

  def spec = suite("burn JSON DTOs")(
    test("ProjectJson maps from DiscIdentity and round-trips"):
      val pj = ProjectJson.from(DiscIdentity("photos", 2, 1, ts))
      assertTrue(
        pj == ProjectJson("photos", 2, 1, "2026-06-01T00:00:00Z"),
        pj.toJson.fromJson[ProjectJson] == Right(pj)),
    test("ManifestJson maps from SessionManifest and round-trips"):
      val m = SessionManifest("photos", 1, 1,
        added = List(ManifestEntry(RelPath("a"), ContentHash("h1"), 10)),
        modified = List(ManifestEntry(RelPath("b"), ContentHash("h2"), 20)),
        deleted = List(RelPath("c")))
      val mj = ManifestJson.from(m)
      assertTrue(
        mj.added == List(ManifestEntryJson("a", "h1", 10)),
        mj.modified == List(ManifestEntryJson("b", "h2", 20)),
        mj.deleted == List("c"),
        mj.toJson.fromJson[ManifestJson] == Right(mj)),
    test("PointersJson maps from ArchivePointers and round-trips"):
      val pj = PointersJson.from(List(
        ArchivePointer(RelPath("a"), ContentHash("h0"), 10, 1, 1, "/data/a", ManifestChangeKind.MODIFIED)))
      assertTrue(
        pj.pointers == List(ArchivePointerJson("a", "h0", 10, 1, 1, "/data/a", "MODIFIED")),
        pj.toJson.fromJson[PointersJson] == Right(pj))
  )
