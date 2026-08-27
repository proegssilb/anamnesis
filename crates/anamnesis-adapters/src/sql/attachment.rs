//! [`AttachmentRepository`] over [`SqlStore`]: paged per task, a file or a
//! link (`docs/DOMAIN.md` §3).

use anamnesis_app::{Attachment, AttachmentId, AttachmentKind, AttachmentRepository, RepoError};
use anamnesis_core::TaskId;
use async_trait::async_trait;
use sqlx::{PgPool, Row, SqlitePool};

use super::{Backend, SqlStore, parse_uuid, timestamp_from_seconds};

#[allow(clippy::too_many_arguments)]
fn assemble(
    id: uuid::Uuid,
    task_id: uuid::Uuid,
    kind: String,
    url: Option<String>,
    blob_key: Option<String>,
    filename: Option<String>,
    mime: Option<String>,
    size: Option<i64>,
    created_at: i64,
) -> Result<Attachment, RepoError> {
    let missing = |col: &str| RepoError::new(format!("stored attachment missing {col}"));
    let kind = match kind.as_str() {
        "link" => AttachmentKind::Link {
            url: url.ok_or_else(|| missing("url"))?,
        },
        "file" => AttachmentKind::File {
            blob_key: blob_key.ok_or_else(|| missing("blob_key"))?,
            filename: filename.ok_or_else(|| missing("filename"))?,
            mime: mime.unwrap_or_default(),
            size: u64::try_from(size.ok_or_else(|| missing("size"))?)
                .map_err(|e| RepoError::from_source("stored attachment size out of range", e))?,
        },
        other => {
            return Err(RepoError::new(format!(
                "invalid stored attachment kind {other:?}"
            )));
        }
    };
    Ok(Attachment {
        id: AttachmentId::new(id),
        task_id: TaskId::new(task_id),
        kind,
        created_at: timestamp_from_seconds(created_at)?,
    })
}

/// The columns each [`AttachmentKind`] variant writes, independent of
/// backend.
struct EncodedAttachment<'a> {
    kind: &'static str,
    url: Option<&'a str>,
    blob_key: Option<&'a str>,
    filename: Option<&'a str>,
    mime: Option<&'a str>,
    size: Option<i64>,
}

fn encode(attachment: &Attachment) -> EncodedAttachment<'_> {
    match &attachment.kind {
        AttachmentKind::Link { url } => EncodedAttachment {
            kind: "link",
            url: Some(url),
            blob_key: None,
            filename: None,
            mime: None,
            size: None,
        },
        AttachmentKind::File {
            blob_key,
            filename,
            mime,
            size,
        } => EncodedAttachment {
            kind: "file",
            url: None,
            blob_key: Some(blob_key),
            filename: Some(filename),
            mime: Some(mime),
            size: Some(*size as i64),
        },
    }
}

mod sqlite_impl {
    use super::*;

    pub(super) async fn list_for_task(
        pool: &SqlitePool,
        task_id: TaskId,
    ) -> Result<Vec<Attachment>, RepoError> {
        let rows = sqlx::query(
            "SELECT id, task_id, kind, url, blob_key, filename, mime, size, created_at \
             FROM attachments WHERE task_id = ? ORDER BY created_at",
        )
        .bind(task_id.as_uuid().to_string())
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to list attachments for task", e))?;
        rows.into_iter()
            .map(|row| {
                assemble(
                    parse_uuid(&row.get::<String, _>("id"))?,
                    parse_uuid(&row.get::<String, _>("task_id"))?,
                    row.get("kind"),
                    row.get("url"),
                    row.get("blob_key"),
                    row.get("filename"),
                    row.get("mime"),
                    row.get::<Option<i64>, _>("size"),
                    row.get::<i64, _>("created_at"),
                )
            })
            .collect()
    }

