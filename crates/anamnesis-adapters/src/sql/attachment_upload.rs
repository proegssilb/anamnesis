//! [`AttachmentUploadRepository`] over [`SqlStore`]: tracks a chunked
//! upload's progress across requests and — since parts are recorded here,
//! not in adapter memory — across instances too (see
//! `anamnesis_app::ports::infra::ChunkedUpload`'s doc comment).

use anamnesis_app::{
    AttachmentUploadId, AttachmentUploadRepository, PartInfo, PendingUpload, RepoError,
};
use anamnesis_core::{TaskId, Timestamp, UserId};
use async_trait::async_trait;
use sqlx::{PgPool, Row, SqlitePool};

use super::{Backend, SqlStore, parse_uuid, timestamp_from_seconds};

/// The `attachment_uploads` row, straight off either backend's `SELECT id,
/// task_id, blob_key, storage_token, filename, mime, bytes_received,
/// created_by, created_at` (each backend still reads its own `id`/`task_id`
/// column differently — SQLite stores them as text, Postgres natively — so
/// callers parse those two before filling this in).
struct UploadRow {
    id: uuid::Uuid,
    task_id: uuid::Uuid,
    blob_key: String,
    storage_token: String,
    filename: String,
    mime: String,
    bytes_received: i64,
    created_by: String,
    created_at: i64,
}

fn assemble(row: UploadRow) -> Result<PendingUpload, RepoError> {
    Ok(PendingUpload {
        id: AttachmentUploadId::new(row.id),
        task_id: TaskId::new(row.task_id),
        blob_key: row.blob_key,
        storage_token: row.storage_token,
        filename: row.filename,
        mime: row.mime,
        bytes_received: u64::try_from(row.bytes_received)
            .map_err(|e| RepoError::from_source("stored bytes_received out of range", e))?,
        created_by: UserId::new(row.created_by),
        created_at: timestamp_from_seconds(row.created_at)?,
    })
}

mod sqlite_impl {
    use super::*;

    pub(super) async fn create(pool: &SqlitePool, upload: &PendingUpload) -> Result<(), RepoError> {
        sqlx::query(
            "INSERT INTO attachment_uploads \
             (id, task_id, blob_key, storage_token, filename, mime, bytes_received, created_by, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, 0, ?, ?)",
        )
        .bind(upload.id.as_uuid().to_string())
        .bind(upload.task_id.as_uuid().to_string())
        .bind(&upload.blob_key)
        .bind(&upload.storage_token)
        .bind(&upload.filename)
        .bind(&upload.mime)
        .bind(upload.created_by.as_str())
        .bind(upload.created_at.unix_seconds())
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to insert pending upload", e))?;
        Ok(())
    }

    pub(super) async fn load(
        pool: &SqlitePool,
        id: AttachmentUploadId,
    ) -> Result<Option<PendingUpload>, RepoError> {
        let Some(row) = sqlx::query(
            "SELECT id, task_id, blob_key, storage_token, filename, mime, bytes_received, \
             created_by, created_at FROM attachment_uploads WHERE id = ?",
        )
        .bind(id.as_uuid().to_string())
        .fetch_optional(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to load pending upload", e))?
        else {
            return Ok(None);
        };
        Ok(Some(assemble(UploadRow {
            id: parse_uuid(&row.get::<String, _>("id"))?,
            task_id: parse_uuid(&row.get::<String, _>("task_id"))?,
            blob_key: row.get("blob_key"),
            storage_token: row.get("storage_token"),
            filename: row.get("filename"),
            mime: row.get("mime"),
            bytes_received: row.get("bytes_received"),
            created_by: row.get("created_by"),
            created_at: row.get("created_at"),
        })?))
    }

    pub(super) async fn record_part(
        pool: &SqlitePool,
        id: AttachmentUploadId,
        part: PartInfo,
    ) -> Result<u64, RepoError> {
        let mut tx = pool
            .begin()
            .await
            .map_err(|e| RepoError::from_source("failed to start transaction", e))?;
        sqlx::query(
            "INSERT INTO attachment_upload_parts (upload_id, part_number, content_id, size) \
             VALUES (?, ?, ?, ?)",
        )
        .bind(id.as_uuid().to_string())
        .bind(part.number)
        .bind(&part.content_id)
        .bind(part.size as i64)
        .execute(&mut *tx)
        .await
        .map_err(|e| RepoError::from_source("failed to record upload part", e))?;
        sqlx::query(
            "UPDATE attachment_uploads SET bytes_received = bytes_received + ? WHERE id = ?",
        )
        .bind(part.size as i64)
        .bind(id.as_uuid().to_string())
        .execute(&mut *tx)
        .await
        .map_err(|e| RepoError::from_source("failed to update bytes_received", e))?;
        let total: i64 = sqlx::query("SELECT bytes_received FROM attachment_uploads WHERE id = ?")
            .bind(id.as_uuid().to_string())
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| RepoError::from_source("failed to read bytes_received", e))?
            .get("bytes_received");
        tx.commit()
            .await
            .map_err(|e| RepoError::from_source("failed to commit upload part", e))?;
        u64::try_from(total).map_err(|e| RepoError::from_source("bytes_received out of range", e))
    }

