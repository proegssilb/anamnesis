//! The scheduled tangle-detection ticker: `run_tangle_detection` and
//! `resolve_frozen_tangles` on a timer under a job lease, rather than inside
//! every board GET.
//!
//! **Why it moved.** Both passes are system-wide reconciliation *writes* —
//! detection reads the whole blocking graph and inserts or stamps `tangles`
//! rows; resolution stamps and moves frozen ones. Running them from
//! `handlers::board`'s `view_board_impl` put those writes on the hottest read
//! path in the application: every viewer, every refresh, every htmx column
//! poll. `anamnesis_app::use_cases::tangle`'s own module doc comment already
//! named a scheduled job as the right home ("Real deployments would run this
//! from a scheduled job, the same way a sweep ticker runs
//! `archive_done_tasks`"); with `JobLease` in place, that is now what happens.
//!
//! The multi-instance case is what makes it more than an optimization. N
//! instances serving board GETs meant N concurrent reconciliation passes over
//! one graph, racing each other's inserts. Leasing the job makes it
//! single-writer — a better fix than locking the reconciliation itself, which
//! was already racy across concurrent requests on a *single* instance.
//!
//! **The staleness window this buys, and what it costs.** Detection used to be
//! immediately consistent: knot two tasks, load the board, see the knot. It is
//! now at most [`DETECTION_INTERVAL`] behind. Two things a user can notice:
//!
//! - The board's tangle indicator appears (or clears) up to a tick late.
//! - The suggestion engine excludes tangled tasks, so within that window it
//!   can still offer one of a freshly knotted pair individually, rather than
//!   offering the knot.
//!
//! Neither is a wrong answer, only an old one, and both self-correct on the
//! next tick. A minute is short enough that a user who has just built a knot
//! and is watching for it will usually see it before they finish reading the
//! board, and long enough that the work is bounded by wall-clock time rather
//! than by traffic.
//!
//! **This is deliberately not event-driven.** The obvious way to erase the
//! window entirely is to kick the ticker whenever a blocking edge changes.
//! That is a real option and a better UX, but it is a different design — an
//! in-process `Notify` only wakes the instance that served the mutation, so
//! making it work across instances means a notification channel through the
//! database, which is a considerably larger change than the interval this
//! module picks. Left for later, on purpose: this change is a move off the
//! read path, and should be reviewable as exactly that.
//!
//! **Honest about the trade.** On a busy deployment this is a large reduction
//! in work; on an idle one it is an *increase*, since a board nobody looks at
//! used to run no detection at all and now runs one pass a minute. That is
//! accepted: the cost is constant and small (one graph read plus one active-
//! tangle read, writing only when something actually changed), and being
//! independent of traffic is the property worth having.
//!
//! **Kept out of the test harness, structurally** — the same property
//! `crate::sweep`'s module doc comment spells out, for the same reason.
//! [`spawn_ticker`] has exactly one call site in this workspace, `main.rs`,
//! and nothing `tests/support` builds can reach it. Tests that need detection
//! to have run call [`refresh_tangles`] directly, which is precisely what the
//! ticker calls once it holds the lease — so they exercise the real pass
//! without a background task existing at all.

use std::sync::Arc;
use std::time::Duration;

use anamnesis_app::{AppError, JobLease, resolve_frozen_tangles, run_tangle_detection};

use crate::state::AppState;

/// The lease name scheduled detection coordinates on.
pub const TANGLE_JOB: &str = "tangle_detection";

/// How long the ticker sleeps between detection passes, and therefore the
/// worst-case staleness of every tangle-derived thing the board renders.
///
/// A minute. See the module doc comment for what a user can notice inside
/// that window; the short version is that a knot shows up a tick late and
/// nothing shows up wrong.
///
/// Unlike `crate::sweep::POLL_INTERVAL` this is the *only* interval the loop
/// has. The sweep needs a second, shorter one because a missed sweep is not
/// retried for a whole day, so a tick that left one unaccounted for has to
/// say so and come back sooner. Detection has no schedule to miss: every tick
/// does the same unconditional pass, so a tick that failed, or that found
/// another instance already detecting, is simply covered by the next ordinary
/// wake a minute later.
pub const DETECTION_INTERVAL: Duration = Duration::from_secs(60);

/// How long the detection lease is held for.
///
/// Released as soon as the pass finishes, so this only ever bounds a *crash*:
/// long enough that a slow pass over a large blocking graph is not overtaken,
/// short enough that an instance killed mid-pass costs a handful of ticks
/// rather than an outage of detection.
///
/// Comfortably longer than [`DETECTION_INTERVAL`] on purpose. A pass that
/// somehow ran longer than the interval must not be joined by the *next*
/// tick — of this instance or any other — halfway through.
const LEASE_TTL: Duration = Duration::from_secs(300);

