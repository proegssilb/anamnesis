//! Garbage-collects abandoned chunked uploads (issue #21): a client that
//! begins an upload and never completes or aborts it (a closed tab, a lost
//! connection) leaves a staging-side upload and an `attachment_uploads` row
//! behind indefinitely. This ticker finds uploads older than
//! [`STALE_AFTER`] and discards both, via `anamnesis_app::expire_stale_uploads`.
//!
//! Deliberately simpler than [`crate::sweep`]'s ticker: there is no
//! user-facing schedule to honour here (`docs/DOMAIN.md` §6's calendar
//! recurrence is what makes that one need `is_due`/catch-up logic), only a
//! fixed interval and a lease so at most one instance runs it at a time.
//!
//! Like [`crate::sweep::spawn_ticker`], this is called from exactly one
//! place (`main.rs`) and never from anything `tests/support::TestApp`
//! builds, so it cannot fire during a test run.

use std::time::Duration;

use anamnesis_app::expire_stale_uploads;

use crate::state::AppState;

/// How often the ticker checks for abandoned uploads.
const POLL_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// How long an upload may sit open before it counts as abandoned.
const STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);
const GC_JOB: &str = "attachment_upload_gc";
/// Comfortably longer than one GC pass should ever take, short enough that
/// a crashed instance's stale claim is not blocking for long.
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
                tracing::error!(error = %err, "attachment upload GC: failed to run");
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
    let removed = expire_stale_uploads(
        state.attachment_uploads.as_ref(),
        state.chunked.as_ref(),
        state.clock.as_ref(),
        STALE_AFTER,
    )
    .await?;
    if removed > 0 {
        tracing::info!(removed, "attachment upload GC: removed abandoned uploads");
    }
    // Best-effort, same reasoning as `sweep::tick_once`'s own release: a
    // failed release just costs the rest of the TTL, not worth failing an
    // otherwise successful pass over.
    let _ = state.leases.release(GC_JOB, owner).await;
    Ok(())
}
