//! [`SearchIndex`] (write side) and [`SearchQuery`] (read side) over
//! [`SqlStore`] — the largest backend divergence in the schema
//! (`docs/DOMAIN.md` §7): SQLite FTS5 vs Postgres `tsvector`/GIN, kept
//! behind one shared contract test (`tests/search.rs`).
//!
//! Matching semantics differ by design, not oversight: FTS5's `MATCH` and
//! Postgres's `plainto_tsquery` are both *token*-based (whole-word) full
//! text search, not substring search — searching `"regrout"` finds a title
//! containing that whole word, not a title merely containing the substring
//! `"grout"`. The shared contract test only ever searches for a complete
//! word from a title for exactly this reason.

use anamnesis_app::{RepoError, SearchHit, SearchIndex, SearchQuery};
use anamnesis_core::{AreaId, ProjectId, TaskId};
use async_trait::async_trait;
use sqlx::{PgPool, Row, SqlitePool};

use super::{Backend, SqlStore, parse_uuid};

fn hit(kind: &str, id: uuid::Uuid, title: String) -> Result<SearchHit, RepoError> {
    match kind {
        "area" => Ok(SearchHit::Area {
            id: AreaId::new(id),
            title,
        }),
        "project" => Ok(SearchHit::Project {
            id: ProjectId::new(id),
            title,
        }),
        "task" => Ok(SearchHit::Task {
            id: TaskId::new(id),
            title,
        }),
        other => Err(RepoError::new(format!(
            "invalid stored search entity kind {other:?}"
        ))),
    }
}

mod sqlite_impl {
    use super::*;

    /// FTS5 virtual tables carry no unique constraint to upsert against, so
    /// indexing an entity that is already present is a delete-then-insert.
    /// Always writes `archived = 0` — the upsert path is create, edit, or
    /// *unarchive* (`crate::ports::infra::SearchIndex`'s trait doc comment),
    /// never a state that should stay flagged archived.
    async fn upsert(
        pool: &SqlitePool,
        kind: &str,
        id: uuid::Uuid,
        title: &str,
    ) -> Result<(), RepoError> {
        let id_text = id.to_string();
        sqlx::query("DELETE FROM search_documents WHERE entity_kind = ? AND entity_id = ?")
            .bind(kind)
            .bind(&id_text)
            .execute(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to clear old search entry", e))?;
        sqlx::query(
            "INSERT INTO search_documents (entity_kind, entity_id, archived, title) \
             VALUES (?, ?, 0, ?)",
        )
        .bind(kind)
        .bind(&id_text)
        .bind(title)
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to index entity", e))?;
        Ok(())
    }

    /// Flags the entry as archived rather than deleting it — see
    /// `crate::ports::infra::SearchIndex`'s trait doc comment. A no-op update
    /// (matches zero rows) if the entity was never indexed in the first
    /// place, which is not an error.
    async fn remove(pool: &SqlitePool, kind: &str, id: uuid::Uuid) -> Result<(), RepoError> {
        sqlx::query(
            "UPDATE search_documents SET archived = 1 WHERE entity_kind = ? AND entity_id = ?",
        )
        .bind(kind)
        .bind(id.to_string())
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to archive search entry", e))?;
        Ok(())
    }

    pub(super) async fn index_area(
        pool: &SqlitePool,
        id: AreaId,
        title: &str,
    ) -> Result<(), RepoError> {
        upsert(pool, "area", id.as_uuid(), title).await
    }
    pub(super) async fn index_project(
        pool: &SqlitePool,
        id: ProjectId,
        title: &str,
    ) -> Result<(), RepoError> {
        upsert(pool, "project", id.as_uuid(), title).await
    }
    pub(super) async fn index_task(
        pool: &SqlitePool,
        id: TaskId,
        title: &str,
    ) -> Result<(), RepoError> {
        upsert(pool, "task", id.as_uuid(), title).await
    }
    pub(super) async fn remove_area(pool: &SqlitePool, id: AreaId) -> Result<(), RepoError> {
        remove(pool, "area", id.as_uuid()).await
    }
    pub(super) async fn remove_project(pool: &SqlitePool, id: ProjectId) -> Result<(), RepoError> {
        remove(pool, "project", id.as_uuid()).await
    }
    pub(super) async fn remove_task(pool: &SqlitePool, id: TaskId) -> Result<(), RepoError> {
        remove(pool, "task", id.as_uuid()).await
    }

    async fn search_filtered(
        pool: &SqlitePool,
        text: &str,
        archived: i64,
    ) -> Result<Vec<SearchHit>, RepoError> {
        if text.trim().is_empty() {
            return Ok(Vec::new());
        }
        // Quote the whole input as one FTS5 phrase so user text can never be
        // parsed as FTS5 query syntax (`OR`, `-`, `*`, ...).
        let phrase = format!("\"{}\"", text.replace('"', "\"\""));
        let rows = sqlx::query(
            "SELECT entity_kind, entity_id, title FROM search_documents \
             WHERE search_documents MATCH ? AND archived = ? ORDER BY rank",
        )
        .bind(phrase)
        .bind(archived)
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to search", e))?;
        rows.into_iter()
            .map(|row| {
                hit(
                    &row.get::<String, _>("entity_kind"),
                    parse_uuid(&row.get::<String, _>("entity_id"))?,
                    row.get("title"),
                )
            })
            .collect()
    }

    pub(super) async fn search(pool: &SqlitePool, text: &str) -> Result<Vec<SearchHit>, RepoError> {
        search_filtered(pool, text, 0).await
    }

    pub(super) async fn search_archived(
        pool: &SqlitePool,
        text: &str,
    ) -> Result<Vec<SearchHit>, RepoError> {
        search_filtered(pool, text, 1).await
    }
}

mod postgres_impl {
    use super::*;

