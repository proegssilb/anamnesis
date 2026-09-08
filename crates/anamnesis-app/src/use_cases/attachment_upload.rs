//! The multi-request half of file attachments (`docs/DOMAIN.md` §3, issue
//! #21): begin an upload, add any number of parts across separate requests,
//! then complete it — a real `Attachment` only exists once
//! [`complete_file_upload`] runs. Deliberately a separate flow from
//! [`crate::use_cases::add_file_attachment`]'s single-request path, not a
//! generalisation of it: the two have different failure modes (a chunked
//! upload can be abandoned mid-way and needs cleanup;
//! [`crate::ports::AttachmentUploadRepository`] and
//! [`crate::ports::ChunkedUpload`] exist only for this flow) and a shared
//! implementation would make both harder to follow for no real reuse.

use anamnesis_core::TaskId;
use anamnesis_core::policy::Role;

use crate::entities::{self, Attachment, AttachmentUploadId, PendingUpload};
use crate::error::AppError;
use crate::policy::{Action, is_allowed};
use crate::ports::{
    AttachmentRepository, AttachmentUploadRepository, ByteStream, ChunkedUpload, Clock, IdGen,
};

/// Begins a chunked upload for `task_id`, minting the `blob_key` its parts
/// will eventually be assembled under (the same pattern
/// [`crate::use_cases::add_file_attachment`] uses, just decided earlier —
/// here nothing about the file's bytes exists yet to size or hash).
#[allow(clippy::too_many_arguments)]
pub async fn begin_file_upload(
    uploads: &dyn AttachmentUploadRepository,
    chunked: &dyn ChunkedUpload,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    role: Option<Role>,
    task_id: TaskId,
    created_by: anamnesis_core::UserId,
    filename: &str,
    mime: &str,
) -> Result<PendingUpload, AppError> {
    if !is_allowed(role, Action::CreateAttachment) {
        return Err(AppError::Forbidden);
    }
    let blob_key = ids.next().to_string();
    let storage_token = chunked.begin(&blob_key, mime).await?;
    let upload = PendingUpload {
        id: AttachmentUploadId::new(ids.next()),
        task_id,
        blob_key,
        storage_token,
        filename: filename.to_string(),
        mime: mime.to_string(),
        bytes_received: 0,
        created_by,
        created_at: clock.now(),
    };
    uploads.create(&upload).await?;
    Ok(upload)
}

/// Uploads one part of an in-progress upload, enforcing `max_attachment_bytes`
/// against the running total *as parts accumulate* — a part that would push
/// the upload over the cap is rejected, and the whole upload is aborted
/// (mirroring how a single-request upload rejects rather than truncates an
/// over-limit body) rather than left half-uploaded for a client to retry
/// into indefinitely.
#[allow(clippy::too_many_arguments)]
pub async fn upload_file_part(
    uploads: &dyn AttachmentUploadRepository,
    chunked: &dyn ChunkedUpload,
    role: Option<Role>,
    max_attachment_bytes: u64,
    upload_id: AttachmentUploadId,
    part_number: u32,
    data: ByteStream<'_>,
) -> Result<(), AppError> {
    if !is_allowed(role, Action::CreateAttachment) {
        return Err(AppError::Forbidden);
    }
    let upload = uploads.load(upload_id).await?.ok_or(AppError::NotFound)?;
    let part = chunked
        .put_part(&upload.blob_key, &upload.storage_token, part_number, data)
        .await?;
    let total = uploads.record_part(upload_id, part).await?;
    if total > max_attachment_bytes {
        abort_upload_storage(uploads, chunked, &upload).await;
        return Err(AppError::Invalid(format!(
            "that attachment is larger than the {max_attachment_bytes}-byte limit"
        )));
    }
    Ok(())
}

/// Completes an upload: assembles its recorded parts into the finished blob
/// and records the resulting [`Attachment`], mirroring the second half of
/// [`crate::use_cases::add_file_attachment`].
pub async fn complete_file_upload(
    attachments: &dyn AttachmentRepository,
    uploads: &dyn AttachmentUploadRepository,
    chunked: &dyn ChunkedUpload,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    role: Option<Role>,
    upload_id: AttachmentUploadId,
) -> Result<Attachment, AppError> {
    if !is_allowed(role, Action::CreateAttachment) {
        return Err(AppError::Forbidden);
    }
    let upload = uploads.load(upload_id).await?.ok_or(AppError::NotFound)?;
    let parts = uploads.list_parts(upload_id).await?;
    let size = chunked
        .complete(&upload.blob_key, &upload.storage_token, &parts)
        .await?;
    let attachment = entities::attach_file(
        crate::entities::AttachmentId::new(ids.next()),
        upload.task_id,
        &upload.blob_key,
        &upload.filename,
        &upload.mime,
        size,
        clock.now(),
    )?;
    attachments.insert(&attachment).await?;
    uploads.delete(upload_id).await?;
    Ok(attachment)
}

/// Aborts an in-progress upload, discarding whatever parts have been
/// received.
pub async fn abort_file_upload(
    uploads: &dyn AttachmentUploadRepository,
    chunked: &dyn ChunkedUpload,
    role: Option<Role>,
    upload_id: AttachmentUploadId,
) -> Result<(), AppError> {
    if !is_allowed(role, Action::CreateAttachment) {
        return Err(AppError::Forbidden);
    }
    let upload = uploads.load(upload_id).await?.ok_or(AppError::NotFound)?;
    abort_upload_storage(uploads, chunked, &upload).await;
    Ok(())
}

/// Shared cleanup for [`upload_file_part`] (over the size cap) and
/// [`abort_file_upload`]: tell the blob store to discard whatever it has,
/// then drop the tracking row regardless of whether that succeeded — a
/// dangling storage-side upload with no tracking row left behind is exactly
/// what the abandoned-upload sweep exists to catch later, so this never
/// needs to be perfect.
async fn abort_upload_storage(
    uploads: &dyn AttachmentUploadRepository,
    chunked: &dyn ChunkedUpload,
    upload: &PendingUpload,
) {
    let _ = chunked.abort(&upload.blob_key, &upload.storage_token).await;
    let _ = uploads.delete(upload.id).await;
}
