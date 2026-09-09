//! Orphan blob garbage collection (issue #23): deletes blob-store bytes that
//! no `Attachment` row references any longer, on a fixed interval. See
//! `anamnesis_app::use_cases::blob_gc`'s module doc comment for the two real
//! orphan sources this backstops (a crash between a blob write committing
//! and its `Attachment` insert committing; a stray `.tmp-*` left by
//! `FsBlobStore`'s interrupted atomic write) and why a grace period exists.
//!
//! Deliberately simpler than [`crate::sweep`]'s ticker, matching
//! [`crate::upload_gc`]'s shape instead: there is no user-facing schedule to
//! honour here, only a fixed interval and a lease so at most one instance
//! runs it at a time. Like [`crate::sweep::spawn_ticker`] and
//! [`crate::upload_gc::spawn_ticker`], this is called from exactly one place
//! (`main.rs`) and never from anything `tests/support::TestApp` builds, so
//! it cannot fire during a test run.

use std::time::Duration;

use anamnesis_app::collect_orphan_blobs;

use crate::state::AppState;

/// How often the ticker checks for orphaned blobs. Same cadence as
/// `upload_gc::POLL_INTERVAL` — this is disk-hygiene work with no urgency,
/// and hourly is frequent enough that orphaned bytes don't accumulate for
/// long.
const POLL_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// How old an unreferenced blob must be before this sweep removes it.
/// Matches `upload_gc::STALE_AFTER`'s own figure so the two janitorial jobs
/// share one "how stale is stale" constant — comfortably longer than the
/// real window this protects (the gap between a blob write committing and
/// its `Attachment` insert committing, normally milliseconds).
const GRACE_PERIOD: Duration = Duration::from_secs(24 * 60 * 60);
const GC_JOB: &str = "blob_gc";
/// Comfortably longer than one GC pass should ever take, short enough that a
/// crashed instance's stale claim is not blocking for long. Same figure as
/// `upload_gc::GC_LEASE_TTL` — a `list()` over the whole blob store plus N
/// deletes is the same order of work as that pass.
const GC_LEASE_TTL: Duration = Duration::from_secs(15 * 60);

/// Spawns the ticker, returning a handle `main.rs` aborts (never awaits) on
/// shutdown — the same fire-and-forget discipline `sweep::spawn_ticker`
/// documents, for the same reason: a GC pass abandoned mid-run leaves
/// nothing worse than what it was already cleaning up.
pub fn spawn_ticker(state: AppState) -> tokio::task::JoinHandle<()> {
    let owner = state.id_gen.next().to_string();
    tokio::spawn(async move {
        loop {
            if let Err(err) = tick_once(&state, &owner).await {
                tracing::error!(error = %err, "orphan blob GC: failed to run");
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    })
}

async fn tick_once(state: &AppState, owner: &str) -> Result<(), anamnesis_app::AppError> {
    let now = state.clock.now();
    if !state
        .leases
        .try_acquire(GC_JOB, owner, now, GC_LEASE_TTL)
        .await?
    {
        return Ok(());
    }
    let removed = collect_orphan_blobs(
        state.blobs.as_ref(),
        state.attachments.as_ref(),
        state.clock.as_ref(),
        GRACE_PERIOD,
    )
    .await?;
    if removed > 0 {
        tracing::info!(removed, "orphan blob GC: removed unreferenced blobs");
    }
    // Best-effort, same reasoning as `sweep::tick_once`'s own release: a
    // failed release just costs the rest of the TTL, not worth failing an
    // otherwise successful pass over.
    let _ = state.leases.release(GC_JOB, owner).await;
    Ok(())
}
