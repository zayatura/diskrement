//! The catalog's write path: projects, burn updates, verification, and the
//! mark-dead event.
//!
//! Writers take `&mut self` because `Connection::transaction` does — which
//! makes "this operation mutates the catalog" a fact the compiler knows rather
//! than a convention. Readers take `&self` and live in `read.rs`.

use std::path::Path;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension, Transaction};

use super::model::{
    CatalogUpdate, DiscWrite, FileWrite, LostReport, LostVersionEntry, Project, ProjectId,
    ProjectInit, ReburnEntry,
};
use super::time;
use super::Catalog;
use crate::error::{is_unique_violation, CatalogError};
use diskrement_core::{ContentHash, DiscKey, DiscStatus, DiscType, ManifestChangeKind, RelPath};

impl Catalog {
    /// Insert a project row, returning its new id.
    pub fn create_project(&mut self, p: &ProjectInit) -> Result<ProjectId, CatalogError> {
        let name = p.name.clone();
        self.tx(|tx| {
            let inserted = tx.execute(
                "insert into project
                   (name, source_root, work_dir, default_disc_type, margin_percent,
                    copies, created_at)
                 values (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    p.name,
                    p.source_root,
                    p.work_dir,
                    p.default_disc_type.as_str(),
                    p.margin_percent,
                    p.copies as i64,
                    time::format(&p.created_at),
                ],
            );
            match inserted {
                Ok(_) => Ok(ProjectId(tx.last_insert_rowid())),
                Err(e) if is_unique_violation(&e) => Err(CatalogError::DuplicateProject(name)),
                Err(e) => Err(CatalogError::from_sqlite(e)),
            }
        })
    }

    /// Look a project up by its unique name.
    pub fn find_project(&self, name: &str) -> Result<Option<Project>, CatalogError> {
        let row = self
            .conn
            .query_row(
                "select id, name, source_root, work_dir, default_disc_type,
                        margin_percent, copies, created_at
                 from project where name = ?1",
                params![name],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, f64>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, String>(7)?,
                    ))
                },
            )
            .optional()
            .map_err(CatalogError::from_sqlite)?;

        row.map(
            |(id, name, source_root, work_dir, disc_type, margin, copies, created_at)| {
                Ok(Project {
                    id: ProjectId(id),
                    name,
                    source_root,
                    work_dir,
                    default_disc_type: DiscType::from_str(&disc_type)
                        .map_err(CatalogError::ConstraintViolation)?,
                    margin_percent: margin,
                    copies: copies as u32,
                    created_at: time::parse(&created_at)?,
                })
            },
        )
        .transpose()
    }

    /// Persist one successful burn: every disc touched, its single new session,
    /// and the file versions, placements and change records that session wrote.
    ///
    /// One transaction, so this is the crash-safe commit point — the invariant
    /// the whole burn flow is built around. Nothing is written before the discs
    /// are staged successfully, and a failure here leaves the catalog exactly
    /// as it was.
    pub fn apply_update(&mut self, u: &CatalogUpdate) -> Result<(), CatalogError> {
        self.tx(|tx| {
            for disc in &u.finalize_disc_ordinals {
                finalize_disc(tx, u.project_id, *disc)?;
            }
            for disc in &u.discs {
                write_disc(tx, u.project_id, disc)?;
            }
            Ok(())
        })
    }

    /// Record that a burned session was read back and matched.
    ///
    /// Only a BURNED session can become VERIFIED; anything else — a missing
    /// session, or one already verified — is an illegal transition.
    pub fn mark_verified(
        &mut self,
        project: ProjectId,
        disc: DiscKey,
        session_ordinal: u32,
    ) -> Result<(), CatalogError> {
        self.tx(|tx| {
            let updated = tx
                .execute(
                    "update session set state = 'VERIFIED'
                     where state = 'BURNED' and ordinal = ?1
                       and disc_id = (select id from disc
                                      where project_id = ?2 and ordinal = ?3
                                        and copy_set = ?4)",
                    params![
                        session_ordinal as i64,
                        project.0,
                        disc.ordinal as i64,
                        disc.copy_set as i64
                    ],
                )
                .map_err(CatalogError::from_sqlite)?;
            if updated == 0 {
                return Err(CatalogError::IllegalStatusTransition(format!(
                    "no BURNED session {session_ordinal} on disc {}",
                    disc_name(tx, project, disc)
                )));
            }
            Ok(())
        })
    }

    /// Append a MARKED_DEAD event for `disc` and report what it cost. A
    /// version whose only live copy was on this disc becomes REBURN (current;
    /// the next cycle re-writes it from source) or LOST (superseded or
    /// deleted; gone). A version with another live copy is not written to
    /// `lost_version`: if current and no live copy remains in `disc`'s copy
    /// set, it is reported as degraded (the next cycle restores that set's
    /// copy); a current version still covered within its own set is not
    /// reported at all; a non-current one is counted as a superseded survivor.
    ///
    /// `deleted` names paths the latest scan found gone from the source whose
    /// deletion is not recorded yet (#9). Their versions are still current
    /// here, but nothing will re-burn them, so they are treated as non-current:
    /// LOST, never REBURN or degraded, and a surviving copy elsewhere counts
    /// as a superseded survivor (#16).
    ///
    /// The `disc` row itself is never touched: dead-ness is derived from
    /// `disc_event` at read time.
    pub fn mark_dead(
        &mut self,
        project: ProjectId,
        disc: DiscKey,
        at: &DateTime<Utc>,
        deleted: &std::collections::BTreeSet<RelPath>,
    ) -> Result<LostReport, CatalogError> {
        self.tx(|tx| {
            let disc_id = disc_id(tx, project, disc)?;
            let already: i64 = tx
                .query_row(
                    "select count(*) from disc_event
                     where disc_id = ?1 and kind = 'MARKED_DEAD'",
                    params![disc_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from_sqlite)?;
            if already > 0 {
                return Err(CatalogError::IllegalStatusTransition(format!(
                    "disc {} is already dead",
                    disc_name(tx, project, disc)
                )));
            }

            let mut versions = disc_versions(tx, project, disc_id)?;
            for v in &mut versions {
                v.is_current = v.is_current && !deleted.contains(&v.rel_path);
            }

            tx.execute(
                "insert into disc_event (disc_id, kind, at) values (?1, 'MARKED_DEAD', ?2)",
                params![disc_id, time::format(at)],
            )
            .map_err(CatalogError::from_sqlite)?;
            let event_id = tx.last_insert_rowid();

            for v in versions.iter().filter(|v| !v.has_other_live_copy) {
                let fate = if v.is_current { "REBURN" } else { "LOST" };
                tx.execute(
                    "insert into lost_version (event_id, file_version_id, fate)
                     values (?1, ?2, ?3)",
                    params![event_id, v.file_version_id, fate],
                )
                .map_err(CatalogError::from_sqlite)?;
            }

            let live_entry = |v: &DiscVersion| ReburnEntry {
                rel_path: v.rel_path.clone(),
                content_hash: v.content_hash.clone(),
                size_bytes: v.size_bytes,
            };
            Ok(LostReport {
                disc,
                lost: versions
                    .iter()
                    .filter(|v| !v.has_other_live_copy && !v.is_current)
                    .map(|v| LostVersionEntry {
                        rel_path: v.rel_path.clone(),
                        content_hash: v.content_hash.clone(),
                        size_bytes: v.size_bytes,
                        session_ordinal: v.session_ordinal,
                    })
                    .collect(),
                reburn: versions
                    .iter()
                    .filter(|v| !v.has_other_live_copy && v.is_current)
                    .map(live_entry)
                    .collect(),
                degraded: versions
                    .iter()
                    .filter(|v| v.is_current && v.has_other_live_copy && !v.has_other_live_same_set)
                    .map(live_entry)
                    .collect(),
                superseded_survivors: versions
                    .iter()
                    .filter(|v| v.has_other_live_copy && !v.is_current)
                    .count(),
            })
        })
    }

    /// Write a consistent full-database snapshot to `dest` via `VACUUM INTO`.
    ///
    /// Runs outside a transaction, and `dest` must not already exist —
    /// `VACUUM INTO` fails if it does, so the burn flow removes any leftover
    /// work-directory snapshot before calling this.
    pub fn snapshot_to(&self, dest: &Path) -> Result<(), CatalogError> {
        self.conn
            .execute("vacuum into ?1", params![dest.to_string_lossy()])
            .map_err(CatalogError::from_sqlite)?;
        Ok(())
    }

    /// Open a standalone catalog file, apply `update`, and close it.
    ///
    /// Used to materialise the post-burn catalog state into the per-disc
    /// snapshot without committing anything to the live catalog: the live
    /// commit happens separately, only after the discs are staged. The disc's
    /// embedded `catalog.sqlite` therefore describes the state *after* its own
    /// burn, so a single disc knows about itself and its siblings.
    pub fn apply_update_to(path: &Path, update: &CatalogUpdate) -> Result<(), CatalogError> {
        let mut catalog = Catalog::open(path)?;
        catalog.apply_update(update)
    }
}

