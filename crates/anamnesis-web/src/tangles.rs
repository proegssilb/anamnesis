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
//! [`refresh_after_graph_change`] before redirecting. That call **spawns**
//! the pass and returns immediately — the request is never held open for
//! it. The instance that served the mutation still runs the pass (spawned
//! onto its own runtime, not handed to anything else), so the graph it
//! reads is guaranteed to include the edge just committed; what is no
//! longer guaranteed is that the pass has *finished* by the time the
//! browser's redirect lands. Detection converges shortly after the request
//! that triggered it, not necessarily within it — see
//! [`refresh_after_graph_change`]'s own doc comment for why immediacy was
//! traded away here on purpose.
//!
//! **Why a contended background task waits instead of giving up quickly.**
//! Try-and-skip loses updates. Suppose instance A is mid-pass — it has
//! already read the blocking graph — when instance B commits a new edge. If
//! B's background task merely tried the lease, failed, and gave up, A's
//! in-flight pass would finish without ever seeing B's edge, and nothing
//! would be scheduled to look again until the backstop. So the loser polls
//! until it wins ([`LEASE_RETRY_TIMEOUT`], [`LEASE_POLL`]) and then runs its
//! own pass, which is therefore guaranteed to begin *after* its own commit
//! and to see it. Nothing user-facing is waiting on this anymore, so the
//! bound is generous — see [`LEASE_RETRY_TIMEOUT`]'s own doc comment — and
//! only a lease store that stays unreachable for that whole window makes a
//! background task concede to the backstop instead.
//!
//! **The backstop.** [`spawn_backstop`] still runs a pass every
//! [`BACKSTOP_INTERVAL`], and *that* one skips rather than waits. It exists
//! because the event path can still miss: a process killed between the
//! commit and the pass, a lease held by an instance that then crashed, a
//! pass that failed against a database that has since recovered, or a
//! background task that gave up after [`LEASE_RETRY_TIMEOUT`] because the
//! lease store itself was unreachable. None of those leave anything partial
//! behind — a pass recomputes its whole answer from the graph — so a slow,
//! unconditional re-derivation is a complete repair, and fifteen minutes is
//! frequent enough to bound the damage without putting the idle case back
//! on a one-minute timer.
//!
//! **Reachable from the test harness, but not directly awaitable.** Unlike
//! [`spawn_backstop`] — which has exactly one call site in this workspace,
//! `main.rs`, and nothing `tests/support` builds can reach — the event
//! path's spawned task *is* reachable from a test: posting to a real
//! relationship route triggers the same `tokio::spawn` a browser-driven
//! request would. What a test cannot do is await that task's `JoinHandle`
//! (it is dropped, deliberately, the moment the task is spawned), so a test
//! that needs the pass's effect either forces a synchronous one with
//! [`refresh_tangles`] directly, or polls for the eventual result.

use std::time::Duration;
use std::time::Instant;

use anamnesis_app::{AppError, RepoError, resolve_frozen_tangles, run_tangle_detection};

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

/// How long the background detection task retries for the lease before
/// conceding to the backstop.
///
/// This no longer bounds a live HTTP request — [`refresh_after_graph_change`]
/// spawns this retry loop and returns immediately, so nothing user-facing is
/// waiting on it. It exists purely so a detached task doesn't retry forever
/// against a lease store that has gone genuinely unreachable: comfortably
/// longer than several [`LEASE_TTL`] turnovers, so ordinary contention
/// between two or three concurrent passes is never the reason a background
/// task gives up, only a lease store that stays unreachable for minutes is.
const LEASE_RETRY_TIMEOUT: Duration = Duration::from_secs(120);

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
/// Takes no lease of its own: every caller holds [`TANGLE_JOB`] around it
/// instead — see [`refresh_tangles_leased`] for the entry point that takes
/// the lease itself, which is what a caller without one of its own (most
/// notably the test harness) needs. Calling this directly without holding
/// the lease is not just a missed optimisation: since [`refresh_after_graph_change`]
/// now spawns a background pass per graph mutation, an unleased caller can
/// race one and insert a duplicate tangle for the same knot.
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

