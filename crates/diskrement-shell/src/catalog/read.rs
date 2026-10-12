//! The catalog's read path.
//!
//! Every query here is transcribed from `CatalogStoreLive`. They look
//! elaborate because each one encodes a domain rule in SQL rather than in Rust:
//! "the current version of a path" is the newest `change_record` for it, and
//! "the bytes still exist" is the absence of a `MARKED_DEAD` event on the disc
//! holding them. Rewriting one of these into something more readable is how a
//! porting defect gets in.
//!
//! Reads take `&self` and open no transaction: a single connection sees a
//! consistent database, and nothing here writes.

use std::collections::BTreeMap;
use std::str::FromStr;

use rusqlite::params;

use super::model::{
    ArchivePointer, DiscRecordedContents, FileFingerprint, FileHistoryEntry, ProjectId, Session,
    SessionState, SupersededVersion, VersionStatus,
};
use super::time;
use super::Catalog;
use crate::error::CatalogError;
use diskrement_core::{
    CatalogEntry, CatalogView, ContentHash, DiscKey, DiscState, DiscStatus, ManifestChangeKind,
    RelPath,
};

impl Catalog {
    /// The project's live files: for each path, its newest version, provided
    /// that version's newest change is not a tombstone and at least one copy of
    /// it sits on a disc that has not been marked dead. Each entry's
    /// `live_sets` names the copy sets holding such a copy; it is never empty,
    /// because a version with no live copy is left out.
    pub fn read_view(&self, project: ProjectId) -> Result<CatalogView, CatalogError> {
        let mut stmt = self
            .conn
            .prepare(
                "select rel_path, content_hash, size_bytes, live_sets from (
                   select fv.rel_path, fv.content_hash, fv.size_bytes,
                          (select group_concat(distinct d.copy_set)
                             from placement p
                             join session s on s.id = p.session_id
                             join disc d    on d.id = s.disc_id
                            where p.file_version_id = fv.id
                              and not exists (select 1 from disc_event e
                                              where e.disc_id = d.id
                                                and e.kind = 'MARKED_DEAD')) as live_sets
                   from (
                     select cr.id as cr_id, cr.kind as kind, cr.file_version_id as fvid,
                            fv.rel_path as rp,
                            row_number() over (partition by fv.rel_path order by cr.id desc) as rn
                     from change_record cr
                     join file_version fv on fv.id = cr.file_version_id
                     where fv.project_id = ?1
                   ) l
                   join file_version fv on fv.id = l.fvid
                   where l.rn = 1 and l.kind <> 'DELETED'
                 ) where live_sets is not null",
            )
            .map_err(CatalogError::from_sqlite)?;

        let rows = stmt
            .query_map(params![project.0], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(CatalogError::from_sqlite)?;

        let mut current = BTreeMap::new();
        for row in rows {
            let (path, hash, size, sets) = row.map_err(CatalogError::from_sqlite)?;
            let live_sets = sets
                .split(',')
                .map(|s| {
                    s.parse::<u32>().map_err(|e| {
                        CatalogError::ConstraintViolation(format!("bad copy set {s:?}: {e}"))
                    })
                })
                .collect::<Result<_, _>>()?;
            let rel_path = RelPath(path);
            current.insert(
                rel_path.clone(),
                CatalogEntry {
                    rel_path,
                    content_hash: ContentHash(hash),
                    size_bytes: size as u64,
                    live_sets,
                },
            );
        }
        Ok(CatalogView { current })
    }