    pub(super) async fn list_parts(
        pool: &SqlitePool,
        id: AttachmentUploadId,
    ) -> Result<Vec<PartInfo>, RepoError> {
        let rows = sqlx::query(
            "SELECT part_number, content_id, size FROM attachment_upload_parts \
             WHERE upload_id = ? ORDER BY part_number",
        )
        .bind(id.as_uuid().to_string())
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to list upload parts", e))?;
        rows.into_iter()
            .map(|row| {
                Ok(PartInfo {
                    number: row.get::<i64, _>("part_number") as u32,
                    content_id: row.get("content_id"),
                    size: u64::try_from(row.get::<i64, _>("size"))
                        .map_err(|e| RepoError::from_source("stored part size out of range", e))?,
                })
            })
            .collect()
    }

    pub(super) async fn delete(pool: &SqlitePool, id: AttachmentUploadId) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM attachment_uploads WHERE id = ?")
            .bind(id.as_uuid().to_string())
            .execute(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to delete pending upload", e))?;
        Ok(())
    }

    pub(super) async fn list_stale(
        pool: &SqlitePool,
        before: Timestamp,
    ) -> Result<Vec<PendingUpload>, RepoError> {
        let rows = sqlx::query(
            "SELECT id, task_id, blob_key, storage_token, filename, mime, bytes_received, \
             created_by, created_at FROM attachment_uploads WHERE created_at < ?",
        )
        .bind(before.unix_seconds())
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to list stale uploads", e))?;
        rows.into_iter()
            .map(|row| {
                assemble(UploadRow {
                    id: parse_uuid(&row.get::<String, _>("id"))?,
                    task_id: parse_uuid(&row.get::<String, _>("task_id"))?,
                    blob_key: row.get("blob_key"),
                    storage_token: row.get("storage_token"),
                    filename: row.get("filename"),
                    mime: row.get("mime"),
                    bytes_received: row.get("bytes_received"),
                    created_by: row.get("created_by"),
                    created_at: row.get("created_at"),
                })
            })
            .collect()
    }
}

mod postgres_impl {
    use super::*;

    pub(super) async fn create(pool: &PgPool, upload: &PendingUpload) -> Result<(), RepoError> {
        sqlx::query(
            "INSERT INTO attachment_uploads \
             (id, task_id, blob_key, storage_token, filename, mime, bytes_received, created_by, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, 0, $7, $8)",
        )
        .bind(upload.id.as_uuid())
        .bind(upload.task_id.as_uuid())
        .bind(&upload.blob_key)
        .bind(&upload.storage_token)
        .bind(&upload.filename)
        .bind(&upload.mime)
        .bind(upload.created_by.as_str())
        .bind(upload.created_at.unix_seconds())
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to insert pending upload", e))?;
        Ok(())
    }

