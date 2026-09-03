//! [`ProjectRepository`] over [`SqlStore`]: a [`Project`] loaded with its
//! [`FieldDefinition`]s and project-local [`RelationshipKind`]s
//! (`docs/DOMAIN.md` §7 — "config-sized", loaded together).

use anamnesis_app::{ProjectAggregate, ProjectRepository, RepoError};
use anamnesis_core::{
    AreaId, FieldDefinition, FieldId, KindId, Project, ProjectId, RelationshipKind,
};
use async_trait::async_trait;
use sqlx::{PgPool, Row, SqlitePool};

use super::{
    Backend, SqlStore, field_kind_from_text, field_kind_to_text, parse_uuid,
    project_status_from_text, project_status_to_text, timestamp_from_seconds, title_from_text,
};

#[allow(clippy::too_many_arguments)]
fn assemble_project(
    id: uuid::Uuid,
    area_id: uuid::Uuid,
    title: String,
    description: String,
    status: String,
    created_at: i64,
    updated_at: i64,
    archived_at: Option<i64>,
) -> Result<Project, RepoError> {
    Ok(Project {
        id: ProjectId::new(id),
        area_id: AreaId::new(area_id),
        title: title_from_text(title)?,
        description,
        status: project_status_from_text(&status)?,
        created_at: timestamp_from_seconds(created_at)?,
        updated_at: timestamp_from_seconds(updated_at)?,
        archived_at: archived_at.map(timestamp_from_seconds).transpose()?,
    })
}

fn assemble_field_definition(
    id: uuid::Uuid,
    project_id: uuid::Uuid,
    name: String,
    kind: String,
    position: i64,
    show_on_card: bool,
) -> Result<FieldDefinition, RepoError> {
    Ok(FieldDefinition {
        id: FieldId::new(id),
        project_id: ProjectId::new(project_id),
        name: title_from_text(name)?,
        kind: field_kind_from_text(&kind)?,
        position: u32::try_from(position)
            .map_err(|e| RepoError::from_source("invalid stored field position", e))?,
        show_on_card,
    })
}

fn assemble_relationship_kind(
    id: uuid::Uuid,
    project_id: uuid::Uuid,
    forward_label: String,
    reverse_label: String,
) -> Result<RelationshipKind, RepoError> {
    Ok(RelationshipKind {
        id: KindId::new(id),
        project_id: Some(ProjectId::new(project_id)),
        forward_label: title_from_text(forward_label)?,
        reverse_label: title_from_text(reverse_label)?,
    })
}

mod sqlite_impl {
    use super::*;

    /// The [`FieldDefinition`]s of one project — split out of `load` so
    /// that function reads as "load the project row, then its two related
    /// collections" instead of interleaving three queries' worth of
    /// binding, fetching and row-mapping in one body.
    async fn load_field_definitions(
        pool: &SqlitePool,
        project_id_text: &str,
    ) -> Result<Vec<FieldDefinition>, RepoError> {
        let rows = sqlx::query(
            "SELECT id, project_id, name, kind, position, show_on_card \
             FROM field_definitions WHERE project_id = ? ORDER BY position",
        )
        .bind(project_id_text)
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to load field definitions", e))?;
        rows.into_iter()
            .map(|row| {
                assemble_field_definition(
                    parse_uuid(&row.get::<String, _>("id"))?,
                    parse_uuid(&row.get::<String, _>("project_id"))?,
                    row.get("name"),
                    row.get("kind"),
                    row.get::<i64, _>("position"),
                    row.get::<i64, _>("show_on_card") != 0,
                )
            })
            .collect()
    }

    /// The project-local [`RelationshipKind`]s of one project — the
    /// counterpart of `load_field_definitions` above.
    async fn load_relationship_kinds(
        pool: &SqlitePool,
        project_id_text: &str,
    ) -> Result<Vec<RelationshipKind>, RepoError> {
        let rows = sqlx::query(
            "SELECT id, project_id, forward_label, reverse_label \
             FROM relationship_kinds WHERE project_id = ?",
        )
        .bind(project_id_text)
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to load relationship kinds", e))?;
        rows.into_iter()
            .map(|row| {
                assemble_relationship_kind(
                    parse_uuid(&row.get::<String, _>("id"))?,
                    parse_uuid(&row.get::<String, _>("project_id"))?,
                    row.get("forward_label"),
                    row.get("reverse_label"),
                )
            })
            .collect()
    }

