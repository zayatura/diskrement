//! The catalog's write path: projects, burn updates, verification, and the
//! mark-dead event.
//!
//! Every method here takes `&mut self` because `Connection::transaction`
//! does — which makes "this operation mutates the catalog" a fact the compiler
//! knows rather than a convention.

use std::str::FromStr;

use rusqlite::{params, OptionalExtension};

use super::model::{Project, ProjectId, ProjectInit};
use super::time;
use super::Catalog;
use crate::error::{is_unique_violation, CatalogError};
use diskrement_core::DiscType;

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
}

#[cfg(test)]
mod tests {
    use diskrement_core::DiscType;

    use crate::catalog::testutil::{project_init, temp_catalog};
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
}