    /// Size, mtime and hash of every live version, for the scanner's
    /// "unchanged?" check.
    ///
    /// Deliberately *not* filtered by dead disc, unlike `read_view`: a dead
    /// disc does not make the source file change, and re-hashing it would be
    /// wasted work. `diff::tombstones` (the plan's deletions and the scan
    /// count) and burn's recorded-tombstone filter also depend on this: they
    /// need every path whose latest change is not `DELETED`, including a path
    /// whose every copy died. Do not "fix" this to filter dead discs.
    pub fn fingerprints(
        &self,
        project: ProjectId,
    ) -> Result<BTreeMap<RelPath, FileFingerprint>, CatalogError> {
        let mut stmt = self
            .conn
            .prepare(
                "select fv.rel_path, fv.size_bytes, fv.mtime, fv.content_hash
                 from (
                   select cr.kind as kind, cr.file_version_id as fvid,
                          row_number() over (partition by fv.rel_path order by cr.id desc) as rn
                   from change_record cr
                   join file_version fv on fv.id = cr.file_version_id
                   where fv.project_id = ?1
                 ) l
                 join file_version fv on fv.id = l.fvid
                 where l.rn = 1 and l.kind <> 'DELETED'",
            )
            .map_err(CatalogError::from_sqlite)?;

        let rows = stmt
            .query_map(params![project.0], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(CatalogError::from_sqlite)?;

        let mut out = BTreeMap::new();
        for row in rows {
            let (path, size, mtime, hash) = row.map_err(CatalogError::from_sqlite)?;
            out.insert(
                RelPath(path),
                FileFingerprint {
                    size_bytes: size as u64,
                    mtime: time::parse(&mtime)?,
                    content_hash: ContentHash(hash),
                },
            );
        }
        Ok(out)
    }

    /// Sessions in one lifecycle state, ordered by disc (ordinal, then copy
    /// set) then session ordinal.
    pub fn sessions_by_state(
        &self,
        project: ProjectId,
        state: SessionState,
    ) -> Result<Vec<Session>, CatalogError> {
        let mut stmt = self
            .conn
            .prepare(
                "select d.ordinal, d.copy_set, s.ordinal, s.state, s.burned_at, s.manifest_hash
                 from session s
                 join disc d on d.id = s.disc_id
                 where d.project_id = ?1 and s.state = ?2
                 order by d.ordinal, d.copy_set, s.ordinal",
            )
            .map_err(CatalogError::from_sqlite)?;

        let rows = stmt
            .query_map(params![project.0, state.as_str()], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            })
            .map_err(CatalogError::from_sqlite)?;

        let mut out = Vec::new();
        for row in rows {
            let (disc_ordinal, copy_set, ordinal, state, burned_at, manifest_hash) =
                row.map_err(CatalogError::from_sqlite)?;
            out.push(Session {
                disc: DiscKey::new(disc_ordinal as u32, copy_set as u32),
                ordinal: ordinal as u32,
                state: SessionState::from_str(&state).map_err(CatalogError::ConstraintViolation)?,
                burned_at: burned_at.as_deref().map(time::parse).transpose()?,
                manifest_hash,
            });
        }
        Ok(out)
    }

    /// Every disc with its derived status and used bytes.
    ///
    /// `DEAD` is computed here, in the case expression, from `disc_event` —
    /// the `disc.status` column itself only ever holds OPEN or FINALIZED.
    pub fn read_disc_states(&self, project: ProjectId) -> Result<Vec<DiscState>, CatalogError> {
        let mut stmt = self
            .conn
            .prepare(
                "select d.ordinal, d.copy_set,
                        case when exists (select 1 from disc_event e
                                          where e.disc_id = d.id and e.kind = 'MARKED_DEAD')
                             then 'DEAD' else d.status end as status,
                        d.usable_bytes,
                        coalesce((select sum(fv.size_bytes)
                                  from placement p
                                  join session s on s.id = p.session_id
                                  join file_version fv on fv.id = p.file_version_id
                                  where s.disc_id = d.id), 0) as used
                 from disc d
                 where d.project_id = ?1
                 order by d.ordinal, d.copy_set",
            )
            .map_err(CatalogError::from_sqlite)?;

        let rows = stmt
            .query_map(params![project.0], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })
            .map_err(CatalogError::from_sqlite)?;

