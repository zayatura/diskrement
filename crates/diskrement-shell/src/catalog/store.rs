//! The catalog's write path: projects, burn updates, verification, and the
//! mark-dead event.
//!
//! Writers take `&mut self` because `Connection::transaction` does — which
//! makes "this operation mutates the catalog" a fact the compiler knows rather
//! than a convention. Readers take `&self` and live in `read.rs`.

use std::str::FromStr;

use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension, Transaction};

use super::model::{CatalogUpdate, DiscWrite, FileWrite, Project, ProjectId, ProjectInit};
use super::time;
use super::Catalog;
use crate::error::{is_unique_violation, CatalogError};
use diskrement_core::{DiscStatus, DiscType, ManifestChangeKind, RelPath};

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
            for ordinal in &u.finalize_disc_ordinals {
                finalize_disc(tx, u.project_id, *ordinal)?;
            }
            for disc in &u.discs {
                write_disc(tx, u.project_id, disc)?;
            }
            Ok(())
        })
    }
}

/// Flip a pre-existing OPEN disc to FINALIZED. Anything else is an illegal
/// transition — including a disc that is already FINALIZED.
fn finalize_disc(
    tx: &Transaction<'_>,
    project: ProjectId,
    ordinal: u32,
) -> Result<(), CatalogError> {
    let updated = tx
        .execute(
            "update disc set status = 'FINALIZED'
             where project_id = ?1 and ordinal = ?2 and status = 'OPEN'",
            params![project.0, ordinal as i64],
        )
        .map_err(CatalogError::from_sqlite)?;
    if updated == 0 {
        return Err(CatalogError::IllegalStatusTransition(format!(
            "disc {ordinal} is not OPEN"
        )));
    }
    Ok(())
}

fn disc_id_by_ordinal(
    tx: &Transaction<'_>,
    project: ProjectId,
    ordinal: u32,
) -> Result<i64, CatalogError> {
    tx.query_row(
        "select id from disc where project_id = ?1 and ordinal = ?2",
        params![project.0, ordinal as i64],
        |row| row.get::<_, i64>(0),
    )
    .optional()
    .map_err(CatalogError::from_sqlite)?
    .ok_or_else(|| CatalogError::ConstraintViolation(format!("no disc with ordinal {ordinal}")))
}

/// Insert or find the disc row, insert its session, then write the session's
/// files and tombstones.
///
/// Sessions are always recorded as BURNED: no PLANNED row is ever persisted.
fn write_disc(tx: &Transaction<'_>, project: ProjectId, d: &DiscWrite) -> Result<(), CatalogError> {
    let disc_id = if d.is_new {
        tx.execute(
            "insert into disc (project_id, ordinal, disc_type, usable_bytes, status, label)
             values (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                project.0,
                d.ordinal as i64,
                d.disc_type.as_str(),
                d.usable_bytes as i64,
                d.status.as_str(),
                d.label,
            ],
        )
        .map_err(CatalogError::from_sqlite)?;
        tx.last_insert_rowid()
    } else {
        let id = disc_id_by_ordinal(tx, project, d.ordinal)?;
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
                    d.ordinal
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
        )?;
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
        params![
            file_version_id,
            session_id,
            f.disc_path,
            f.copy_index as i64
        ],
    )
    .map_err(CatalogError::from_sqlite)?;

    tx.execute(
        "insert into change_record (session_id, file_version_id, kind) values (?1, ?2, ?3)",
        params![session_id, file_version_id, kind.as_str()],
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
            finalize_disc_ordinals: vec![1],
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
}
