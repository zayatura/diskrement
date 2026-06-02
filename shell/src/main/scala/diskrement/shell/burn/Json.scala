package diskrement.shell.burn

import diskrement.core.{ManifestEntry, SessionManifest}
import diskrement.shell.model.ArchivePointer
import zio.json._

final case class ProjectJson(
    projectName: String,
    discOrdinal: Int,
    discCount: Int,
    sessionOrdinal: Int,
    createdAt: String
)
object ProjectJson:
  given JsonCodec[ProjectJson] = DeriveJsonCodec.gen[ProjectJson]
  def from(id: DiscIdentity): ProjectJson =
    ProjectJson(id.projectName, id.discOrdinal, id.discCount, id.sessionOrdinal, id.createdAt.toString)

final case class ManifestEntryJson(relPath: String, contentHash: String, sizeBytes: Long)
object ManifestEntryJson:
  given JsonCodec[ManifestEntryJson] = DeriveJsonCodec.gen[ManifestEntryJson]
  def from(e: ManifestEntry): ManifestEntryJson =
    ManifestEntryJson(e.relPath.value, e.contentHash.value, e.sizeBytes)

final case class ManifestJson(
    projectName: String,
    discOrdinal: Int,
    sessionOrdinal: Int,
    added: List[ManifestEntryJson],
    modified: List[ManifestEntryJson],
    deleted: List[String]
)
object ManifestJson:
  given JsonCodec[ManifestJson] = DeriveJsonCodec.gen[ManifestJson]
  def from(m: SessionManifest): ManifestJson =
    ManifestJson(
      m.projectName, m.discOrdinal, m.sessionOrdinal,
      m.added.map(ManifestEntryJson.from), m.modified.map(ManifestEntryJson.from),
      m.deleted.map(_.value))

final case class ArchivePointerJson(
    relPath: String,
    contentHash: String,
    sizeBytes: Long,
    discOrdinal: Int,
    sessionOrdinal: Int,
    discPath: String,
    kind: String
)
object ArchivePointerJson:
  given JsonCodec[ArchivePointerJson] = DeriveJsonCodec.gen[ArchivePointerJson]
  def from(p: ArchivePointer): ArchivePointerJson =
    ArchivePointerJson(p.relPath.value, p.contentHash.value, p.sizeBytes,
      p.discOrdinal, p.sessionOrdinal, p.discPath, p.kind.toString)

final case class PointersJson(pointers: List[ArchivePointerJson])
object PointersJson:
  given JsonCodec[PointersJson] = DeriveJsonCodec.gen[PointersJson]
  def from(ps: List[ArchivePointer]): PointersJson =
    PointersJson(ps.map(ArchivePointerJson.from))