/// `disc` as every other message names it: `DiscKey::name` under the
/// project's `copies` (CLAUDE.md: never format a disc ordinal by hand).
///
/// Only error messages use this. When the project row cannot be read, the
/// lettered form is used, since it is never ambiguous.
fn disc_name(tx: &Transaction<'_>, project: ProjectId, disc: DiscKey) -> String {
    let copies = tx
        .query_row(
            "select copies from project where id = ?1",
            params![project.0],
            |row| row.get::<_, i64>(0),
        )
        // Any `copies` above 1 letters the name.
        .map_or(2, |c| c as u32);
    disc.name(copies)
}

fn disc_id(tx: &Transaction<'_>, project: ProjectId, disc: DiscKey) -> Result<i64, CatalogError> {
    tx.query_row(
        "select id from disc where project_id = ?1 and ordinal = ?2 and copy_set = ?3",
        params![project.0, disc.ordinal as i64, disc.copy_set as i64],
        |row| row.get::<_, i64>(0),
    )
    .optional()
    .map_err(CatalogError::from_sqlite)?
    .ok_or_else(|| {
        CatalogError::ConstraintViolation(format!(
            "no disc with ordinal {}",
            disc_name(tx, project, disc)
        ))
    })
}

/// Flip a pre-existing OPEN disc to FINALIZED. Anything else is an illegal
/// transition — including a disc that is already FINALIZED.
fn finalize_disc(
    tx: &Transaction<'_>,
    project: ProjectId,
    disc: DiscKey,
) -> Result<(), CatalogError> {
    let updated = tx
        .execute(
            "update disc set status = 'FINALIZED'
             where project_id = ?1 and ordinal = ?2 and copy_set = ?3 and status = 'OPEN'",
            params![project.0, disc.ordinal as i64, disc.copy_set as i64],
        )
        .map_err(CatalogError::from_sqlite)?;
    if updated == 0 {
        return Err(CatalogError::IllegalStatusTransition(format!(
            "disc {} is not OPEN",
            disc_name(tx, project, disc)
        )));
    }
    Ok(())
}

/// Insert or find the disc row, insert its session, then write the session's
/// files and tombstones.
///
/// Sessions are always recorded as BURNED: no PLANNED row is ever persisted.
fn write_disc(tx: &Transaction<'_>, project: ProjectId, d: &DiscWrite) -> Result<(), CatalogError> {
    let disc_id = if d.is_new {
        tx.execute(
            "insert into disc (project_id, ordinal, disc_type, usable_bytes, status, label, copy_set)
             values (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                project.0,
                d.ordinal as i64,
                d.disc_type.as_str(),
                d.usable_bytes as i64,
                d.status.as_str(),
                d.label,
                d.copy_set as i64,
            ],
        )
        .map_err(CatalogError::from_sqlite)?;
        tx.last_insert_rowid()
    } else {
        let id = disc_id(tx, project, DiscKey::new(d.ordinal, d.copy_set))?;
        if d.status == DiscStatus::Finalized {
            let updated = tx
                .execute(
                    "update disc set status = 'FINALIZED' where id = ?1 and status = 'OPEN'",
                    params![id],
                )
                .map_err(CatalogError::from_sqlite)?;
            if updated == 0 {
                return Err(CatalogError::IllegalStatusTransition(format!(
                    "disc {} is not OPEN",
                    disc_name(tx, project, DiscKey::new(d.ordinal, d.copy_set))
                )));
            }
        }
        id
    };

    tx.execute(
        "insert into session (disc_id, ordinal, state, burned_at, manifest_hash)
         values (?1, ?2, 'BURNED', ?3, ?4)",
        params![
            disc_id,
            d.session.ordinal as i64,
            time::format(&d.session.burned_at),
            d.session.manifest_hash,
        ],
    )
    .map_err(CatalogError::from_sqlite)?;
    let session_id = tx.last_insert_rowid();

    for file in &d.session.added {
        write_file(
            tx,
            project,
            session_id,
            file,
            ManifestChangeKind::Added,
            &d.session.burned_at,
            d.copy_set,
        )?;
    }
    for file in &d.session.modified {
        write_file(
            tx,
            project,
            session_id,
            file,
            ManifestChangeKind::Modified,
            &d.session.burned_at,
            d.copy_set,
        )?;
    }
    for file in &d.session.copied {
        write_copy(tx, project, session_id, disc_id, d.copy_set, file)?;
    }
    for path in &d.session.deleted {
        write_tombstone(tx, project, session_id, path)?;
    }
    Ok(())
}

fn write_file(
    tx: &Transaction<'_>,
    project: ProjectId,
    session_id: i64,
    f: &FileWrite,
    kind: ManifestChangeKind,
    first_seen_at: &DateTime<Utc>,
    copy_set: u32,
) -> Result<(), CatalogError> {
    tx.execute(
        "insert into file_version
           (project_id, rel_path, content_hash, size_bytes, mtime, first_seen_at)
         values (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            project.0,
            f.rel_path.as_str(),
            f.content_hash.as_str(),
            f.size_bytes as i64,
            time::format(&f.mtime),
            time::format(first_seen_at),
        ],
    )
    .map_err(CatalogError::from_sqlite)?;
    let file_version_id = tx.last_insert_rowid();

    tx.execute(
        "insert into placement (file_version_id, session_id, disc_path, copy_index)
         values (?1, ?2, ?3, ?4)",
        params![file_version_id, session_id, f.disc_path, copy_set as i64],
    )
    .map_err(CatalogError::from_sqlite)?;

    tx.execute(
        "insert into change_record (session_id, file_version_id, kind) values (?1, ?2, ?3)",
        params![session_id, file_version_id, kind.as_str()],
    )
    .map_err(CatalogError::from_sqlite)?;
    Ok(())
}

