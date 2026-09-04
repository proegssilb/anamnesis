//! Tangle detection: `run_tangle_detection` and `resolve_frozen_tangles`,
//! driven by the events that can change their answer, under a job lease,
//! rather than inside every board GET.
//!
//! **Why it left the read path.** Both passes are system-wide reconciliation
//! *writes* — detection reads the whole blocking graph and inserts or stamps
//! `tangles` rows; resolution stamps and moves frozen ones. Running them from
//! `handlers::board`'s `view_board_impl` put those writes on the hottest read
//! path in the application: every viewer, every refresh, every htmx column
//! poll. Worse across instances: N instances serving board GETs meant N
//! concurrent reconciliation passes over one graph, racing each other's
//! inserts.
//!
//! **What drives it instead.** The passes have exactly two inputs — the set
//! of `blocks` edges (`RelationshipRepository::list_blocking`) and, for
//! resolution, which tasks sit in the board's `is_done` column. Only two
//! events can change the first: creating a `blocks` edge and deleting one.
//! Both go through `handlers::tasks::relationships`, and both call
//! [`refresh_after_graph_change`] before redirecting — so the instance that
//! served the mutation runs the pass, synchronously, and the browser's
//! redirect lands on a board that already reflects it. Detection is
//! immediately consistent again, without a write on any read.
//!
//! **Why the loser of the lease waits instead of skipping.** Try-and-skip
//! loses updates. Suppose instance A is mid-pass — it has already read the
//! blocking graph — when instance B commits a new edge. If B merely tried the
//! lease, failed, and moved on, A's in-flight pass would finish without ever
//! seeing B's edge, and nothing would be scheduled to look again. So the
//! loser polls until it wins ([`LEASE_WAIT`], [`LEASE_POLL`]) and then runs
//! its own pass, which is therefore guaranteed to begin *after* its own
//! commit and to see it. The lease is doing what a lease is for: making the
//! reconciliation single-writer without making it lossy.
//!
//! The cost is that a `blocks` mutation can block for up to [`LEASE_WAIT`]
//! behind another one. That is contention between two concurrent
//! relationship edits, which is rare, and it is bounded — see
//! [`refresh_after_graph_change`] for what happens when the wait runs out.
//!
//! **The backstop.** [`spawn_backstop`] still runs a pass every
//! [`BACKSTOP_INTERVAL`], and *that* one skips rather than waits. It exists
//! because the event path can miss: a process killed between the commit and
//! the pass, a lease held by an instance that then crashed, a pass that
//! failed against a database that has since recovered. None of those leave
//! anything partial behind — a pass recomputes its whole answer from the
//! graph — so a slow, unconditional re-derivation is a complete repair, and
//! fifteen minutes is frequent enough to bound the damage without putting the
//! idle case back on a one-minute timer.
//!
//! **Kept out of the test harness, structurally** — the same property
//! `crate::sweep`'s module doc comment spells out, for the same reason.
//! [`spawn_backstop`] has exactly one call site in this workspace, `main.rs`,
//! and nothing `tests/support` builds can reach it. Tests exercise the event
//! path by making relationship edits through the real routes, which is what a
//! browser does; tests that need a pass without a mutation call
//! [`refresh_tangles`] directly.

use std::time::Duration;
use std::time::Instant;

use anamnesis_app::{AppError, resolve_frozen_tangles, run_tangle_detection};

use crate::state::AppState;

/// The lease name every detection pass coordinates on — event-driven and
/// backstop alike, so the two can never overlap each other either.
pub const TANGLE_JOB: &str = "tangle_detection";

/// How long the backstop sleeps between unconditional passes.
///
/// This is *not* the staleness of a normal tangle update — the event path
/// makes those immediate. It is the worst-case repair time after the event
/// path has failed outright (see the module doc comment for how), so it
/// trades against nothing a user normally sees.
pub const BACKSTOP_INTERVAL: Duration = Duration::from_secs(15 * 60);

/// How long the detection lease is held for.
///
/// Released as soon as the pass finishes, so this only ever bounds a *crash*.
/// Deliberately short: a holder that dies mid-pass stalls live requests, not
/// just a background timer, and a request that cannot get the lease gives up
/// with a stale board. Thirty seconds is comfortably longer than a pass over
/// a large blocking graph and short enough that a crashed holder costs a
/// handful of mutations rather than a quarter of an hour of them.
const LEASE_TTL: Duration = Duration::from_secs(30);

/// How long a mutating request waits for the lease before giving up.
///
/// Bounds how long a `blocks` edit can sit behind another one. Long enough to
/// outlast an ordinary contending pass by a wide margin, short enough that a
/// crashed lease holder never turns into a request that looks hung.
const LEASE_WAIT: Duration = Duration::from_secs(5);

const LEASE_POLL: Duration = Duration::from_millis(50);

/// One full reconciliation of stored tangles against the live blocking graph:
/// detection first, then resolution of frozen (placed) tangles.
///
/// Both passes are needed and neither subsumes the other. Detection never
/// touches a frozen tangle — see `run_tangle_detection`'s own doc comment —
/// so `resolve_frozen_tangles` is the separate pass that closes a placed knot
/// out once its frozen task set is no longer cyclic, moving it into the
/// board's `is_done` column if one is configured.
///
/// Takes no lease of its own: both callers below hold [`TANGLE_JOB`] around
/// it. Public because it is the seam integration tests drive when they need a
/// pass without a mutation to trigger one.
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

    // Only when something actually changed. The backstop runs forever and the
    // overwhelmingly common outcome is "the tangle set is exactly as it was";
    // logging that at `info` would bury every other line in the process's
    // output.
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