    pub(super) async fn load(
        pool: &SqlitePool,
        id: AttachmentId,
    ) -> Result<Option<Attachment>, RepoError> {
        let Some(row) = sqlx::query(
            "SELECT id, task_id, kind, url, blob_key, filename, mime, size, created_at \
             FROM attachments WHERE id = ?",
        )
        .bind(id.as_uuid().to_string())
        .fetch_optional(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to load attachment", e))?
        else {
            return Ok(None);
        };
        Ok(Some(assemble(
            parse_uuid(&row.get::<String, _>("id"))?,
            parse_uuid(&row.get::<String, _>("task_id"))?,
            row.get("kind"),
            row.get("url"),
            row.get("blob_key"),
            row.get("filename"),
            row.get("mime"),
            row.get::<Option<i64>, _>("size"),
            row.get::<i64, _>("created_at"),
        )?))
    }

    pub(super) async fn insert(
        pool: &SqlitePool,
        attachment: &Attachment,
    ) -> Result<(), RepoError> {
        let e = encode(attachment);
        sqlx::query(
            "INSERT INTO attachments (id, task_id, kind, url, blob_key, filename, mime, size, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(attachment.id.as_uuid().to_string())
        .bind(attachment.task_id.as_uuid().to_string())
        .bind(e.kind)
        .bind(e.url)
        .bind(e.blob_key)
        .bind(e.filename)
        .bind(e.mime)
        .bind(e.size)
        .bind(attachment.created_at.unix_seconds())
        .execute(pool)
        .await
        .map_err(|err| RepoError::from_source("failed to insert attachment", err))?;
        Ok(())
    }

    pub(super) async fn delete(pool: &SqlitePool, id: AttachmentId) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM attachments WHERE id = ?")
            .bind(id.as_uuid().to_string())
            .execute(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to delete attachment", e))?;
        Ok(())
    }
}

mod postgres_impl {
    use super::*;

    pub(super) async fn list_for_task(
        pool: &PgPool,
        task_id: TaskId,
    ) -> Result<Vec<Attachment>, RepoError> {
        let rows = sqlx::query(
            "SELECT id, task_id, kind, url, blob_key, filename, mime, size, created_at \
             FROM attachments WHERE task_id = $1 ORDER BY created_at",
        )
        .bind(task_id.as_uuid())
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to list attachments for task", e))?;
        rows.into_iter()
            .map(|row| {
                assemble(
                    row.get::<uuid::Uuid, _>("id"),
                    row.get::<uuid::Uuid, _>("task_id"),
                    row.get("kind"),
                    row.get("url"),
                    row.get("blob_key"),
                    row.get("filename"),
                    row.get("mime"),
                    row.get::<Option<i64>, _>("size"),
                    row.get::<i64, _>("created_at"),
                )
            })
            .collect()
    }

    pub(super) async fn load(
        pool: &PgPool,
        id: AttachmentId,
    ) -> Result<Option<Attachment>, RepoError> {
        let Some(row) = sqlx::query(
            "SELECT id, task_id, kind, url, blob_key, filename, mime, size, created_at \
             FROM attachments WHERE id = $1",
        )
        .bind(id.as_uuid())
        .fetch_optional(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to load attachment", e))?
        else {
            return Ok(None);
        };
        Ok(Some(assemble(
            row.get::<uuid::Uuid, _>("id"),
            row.get::<uuid::Uuid, _>("task_id"),
            row.get("kind"),
            row.get("url"),
            row.get("blob_key"),
            row.get("filename"),
            row.get("mime"),
            row.get::<Option<i64>, _>("size"),
            row.get::<i64, _>("created_at"),
        )?))
    }

    pub(super) async fn insert(pool: &PgPool, attachment: &Attachment) -> Result<(), RepoError> {
        let e = encode(attachment);
        sqlx::query(
            "INSERT INTO attachments (id, task_id, kind, url, blob_key, filename, mime, size, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(attachment.id.as_uuid())
        .bind(attachment.task_id.as_uuid())
        .bind(e.kind)
        .bind(e.url)
        .bind(e.blob_key)
        .bind(e.filename)
        .bind(e.mime)
        .bind(e.size)
        .bind(attachment.created_at.unix_seconds())
        .execute(pool)
        .await
        .map_err(|err| RepoError::from_source("failed to insert attachment", err))?;
        Ok(())
    }

    pub(super) async fn delete(pool: &PgPool, id: AttachmentId) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM attachments WHERE id = $1")
            .bind(id.as_uuid())
            .execute(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to delete attachment", e))?;
        Ok(())
    }
}

#[async_trait]
impl AttachmentRepository for SqlStore {
    async fn list_for_task(&self, task_id: TaskId) -> Result<Vec<Attachment>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::list_for_task(pool, task_id).await,
            Backend::Postgres(pool) => postgres_impl::list_for_task(pool, task_id).await,
        }
    }

    async fn load(&self, id: AttachmentId) -> Result<Option<Attachment>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::load(pool, id).await,
            Backend::Postgres(pool) => postgres_impl::load(pool, id).await,
        }
    }

    async fn insert(&self, attachment: &Attachment) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::insert(pool, attachment).await,
            Backend::Postgres(pool) => postgres_impl::insert(pool, attachment).await,
        }
    }

    async fn delete(&self, id: AttachmentId) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::delete(pool, id).await,
            Backend::Postgres(pool) => postgres_impl::delete(pool, id).await,
        }
    }
}