        let mut out = Vec::new();
        for row in rows {
            let (ordinal, copy_set, status, usable, used) =
                row.map_err(CatalogError::from_sqlite)?;
            out.push(DiscState {
                ordinal: ordinal as u32,
                copy_set: copy_set as u32,
                status: DiscStatus::from_str(&status).map_err(CatalogError::ConstraintViolation)?,
                usable_bytes: usable as u64,
                used_bytes: used as u64,
            });
        }
        Ok(out)
    }

    /// Where the bytes of every non-current version physically live.
    ///
    /// This is what gets baked into each burned disc's `pointers.json`, which
    /// is why the known gap matters: there is no dead flag here, so a pointer
    /// to a dead disc is durable misinformation on already-written media.
    /// Ported as-is (spec §1) — the fix is a `lost` column, not a filter.
    pub fn archive_pointers(
        &self,
        project: ProjectId,
    ) -> Result<Vec<ArchivePointer>, CatalogError> {
        let mut stmt = self
            .conn
            .prepare(
                "select fv.rel_path, fv.content_hash, fv.size_bytes,
                        d.ordinal, d.copy_set, s.ordinal, p.disc_path, fvk.kind
                 from file_version fv
                 join placement p on p.file_version_id = fv.id
                 join session s   on s.id = p.session_id
                 join disc d      on d.id = s.disc_id
                 join (
                   select cr.file_version_id as fvid, cr.kind as kind,
                          row_number() over (partition by cr.file_version_id
                                             order by cr.id desc) as rn
                   from change_record cr
                 ) fvk on fvk.fvid = fv.id and fvk.rn = 1
                 where fv.project_id = ?1
                   and fv.id not in (
                     select fvid from (
                       select cr.file_version_id as fvid, cr.kind as kind,
                              row_number() over (partition by fv2.rel_path
                                                 order by cr.id desc) as rn
                       from change_record cr
                       join file_version fv2 on fv2.id = cr.file_version_id
                       where fv2.project_id = ?1
                     ) cur
                     where cur.rn = 1 and cur.kind <> 'DELETED'
                   )
                 order by fv.rel_path, d.ordinal, d.copy_set, s.ordinal",
            )
            .map_err(CatalogError::from_sqlite)?;

        let rows = stmt
            .query_map(params![project.0], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                ))
            })
            .map_err(CatalogError::from_sqlite)?;

        let mut out = Vec::new();
        for row in rows {
            let (path, hash, size, disc_ordinal, copy_set, session_ordinal, disc_path, kind) =
                row.map_err(CatalogError::from_sqlite)?;
            out.push(ArchivePointer {
                rel_path: RelPath(path),
                content_hash: ContentHash(hash),
                size_bytes: size as u64,
                disc_ordinal: disc_ordinal as u32,
                copy_set: copy_set as u32,
                session_ordinal: session_ordinal as u32,
                disc_path,
                kind: ManifestChangeKind::from_str(&kind)
                    .map_err(CatalogError::ConstraintViolation)?,
            });
        }
        Ok(out)
    }

    /// Every version of one path, oldest first, with where it lives and how it
    /// stands now.
    ///
    /// The status precedence is the rule to preserve: current wins over
    /// dead-disc. A path's current version whose only copy sits on a dead disc
    /// still reads `Current`, because the source file still exists and the next
    /// burn re-writes it. Only a *non-current* version on a dead disc is
    /// `Lost` — which is why `Lost` is per physical copy, not per content hash.
    ///
    /// One entry per copy (placement), grouped by version, twins together
    /// (1A, 1B, 2A). `on_dead_disc` says which copies are gone even where the
    /// status reads `Current`.
    pub fn file_history(
        &self,
        project: ProjectId,
        rel_path: &RelPath,
    ) -> Result<Vec<FileHistoryEntry>, CatalogError> {
        let mut stmt = self
            .conn
            .prepare(
                "select fv.content_hash, fv.size_bytes, d.ordinal, d.copy_set, s.ordinal, fvk.kind,
                        case when fv.id = cur.fvid then 1 else 0 end as is_current,
                        case when exists (select 1 from disc_event e
                                          where e.disc_id = d.id and e.kind = 'MARKED_DEAD')
                             then 1 else 0 end as disc_dead,
                        dense_rank() over (order by fv.id) as version
                 from file_version fv
                 join placement p on p.file_version_id = fv.id
                 join session s   on s.id = p.session_id
                 join disc d      on d.id = s.disc_id
                 join (
                   select cr.file_version_id as fvid, cr.kind as kind,
                          row_number() over (partition by cr.file_version_id
                                             order by cr.id desc) as rn
                   from change_record cr
                 ) fvk on fvk.fvid = fv.id and fvk.rn = 1
                 left join (
                   select fvid from (
                     select cr.file_version_id as fvid, cr.kind as kind,
                            row_number() over (partition by fv2.rel_path
                                               order by cr.id desc) as rn
                     from change_record cr
                     join file_version fv2 on fv2.id = cr.file_version_id
                     where fv2.project_id = ?1 and fv2.rel_path = ?2
                   ) c where c.rn = 1 and c.kind <> 'DELETED'
                 ) cur on cur.fvid = fv.id
                 where fv.project_id = ?1 and fv.rel_path = ?2
                 order by fv.id, d.ordinal, d.copy_set, s.ordinal",
            )
            .map_err(CatalogError::from_sqlite)?;

        let rows = stmt
            .query_map(params![project.0, rel_path.as_str()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                ))
            })
            .map_err(CatalogError::from_sqlite)?;

        let mut out = Vec::new();
        for row in rows {
            let (
                hash,
                size,
                disc_ordinal,
                copy_set,
                session_ordinal,
                kind,
                is_current,
                disc_dead,
                version,
            ) = row.map_err(CatalogError::from_sqlite)?;
            let status = if is_current == 1 {
                VersionStatus::Current
            } else if disc_dead == 1 {
                VersionStatus::Lost
            } else if kind == "DELETED" {
                VersionStatus::Deleted
            } else {
                VersionStatus::Superseded
            };
            out.push(FileHistoryEntry {
                version: version as u32,
                content_hash: ContentHash(hash),
                size_bytes: size as u64,
                disc: DiscKey::new(disc_ordinal as u32, copy_set as u32),
                session_ordinal: session_ordinal as u32,
                status,
                on_dead_disc: disc_dead == 1,
            });
        }
        Ok(out)
    }

    /// What the burner staged on `disc`'s LATEST session — used by
    /// verify to check a disc against its own recorded contents rather than
    /// against the current view.
    ///
    /// A seal session (`plan --finalize`) places no files, so this is
    /// legitimately empty for a disc whose last session was a seal.
    pub fn placements_for_disc(
        &self,
        project: ProjectId,
        disc: DiscKey,
    ) -> Result<BTreeMap<RelPath, ContentHash>, CatalogError> {
        let mut stmt = self
            .conn
            .prepare(
                "select fv.rel_path, fv.content_hash
                 from placement p
                 join session s   on s.id = p.session_id
                 join disc d      on d.id = s.disc_id
                 join file_version fv on fv.id = p.file_version_id
                 where d.project_id = ?1 and d.ordinal = ?2 and d.copy_set = ?3
                   and s.ordinal = (select max(s2.ordinal) from session s2
                                    where s2.disc_id = d.id)",
            )
            .map_err(CatalogError::from_sqlite)?;

        let rows = stmt
            .query_map(
                params![project.0, disc.ordinal as i64, disc.copy_set as i64],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .map_err(CatalogError::from_sqlite)?;

        let mut out = BTreeMap::new();
        for row in rows {
            let (path, hash) = row.map_err(CatalogError::from_sqlite)?;
            out.insert(RelPath(path), ContentHash(hash));
        }
        Ok(out)
    }

    /// Everything recorded as physically on one disc: the merged view a mount
    /// shows, plus the versions later sessions of that same disc hid.
    ///
    /// Powers the two-tier read-back verify — the visible set is checked by
    /// mounting, the superseded set by extracting the session that wrote it.
    pub fn disc_recorded_contents(
        &self,
        project: ProjectId,
        disc: DiscKey,
    ) -> Result<DiscRecordedContents, CatalogError> {
        let mut stmt = self
            .conn
            .prepare(
                "select rp, ch, sess_ord, rn, sz from (
                   select fv.rel_path as rp, fv.content_hash as ch, s.ordinal as sess_ord,
                          fv.size_bytes as sz,
                          row_number() over (partition by fv.rel_path
                                             order by s.ordinal desc, p.id desc) as rn
                   from placement p
                   join session s on s.id = p.session_id
                   join disc d on d.id = s.disc_id
                   join file_version fv on fv.id = p.file_version_id
                   where d.project_id = ?1 and d.ordinal = ?2 and d.copy_set = ?3
                 ) order by rp, rn",
            )
            .map_err(CatalogError::from_sqlite)?;

        let rows = stmt
            .query_map(
                params![project.0, disc.ordinal as i64, disc.copy_set as i64],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )
            .map_err(CatalogError::from_sqlite)?;

        let mut visible = BTreeMap::new();
        let mut superseded = Vec::new();
        for row in rows {
            let (path, hash, session_ordinal, rank, size) =
                row.map_err(CatalogError::from_sqlite)?;
            if rank == 1 {
                visible.insert(RelPath(path), ContentHash(hash));
            } else {
                superseded.push(SupersededVersion {
                    session_ordinal: session_ordinal as u32,
                    rel_path: RelPath(path),
                    content_hash: ContentHash(hash),
                    size_bytes: size as u64,
                });
            }
        }
        Ok(DiscRecordedContents {
            visible,
            superseded,
        })
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use diskrement_core::DiscKey;
    use diskrement_core::{ContentHash, DiscState, DiscStatus, ManifestChangeKind, RelPath};

    use crate::catalog::model::{
        ArchivePointer, FileFingerprint, FileHistoryEntry, SessionState, VersionStatus,
    };
    use crate::catalog::testutil::{count, fw, fw_at, project_init, temp_catalog, update};

    #[test]
    fn read_view_adds_supersedes_and_forgets_deleted_paths() {
        let (_dir, mut cat) = temp_catalog();
        let pid = cat.create_project(&project_init("p")).expect("project");
        cat.apply_update(&update(
            pid,
            1,
            1,
            vec![fw("a", "a1", 10), fw("b", "b1", 20), fw("c", "c1", 30)],
            vec![],
            vec![],
        ))
        .expect("first session");
        cat.apply_update(&update(
            pid,
            1,
            2,
            vec![],
            vec![fw("b", "b2", 25)],
            vec![RelPath("c".to_string())],
        ))
        .expect("second session");

        let view = cat.read_view(pid).expect("view reads");
        let paths: Vec<&str> = view.current.keys().map(RelPath::as_str).collect();
        assert_eq!(paths, vec!["a", "b"]);
        assert_eq!(
            view.current[&RelPath("a".to_string())].content_hash,
            ContentHash("a1".to_string())
        );
        assert_eq!(
            view.current[&RelPath("b".to_string())].content_hash,
            ContentHash("b2".to_string())
        );
        // The deleted path's version row is still there: the catalog is
        // append-only, and the tombstone is a change record, not a deletion.
        assert!(count(&cat, "file_version") >= 4);
    }

    #[test]
    fn fingerprints_return_the_current_version_with_its_mtime() {
        let (_dir, mut cat) = temp_catalog();
        let pid = cat.create_project(&project_init("p")).expect("project");
        let mt_a = Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
        let mt_b1 = Utc.with_ymd_and_hms(2026, 5, 2, 0, 0, 0).unwrap();
        let mt_b2 = Utc.with_ymd_and_hms(2026, 5, 3, 0, 0, 0).unwrap();
        let mt_c = Utc.with_ymd_and_hms(2026, 5, 4, 0, 0, 0).unwrap();

        cat.apply_update(&update(
            pid,
            1,
            1,
            vec![
                fw_at("a", "a1", 10, mt_a),
                fw_at("b", "b1", 20, mt_b1),
                fw_at("c", "c1", 30, mt_c),
            ],
            vec![],
            vec![],
        ))
        .expect("first session");
        cat.apply_update(&update(
            pid,
            1,
            2,
            vec![],
            vec![fw_at("b", "b2", 25, mt_b2)],
            vec![RelPath("c".to_string())],
        ))
        .expect("second session");

        let fps = cat.fingerprints(pid).expect("fingerprints read");
        let paths: Vec<&str> = fps.keys().map(RelPath::as_str).collect();
        assert_eq!(paths, vec!["a", "b"]);
        assert_eq!(
            fps[&RelPath("a".to_string())],
            FileFingerprint {
                size_bytes: 10,
                mtime: mt_a,
                content_hash: ContentHash("a1".to_string()),
            }
        );
        assert_eq!(
            fps[&RelPath("b".to_string())],
            FileFingerprint {
                size_bytes: 25,
                mtime: mt_b2,
                content_hash: ContentHash("b2".to_string()),
            }
        );
    }

    #[test]
    fn apply_update_records_sessions_as_burned_and_never_planned() {
        let (_dir, mut cat) = temp_catalog();
        let pid = cat.create_project(&project_init("p")).expect("project");
        cat.apply_update(&update(pid, 1, 1, vec![fw("a", "h", 10)], vec![], vec![]))
            .expect("burn");

        let burned = cat
            .sessions_by_state(pid, SessionState::Burned)
            .expect("query succeeds");
        assert_eq!(burned.len(), 1);
        assert_eq!(burned[0].disc, DiscKey::new(1, 0));
        assert_eq!(burned[0].ordinal, 1);
        assert_eq!(burned[0].state, SessionState::Burned);
        assert_eq!(burned[0].manifest_hash.as_deref(), Some("m1-1"));

        assert!(cat
            .sessions_by_state(pid, SessionState::Planned)
            .expect("query succeeds")
            .is_empty());
    }

    #[test]
    fn read_disc_states_derives_used_bytes_from_placements() {
        let (_dir, mut cat) = temp_catalog();
        let pid = cat.create_project(&project_init("p")).expect("project");
        cat.apply_update(&update(
            pid,
            1,
            1,
            vec![fw("a", "h-a", 10), fw("b", "h-b", 20)],
            vec![],
            vec![],
        ))
        .expect("disc 1");
        cat.apply_update(&update(pid, 2, 1, vec![fw("c", "h-c", 10)], vec![], vec![]))
            .expect("disc 2");

        let states = cat.read_disc_states(pid).expect("states read");
        assert_eq!(
            states,
            vec![
                DiscState {
                    ordinal: 1,
                    copy_set: 0,
                    status: DiscStatus::Open,
                    usable_bytes: 1_000_000,
                    used_bytes: 30,
                },
                DiscState {
                    ordinal: 2,
                    copy_set: 0,
                    status: DiscStatus::Open,
                    usable_bytes: 1_000_000,
                    used_bytes: 10,
                },
            ]
        );
    }

    #[test]
    fn archive_pointers_locate_superseded_and_deleted_versions_only() {
        let (_dir, mut cat) = temp_catalog();
        let pid = cat.create_project(&project_init("p")).expect("project");
        cat.apply_update(&update(
            pid,
            1,
            1,
            vec![fw("a", "a1", 10), fw("b", "b1", 20), fw("c", "c1", 30)],
            vec![],
            vec![],
        ))
        .expect("first session");
        cat.apply_update(&update(
            pid,
            1,
            2,
            vec![],
            vec![fw("b", "b2", 25)],
            vec![RelPath("c".to_string())],
        ))
        .expect("second session");

        let pointers = cat.archive_pointers(pid).expect("pointers read");
        assert_eq!(
            pointers,
            vec![
                ArchivePointer {
                    rel_path: RelPath("b".to_string()),
                    content_hash: ContentHash("b1".to_string()),
                    size_bytes: 20,
                    disc_ordinal: 1,
                    copy_set: 0,
                    session_ordinal: 1,
                    disc_path: "/data/b".to_string(),
                    kind: ManifestChangeKind::Added,
                },
                ArchivePointer {
                    rel_path: RelPath("c".to_string()),
                    content_hash: ContentHash("c1".to_string()),
                    size_bytes: 30,
                    disc_ordinal: 1,
                    copy_set: 0,
                    session_ordinal: 1,
                    disc_path: "/data/c".to_string(),
                    kind: ManifestChangeKind::Deleted,
                },
            ]
        );
    }

    #[test]
    fn a_re_added_path_leaves_exactly_one_deleted_pointer_behind() {
        let (_dir, mut cat) = temp_catalog();
        let pid = cat.create_project(&project_init("p")).expect("project");
        cat.apply_update(&update(pid, 1, 1, vec![fw("x", "x1", 10)], vec![], vec![]))
            .expect("add");
        cat.apply_update(&update(
            pid,
            1,
            2,
            vec![],
            vec![],
            vec![RelPath("x".to_string())],
        ))
        .expect("delete");
        cat.apply_update(&update(pid, 1, 3, vec![fw("x", "x2", 15)], vec![], vec![]))
            .expect("re-add");

        let pointers = cat.archive_pointers(pid).expect("pointers read");
        assert_eq!(pointers.len(), 1);
        assert_eq!(pointers[0].content_hash, ContentHash("x1".to_string()));
        assert_eq!(pointers[0].kind, ManifestChangeKind::Deleted);
    }

    #[test]
    fn file_history_orders_versions_oldest_first_with_their_status() {
        let (_dir, mut cat) = temp_catalog();
        let pid = cat.create_project(&project_init("p")).expect("project");
        cat.apply_update(&update(
            pid,
            1,
            1,
            vec![fw("a", "a1", 10), fw("b", "b1", 20)],
            vec![],
            vec![],
        ))
        .expect("first session");
        cat.apply_update(&update(pid, 1, 2, vec![], vec![fw("a", "a2", 15)], vec![]))
            .expect("modify a");
        cat.apply_update(&update(
            pid,
            1,
            3,
            vec![],
            vec![],
            vec![RelPath("b".to_string())],
        ))
        .expect("delete b");

        assert_eq!(
            cat.file_history(pid, &RelPath("a".to_string()))
                .expect("history reads"),
            vec![
                FileHistoryEntry {
                    version: 1,
                    content_hash: ContentHash("a1".to_string()),
                    size_bytes: 10,
                    disc: DiscKey::new(1, 0),
                    session_ordinal: 1,
                    status: VersionStatus::Superseded,
                    on_dead_disc: false,
                },
                FileHistoryEntry {
                    version: 2,
                    content_hash: ContentHash("a2".to_string()),
                    size_bytes: 15,
                    disc: DiscKey::new(1, 0),
                    session_ordinal: 2,
                    status: VersionStatus::Current,
                    on_dead_disc: false,
                },
            ]
        );
        assert_eq!(
            cat.file_history(pid, &RelPath("b".to_string()))
                .expect("history reads"),
            vec![FileHistoryEntry {
                version: 1,
                content_hash: ContentHash("b1".to_string()),
                size_bytes: 20,
                disc: DiscKey::new(1, 0),
                session_ordinal: 1,
                status: VersionStatus::Deleted,
                on_dead_disc: false,
            }]
        );
    }

    use crate::catalog::model::{DiscRecordedContents, SupersededVersion};
    use std::collections::BTreeMap;

    #[test]
    fn placements_for_disc_report_the_latest_sessions_files_only() {
        let (_dir, mut cat) = temp_catalog();
        let pid = cat.create_project(&project_init("p")).expect("project");
        cat.apply_update(&update(pid, 1, 1, vec![fw("a", "a1", 10)], vec![], vec![]))
            .expect("session 1");
        cat.apply_update(&update(pid, 1, 2, vec![fw("b", "b1", 20)], vec![], vec![]))
            .expect("session 2");

        let placements = cat
            .placements_for_disc(pid, DiscKey::new(1, 0))
            .expect("query succeeds");
        assert_eq!(
            placements,
            BTreeMap::from([(RelPath("b".to_string()), ContentHash("b1".to_string()))])
        );
    }

    #[test]
    fn disc_recorded_contents_hide_earlier_versions_but_still_list_them() {
        let (_dir, mut cat) = temp_catalog();
        let pid = cat.create_project(&project_init("p")).expect("project");
        cat.apply_update(&update(
            pid,
            1,
            1,
            vec![fw("a.txt", "aa", 10), fw("b.txt", "bb", 20)],
            vec![],
            vec![],
        ))
        .expect("session 1");
        cat.apply_update(&update(
            pid,
            1,
            2,
            vec![],
            vec![fw("a.txt", "a2", 12)],
            vec![],
        ))
        .expect("session 2");

        let recorded = cat
            .disc_recorded_contents(pid, DiscKey::new(1, 0))
            .expect("query succeeds");
        assert_eq!(
            recorded.visible,
            BTreeMap::from([
                (RelPath("a.txt".to_string()), ContentHash("a2".to_string())),
                (RelPath("b.txt".to_string()), ContentHash("bb".to_string())),
            ])
        );
        assert_eq!(
            recorded.superseded,
            vec![SupersededVersion {
                session_ordinal: 1,
                rel_path: RelPath("a.txt".to_string()),
                content_hash: ContentHash("aa".to_string()),
                size_bytes: 10,
            }]
        );
    }

    #[test]
    fn another_discs_versions_do_not_leak_into_recorded_contents() {
        let (_dir, mut cat) = temp_catalog();
        let pid = cat.create_project(&project_init("p")).expect("project");
        cat.apply_update(&update(
            pid,
            1,
            1,
            vec![fw("a.txt", "aa", 10)],
            vec![],
            vec![],
        ))
        .expect("disc 1");
        cat.apply_update(&update(
            pid,
            2,
            1,
            vec![fw("z.txt", "zz", 30)],
            vec![],
            vec![],
        ))
        .expect("disc 2");

        assert_eq!(
            cat.disc_recorded_contents(pid, DiscKey::new(1, 0))
                .expect("query succeeds"),
            DiscRecordedContents {
                visible: BTreeMap::from([(
                    RelPath("a.txt".to_string()),
                    ContentHash("aa".to_string())
                )]),
                superseded: vec![],
            }
        );
        assert_eq!(
            cat.disc_recorded_contents(pid, DiscKey::new(2, 0))
                .expect("query succeeds"),
            DiscRecordedContents {
                visible: BTreeMap::from([(
                    RelPath("z.txt".to_string()),
                    ContentHash("zz".to_string())
                )]),
                superseded: vec![],
            }
        );
    }

    /// One session on a disc in `copy_set`, its files placed as new versions.
    fn update_in(
        project: crate::catalog::model::ProjectId,
        ordinal: u32,
        copy_set: u32,
        session: u32,
        added: Vec<crate::catalog::model::FileWrite>,
    ) -> crate::catalog::model::CatalogUpdate {
        let mut u = update(project, ordinal, session, added, vec![], vec![]);
        u.discs[0].copy_set = copy_set;
        u
    }

    #[test]
    fn read_disc_states_report_each_discs_copy_set_ordered_by_ordinal_then_set() {
        let (_dir, mut cat) = temp_catalog();
        let p = cat.create_project(&project_init("p")).unwrap();
        cat.apply_update(&update_in(p, 1, 1, 1, vec![fw("b", "hb", 1)]))
            .unwrap();
        cat.apply_update(&update_in(p, 1, 0, 1, vec![fw("a", "ha", 1)]))
            .unwrap();
        let keys: Vec<DiscKey> = cat
            .read_disc_states(p)
            .unwrap()
            .iter()
            .map(DiscState::key)
            .collect();
        assert_eq!(keys, vec![DiscKey::new(1, 0), DiscKey::new(1, 1)]);
    }

    #[test]
    fn read_view_reports_the_sets_holding_a_live_copy() {
        let (_dir, mut cat) = temp_catalog();
        let p = cat.create_project(&project_init("p")).unwrap();
        cat.apply_update(&update_in(p, 1, 0, 1, vec![fw("a", "ha", 1)]))
            .unwrap();
        let view = cat.read_view(p).unwrap();
        assert_eq!(
            view.current[&RelPath("a".into())].live_sets,
            std::collections::BTreeSet::from([0])
        );
    }

    #[test]
    fn placements_for_disc_and_recorded_contents_are_per_copy_set() {
        let (_dir, mut cat) = temp_catalog();
        let p = cat.create_project(&project_init("p")).unwrap();
        cat.apply_update(&update_in(p, 1, 0, 1, vec![fw("a", "ha", 1)]))
            .unwrap();
        cat.apply_update(&update_in(p, 1, 1, 1, vec![fw("b", "hb", 1)]))
            .unwrap();
        let on_1b = cat.placements_for_disc(p, DiscKey::new(1, 1)).unwrap();
        assert_eq!(
            on_1b.keys().cloned().collect::<Vec<_>>(),
            vec![RelPath("b".into())]
        );
        let recorded = cat.disc_recorded_contents(p, DiscKey::new(1, 0)).unwrap();
        assert_eq!(
            recorded.visible.keys().cloned().collect::<Vec<_>>(),
            vec![RelPath("a".into())]
        );
    }
}
