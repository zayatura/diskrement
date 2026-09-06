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

use super::model::{FileFingerprint, ProjectId, Session, SessionState};
use super::time;
use super::Catalog;
use crate::error::CatalogError;
use diskrement_core::{CatalogEntry, CatalogView, ContentHash, DiscState, DiscStatus, RelPath};

impl Catalog {
    /// The project's live files: for each path, its newest version, provided
    /// that version's newest change is not a tombstone and at least one copy of
    /// it sits on a disc that has not been marked dead.
    pub fn read_view(&self, project: ProjectId) -> Result<CatalogView, CatalogError> {
        let mut stmt = self
            .conn
            .prepare(
                "select fv.rel_path, fv.content_hash, fv.size_bytes
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
                   and exists (
                     select 1 from placement p
                     join session s on s.id = p.session_id
                     where p.file_version_id = fv.id
                       and not exists (select 1 from disc_event e
                                       where e.disc_id = s.disc_id and e.kind = 'MARKED_DEAD'))",
            )
            .map_err(CatalogError::from_sqlite)?;

        let rows = stmt
            .query_map(params![project.0], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(CatalogError::from_sqlite)?;

        let mut current = BTreeMap::new();
        for row in rows {
            let (path, hash, size) = row.map_err(CatalogError::from_sqlite)?;
            let rel_path = RelPath(path);
            current.insert(
                rel_path.clone(),
                CatalogEntry {
                    rel_path,
                    content_hash: ContentHash(hash),
                    size_bytes: size as u64,
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
    /// wasted work. Ported as-is with the rest of the deferred debt (spec §1).
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

    /// Sessions in one lifecycle state, ordered by disc then session ordinal.
    pub fn sessions_by_state(
        &self,
        project: ProjectId,
        state: SessionState,
    ) -> Result<Vec<Session>, CatalogError> {
        let mut stmt = self
            .conn
            .prepare(
                "select d.ordinal, s.ordinal, s.state, s.burned_at, s.manifest_hash
                 from session s
                 join disc d on d.id = s.disc_id
                 where d.project_id = ?1 and s.state = ?2
                 order by d.ordinal, s.ordinal",
            )
            .map_err(CatalogError::from_sqlite)?;

        let rows = stmt
            .query_map(params![project.0, state.as_str()], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            })
            .map_err(CatalogError::from_sqlite)?;

        let mut out = Vec::new();
        for row in rows {
            let (disc_ordinal, ordinal, state, burned_at, manifest_hash) =
                row.map_err(CatalogError::from_sqlite)?;
            out.push(Session {
                disc_ordinal: disc_ordinal as u32,
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
                "select d.ordinal,
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
                 order by d.ordinal",
            )
            .map_err(CatalogError::from_sqlite)?;

        let rows = stmt
            .query_map(params![project.0], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .map_err(CatalogError::from_sqlite)?;

        let mut out = Vec::new();
        for row in rows {
            let (ordinal, status, usable, used) = row.map_err(CatalogError::from_sqlite)?;
            out.push(DiscState {
                ordinal: ordinal as u32,
                status: DiscStatus::from_str(&status).map_err(CatalogError::ConstraintViolation)?,
                usable_bytes: usable as u64,
                used_bytes: used as u64,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use diskrement_core::{ContentHash, DiscState, DiscStatus, RelPath};

    use crate::catalog::model::{FileFingerprint, SessionState};
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
        assert_eq!(burned[0].disc_ordinal, 1);
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
                    status: DiscStatus::Open,
                    usable_bytes: 1_000_000,
                    used_bytes: 30,
                },
                DiscState {
                    ordinal: 2,
                    status: DiscStatus::Open,
                    usable_bytes: 1_000_000,
                    used_bytes: 10,
                },
            ]
        );
    }
}