/// Runs [`refresh_tangles`] and releases the lease, whatever the outcome.
///
/// The caller must already hold [`TANGLE_JOB`] as `owner`. The lease is taken
/// around the whole pass rather than around each write, because the thing
/// that must not happen concurrently is the *reconciliation* — read the
/// graph, compare against stored tangles, write the difference — not any
/// individual statement in it.
async fn run_and_release(state: &AppState, owner: &str) -> Result<(), AppError> {
    let outcome = refresh_tangles(state).await;

    // Best-effort, and released immediately rather than held for the whole
    // TTL: a stale claim stalls the next mutation's pass for no reason. A
    // failed release costs the rest of the TTL and is not worth failing an
    // otherwise successful pass over.
    if let Err(err) = state.leases.release(TANGLE_JOB, owner).await {
        tracing::warn!(
            error = %err,
            "tangle detection: could not release the lease; it will expire on its own"
        );
    }
    outcome
}

/// Polls for [`TANGLE_JOB`] until this pass holds it, or [`LEASE_WAIT`]
/// elapses. `Ok(false)` means the wait ran out.
async fn wait_for_lease(state: &AppState, owner: &str) -> Result<bool, AppError> {
    let deadline = Instant::now() + LEASE_WAIT;
    loop {
        if state
            .leases
            .try_acquire(TANGLE_JOB, owner, state.clock.now(), LEASE_TTL)
            .await?
        {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        tokio::time::sleep(LEASE_POLL).await;
    }
}

/// Re-derives the tangle set after this request changed a `blocks` edge.
///
/// Called from `crate::handlers::tasks::relationships` after a create or a
/// delete has committed, and only for the `blocks` kind — no other edge kind
/// is an input to detection at all.
///
/// Synchronous rather than spawned, so the redirect this handler is about to
/// issue lands on a board that already reflects the change. It is also what
/// makes the wait in [`wait_for_lease`] meaningful: the pass is guaranteed to
/// start after this request's own commit, so it cannot miss it.
///
/// Returns nothing and fails nowhere. A detection failure must not turn a
/// relationship edit that already committed into an error page — the edge is
/// saved either way, and the tangle set is derived state that the backstop
/// re-derives from scratch. Every outcome that is not a completed pass is
/// logged and left to it.
pub async fn refresh_after_graph_change(state: &AppState) {
    let owner = state.id_gen.next().to_string();
    match wait_for_lease(state, &owner).await {
        Ok(true) => {
            if let Err(err) = run_and_release(state, &owner).await {
                tracing::error!(
                    error = %err,
                    "tangle detection after a blocking-edge change failed; the backstop will \
                     re-derive it"
                );
            }
        }
        Ok(false) => tracing::warn!(
            "tangle detection after a blocking-edge change could not get the {TANGLE_JOB:?} lease \
             within {}s; the backstop will re-derive it",
            LEASE_WAIT.as_secs()
        ),
        Err(err) => tracing::error!(
            error = %err,
            "tangle detection after a blocking-edge change could not reach the lease store; the \
             backstop will re-derive it"
        ),
    }
}

/// One backstop pass: claim [`TANGLE_JOB`], run [`refresh_tangles`] if the
/// claim succeeded, release.
///
/// Skips rather than waits, unlike the event path. The backstop has no commit
/// of its own to be sure of seeing — it is an unconditional re-derivation, so
/// whoever holds the lease is already doing exactly the work this tick would
/// have done, and the next tick is another one anyway.
async fn backstop_tick(state: &AppState, owner: &str) -> Result<(), AppError> {
    let now = state.clock.now();
    if !state
        .leases
        .try_acquire(TANGLE_JOB, owner, now, LEASE_TTL)
        .await?
    {
        tracing::debug!("tangle backstop: a detection pass is already running");
        return Ok(());
    }
    run_and_release(state, owner).await
}

/// Spawns the background backstop as a detached `tokio` task and returns its
/// `JoinHandle`.
///
/// Runs a pass *before* its first sleep, so an instance that has just started
/// (or a deployment that has just been upgraded into this behaviour) repairs
/// anything the event path missed while it was down, rather than a quarter of
/// an hour later.
///
/// See the module doc comment for why this function has exactly one call site
/// in the whole workspace.
pub fn spawn_backstop(state: AppState) -> tokio::task::JoinHandle<()> {
    // One identity for the life of the process, as in `crate::sweep`: the
    // owner string is what lets this task release its own claim rather than
    // contend with itself.
    let owner = state.id_gen.next().to_string();
    tokio::spawn(async move {
        loop {
            if let Err(err) = backstop_tick(&state, &owner).await {
                // Nothing to recover: the next tick re-reads the whole graph
                // and recomputes the whole answer, so a failed pass leaves no
                // partial state that a shorter retry would need to clean up.
                tracing::error!(error = %err, "tangle backstop: a detection pass failed");
            }
            tokio::time::sleep(BACKSTOP_INTERVAL).await;
        }
    })
}
