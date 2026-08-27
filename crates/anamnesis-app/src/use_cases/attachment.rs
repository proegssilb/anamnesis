//! Attachments on a task (`docs/DOMAIN.md` §3): a link, or a file uploaded
//! through the [`BlobStore`] port. Deletion only removes the file's
//! metadata row plus, for a `File` attachment, its blob — links carry
//! nothing else to clean up.

use anamnesis_core::TaskId;
use anamnesis_core::policy::Role;

use crate::entities::{self, Attachment, AttachmentId, AttachmentKind};
use crate::error::AppError;
use crate::policy::{Action, is_allowed};
use crate::ports::{AttachmentRepository, BlobStore, Clock, IdGen};

/// Attaches an external link to a task.
pub async fn add_link_attachment(
    repo: &dyn AttachmentRepository,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    role: Option<Role>,
    task_id: TaskId,
    url: &str,
) -> Result<Attachment, AppError> {
    if !is_allowed(role, Action::CreateAttachment) {
        return Err(AppError::Forbidden);
    }
    let attachment =
        entities::attach_link(AttachmentId::new(ids.next()), task_id, url, clock.now())?;
    repo.insert(&attachment).await?;
    Ok(attachment)
}

/// Uploads `bytes` to the blob store and attaches the resulting file to a
/// task. `blob_key` is minted by the caller (the id generator is the
/// simplest deterministic source), used both as the blob store's key and as
/// the `blob_key` recorded on the attachment.
#[allow(clippy::too_many_arguments)]
pub async fn add_file_attachment(
    repo: &dyn AttachmentRepository,
    blobs: &dyn BlobStore,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    role: Option<Role>,
    task_id: TaskId,
    filename: &str,
    mime: &str,
    bytes: Vec<u8>,
) -> Result<Attachment, AppError> {
    if !is_allowed(role, Action::CreateAttachment) {
        return Err(AppError::Forbidden);
    }
    let blob_key = ids.next().to_string();
    let size = bytes.len() as u64;
    blobs.put(&blob_key, bytes, mime).await?;
    let attachment = entities::attach_file(
        AttachmentId::new(ids.next()),
        task_id,
        &blob_key,
        filename,
        mime,
        size,
        clock.now(),
    )?;
    repo.insert(&attachment).await?;
    Ok(attachment)
}

/// Lists a task's attachments.
pub async fn list_attachments(
    repo: &dyn AttachmentRepository,
    role: Option<Role>,
    task_id: TaskId,
) -> Result<Vec<Attachment>, AppError> {
    if !is_allowed(role, Action::ViewTask) {
        return Err(AppError::Forbidden);
    }
    Ok(repo.list_for_task(task_id).await?)
}

/// Deletes an attachment, and the underlying blob if it was a `File`.
pub async fn delete_attachment(
    repo: &dyn AttachmentRepository,
    blobs: &dyn BlobStore,
    role: Option<Role>,
    id: AttachmentId,
) -> Result<(), AppError> {
    if !is_allowed(role, Action::DeleteAttachment) {
        return Err(AppError::Forbidden);
    }
    let attachment = repo.load(id).await?.ok_or(AppError::NotFound)?;
    repo.delete(id).await?;
    if let AttachmentKind::File { blob_key, .. } = attachment.kind {
        blobs.delete(&blob_key).await?;
    }
    Ok(())
}