    pub(super) async fn load(
        pool: &SqlitePool,
        id: ProjectId,
    ) -> Result<Option<ProjectAggregate>, RepoError> {
        let id_text = id.as_uuid().to_string();
        let Some(row) = sqlx::query(
            "SELECT id, area_id, title, description, status, created_at, updated_at, archived_at \
             FROM projects WHERE id = ?",
        )
        .bind(&id_text)
        .fetch_optional(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to load project", e))?
        else {
            return Ok(None);
        };
        let project = assemble_project(
            parse_uuid(&row.get::<String, _>("id"))?,
            parse_uuid(&row.get::<String, _>("area_id"))?,
            row.get("title"),
            row.get("description"),
            row.get("status"),
            row.get::<i64, _>("created_at"),
            row.get::<i64, _>("updated_at"),
            row.get::<Option<i64>, _>("archived_at"),
        )?;

        let field_definitions = load_field_definitions(pool, &id_text).await?;
        let relationship_kinds = load_relationship_kinds(pool, &id_text).await?;

        Ok(Some(ProjectAggregate {
            project,
            field_definitions,
            relationship_kinds,
        }))
    }

    pub(super) async fn list_by_area(
        pool: &SqlitePool,
        area_id: AreaId,
    ) -> Result<Vec<Project>, RepoError> {
        let rows = sqlx::query(
            "SELECT id, area_id, title, description, status, created_at, updated_at, archived_at \
             FROM projects WHERE area_id = ? AND archived_at IS NULL ORDER BY created_at",
        )
        .bind(area_id.as_uuid().to_string())
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to list projects for area", e))?;
        rows.into_iter()
            .map(|row| {
                assemble_project(
                    parse_uuid(&row.get::<String, _>("id"))?,
                    parse_uuid(&row.get::<String, _>("area_id"))?,
                    row.get("title"),
                    row.get("description"),
                    row.get("status"),
                    row.get::<i64, _>("created_at"),
                    row.get::<i64, _>("updated_at"),
                    row.get::<Option<i64>, _>("archived_at"),
                )
            })
            .collect()
    }

    pub(super) async fn list_all(pool: &SqlitePool) -> Result<Vec<Project>, RepoError> {
        let rows = sqlx::query(
            "SELECT id, area_id, title, description, status, created_at, updated_at, archived_at \
             FROM projects ORDER BY created_at",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to list all projects", e))?;
        rows.into_iter()
            .map(|row| {
                assemble_project(
                    parse_uuid(&row.get::<String, _>("id"))?,
                    parse_uuid(&row.get::<String, _>("area_id"))?,
                    row.get("title"),
                    row.get("description"),
                    row.get("status"),
                    row.get::<i64, _>("created_at"),
                    row.get::<i64, _>("updated_at"),
                    row.get::<Option<i64>, _>("archived_at"),
                )
            })
            .collect()
    }

    pub(super) async fn count_active(
        pool: &SqlitePool,
        excluding: Option<ProjectId>,
    ) -> Result<u32, RepoError> {
        let row = sqlx::query(
            "SELECT COUNT(*) AS n FROM projects WHERE status = 'active' \
             AND (?1 IS NULL OR id != ?1)",
        )
        .bind(excluding.map(|id| id.as_uuid().to_string()))
        .fetch_one(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to count active projects", e))?;
        u32::try_from(row.get::<i64, _>("n"))
            .map_err(|e| RepoError::from_source("active project count out of range", e))
    }

    pub(super) async fn insert(pool: &SqlitePool, project: &Project) -> Result<(), RepoError> {
        sqlx::query(
            "INSERT INTO projects \
             (id, area_id, title, description, status, created_at, updated_at, archived_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(project.id.as_uuid().to_string())
        .bind(project.area_id.as_uuid().to_string())
        .bind(project.title.as_str())
        .bind(&project.description)
        .bind(project_status_to_text(project.status))
        .bind(project.created_at.unix_seconds())
        .bind(project.updated_at.unix_seconds())
        .bind(project.archived_at.map(|t| t.unix_seconds()))
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to insert project", e))?;
        Ok(())
    }

    pub(super) async fn update(pool: &SqlitePool, project: &Project) -> Result<(), RepoError> {
        sqlx::query(
            "UPDATE projects SET title = ?, description = ?, status = ?, updated_at = ?, \
             archived_at = ? WHERE id = ?",
        )
        .bind(project.title.as_str())
        .bind(&project.description)
        .bind(project_status_to_text(project.status))
        .bind(project.updated_at.unix_seconds())
        .bind(project.archived_at.map(|t| t.unix_seconds()))
        .bind(project.id.as_uuid().to_string())
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to update project", e))?;
        Ok(())
    }

    pub(super) async fn insert_field_definition(
        pool: &SqlitePool,
        definition: &FieldDefinition,
    ) -> Result<(), RepoError> {
        sqlx::query(
            "INSERT INTO field_definitions (id, project_id, name, kind, position, show_on_card) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(definition.id.as_uuid().to_string())
        .bind(definition.project_id.as_uuid().to_string())
        .bind(definition.name.as_str())
        .bind(field_kind_to_text(definition.kind))
        .bind(i64::from(definition.position))
        .bind(definition.show_on_card)
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to insert field definition", e))?;
        Ok(())
    }

    pub(super) async fn update_field_definition(
        pool: &SqlitePool,
        definition: &FieldDefinition,
    ) -> Result<(), RepoError> {
        sqlx::query(
            "UPDATE field_definitions SET name = ?, position = ?, show_on_card = ? WHERE id = ?",
        )
        .bind(definition.name.as_str())
        .bind(i64::from(definition.position))
        .bind(definition.show_on_card)
        .bind(definition.id.as_uuid().to_string())
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to update field definition", e))?;
        Ok(())
    }

    pub(super) async fn insert_relationship_kind(
        pool: &SqlitePool,
        kind: &RelationshipKind,
    ) -> Result<(), RepoError> {
        let project_id = kind
            .project_id
            .ok_or_else(|| RepoError::new("cannot store a builtin relationship kind"))?;
        sqlx::query(
            "INSERT INTO relationship_kinds (id, project_id, forward_label, reverse_label) \
             VALUES (?, ?, ?, ?)",
        )
        .bind(kind.id.as_uuid().to_string())
        .bind(project_id.as_uuid().to_string())
        .bind(kind.forward_label.as_str())
        .bind(kind.reverse_label.as_str())
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to insert relationship kind", e))?;
        Ok(())
    }

    pub(super) async fn load_relationship_kind(
        pool: &SqlitePool,
        id: KindId,
    ) -> Result<Option<RelationshipKind>, RepoError> {
        let Some(row) = sqlx::query(
            "SELECT id, project_id, forward_label, reverse_label \
             FROM relationship_kinds WHERE id = ?",
        )
        .bind(id.as_uuid().to_string())
        .fetch_optional(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to load relationship kind", e))?
        else {
            return Ok(None);
        };
        Ok(Some(assemble_relationship_kind(
            parse_uuid(&row.get::<String, _>("id"))?,
            parse_uuid(&row.get::<String, _>("project_id"))?,
            row.get("forward_label"),
            row.get("reverse_label"),
        )?))
    }
}

mod postgres_impl {
    use super::*;

    /// See `sqlite_impl::load_field_definitions` — same split, Postgres
    /// row types.
    async fn load_field_definitions(
        pool: &PgPool,
        project_id: uuid::Uuid,
    ) -> Result<Vec<FieldDefinition>, RepoError> {
        let rows = sqlx::query(
            "SELECT id, project_id, name, kind, position, show_on_card \
             FROM field_definitions WHERE project_id = $1 ORDER BY position",
        )
        .bind(project_id)
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to load field definitions", e))?;
        rows.into_iter()
            .map(|row| {
                assemble_field_definition(
                    row.get::<uuid::Uuid, _>("id"),
                    row.get::<uuid::Uuid, _>("project_id"),
                    row.get("name"),
                    row.get("kind"),
                    i64::from(row.get::<i32, _>("position")),
                    row.get("show_on_card"),
                )
            })
            .collect()
    }

    /// See `sqlite_impl::load_relationship_kinds` — same split, Postgres
    /// row types.
    async fn load_relationship_kinds(
        pool: &PgPool,
        project_id: uuid::Uuid,
    ) -> Result<Vec<RelationshipKind>, RepoError> {
        let rows = sqlx::query(
            "SELECT id, project_id, forward_label, reverse_label \
             FROM relationship_kinds WHERE project_id = $1",
        )
        .bind(project_id)
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to load relationship kinds", e))?;
        rows.into_iter()
            .map(|row| {
                assemble_relationship_kind(
                    row.get::<uuid::Uuid, _>("id"),
                    row.get::<uuid::Uuid, _>("project_id"),
                    row.get("forward_label"),
                    row.get("reverse_label"),
                )
            })
            .collect()
    }

    pub(super) async fn load(
        pool: &PgPool,
        id: ProjectId,
    ) -> Result<Option<ProjectAggregate>, RepoError> {
        let Some(row) = sqlx::query(
            "SELECT id, area_id, title, description, status, created_at, updated_at, archived_at \
             FROM projects WHERE id = $1",
        )
        .bind(id.as_uuid())
        .fetch_optional(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to load project", e))?
        else {
            return Ok(None);
        };
        let project = assemble_project(
            row.get::<uuid::Uuid, _>("id"),
            row.get::<uuid::Uuid, _>("area_id"),
            row.get("title"),
            row.get("description"),
            row.get("status"),
            row.get::<i64, _>("created_at"),
            row.get::<i64, _>("updated_at"),
            row.get::<Option<i64>, _>("archived_at"),
        )?;

        let field_definitions = load_field_definitions(pool, id.as_uuid()).await?;
        let relationship_kinds = load_relationship_kinds(pool, id.as_uuid()).await?;

        Ok(Some(ProjectAggregate {
            project,
            field_definitions,
            relationship_kinds,
        }))
    }

    pub(super) async fn list_by_area(
        pool: &PgPool,
        area_id: AreaId,
    ) -> Result<Vec<Project>, RepoError> {
        let rows = sqlx::query(
            "SELECT id, area_id, title, description, status, created_at, updated_at, archived_at \
             FROM projects WHERE area_id = $1 AND archived_at IS NULL ORDER BY created_at",
        )
        .bind(area_id.as_uuid())
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to list projects for area", e))?;
        rows.into_iter()
            .map(|row| {
                assemble_project(
                    row.get::<uuid::Uuid, _>("id"),
                    row.get::<uuid::Uuid, _>("area_id"),
                    row.get("title"),
                    row.get("description"),
                    row.get("status"),
                    row.get::<i64, _>("created_at"),
                    row.get::<i64, _>("updated_at"),
                    row.get::<Option<i64>, _>("archived_at"),
                )
            })
            .collect()
    }

    pub(super) async fn list_all(pool: &PgPool) -> Result<Vec<Project>, RepoError> {
        let rows = sqlx::query(
            "SELECT id, area_id, title, description, status, created_at, updated_at, archived_at \
             FROM projects ORDER BY created_at",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to list all projects", e))?;
        rows.into_iter()
            .map(|row| {
                assemble_project(
                    row.get::<uuid::Uuid, _>("id"),
                    row.get::<uuid::Uuid, _>("area_id"),
                    row.get("title"),
                    row.get("description"),
                    row.get("status"),
                    row.get::<i64, _>("created_at"),
                    row.get::<i64, _>("updated_at"),
                    row.get::<Option<i64>, _>("archived_at"),
                )
            })
            .collect()
    }

    pub(super) async fn count_active(
        pool: &PgPool,
        excluding: Option<ProjectId>,
    ) -> Result<u32, RepoError> {
        let row = sqlx::query(
            "SELECT COUNT(*) AS n FROM projects WHERE status = 'active' \
             AND ($1::uuid IS NULL OR id != $1)",
        )
        .bind(excluding.map(|id| id.as_uuid()))
        .fetch_one(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to count active projects", e))?;
        u32::try_from(row.get::<i64, _>("n"))
            .map_err(|e| RepoError::from_source("active project count out of range", e))
    }

    pub(super) async fn insert(pool: &PgPool, project: &Project) -> Result<(), RepoError> {
        sqlx::query(
            "INSERT INTO projects \
             (id, area_id, title, description, status, created_at, updated_at, archived_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(project.id.as_uuid())
        .bind(project.area_id.as_uuid())
        .bind(project.title.as_str())
        .bind(&project.description)
        .bind(project_status_to_text(project.status))
        .bind(project.created_at.unix_seconds())
        .bind(project.updated_at.unix_seconds())
        .bind(project.archived_at.map(|t| t.unix_seconds()))
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to insert project", e))?;
        Ok(())
    }

    pub(super) async fn update(pool: &PgPool, project: &Project) -> Result<(), RepoError> {
        sqlx::query(
            "UPDATE projects SET title = $1, description = $2, status = $3, updated_at = $4, \
             archived_at = $5 WHERE id = $6",
        )
        .bind(project.title.as_str())
        .bind(&project.description)
        .bind(project_status_to_text(project.status))
        .bind(project.updated_at.unix_seconds())
        .bind(project.archived_at.map(|t| t.unix_seconds()))
        .bind(project.id.as_uuid())
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to update project", e))?;
        Ok(())
    }

    pub(super) async fn insert_field_definition(
        pool: &PgPool,
        definition: &FieldDefinition,
    ) -> Result<(), RepoError> {
        let position = i32::try_from(definition.position)
            .map_err(|e| RepoError::from_source("field position out of range", e))?;
        sqlx::query(
            "INSERT INTO field_definitions (id, project_id, name, kind, position, show_on_card) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(definition.id.as_uuid())
        .bind(definition.project_id.as_uuid())
        .bind(definition.name.as_str())
        .bind(field_kind_to_text(definition.kind))
        .bind(position)
        .bind(definition.show_on_card)
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to insert field definition", e))?;
        Ok(())
    }

    pub(super) async fn update_field_definition(
        pool: &PgPool,
        definition: &FieldDefinition,
    ) -> Result<(), RepoError> {
        let position = i32::try_from(definition.position)
            .map_err(|e| RepoError::from_source("field position out of range", e))?;
        sqlx::query(
            "UPDATE field_definitions SET name = $1, position = $2, show_on_card = $3 WHERE id = $4",
        )
        .bind(definition.name.as_str())
        .bind(position)
        .bind(definition.show_on_card)
        .bind(definition.id.as_uuid())
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to update field definition", e))?;
        Ok(())
    }

    pub(super) async fn insert_relationship_kind(
        pool: &PgPool,
        kind: &RelationshipKind,
    ) -> Result<(), RepoError> {
        let project_id = kind
            .project_id
            .ok_or_else(|| RepoError::new("cannot store a builtin relationship kind"))?;
        sqlx::query(
            "INSERT INTO relationship_kinds (id, project_id, forward_label, reverse_label) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(kind.id.as_uuid())
        .bind(project_id.as_uuid())
        .bind(kind.forward_label.as_str())
        .bind(kind.reverse_label.as_str())
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to insert relationship kind", e))?;
        Ok(())
    }

    pub(super) async fn load_relationship_kind(
        pool: &PgPool,
        id: KindId,
    ) -> Result<Option<RelationshipKind>, RepoError> {
        let Some(row) = sqlx::query(
            "SELECT id, project_id, forward_label, reverse_label \
             FROM relationship_kinds WHERE id = $1",
        )
        .bind(id.as_uuid())
        .fetch_optional(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to load relationship kind", e))?
        else {
            return Ok(None);
        };
        Ok(Some(assemble_relationship_kind(
            row.get::<uuid::Uuid, _>("id"),
            row.get::<uuid::Uuid, _>("project_id"),
            row.get("forward_label"),
            row.get("reverse_label"),
        )?))
    }
}

#[async_trait]
impl ProjectRepository for SqlStore {
    async fn load(&self, id: ProjectId) -> Result<Option<ProjectAggregate>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::load(pool, id).await,
            Backend::Postgres(pool) => postgres_impl::load(pool, id).await,
        }
    }

    async fn list_by_area(&self, area_id: AreaId) -> Result<Vec<Project>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::list_by_area(pool, area_id).await,
            Backend::Postgres(pool) => postgres_impl::list_by_area(pool, area_id).await,
        }
    }

    async fn list_all(&self) -> Result<Vec<Project>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::list_all(pool).await,
            Backend::Postgres(pool) => postgres_impl::list_all(pool).await,
        }
    }

    async fn count_active(&self, excluding: Option<ProjectId>) -> Result<u32, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::count_active(pool, excluding).await,
            Backend::Postgres(pool) => postgres_impl::count_active(pool, excluding).await,
        }
    }

    async fn insert(&self, project: &Project) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::insert(pool, project).await,
            Backend::Postgres(pool) => postgres_impl::insert(pool, project).await,
        }
    }

    async fn update(&self, project: &Project) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::update(pool, project).await,
            Backend::Postgres(pool) => postgres_impl::update(pool, project).await,
        }
    }

    async fn insert_field_definition(&self, definition: &FieldDefinition) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::insert_field_definition(pool, definition).await,
            Backend::Postgres(pool) => {
                postgres_impl::insert_field_definition(pool, definition).await
            }
        }
    }

    async fn update_field_definition(&self, definition: &FieldDefinition) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::update_field_definition(pool, definition).await,
            Backend::Postgres(pool) => {
                postgres_impl::update_field_definition(pool, definition).await
            }
        }
    }

    async fn insert_relationship_kind(&self, kind: &RelationshipKind) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::insert_relationship_kind(pool, kind).await,
            Backend::Postgres(pool) => postgres_impl::insert_relationship_kind(pool, kind).await,
        }
    }

    async fn load_relationship_kind(
        &self,
        id: KindId,
    ) -> Result<Option<RelationshipKind>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::load_relationship_kind(pool, id).await,
            Backend::Postgres(pool) => postgres_impl::load_relationship_kind(pool, id).await,
        }
    }
}