/// Place another copy of a path's current version on this disc: one
/// `placement` row and nothing else.
///
/// The version must still be current and carry the same hash — the pipeline
/// classified the file against the live catalog a moment ago, so anything else
/// is a bug — and must not already be on this disc, where a second copy would
/// protect nothing.
fn write_copy(
    tx: &Transaction<'_>,
    project: ProjectId,
    session_id: i64,
    disc_id: i64,
    copy_set: u32,
    f: &FileWrite,
) -> Result<(), CatalogError> {
    let current: Option<(i64, String, String)> = tx
        .query_row(
            "select fv.id, fv.content_hash, l.kind
             from (select cr.file_version_id as fvid, cr.kind as kind
                   from change_record cr
                   join file_version fv on fv.id = cr.file_version_id
                   where fv.project_id = ?1 and fv.rel_path = ?2
                   order by cr.id desc limit 1) l
             join file_version fv on fv.id = l.fvid",
            params![project.0, f.rel_path.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(CatalogError::from_sqlite)?;
    let file_version_id = match current {
        Some((id, hash, kind)) if kind != "DELETED" && hash == f.content_hash.as_str() => id,
        _ => {
            return Err(CatalogError::ConstraintViolation(format!(
                "cannot copy {}: no current version with hash {}",
                f.rel_path, f.content_hash
            )))
        }
    };
    let already_here: bool = tx
        .query_row(
            "select exists (select 1 from placement p
                            join session s on s.id = p.session_id
                            where p.file_version_id = ?1 and s.disc_id = ?2)",
            params![file_version_id, disc_id],
            |row| row.get(0),
        )
        .map_err(CatalogError::from_sqlite)?;
    if already_here {
        return Err(CatalogError::ConstraintViolation(format!(
            "{} already has a copy on this disc",
            f.rel_path
        )));
    }
    tx.execute(
        "insert into placement (file_version_id, session_id, disc_path, copy_index)
         values (?1, ?2, ?3, ?4)",
        params![file_version_id, session_id, f.disc_path, copy_set as i64],
    )
    .map_err(CatalogError::from_sqlite)?;
    Ok(())
}

/// Tombstone the path's current version — the latest `file_version` row for
/// that path. A tombstone writes no new version: it is a change record against
/// the version that was live.
fn write_tombstone(
    tx: &Transaction<'_>,
    project: ProjectId,
    session_id: i64,
    rel_path: &RelPath,
) -> Result<(), CatalogError> {
    let file_version_id: Option<i64> = tx
        .query_row(
            "select id from file_version
             where project_id = ?1 and rel_path = ?2
             order by id desc limit 1",
            params![project.0, rel_path.as_str()],
            |row| row.get(0),
        )
        .optional()
        .map_err(CatalogError::from_sqlite)?;

    match file_version_id {
        Some(id) => {
            tx.execute(
                "insert into change_record (session_id, file_version_id, kind)
                 values (?1, ?2, 'DELETED')",
                params![session_id, id],
            )
            .map_err(CatalogError::from_sqlite)?;
            Ok(())
        }
        None => Err(CatalogError::ConstraintViolation(format!(
            "cannot tombstone unknown path {}",
            rel_path.as_str()
        ))),
    }
}

/// One version placed on the dying disc.
struct DiscVersion {
    file_version_id: i64,
    rel_path: RelPath,
    content_hash: ContentHash,
    size_bytes: u64,
    session_ordinal: u32,
    is_current: bool,
    /// Another disc that is not dead also holds this version.
    has_other_live_copy: bool,
    /// Such a disc is in the dying disc's own copy set, so that set still
    /// covers the version and nothing needs restoring.
    has_other_live_same_set: bool,
}

/// Every version placed on the dying disc, flagged with whether it is its
/// path's current version, whether a live copy survives elsewhere, and
/// whether one survives in the dying disc's own copy set.
///
/// Grouped by version: a version with several placements on this disc (a
/// later session re-placing it) is one row, because `lost_version` is
/// append-only and one event costs a version once.
fn disc_versions(
    tx: &Transaction<'_>,
    project: ProjectId,
    disc_id: i64,
) -> Result<Vec<DiscVersion>, CatalogError> {
    let mut stmt = tx
        .prepare(
            "select fv.id, fv.rel_path, fv.content_hash, fv.size_bytes,
                    min(s.ordinal) as sess_ord,
                    max(case when cur.fvid is not null then 1 else 0 end) as is_current,
                    exists (
                      select 1 from placement p2
                      join session s2 on s2.id = p2.session_id
                      join disc d2 on d2.id = s2.disc_id
                      where p2.file_version_id = fv.id and d2.id <> ?2
                        and not exists (select 1 from disc_event e
                                        where e.disc_id = d2.id and e.kind = 'MARKED_DEAD')
                    ) as other_live,
                    exists (
                      select 1 from placement p2
                      join session s2 on s2.id = p2.session_id
                      join disc d2 on d2.id = s2.disc_id
                      where p2.file_version_id = fv.id and d2.id <> ?2
                        and d2.copy_set = (select copy_set from disc where id = ?2)
                        and not exists (select 1 from disc_event e
                                        where e.disc_id = d2.id and e.kind = 'MARKED_DEAD')
                    ) as other_live_same_set
             from placement p
             join session s on s.id = p.session_id
             join file_version fv on fv.id = p.file_version_id
             left join (
               select fvid from (
                 select cr.file_version_id as fvid, cr.kind as kind,
                        row_number() over (partition by fv2.rel_path
                                           order by cr.id desc) as rn
                 from change_record cr
                 join file_version fv2 on fv2.id = cr.file_version_id
                 where fv2.project_id = ?1
               ) c where c.rn = 1 and c.kind <> 'DELETED'
             ) cur on cur.fvid = fv.id
             where s.disc_id = ?2
             group by fv.id, fv.rel_path, fv.content_hash, fv.size_bytes
             order by fv.rel_path, sess_ord",
        )
        .map_err(CatalogError::from_sqlite)?;

    let rows = stmt
        .query_map(params![project.0, disc_id], |row| {
            Ok(DiscVersion {
                file_version_id: row.get::<_, i64>(0)?,
                rel_path: RelPath(row.get::<_, String>(1)?),
                content_hash: ContentHash(row.get::<_, String>(2)?),
                size_bytes: row.get::<_, i64>(3)? as u64,
                session_ordinal: row.get::<_, i64>(4)? as u32,
                is_current: row.get::<_, i64>(5)? == 1,
                has_other_live_copy: row.get::<_, i64>(6)? == 1,
                has_other_live_same_set: row.get::<_, i64>(7)? == 1,
            })
        })
        .map_err(CatalogError::from_sqlite)?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(CatalogError::from_sqlite)
}

#[cfg(test)]
mod tests {
    use diskrement_core::{DiscStatus, DiscType};

    use crate::catalog::model::{CatalogUpdate, DiscWrite, SessionWrite};
    use crate::catalog::testutil::{count, fw, project_init, temp_catalog, ts, update};
    use crate::error::CatalogError;
    #[test]
    fn create_project_then_find_project_round_trips() {
        let (_dir, mut cat) = temp_catalog();
        let init = project_init("photos");
        let id = cat.create_project(&init).expect("project is created");

        let found = cat.find_project("photos").expect("query succeeds");
        let project = found.expect("the project exists");
        assert_eq!(project.id, id);
        assert_eq!(project.name, "photos");
        assert_eq!(project.default_disc_type, DiscType::BdR);
        assert_eq!(project.source_root, "/src");
        assert_eq!(project.work_dir, "/work");
        assert_eq!(project.margin_percent, 2.0);
        assert_eq!(project.copies, 1);
        // The timestamp survives the trip through TEXT storage unchanged.
        assert_eq!(project.created_at, init.created_at);
    }

    #[test]
    fn creating_a_duplicate_name_fails_with_duplicate_project() {
        let (_dir, mut cat) = temp_catalog();
        cat.create_project(&project_init("photos"))
            .expect("first create");
        let err = cat
            .create_project(&project_init("photos"))
            .expect_err("the unique index rejects the second");
        assert!(
            matches!(&err, CatalogError::DuplicateProject(name) if name == "photos"),
            "expected DuplicateProject, got {err:?}"
        );
    }

    #[test]
    fn find_project_returns_none_for_an_unknown_name() {
        let (_dir, cat) = temp_catalog();
        assert!(cat.find_project("nope").expect("query succeeds").is_none());
    }

    #[test]
    fn apply_update_writes_discs_sessions_versions_placements_and_changes() {
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
        .expect("update applies");

        assert_eq!(count(&cat, "disc"), 1);
        assert_eq!(count(&cat, "session"), 1);
        assert_eq!(count(&cat, "file_version"), 2);
        assert_eq!(count(&cat, "placement"), 2);
        assert_eq!(count(&cat, "change_record"), 2);
    }

    #[test]
    fn a_failing_apply_update_rolls_back_every_insert() {
        let (_dir, mut cat) = temp_catalog();
        let pid = cat.create_project(&project_init("p")).expect("project");

        // A second disc marked `is_new = false` that does not exist: the id
        // lookup fails partway through, after the first disc's rows are in.
        let mut bad = update(pid, 1, 1, vec![fw("a", "h-a", 10)], vec![], vec![]);
        bad.discs.push(DiscWrite {
            ordinal: 99,
            copy_set: 0,
            is_new: false,
            disc_type: diskrement_core::DiscType::BdR,
            usable_bytes: 100,
            status: DiscStatus::Finalized,
            label: "ghost".to_string(),
            session: SessionWrite {
                ordinal: 2,
                burned_at: ts(),
                manifest_hash: "m2".to_string(),
                added: vec![],
                modified: vec![],
                copied: Vec::new(),
                deleted: vec![],
            },
        });

        let err = cat.apply_update(&bad).expect_err("the ghost disc fails");
        assert!(
            matches!(err, CatalogError::ConstraintViolation(_)),
            "expected ConstraintViolation, got {err:?}"
        );
        assert_eq!(count(&cat, "disc"), 0);
        assert_eq!(count(&cat, "session"), 0);
        assert_eq!(count(&cat, "file_version"), 0);
        assert_eq!(count(&cat, "placement"), 0);
        assert_eq!(count(&cat, "change_record"), 0);
    }

    #[test]
    fn finalize_disc_ordinals_seal_an_open_disc_and_refuse_a_sealed_one() {
        let (_dir, mut cat) = temp_catalog();
        let pid = cat.create_project(&project_init("p")).expect("project");
        cat.apply_update(&update(pid, 1, 1, vec![fw("a", "h-a", 10)], vec![], vec![]))
            .expect("first burn");

        let seal = CatalogUpdate {
            project_id: pid,
            finalize_disc_ordinals: vec![DiscKey::new(1, 0)],
            discs: vec![],
        };
        cat.apply_update(&seal).expect("sealing an OPEN disc works");

        let status: String = cat
            .raw_connection_for_tests()
            .query_row("select status from disc where ordinal = 1", [], |r| {
                r.get(0)
            })
            .expect("disc row exists");
        assert_eq!(status, "FINALIZED");

        let err = cat.apply_update(&seal).expect_err("sealing twice fails");
        assert!(
            matches!(err, CatalogError::IllegalStatusTransition(_)),
            "expected IllegalStatusTransition, got {err:?}"
        );
    }

    #[test]
    fn a_tombstone_for_an_unknown_path_is_refused() {
        let (_dir, mut cat) = temp_catalog();
        let pid = cat.create_project(&project_init("p")).expect("project");
        let err = cat
            .apply_update(&update(
                pid,
                1,
                1,
                vec![],
                vec![],
                vec![diskrement_core::RelPath("ghost".to_string())],
            ))
            .expect_err("there is no version to tombstone");
        assert!(
            matches!(err, CatalogError::ConstraintViolation(_)),
            "expected ConstraintViolation, got {err:?}"
        );
    }

    use crate::catalog::model::SessionState;

    #[test]
    fn mark_verified_flips_a_burned_session() {
        let (_dir, mut cat) = temp_catalog();
        let pid = cat.create_project(&project_init("p")).expect("project");
        cat.apply_update(&update(pid, 1, 1, vec![fw("a", "h", 10)], vec![], vec![]))
            .expect("burn");

        cat.mark_verified(pid, DiscKey::new(1, 0), 1)
            .expect("verification recorded");

        let verified = cat
            .sessions_by_state(pid, SessionState::Verified)
            .expect("query succeeds");
        assert_eq!(verified.len(), 1);
        assert_eq!((verified[0].disc.ordinal, verified[0].ordinal), (1, 1));
        assert!(cat
            .sessions_by_state(pid, SessionState::Burned)
            .expect("query succeeds")
            .is_empty());
    }

    #[test]
    fn marking_a_session_that_is_not_burned_is_an_illegal_transition() {
        let (_dir, mut cat) = temp_catalog();
        let pid = cat.create_project(&project_init("p")).expect("project");
        let err = cat
            .mark_verified(pid, DiscKey::new(1, 0), 1)
            .expect_err("there is no such session");
        assert!(
            matches!(err, CatalogError::IllegalStatusTransition(_)),
            "expected IllegalStatusTransition, got {err:?}"
        );
    }

    use chrono::{DateTime, TimeZone, Utc};
    use diskrement_core::RelPath;

    fn dead_at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 2, 9, 0, 0).unwrap()
    }

    fn dead_at2() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 3, 9, 0, 0).unwrap()
    }

    /// Disc 1, session 1 adds a@a1 and b@b1; session 2 modifies b to b2 — so
    /// b@b1 is superseded in place, on the same disc.
    fn seed(cat: &mut crate::catalog::Catalog) -> crate::catalog::model::ProjectId {
        let pid = cat.create_project(&project_init("p")).expect("project");
        cat.apply_update(&update(
            pid,
            1,
            1,
            vec![fw("a", "a1", 10), fw("b", "b1", 20)],
            vec![],
            vec![],
        ))
        .expect("session 1");
        cat.apply_update(&update(pid, 1, 2, vec![], vec![fw("b", "b2", 25)], vec![]))
            .expect("session 2");
        pid
    }

    /// Put a's version on disc 2 (set 0) as well, with a raw extra placement:
    /// a second live copy in the *same* copy set as disc 1. `copy_index` is
    /// the disc's copy set, so 0.
    fn also_place_a_on_disc_2(cat: &crate::catalog::Catalog) {
        cat.raw_connection_for_tests()
            .execute(
                "insert into placement (file_version_id, session_id, disc_path, copy_index)
                 select (select id from file_version where rel_path = 'a'),
                        (select s.id from session s join disc d on d.id = s.disc_id
                         where d.ordinal = 2),
                        '/data/a', 0",
                [],
            )
            .expect("the extra placement inserts");
    }

    /// Every recorded casualty, as (relPath, fate), sorted.
    fn fates(cat: &crate::catalog::Catalog) -> Vec<(String, String)> {
        let conn = cat.raw_connection_for_tests();
        let mut stmt = conn
            .prepare(
                "select fv.rel_path, lv.fate from lost_version lv
                 join file_version fv on fv.id = lv.file_version_id",
            )
            .expect("query prepares");
        let mut rows: Vec<(String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .expect("rows are text")
            .map(|r| r.expect("readable row"))
            .collect();
        rows.sort();
        rows
    }

    #[test]
    fn mark_dead_splits_versions_into_lost_and_reburn_and_records_both() {
        let (_dir, mut cat) = temp_catalog();
        let pid = seed(&mut cat);
        let report = cat
            .mark_dead(pid, DiscKey::new(1, 0), &dead_at(), &Default::default())
            .expect("disc 1 dies");

        assert_eq!(report.disc, DiscKey::new(1, 0));
        assert_eq!(
            report
                .lost
                .iter()
                .map(|e| (
                    e.rel_path.as_str(),
                    e.content_hash.as_str(),
                    e.session_ordinal
                ))
                .collect::<Vec<_>>(),
            vec![("b", "b1", 1)]
        );
        let mut reburn: Vec<(&str, &str)> = report
            .reburn
            .iter()
            .map(|e| (e.rel_path.as_str(), e.content_hash.as_str()))
            .collect();
        reburn.sort();
        assert_eq!(reburn, vec![("a", "a1"), ("b", "b2")]);

        assert_eq!(
            cat.read_disc_states(pid).expect("states read"),
            vec![diskrement_core::DiscState {
                ordinal: 1,
                copy_set: 0,
                status: diskrement_core::DiscStatus::Dead,
                usable_bytes: 1_000_000,
                used_bytes: 55,
            }]
        );
        assert_eq!(
            fates(&cat),
            vec![
                ("a".to_string(), "REBURN".to_string()),
                ("b".to_string(), "LOST".to_string()),
                ("b".to_string(), "REBURN".to_string()),
            ]
        );
    }

    #[test]
    fn deadness_is_derived_so_the_disc_row_is_untouched() {
        // The status column keeps its OPEN/FINALIZED check forever; DEAD exists
        // only in the derived DiscState. Without this assertion, an
        // update-in-place implementation would still satisfy the read_disc_states
        // assertion above.
        let (_dir, mut cat) = temp_catalog();
        let pid = seed(&mut cat);
        cat.mark_dead(pid, DiscKey::new(1, 0), &dead_at(), &Default::default())
            .expect("disc 1 dies");

        let conn = cat.raw_connection_for_tests();
        let status: String = conn
            .query_row("select status from disc where ordinal = 1", [], |r| {
                r.get(0)
            })
            .expect("the disc row is still there");
        assert_eq!(status, "OPEN");

        let mut stmt = conn
            .prepare("select kind from disc_event")
            .expect("prepares");
        let events: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .expect("rows are text")
            .map(|r| r.expect("readable row"))
            .collect();
        assert_eq!(events, vec!["MARKED_DEAD".to_string()]);
    }

    #[test]
    fn a_version_alive_on_another_disc_is_neither_lost_nor_reburned() {
        let (_dir, mut cat) = temp_catalog();
        let pid = seed(&mut cat);
        cat.apply_update(&update(pid, 2, 1, vec![fw("c", "c1", 5)], vec![], vec![]))
            .expect("disc 2 exists");
        also_place_a_on_disc_2(&cat);

        let report = cat
            .mark_dead(pid, DiscKey::new(1, 0), &dead_at(), &Default::default())
            .expect("disc 1 dies");
        assert_eq!(
            report
                .reburn
                .iter()
                .map(|e| e.rel_path.as_str())
                .collect::<Vec<_>>(),
            vec!["b"]
        );
        assert_eq!(
            report
                .lost
                .iter()
                .map(|e| e.rel_path.as_str())
                .collect::<Vec<_>>(),
            vec!["b"]
        );
        // a's other copy is on 2A, in the same set: still covered, not degraded.
        assert!(report.degraded.is_empty());
        // Nothing was recorded against a: its bytes still exist on live disc 2.
        assert_eq!(
            fates(&cat),
            vec![
                ("b".to_string(), "LOST".to_string()),
                ("b".to_string(), "REBURN".to_string()),
            ]
        );
    }

    #[test]
    fn a_copy_on_an_already_dead_disc_no_longer_shields_a_version() {
        let (_dir, mut cat) = temp_catalog();
        let pid = seed(&mut cat);
        cat.apply_update(&update(pid, 2, 1, vec![fw("c", "c1", 5)], vec![], vec![]))
            .expect("disc 2 exists");
        also_place_a_on_disc_2(&cat);

        let second = cat
            .mark_dead(pid, DiscKey::new(2, 0), &dead_at(), &Default::default())
            .expect("disc 2 dies");
        // While disc 1 lived, a's copy there kept it off disc 2's casualty list.
        assert_eq!(
            second
                .reburn
                .iter()
                .map(|e| e.rel_path.as_str())
                .collect::<Vec<_>>(),
            vec!["c"]
        );
        assert!(second.lost.is_empty());

        let first = cat
            .mark_dead(pid, DiscKey::new(1, 0), &dead_at2(), &Default::default())
            .expect("disc 1 dies");
        // Now that disc 2 is dead too, a's only surviving copy is gone.
        assert_eq!(
            first
                .reburn
                .iter()
                .map(|e| (e.rel_path.as_str(), e.content_hash.as_str()))
                .collect::<Vec<_>>(),
            vec![("a", "a1"), ("b", "b2")]
        );
        assert_eq!(
            first
                .lost
                .iter()
                .map(|e| e.rel_path.as_str())
                .collect::<Vec<_>>(),
            vec!["b"]
        );
    }

    #[test]
    fn a_tombstoned_path_is_lost_never_reburned() {
        let (_dir, mut cat) = temp_catalog();
        let pid = seed(&mut cat);
        cat.apply_update(&update(
            pid,
            1,
            3,
            vec![],
            vec![],
            vec![RelPath("a".to_string())],
        ))
        .expect("delete a");

        let report = cat
            .mark_dead(pid, DiscKey::new(1, 0), &dead_at(), &Default::default())
            .expect("disc 1 dies");
        assert_eq!(
            report
                .lost
                .iter()
                .map(|e| (
                    e.rel_path.as_str(),
                    e.content_hash.as_str(),
                    e.session_ordinal
                ))
                .collect::<Vec<_>>(),
            vec![("a", "a1", 1), ("b", "b1", 1)]
        );
        assert_eq!(
            report
                .reburn
                .iter()
                .map(|e| (e.rel_path.as_str(), e.content_hash.as_str()))
                .collect::<Vec<_>>(),
            vec![("b", "b2")]
        );
        assert_eq!(
            fates(&cat),
            vec![
                ("a".to_string(), "LOST".to_string()),
                ("b".to_string(), "LOST".to_string()),
                ("b".to_string(), "REBURN".to_string()),
            ]
        );
    }

    #[test]
    fn several_placements_of_one_version_make_one_casualty() {
        let (_dir, mut cat) = temp_catalog();
        let pid = seed(&mut cat);
        // b@b2 was burned in session 2; pretend a second copy also landed in
        // session 1. `lost_version` is append-only, so one version must still
        // produce exactly one row.
        cat.raw_connection_for_tests()
            .execute(
                "insert into placement (file_version_id, session_id, disc_path, copy_index)
                 select (select max(id) from file_version where rel_path = 'b'),
                        (select s.id from session s join disc d on d.id = s.disc_id
                         where d.ordinal = 1 and s.ordinal = 1),
                        '/data/b', 0",
                [],
            )
            .expect("the extra placement inserts");

        let report = cat
            .mark_dead(pid, DiscKey::new(1, 0), &dead_at(), &Default::default())
            .expect("disc 1 dies");
        assert_eq!(
            report
                .reburn
                .iter()
                .map(|e| (e.rel_path.as_str(), e.content_hash.as_str()))
                .collect::<Vec<_>>(),
            vec![("a", "a1"), ("b", "b2")]
        );
        assert_eq!(
            report
                .lost
                .iter()
                .map(|e| e.rel_path.as_str())
                .collect::<Vec<_>>(),
            vec!["b"]
        );
        assert_eq!(
            fates(&cat),
            vec![
                ("a".to_string(), "REBURN".to_string()),
                ("b".to_string(), "LOST".to_string()),
                ("b".to_string(), "REBURN".to_string()),
            ]
        );
    }

    /// A current version whose path the latest scan found deleted is not
    /// re-burned from a source that no longer has it: its last copy dying
    /// makes it LOST (#16). The other current version is still REBURN.
    #[test]
    fn a_pending_deletion_on_a_dying_disc_is_lost_not_reburn() {
        let (_dir, mut cat) = temp_catalog();
        let pid = seed(&mut cat);
        let deleted = [RelPath("a".to_string())].into_iter().collect();

        let report = cat
            .mark_dead(pid, DiscKey::new(1, 0), &dead_at(), &deleted)
            .expect("disc 1 dies");

        assert_eq!(
            fates(&cat),
            vec![
                ("a".to_string(), "LOST".to_string()),
                ("b".to_string(), "LOST".to_string()),
                ("b".to_string(), "REBURN".to_string()),
            ]
        );
        assert!(report.lost.iter().any(|e| e.rel_path.as_str() == "a"));
        assert!(report.reburn.iter().all(|e| e.rel_path.as_str() != "a"));
    }

    #[test]
    fn marking_a_dead_disc_again_fails_and_so_does_an_unknown_ordinal() {
        let (_dir, mut cat) = temp_catalog();
        let pid = seed(&mut cat);
        cat.mark_dead(pid, DiscKey::new(1, 0), &dead_at(), &Default::default())
            .expect("disc 1 dies");

        let duplicate = cat
            .mark_dead(pid, DiscKey::new(1, 0), &dead_at(), &Default::default())
            .expect_err("already dead");
        assert!(
            matches!(duplicate, CatalogError::IllegalStatusTransition(_)),
            "expected IllegalStatusTransition, got {duplicate:?}"
        );
        assert!(
            duplicate.to_string().contains("disc 1 is already dead"),
            "{duplicate}"
        );

        let unknown = cat
            .mark_dead(pid, DiscKey::new(9, 0), &dead_at(), &Default::default())
            .expect_err("no such disc");
        assert!(
            matches!(unknown, CatalogError::ConstraintViolation(_)),
            "expected ConstraintViolation, got {unknown:?}"
        );
    }

    /// Store errors name discs through `DiscKey::name`, so at copies=2 they
    /// carry the set letter (#8).
    #[test]
    fn store_errors_name_discs_with_the_projects_copies() {
        let (_dir, mut cat) = temp_catalog();
        let mut init = project_init("p");
        init.copies = 2;
        let pid = cat.create_project(&init).expect("project");
        let mut sealed = update(pid, 1, 1, vec![fw("a", "a1", 10)], vec![], vec![]);
        sealed.discs[0].copy_set = 1;
        sealed.discs[0].status = DiscStatus::Finalized;
        cat.apply_update(&sealed).expect("1B burned and sealed");

        let unknown = cat
            .mark_dead(pid, DiscKey::new(9, 1), &dead_at(), &Default::default())
            .expect_err("no such disc");
        assert!(
            unknown.to_string().contains("no disc with ordinal 9B"),
            "{unknown}"
        );

        let unverifiable = cat
            .mark_verified(pid, DiscKey::new(1, 1), 7)
            .expect_err("no session 7");
        assert!(
            unverifiable
                .to_string()
                .contains("no BURNED session 7 on disc 1B"),
            "{unverifiable}"
        );

        let mut again = update(pid, 1, 2, vec![fw("b", "b1", 10)], vec![], vec![]);
        again.discs[0].copy_set = 1;
        again.discs[0].status = DiscStatus::Finalized;
        let resealed = cat.apply_update(&again).expect_err("1B is sealed");
        assert!(
            resealed.to_string().contains("disc 1B is not OPEN"),
            "{resealed}"
        );

        // The `plan --finalize` path seals through `finalize_disc`.
        let mut seal = update(pid, 2, 1, vec![fw("c", "c1", 10)], vec![], vec![]);
        seal.finalize_disc_ordinals = vec![DiscKey::new(1, 1)];
        let sealed_twice = cat.apply_update(&seal).expect_err("1B is sealed");
        assert!(
            sealed_twice.to_string().contains("disc 1B is not OPEN"),
            "{sealed_twice}"
        );
    }

    #[test]
    fn read_view_drops_current_versions_whose_only_copy_is_dead() {
        let (_dir, mut cat) = temp_catalog();
        let pid = seed(&mut cat);
        let before = cat.read_view(pid).expect("view reads");
        assert_eq!(
            before
                .current
                .keys()
                .map(RelPath::as_str)
                .collect::<Vec<_>>(),
            vec!["a", "b"]
        );

        cat.mark_dead(pid, DiscKey::new(1, 0), &dead_at(), &Default::default())
            .expect("disc 1 dies");
        assert!(cat.read_view(pid).expect("view reads").current.is_empty());
    }

    #[test]
    fn a_current_version_with_a_live_copy_stays_in_the_view() {
        let (_dir, mut cat) = temp_catalog();
        let pid = seed(&mut cat);
        cat.apply_update(&update(pid, 2, 1, vec![fw("c", "c1", 5)], vec![], vec![]))
            .expect("disc 2 exists");
        also_place_a_on_disc_2(&cat);
        cat.mark_dead(pid, DiscKey::new(1, 0), &dead_at(), &Default::default())
            .expect("disc 1 dies");

        // a survives on live disc 2; b@b2 is disc-1-only and drops; c is native
        // to disc 2.
        assert_eq!(
            cat.read_view(pid)
                .expect("view reads")
                .current
                .keys()
                .map(RelPath::as_str)
                .collect::<Vec<_>>(),
            vec!["a", "c"]
        );
    }

    #[test]
    fn file_history_marks_dead_disc_versions_lost() {
        let (_dir, mut cat) = temp_catalog();
        let pid = seed(&mut cat);
        cat.mark_dead(pid, DiscKey::new(1, 0), &dead_at(), &Default::default())
            .expect("disc 1 dies");

        let history = cat
            .file_history(pid, &RelPath("b".to_string()))
            .expect("history reads");
        assert_eq!(
            history.iter().map(|e| e.status).collect::<Vec<_>>(),
            vec![
                crate::catalog::model::VersionStatus::Lost,
                crate::catalog::model::VersionStatus::Current,
            ]
        );
    }

    #[test]
    fn a_current_version_on_a_dead_disc_still_renders_current() {
        // a@a1 is both a's only version and its current one, and disc 1 — its
        // only copy — is dead. is_current wins over disc_dead: the source still
        // has the file and it is about to be re-burned.
        let (_dir, mut cat) = temp_catalog();
        let pid = seed(&mut cat);
        cat.mark_dead(pid, DiscKey::new(1, 0), &dead_at(), &Default::default())
            .expect("disc 1 dies");

        let history = cat
            .file_history(pid, &RelPath("a".to_string()))
            .expect("history reads");
        assert_eq!(
            history.iter().map(|e| e.status).collect::<Vec<_>>(),
            vec![crate::catalog::model::VersionStatus::Current]
        );
    }

    #[test]
    fn snapshot_to_writes_a_self_contained_copy() {
        let (dir, mut cat) = temp_catalog();
        let pid = cat.create_project(&project_init("p")).expect("project");
        cat.apply_update(&update(
            pid,
            1,
            1,
            vec![fw("a", "a1", 10), fw("b", "b1", 20)],
            vec![],
            vec![],
        ))
        .expect("burn");

        let dest = dir.path().join("snapshot.sqlite");
        cat.snapshot_to(&dest).expect("vacuum into succeeds");
        assert!(dest.is_file());

        let copy = rusqlite::Connection::open(&dest).expect("the snapshot opens");
        let versions: i64 = copy
            .query_row("select count(*) from file_version", [], |r| r.get(0))
            .expect("the snapshot has the source's rows");
        assert_eq!(versions, 2);

        // VACUUM INTO refuses an existing destination; the burn flow relies on
        // that rather than on silently overwriting a stale snapshot.
        assert!(cat.snapshot_to(&dest).is_err());
    }

    #[test]
    fn apply_update_to_writes_a_standalone_catalog_without_touching_the_live_one() {
        let (dir, mut cat) = temp_catalog();
        let pid = cat.create_project(&project_init("p")).expect("project");
        cat.apply_update(&update(pid, 1, 1, vec![fw("a", "a1", 10)], vec![], vec![]))
            .expect("first burn");

        let work_copy = dir.path().join("burn-catalog.sqlite");
        cat.snapshot_to(&work_copy).expect("snapshot taken");

        let next = update(pid, 1, 2, vec![fw("b", "b1", 20)], vec![], vec![]);
        crate::catalog::Catalog::apply_update_to(&work_copy, &next)
            .expect("the work copy takes the update");

        let copy = crate::catalog::Catalog::open(&work_copy).expect("work copy opens");
        assert_eq!(count(&copy, "file_version"), 2);
        // The live catalog has not moved: this is the post-burn snapshot path,
        // not the commit point.
        assert_eq!(count(&cat, "file_version"), 1);
    }

    use crate::catalog::model::{FileWrite, ProjectId};
    use diskrement_core::{CatalogView, ChangeSet, ContentHash, ScannedFile};
    use proptest::prelude::*;
    use std::collections::BTreeMap;

    /// A generated scan: a set of paths, each mapped to a content hash.
    fn scanned_of(scan: &BTreeMap<String, String>) -> Vec<ScannedFile> {
        scan.iter()
            .map(|(path, hash)| ScannedFile {
                rel_path: RelPath(path.clone()),
                content_hash: ContentHash(hash.clone()),
                size_bytes: 1,
                mtime: ts(),
            })
            .collect()
    }

    /// Turn a change set into a one-disc update — the mini-orchestration
    /// `Pipeline::burn` does for real.
    fn to_update(project: ProjectId, session_ordinal: u32, changes: &ChangeSet) -> CatalogUpdate {
        let as_write = |f: &ScannedFile| FileWrite {
            rel_path: f.rel_path.clone(),
            content_hash: f.content_hash.clone(),
            size_bytes: f.size_bytes,
            mtime: ts(),
            disc_path: format!("/data/{}", f.rel_path.as_str()),
        };
        update(
            project,
            1,
            session_ordinal,
            changes.added.iter().map(as_write).collect(),
            changes.modified.iter().map(as_write).collect(),
            changes.deleted.clone(),
        )
    }

    proptest! {
        // Forty cases, matching the Scala's `TestAspect.samples(40)`. Each case
        // builds a fresh catalog file, so this is the slowest test in the
        // crate; more cases buy little.
        #![proptest_config(ProptestConfig::with_cases(40))]

        #[test]
        fn the_view_tracks_the_latest_scan_while_file_version_only_grows(
            scans in prop::collection::vec(
                prop::collection::btree_map(
                    prop::sample::select(vec!["a", "b", "c", "d", "e"]).prop_map(String::from),
                    prop::sample::select(vec!["h1", "h2", "h3"]).prop_map(String::from),
                    0..5usize,
                ),
                1..6usize,
            )
        ) {
            let (_dir, mut cat) = temp_catalog();
            let pid = cat.create_project(&project_init("p")).expect("project");

            let mut view = CatalogView::empty();
            let mut previous_rows = 0i64;
            for (index, scan) in scans.iter().enumerate() {
                let scanned = scanned_of(scan);
                let changes = diskrement_core::diff::diff(&view, &scanned);
                cat.apply_update(&to_update(pid, index as u32 + 1, &changes))
                    .expect("the session applies");

                let rows = count(&cat, "file_version");
                prop_assert!(
                    rows >= previous_rows,
                    "file_version shrank: {} -> {}",
                    previous_rows,
                    rows
                );
                previous_rows = rows;
                view = CatalogView::from_scanned(&scanned);
            }

            let stored = cat.read_view(pid).expect("view reads");
            let expected: Vec<(&str, &str)> = view
                .current
                .values()
                .map(|e| (e.rel_path.as_str(), e.content_hash.as_str()))
                .collect();
            let actual: Vec<(&str, &str)> = stored
                .current
                .values()
                .map(|e| (e.rel_path.as_str(), e.content_hash.as_str()))
                .collect();
            prop_assert_eq!(actual, expected);
        }
    }

    use diskrement_core::DiscKey;

    fn copy_update(
        project: crate::catalog::model::ProjectId,
        ordinal: u32,
        copy_set: u32,
        session: u32,
        copied: Vec<crate::catalog::model::FileWrite>,
    ) -> CatalogUpdate {
        let mut u =
            crate::catalog::testutil::update(project, ordinal, session, vec![], vec![], vec![]);
        u.discs[0].copy_set = copy_set;
        u.discs[0].session.copied = copied;
        u
    }

    #[test]
    fn a_copy_placement_adds_one_placement_and_no_version_or_change() {
        let (_dir, mut cat) = crate::catalog::testutil::temp_catalog();
        let p = cat
            .create_project(&crate::catalog::testutil::project_init("p"))
            .unwrap();
        cat.apply_update(&crate::catalog::testutil::update(
            p,
            1,
            1,
            vec![crate::catalog::testutil::fw("a", "ha", 4)],
            vec![],
            vec![],
        ))
        .unwrap();
        let count = |t| crate::catalog::testutil::count(&cat, t);
        let before = (
            count("file_version"),
            count("change_record"),
            count("placement"),
        );

        cat.apply_update(&copy_update(
            p,
            1,
            1,
            1,
            vec![crate::catalog::testutil::fw("a", "ha", 4)],
        ))
        .unwrap();

        let count = |t| crate::catalog::testutil::count(&cat, t);
        assert_eq!(count("file_version"), before.0);
        assert_eq!(count("change_record"), before.1);
        assert_eq!(count("placement"), before.2 + 1);
        let copy_index: i64 = cat
            .raw_connection_for_tests()
            .query_row(
                "select copy_index from placement order by id desc limit 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(copy_index, 1, "copy_index is the disc's copy set");
        let view = cat.read_view(p).unwrap();
        assert_eq!(
            view.current[&RelPath("a".into())].live_sets,
            std::collections::BTreeSet::from([0, 1])
        );
    }

    #[test]
    fn a_copy_of_a_hash_that_is_not_current_is_refused_and_rolls_back() {
        let (_dir, mut cat) = crate::catalog::testutil::temp_catalog();
        let p = cat
            .create_project(&crate::catalog::testutil::project_init("p"))
            .unwrap();
        cat.apply_update(&crate::catalog::testutil::update(
            p,
            1,
            1,
            vec![crate::catalog::testutil::fw("a", "ha", 4)],
            vec![],
            vec![],
        ))
        .unwrap();
        let discs_before = crate::catalog::testutil::count(&cat, "disc");
        let err = cat
            .apply_update(&copy_update(
                p,
                1,
                1,
                1,
                vec![crate::catalog::testutil::fw("a", "other", 4)],
            ))
            .unwrap_err();
        assert!(matches!(err, CatalogError::ConstraintViolation(_)), "{err}");
        assert_eq!(crate::catalog::testutil::count(&cat, "disc"), discs_before);
    }

    #[test]
    fn a_second_copy_on_the_same_disc_is_refused() {
        let (_dir, mut cat) = crate::catalog::testutil::temp_catalog();
        let p = cat
            .create_project(&crate::catalog::testutil::project_init("p"))
            .unwrap();
        cat.apply_update(&crate::catalog::testutil::update(
            p,
            1,
            1,
            vec![crate::catalog::testutil::fw("a", "ha", 4)],
            vec![],
            vec![],
        ))
        .unwrap();
        // Session 2 of the same disc 1A tries to place `a` again.
        let mut again = copy_update(p, 1, 0, 2, vec![crate::catalog::testutil::fw("a", "ha", 4)]);
        again.discs[0].is_new = false;
        let err = cat.apply_update(&again).unwrap_err();
        assert!(matches!(err, CatalogError::ConstraintViolation(_)), "{err}");
    }

    #[test]
    fn finalizing_and_verifying_touch_only_the_named_set() {
        let (_dir, mut cat) = crate::catalog::testutil::temp_catalog();
        let p = cat
            .create_project(&crate::catalog::testutil::project_init("p"))
            .unwrap();
        cat.apply_update(&crate::catalog::testutil::update(
            p,
            1,
            1,
            vec![crate::catalog::testutil::fw("a", "ha", 4)],
            vec![],
            vec![],
        ))
        .unwrap();
        cat.apply_update(&copy_update(
            p,
            1,
            1,
            1,
            vec![crate::catalog::testutil::fw("a", "ha", 4)],
        ))
        .unwrap();

        cat.mark_verified(p, DiscKey::new(1, 1), 1).unwrap();
        cat.apply_update(&CatalogUpdate {
            project_id: p,
            finalize_disc_ordinals: vec![DiscKey::new(1, 1)],
            discs: vec![],
        })
        .unwrap();

        let states = cat.read_disc_states(p).unwrap();
        assert_eq!(states[0].status, DiscStatus::Open, "1A untouched");
        assert_eq!(states[1].status, DiscStatus::Finalized, "1B sealed");
        let verified = cat
            .sessions_by_state(p, crate::catalog::model::SessionState::Verified)
            .unwrap();
        assert_eq!(
            verified.iter().map(|s| s.disc).collect::<Vec<_>>(),
            vec![DiscKey::new(1, 1)]
        );
    }

    #[test]
    fn a_current_version_whose_set_lost_its_last_copy_is_degraded_not_reburned() {
        let (_dir, mut cat) = crate::catalog::testutil::temp_catalog();
        let p = seed(&mut cat);
        // 1B holds a copy of a@a1, written through the copy-placement path.
        cat.apply_update(&copy_update(p, 1, 1, 1, vec![fw("a", "a1", 10)]))
            .expect("1B is burned");
        // `a` is current (a@a1, never modified) and lives in sets 0 and 1.
        let before = cat.read_view(p).unwrap();
        let a = &before.current[&RelPath("a".into())];
        assert_eq!(a.content_hash.as_str(), "a1");
        assert_eq!(a.live_sets, std::collections::BTreeSet::from([0, 1]));

        let report = cat
            .mark_dead(p, DiscKey::new(1, 0), &dead_at(), &Default::default())
            .unwrap();
        assert_eq!(report.disc, DiscKey::new(1, 0));
        assert!(report.reburn.iter().all(|r| r.rel_path.as_str() != "a"));
        assert!(report.lost.iter().all(|l| l.rel_path.as_str() != "a"));
        assert_eq!(
            report
                .degraded
                .iter()
                .map(|d| (d.rel_path.as_str(), d.content_hash.as_str()))
                .collect::<Vec<_>>(),
            vec![("a", "a1")]
        );
        assert_eq!(report.superseded_survivors, 0);
        // Degraded is derived: nothing about `a` is written to lost_version.
        assert!(fates(&cat).iter().all(|(path, _)| path != "a"));
    }

    #[test]
    fn a_current_version_still_covered_in_its_own_set_is_not_degraded() {
        let (_dir, mut cat) = crate::catalog::testutil::temp_catalog();
        let p = seed(&mut cat);
        cat.apply_update(&update(p, 2, 1, vec![fw("c", "c1", 5)], vec![], vec![]))
            .expect("disc 2 exists");
        also_place_a_on_disc_2(&cat);

        let report = cat
            .mark_dead(p, DiscKey::new(1, 0), &dead_at(), &Default::default())
            .unwrap();
        // Set 0 still holds a@a1 on 2A: nothing to restore, nothing lost.
        assert!(report.degraded.is_empty());
        assert!(report.reburn.iter().all(|r| r.rel_path.as_str() != "a"));
        assert!(report.lost.iter().all(|l| l.rel_path.as_str() != "a"));
        assert_eq!(report.superseded_survivors, 0);
        assert!(fates(&cat).iter().all(|(path, _)| path != "a"));
    }

    #[test]
    fn a_superseded_version_with_another_live_copy_is_a_survivor_not_lost() {
        let (_dir, mut cat) = crate::catalog::testutil::temp_catalog();
        let p = seed(&mut cat);
        cat.apply_update(&update(p, 2, 1, vec![fw("c", "c1", 5)], vec![], vec![]))
            .expect("disc 2 exists");
        // b@b1 is superseded by b@b2; give it a copy on live disc 2A (set 0,
        // so copy_index 0). The copy path refuses a non-current hash, so the
        // row is written raw.
        cat.raw_connection_for_tests()
            .execute(
                "insert into placement (file_version_id, session_id, disc_path, copy_index)
                 select (select min(id) from file_version where rel_path = 'b'),
                        (select s.id from session s join disc d on d.id = s.disc_id
                         where d.ordinal = 2),
                        '/data/b', 0",
                [],
            )
            .expect("the extra placement inserts");

        let report = cat
            .mark_dead(p, DiscKey::new(1, 0), &dead_at(), &Default::default())
            .unwrap();
        assert!(report.lost.is_empty());
        assert!(report.degraded.is_empty());
        assert_eq!(report.superseded_survivors, 1);
        assert_eq!(
            fates(&cat),
            vec![
                ("a".to_string(), "REBURN".to_string()),
                ("b".to_string(), "REBURN".to_string()),
            ]
        );
    }

    #[test]
    fn a_single_copy_project_never_reports_degraded_versions() {
        let (_dir, mut cat) = crate::catalog::testutil::temp_catalog();
        let p = seed(&mut cat);
        let report = cat
            .mark_dead(p, DiscKey::new(1, 0), &dead_at(), &Default::default())
            .unwrap();
        assert!(report.degraded.is_empty());
        assert_eq!(report.superseded_survivors, 0);
    }

    #[test]
    fn live_sets_lists_both_sets_until_one_copy_dies() {
        let (_dir, mut cat) = crate::catalog::testutil::temp_catalog();
        let p = cat
            .create_project(&crate::catalog::testutil::project_init("p"))
            .unwrap();
        cat.apply_update(&update(p, 1, 1, vec![fw("a", "ha", 4)], vec![], vec![]))
            .unwrap();
        cat.apply_update(&copy_update(p, 1, 1, 1, vec![fw("a", "ha", 4)]))
            .unwrap();
        let live = |cat: &crate::catalog::Catalog| {
            cat.read_view(p).unwrap().current[&RelPath("a".into())]
                .live_sets
                .clone()
        };
        assert_eq!(live(&cat), std::collections::BTreeSet::from([0, 1]));

        let report = cat
            .mark_dead(p, DiscKey::new(1, 1), &dead_at(), &Default::default())
            .unwrap();
        assert_eq!(live(&cat), std::collections::BTreeSet::from([0]));
        // 1A still holds `a`, so losing 1B degrades it rather than reburning.
        assert!(report.reburn.is_empty());
        assert_eq!(report.degraded.len(), 1);
    }
}
