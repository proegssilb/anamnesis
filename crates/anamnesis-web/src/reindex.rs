//! Rebuilds the search index from scratch on a fixed schedule (issue #22):
//! a backstop for the index-write failures `anamnesis_app::use_cases::
//! indexing`'s doc comment already documents as logged-and-non-fatal on
//! every ordinary create/edit/archive path. Deliberately simpler than
//! [`crate::sweep`]'s ticker, matching [`crate::upload_gc`]'s shape instead:
//! there is no user-facing schedule to honour here, only a fixed interval
//! and a lease so at most one instance runs it at a time.
//!
//! Like [`crate::sweep::spawn_ticker`] and [`crate::upload_gc::spawn_ticker`],
//! this is called from exactly one place (`main.rs`) and never from anything
//! `tests/support::TestApp` builds, so it cannot fire during a test run.
//!
//! `anamnesis_app::reindex_all` explains why re-asserting every current
//! area's/project's/task's indexed state is a real "from scratch" rebuild
//! for this domain model, not an approximation of one — nothing is ever
//! hard-deleted, so the search table can never hold a truly stale row for
//! an entity that no longer exists.

use std::time::Duration;

use anamnesis_app::reindex_all;

use crate::state::AppState;

/// A day: this is a rare-failure backstop, not a freshness mechanism —
/// every ordinary create/edit/archive already indexes itself immediately.
/// A full pass reads every area/project/task in the system, real work worth
/// doing sparingly, the same trade-off `sweep::POLL_INTERVAL` makes.
const POLL_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const REINDEX_JOB: &str = "reindex_sweep";
/// Comfortably longer than a full pass over a realistically large
/// installation's areas/projects/tasks should take — sized up from
/// `upload_gc::GC_LEASE_TTL` because this pass touches every task in the
/// system, not a filtered stale subset.
const REINDEX_LEASE_TTL: Duration = Duration::from_secs(30 * 60);

/// Spawns the ticker, returning a handle `main.rs` aborts (never awaits) on
/// shutdown — the same fire-and-forget discipline `sweep::spawn_ticker`
/// documents: a reindex pass abandoned mid-run leaves the index exactly as
/// stale as it already was, never worse, since every write it makes is
/// per-entity idempotent (`anamnesis_app::reindex_all`'s doc comment).
pub fn spawn_ticker(state: AppState) -> tokio::task::JoinHandle<()> {
    let owner = state.id_gen.next().to_string();
    tokio::spawn(async move {
        loop {
            if let Err(err) = tick_once(&state, &owner).await {
                tracing::error!(error = %err, "reindex sweep: failed to run");
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    })
}

async fn tick_once(state: &AppState, owner: &str) -> Result<(), anamnesis_app::AppError> {
    let now = state.clock.now();
    if !state
        .leases
        .try_acquire(REINDEX_JOB, owner, now, REINDEX_LEASE_TTL)
        .await?
    {
        return Ok(());
    }
    let result = reindex_all(
        state.areas.as_ref(),
        state.projects.as_ref(),
        state.tasks.as_ref(),
        state.search_index.as_ref(),
    )
    .await;
    if let Ok(outcome) = &result {
        tracing::info!(
            areas = outcome.areas,
            projects = outcome.projects,
            tasks = outcome.tasks,
            "reindex sweep: rebuilt the search index"
        );
    }
    // Best-effort, same reasoning as `sweep::tick_once`'s own release: a
    // failed release just costs the rest of the TTL, not worth failing an
    // otherwise successful pass over.
    let _ = state.leases.release(REINDEX_JOB, owner).await;
    result.map(|_| ())
}
