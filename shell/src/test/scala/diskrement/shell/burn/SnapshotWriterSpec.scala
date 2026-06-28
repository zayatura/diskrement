package diskrement.shell.burn

import diskrement.core._
import diskrement.shell.model.ArchivePointer
import zio._
import zio.json._
import zio.test._
import java.nio.file.{Files, Path}
import java.time.Instant

object SnapshotWriterSpec extends ZIOSpecDefault:
  private val ts = Instant.parse("2026-06-01T00:00:00Z")

  private def deleteRecursive(dir: Path): Unit =
    val s = Files.walk(dir)
    try s.sorted(java.util.Comparator.reverseOrder()).forEach(p => Files.deleteIfExists(p))
    finally s.close()

  def spec = suite("SnapshotWriter")(
    test("writes the four /.diskrement files with correct content"):
      ZIO.scoped {
        for
          tmp <- ZIO.acquireRelease(ZIO.attempt(Files.createTempDirectory("snapwriter")))(
                   d => ZIO.attempt(deleteRecursive(d)).orDie)
          catSrc = tmp.resolve("src-catalog.sqlite")
          _   <- ZIO.attempt(Files.write(catSrc, Array[Byte](1, 2, 3, 4)))
          ddir = tmp.resolve("disc/.diskrement")
          meta = DiscMetadata(
                   identity = DiscIdentity("photos", 1, 1, ts),
                   manifest = SessionManifest("photos", 1, 1,
                     added = List(ManifestEntry(RelPath("a"), ContentHash("h1"), 10)),
                     modified = Nil, deleted = Nil),
                   catalogSnapshot = catSrc,
                   pointers = List(ArchivePointer(
                     RelPath("b"), ContentHash("h0"), 5, 1, 1, "/data/b", ManifestChangeKind.ADDED)))
          _   <- ZIO.serviceWithZIO[SnapshotWriter](_.write(ddir, meta))
          pjson  <- ZIO.attempt(Files.readString(ddir.resolve("project.json")))
          mjson  <- ZIO.attempt(Files.readString(ddir.resolve("manifest.json")))
          ptjson <- ZIO.attempt(Files.readString(ddir.resolve("pointers.json")))
          catB   <- ZIO.attempt(Files.readAllBytes(ddir.resolve("catalog.sqlite")).toList)
        yield assertTrue(
          pjson.fromJson[ProjectJson] == Right(ProjectJson("photos", 1, 1, "2026-06-01T00:00:00Z")),
          mjson.fromJson[ManifestJson] == Right(ManifestJson.from(meta.manifest)),
          ptjson.fromJson[PointersJson] == Right(PointersJson.from(meta.pointers)),
          catB == List[Byte](1, 2, 3, 4))
      }
  ).provideLayer(SnapshotWriter.live)
