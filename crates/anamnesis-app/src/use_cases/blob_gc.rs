//! Orphan blob garbage collection (issue #23): deletes blobs sitting in
//! [`BlobStore`] that no [`crate::entities::Attachment`] references any
//! longer.
//!
//! **The two real orphan sources today**, per `crate::use_cases::attachment`'s
//! and `crate::use_cases::attachment_upload`'s own doc comments and
//! `crates/anamnesis-adapters/src/blob_store/fs.rs`'s `write_atomically`:
//! a crash or failure between a blob's bytes committing to storage
//! (`BlobStore::put`/`ChunkedUpload::complete` succeeding) and the owning
//! `Attachment` row committing afterward, and a stray `.tmp-*` file left by
//! an interrupted atomic write on the filesystem backend. Neither can be
//! ruled out structurally, so this sweep exists as the backstop that finds
//! and removes both.
//!
//! **Why `grace_period` exists.** A blob that was *just* written may not
//! have its `Attachment` row committed yet — deleting it out from under a
//! write still in flight would be a real correctness bug, not a rare one,
//! given how routine that gap is. Only blobs whose `last_modified` is older
//! than `now - grace_period` are ever considered.

use std::collections::HashSet;
use std::time::Duration;

use anamnesis_core::Timestamp;

use crate::error::AppError;
use crate::ports::{AttachmentRepository, BlobStore, Clock};

/// Deletes every blob unreferenced by any `Attachment` whose `last_modified`
/// is older than `now - grace_period`, returning how many were removed.
/// Called only by the scheduled ticker (`anamnesis-web` spawns it in
/// `main.rs`), like `crate::use_cases::reindex_all` — no `Role`, no
/// permission check.
pub async fn collect_orphan_blobs(
    blobs: &dyn BlobStore,
    attachments: &dyn AttachmentRepository,
    clock: &dyn Clock,
    grace_period: Duration,
) -> Result<usize, AppError> {
    let orphans = find_orphan_blobs(blobs, attachments, clock, grace_period).await?;
    Ok(delete_all(blobs, &orphans).await)
}

/// The "what needs deleting" half: every blob key currently in storage,
/// older than the grace period, unreferenced by any attachment.
async fn find_orphan_blobs(
    blobs: &dyn BlobStore,
    attachments: &dyn AttachmentRepository,
    clock: &dyn Clock,
    grace_period: Duration,
) -> Result<Vec<String>, AppError> {
    let referenced: HashSet<String> = attachments
        .list_all_blob_keys()
        .await?
        .into_iter()
        .collect();
    let cutoff = cutoff_before(clock.now(), grace_period);
    Ok(blobs
        .list()
        .await?
        .into_iter()
        .filter(|b| b.last_modified < cutoff && !referenced.contains(&b.key))
        .map(|b| b.key)
        .collect())
}

/// `now - grace_period`, saturating at the Unix epoch rather than
/// underflowing.
fn cutoff_before(now: Timestamp, grace_period: Duration) -> Timestamp {
    let seconds = now
        .unix_seconds()
        .saturating_sub(grace_period.as_secs() as i64)
        .max(0);
    Timestamp::from_unix_seconds(seconds).unwrap_or(now)
}

/// The "delete it" half: removes each orphan, logging and continuing past
/// one failure — the same per-item philosophy
/// `crate::use_cases::archive::archive_swept_tasks` applies to a failed
/// index write. A blob this pass could not delete is simply reconsidered
/// (against a fresh cutoff) on the sweep's next run.
async fn delete_all(blobs: &dyn BlobStore, keys: &[String]) -> usize {
    let mut removed = 0;
    for key in keys {
        match blobs.delete(key).await {
            Ok(()) => removed += 1,
            Err(err) => eprintln!("anamnesis: orphan blob GC failed to delete {key:?}: {err}"),
        }
    }
    removed
}