/// Polls for [`TANGLE_JOB`] until this pass holds it, or
/// [`LEASE_RETRY_TIMEOUT`] elapses. `Ok(false)` means the wait ran out.
async fn wait_for_lease(state: &AppState, owner: &str) -> Result<bool, AppError> {
    let deadline = Instant::now() + LEASE_RETRY_TIMEOUT;
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

/// Runs one leased, synchronous tangle-detection pass: takes [`TANGLE_JOB`]
/// itself (waiting out any pass already in flight, exactly as
/// [`refresh_after_graph_change`]'s spawned task does), runs
/// [`refresh_tangles`], and releases it.
///
/// This is [`refresh_tangles`]'s safe-to-call-from-anywhere sibling. Nothing
/// in production needs it — every production caller already holds the lease
/// some other way — but the test harness does: once a graph mutation can
/// leave a background pass in flight, a test that wants a deterministic
/// settle point can no longer call [`refresh_tangles`] directly without
/// risking exactly the race this whole lease exists to prevent (two
/// concurrent, unleased-relative-to-each-other passes each minting their own
/// tangle for the same knot). Waiting for the same lease the background path
/// uses means this either runs after any in-flight pass has released it, or
/// — if none is in flight — runs immediately, uncontended.
pub async fn refresh_tangles_leased(state: &AppState) -> Result<(), AppError> {
    let owner = state.id_gen.next().to_string();
    if !wait_for_lease(state, &owner).await? {
        return Err(AppError::Repo(RepoError::new(format!(
            "could not acquire the {TANGLE_JOB:?} lease within {}s",
            LEASE_RETRY_TIMEOUT.as_secs()
        ))));
    }
    run_and_release(state, &owner).await
}

/// Schedules a re-derivation of the tangle set after this request changed a
/// `blocks` edge, without waiting for it to finish.
///
/// Called from `crate::handlers::tasks::relationships` after a create or a
/// delete has committed, and only for the `blocks` kind — no other edge kind
/// is an input to detection at all.
///
/// Spawned rather than awaited, on purpose: the request that changed the
/// graph must not pay for however long a detection pass takes (a large
/// blocking graph, or contention with another instance's pass), so it
/// returns as soon as the background task is scheduled rather than once the
/// pass has actually run. The trade is immediacy for reliability — the
/// redirect this handler is about to issue is no longer guaranteed to land
/// on a board that already reflects the change, but the spawned task itself
/// still reliably runs to completion and still updates stored tangle state:
/// a `tokio` task keeps running once spawned whether or not anything holds
/// or awaits its `JoinHandle` (dropped here, deliberately), and
/// [`wait_for_lease`]'s retry loop ([`LEASE_RETRY_TIMEOUT`]) means ordinary
/// contention with another pass no longer causes it to give up early either
/// — nothing user-facing is waiting on it, so it can afford to keep trying.
/// Detection is therefore eventually, not immediately, consistent after such
/// an edit; see this module's own doc comment for what still bounds "eventually".
///
/// Fails nowhere visible to the caller — there is nothing to return, since
/// nothing here is awaited by the handler. A detection failure inside the
/// spawned task must not turn a relationship edit that already committed
/// into an error page anyway — the edge is saved either way, and the tangle
/// set is derived state that the backstop re-derives from scratch. Every
/// outcome that is not a completed pass is logged and left to it.
pub fn refresh_after_graph_change(state: &AppState) {
    let state = state.clone();
    tokio::spawn(async move {
        let owner = state.id_gen.next().to_string();
        match wait_for_lease(&state, &owner).await {
            Ok(true) => {
                if let Err(err) = run_and_release(&state, &owner).await {
                    tracing::error!(
                        error = %err,
                        "tangle detection after a blocking-edge change failed; the backstop will \
                         re-derive it"
                    );
                }
            }
            Ok(false) => tracing::warn!(
                "tangle detection after a blocking-edge change could not get the {TANGLE_JOB:?} \
                 lease within {}s; the backstop will re-derive it",
                LEASE_RETRY_TIMEOUT.as_secs()
            ),
            Err(err) => tracing::error!(
                error = %err,
                "tangle detection after a blocking-edge change could not reach the lease store; \
                 the backstop will re-derive it"
            ),
        }
    });
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