/// One full reconciliation of stored tangles against the live blocking graph:
/// detection first, then resolution of frozen (placed) tangles.
///
/// Both passes are needed and neither subsumes the other. Detection never
/// touches a frozen tangle — see `run_tangle_detection`'s own doc comment —
/// so `resolve_frozen_tangles` is the separate pass that closes a placed knot
/// out once its frozen task set is no longer cyclic, moving it into the
/// board's `is_done` column if one is configured.
///
/// Public because it is the seam integration tests drive instead of spawning
/// a ticker (see the module doc comment).
pub async fn refresh_tangles(state: &AppState) -> Result<(), AppError> {
    let reconciliation = run_tangle_detection(
        state.relationships.as_ref(),
        state.tangles.as_ref(),
        state.id_gen.as_ref(),
        state.clock.as_ref(),
    )
    .await?;

    let done_column = state
        .board
        .columns_with_items()
        .await?
        .into_iter()
        .find(|bc| bc.column.is_done)
        .map(|bc| bc.column.id);
    let closed = resolve_frozen_tangles(
        state.relationships.as_ref(),
        state.tangles.as_ref(),
        state.board.as_ref(),
        state.clock.as_ref(),
        done_column,
    )
    .await?;

    // Only when something actually changed. A pass runs every
    // `DETECTION_INTERVAL` forever, and the overwhelmingly common outcome is
    // "the tangle set is exactly as it was" -- logging that at `info` would
    // bury every other line in the process's output within a day.
    if !reconciliation.newly_detected.is_empty()
        || !reconciliation.resolved.is_empty()
        || !closed.is_empty()
    {
        tracing::info!(
            newly_detected = reconciliation.newly_detected.len(),
            resolved = reconciliation.resolved.len(),
            frozen_closed = closed.len(),
            "tangle detection changed the tangle set"
        );
    }
    Ok(())
}

/// One pass of the ticker: claim [`TANGLE_JOB`], run [`refresh_tangles`] if
/// the claim succeeded, release.
///
/// The lease is taken around the whole pass rather than around each write,
/// because the thing that must not happen concurrently is the *reconciliation*
/// — read the graph, compare against stored tangles, write the difference —
/// not any individual statement in it.
///
/// No due-ness check precedes the claim, unlike `crate::sweep::tick_once`:
/// there is no schedule here, so every tick is due and the lease is the only
/// question. Losing the claim is an ordinary outcome, not a deferral to
/// report — whichever instance holds it is running the same pass this one
/// would have.
async fn tick_once(state: &AppState, leases: &dyn JobLease, owner: &str) -> Result<(), AppError> {
    let now = state.clock.now();
    if !leases
        .try_acquire(TANGLE_JOB, owner, now, LEASE_TTL)
        .await?
    {
        tracing::debug!("tangle ticker: another instance is running detection this tick");
        return Ok(());
    }

    let outcome = refresh_tangles(state).await;

    // Best-effort, and released immediately rather than held for the whole
    // TTL: the next tick is only a minute away, and a stale claim would cost
    // several of them for nothing. A failed release costs the rest of the TTL
    // and is not worth failing an otherwise successful pass over.
    if let Err(err) = leases.release(TANGLE_JOB, owner).await {
        tracing::warn!(
            error = %err,
            "tangle ticker: could not release the detection lease; it will expire on its own"
        );
    }
    outcome
}

/// Spawns the background detection ticker as a detached `tokio` task and
/// returns its `JoinHandle`.
///
/// Runs a pass *before* its first sleep, so an instance that has just started
/// (or a deployment that has just been upgraded into this behaviour) does not
/// serve a board with a minute-old view of a tangle set nobody has ever
/// computed.
///
/// See the module doc comment for why this function has exactly one call site
/// in the whole workspace, and why `leases` is a parameter rather than an
/// `AppState` field.
pub fn spawn_ticker(state: AppState, leases: Arc<dyn JobLease>) -> tokio::task::JoinHandle<()> {
    // One identity for the life of the process, as in `crate::sweep`: the
    // owner string is what lets this instance renew and release its own claim
    // rather than contend with itself.
    let owner = state.id_gen.next().to_string();
    tokio::spawn(async move {
        loop {
            if let Err(err) = tick_once(&state, leases.as_ref(), &owner).await {
                // Nothing to recover: the next tick re-reads the whole graph
                // and recomputes the whole answer, so a failed pass leaves no
                // partial state that a shorter retry would need to clean up.
                tracing::error!(error = %err, "tangle ticker: a detection pass failed");
            }
            tokio::time::sleep(DETECTION_INTERVAL).await;
        }
    })
}