    pub(super) async fn load(
        pool: &PgPool,
        id: AttachmentUploadId,
    ) -> Result<Option<PendingUpload>, RepoError> {
        let Some(row) = sqlx::query(
            "SELECT id, task_id, blob_key, storage_token, filename, mime, bytes_received, \
             created_by, created_at FROM attachment_uploads WHERE id = $1",
        )
        .bind(id.as_uuid())
        .fetch_optional(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to load pending upload", e))?
        else {
            return Ok(None);
        };
        Ok(Some(assemble(UploadRow {
            id: row.get("id"),
            task_id: row.get("task_id"),
            blob_key: row.get("blob_key"),
            storage_token: row.get("storage_token"),
            filename: row.get("filename"),
            mime: row.get("mime"),
            bytes_received: row.get("bytes_received"),
            created_by: row.get("created_by"),
            created_at: row.get("created_at"),
        })?))
    }

    pub(super) async fn record_part(
        pool: &PgPool,
        id: AttachmentUploadId,
        part: PartInfo,
    ) -> Result<u64, RepoError> {
        let mut tx = pool
            .begin()
            .await
            .map_err(|e| RepoError::from_source("failed to start transaction", e))?;
        sqlx::query(
            "INSERT INTO attachment_upload_parts (upload_id, part_number, content_id, size) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(id.as_uuid())
        .bind(part.number as i32)
        .bind(&part.content_id)
        .bind(part.size as i64)
        .execute(&mut *tx)
        .await
        .map_err(|e| RepoError::from_source("failed to record upload part", e))?;
        sqlx::query(
            "UPDATE attachment_uploads SET bytes_received = bytes_received + $1 WHERE id = $2",
        )
        .bind(part.size as i64)
        .bind(id.as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(|e| RepoError::from_source("failed to update bytes_received", e))?;
        let total: i64 = sqlx::query("SELECT bytes_received FROM attachment_uploads WHERE id = $1")
            .bind(id.as_uuid())
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| RepoError::from_source("failed to read bytes_received", e))?
            .get("bytes_received");
        tx.commit()
            .await
            .map_err(|e| RepoError::from_source("failed to commit upload part", e))?;
        u64::try_from(total).map_err(|e| RepoError::from_source("bytes_received out of range", e))
    }

    pub(super) async fn list_parts(
        pool: &PgPool,
        id: AttachmentUploadId,
    ) -> Result<Vec<PartInfo>, RepoError> {
        let rows = sqlx::query(
            "SELECT part_number, content_id, size FROM attachment_upload_parts \
             WHERE upload_id = $1 ORDER BY part_number",
        )
        .bind(id.as_uuid())
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to list upload parts", e))?;
        rows.into_iter()
            .map(|row| {
                Ok(PartInfo {
                    number: row.get::<i32, _>("part_number") as u32,
                    content_id: row.get("content_id"),
                    size: u64::try_from(row.get::<i64, _>("size"))
                        .map_err(|e| RepoError::from_source("stored part size out of range", e))?,
                })
            })
            .collect()
    }

    pub(super) async fn delete(pool: &PgPool, id: AttachmentUploadId) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM attachment_uploads WHERE id = $1")
            .bind(id.as_uuid())
            .execute(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to delete pending upload", e))?;
        Ok(())
    }

    pub(super) async fn list_stale(
        pool: &PgPool,
        before: Timestamp,
    ) -> Result<Vec<PendingUpload>, RepoError> {
        let rows = sqlx::query(
            "SELECT id, task_id, blob_key, storage_token, filename, mime, bytes_received, \
             created_by, created_at FROM attachment_uploads WHERE created_at < $1",
        )
        .bind(before.unix_seconds())
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to list stale uploads", e))?;
        rows.into_iter()
            .map(|row| {
                assemble(UploadRow {
                    id: row.get("id"),
                    task_id: row.get("task_id"),
                    blob_key: row.get("blob_key"),
                    storage_token: row.get("storage_token"),
                    filename: row.get("filename"),
                    mime: row.get("mime"),
                    bytes_received: row.get("bytes_received"),
                    created_by: row.get("created_by"),
                    created_at: row.get("created_at"),
                })
            })
            .collect()
    }
}

#[async_trait]
impl AttachmentUploadRepository for SqlStore {
    async fn create(&self, upload: &PendingUpload) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::create(pool, upload).await,
            Backend::Postgres(pool) => postgres_impl::create(pool, upload).await,
        }
    }

    async fn load(&self, id: AttachmentUploadId) -> Result<Option<PendingUpload>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::load(pool, id).await,
            Backend::Postgres(pool) => postgres_impl::load(pool, id).await,
        }
    }

    async fn record_part(&self, id: AttachmentUploadId, part: PartInfo) -> Result<u64, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::record_part(pool, id, part).await,
            Backend::Postgres(pool) => postgres_impl::record_part(pool, id, part).await,
        }
    }

    async fn list_parts(&self, id: AttachmentUploadId) -> Result<Vec<PartInfo>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::list_parts(pool, id).await,
            Backend::Postgres(pool) => postgres_impl::list_parts(pool, id).await,
        }
    }

    async fn delete(&self, id: AttachmentUploadId) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::delete(pool, id).await,
            Backend::Postgres(pool) => postgres_impl::delete(pool, id).await,
        }
    }

    async fn list_stale(&self, before: Timestamp) -> Result<Vec<PendingUpload>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::list_stale(pool, before).await,
            Backend::Postgres(pool) => postgres_impl::list_stale(pool, before).await,
        }
    }
}