    /// Always writes `archived = false` — the upsert path is create, edit,
    /// or *unarchive* (`crate::ports::infra::SearchIndex`'s trait doc
    /// comment), never a state that should stay flagged archived.
    async fn upsert(
        pool: &PgPool,
        kind: &str,
        id: uuid::Uuid,
        title: &str,
    ) -> Result<(), RepoError> {
        sqlx::query(
            "INSERT INTO search_documents (entity_kind, entity_id, archived, title) \
             VALUES ($1, $2, false, $3) \
             ON CONFLICT (entity_kind, entity_id) \
             DO UPDATE SET title = excluded.title, archived = false",
        )
        .bind(kind)
        .bind(id)
        .bind(title)
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to index entity", e))?;
        Ok(())
    }

    /// Flags the entry as archived rather than deleting it — see
    /// `crate::ports::infra::SearchIndex`'s trait doc comment. A no-op update
    /// (matches zero rows) if the entity was never indexed in the first
    /// place, which is not an error.
    async fn remove(pool: &PgPool, kind: &str, id: uuid::Uuid) -> Result<(), RepoError> {
        sqlx::query(
            "UPDATE search_documents SET archived = true WHERE entity_kind = $1 AND entity_id = $2",
        )
        .bind(kind)
        .bind(id)
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to archive search entry", e))?;
        Ok(())
    }

    pub(super) async fn index_area(
        pool: &PgPool,
        id: AreaId,
        title: &str,
    ) -> Result<(), RepoError> {
        upsert(pool, "area", id.as_uuid(), title).await
    }
    pub(super) async fn index_project(
        pool: &PgPool,
        id: ProjectId,
        title: &str,
    ) -> Result<(), RepoError> {
        upsert(pool, "project", id.as_uuid(), title).await
    }
    pub(super) async fn index_task(
        pool: &PgPool,
        id: TaskId,
        title: &str,
    ) -> Result<(), RepoError> {
        upsert(pool, "task", id.as_uuid(), title).await
    }
    pub(super) async fn remove_area(pool: &PgPool, id: AreaId) -> Result<(), RepoError> {
        remove(pool, "area", id.as_uuid()).await
    }
    pub(super) async fn remove_project(pool: &PgPool, id: ProjectId) -> Result<(), RepoError> {
        remove(pool, "project", id.as_uuid()).await
    }
    pub(super) async fn remove_task(pool: &PgPool, id: TaskId) -> Result<(), RepoError> {
        remove(pool, "task", id.as_uuid()).await
    }

    async fn search_filtered(
        pool: &PgPool,
        text: &str,
        archived: bool,
    ) -> Result<Vec<SearchHit>, RepoError> {
        if text.trim().is_empty() {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(
            "SELECT entity_kind, entity_id, title FROM search_documents \
             WHERE tsv @@ plainto_tsquery('english', $1) AND archived = $2 \
             ORDER BY ts_rank(tsv, plainto_tsquery('english', $1)) DESC",
        )
        .bind(text)
        .bind(archived)
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to search", e))?;
        rows.into_iter()
            .map(|row| {
                hit(
                    &row.get::<String, _>("entity_kind"),
                    row.get::<uuid::Uuid, _>("entity_id"),
                    row.get("title"),
                )
            })
            .collect()
    }

    pub(super) async fn search(pool: &PgPool, text: &str) -> Result<Vec<SearchHit>, RepoError> {
        search_filtered(pool, text, false).await
    }

    pub(super) async fn search_archived(
        pool: &PgPool,
        text: &str,
    ) -> Result<Vec<SearchHit>, RepoError> {
        search_filtered(pool, text, true).await
    }
}

#[async_trait]
impl SearchIndex for SqlStore {
    async fn index_area(&self, id: AreaId, title: &str) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::index_area(pool, id, title).await,
            Backend::Postgres(pool) => postgres_impl::index_area(pool, id, title).await,
        }
    }

    async fn index_project(&self, id: ProjectId, title: &str) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::index_project(pool, id, title).await,
            Backend::Postgres(pool) => postgres_impl::index_project(pool, id, title).await,
        }
    }

    async fn index_task(&self, id: TaskId, title: &str) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::index_task(pool, id, title).await,
            Backend::Postgres(pool) => postgres_impl::index_task(pool, id, title).await,
        }
    }

    async fn remove_area(&self, id: AreaId) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::remove_area(pool, id).await,
            Backend::Postgres(pool) => postgres_impl::remove_area(pool, id).await,
        }
    }

    async fn remove_project(&self, id: ProjectId) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::remove_project(pool, id).await,
            Backend::Postgres(pool) => postgres_impl::remove_project(pool, id).await,
        }
    }

    async fn remove_task(&self, id: TaskId) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::remove_task(pool, id).await,
            Backend::Postgres(pool) => postgres_impl::remove_task(pool, id).await,
        }
    }
}

#[async_trait]
impl SearchQuery for SqlStore {
    async fn search(&self, text: &str) -> Result<Vec<SearchHit>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::search(pool, text).await,
            Backend::Postgres(pool) => postgres_impl::search(pool, text).await,
        }
    }

    async fn search_archived(&self, text: &str) -> Result<Vec<SearchHit>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::search_archived(pool, text).await,
            Backend::Postgres(pool) => postgres_impl::search_archived(pool, text).await,
        }
    }
}
