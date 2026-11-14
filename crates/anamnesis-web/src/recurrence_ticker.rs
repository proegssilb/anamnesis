//! The recurring-task ticker: wakes when the next chore is due to pop in or
//! expire, and asks `anamnesis_app::run_recurrence_tick` to apply whatever is
//! due.
//!
//! Unlike the archive sweep's daily poll, a chore with a pop-in time ("at
//! 6pm") must fire close to that time, so the sleep is computed from the data:
//! [`sleep_for`] aims at the earliest upcoming event, capped at
//! [`MAX_SLEEP`] so an event scheduled while the ticker sleeps (another
//! instance, or a schedule edited in the UI) is noticed within the cap.
//!
//! Everything `sweep.rs` argues for carries over and is not repeated here:
//! [`spawn_ticker`] has exactly one call site (`main.rs`), so the test
//! harness structurally cannot start it; the loop checks before its first
//! sleep, so a pop-in that fell during downtime fires on startup; the lease is
//! acquired inside the due branch and released afterwards; and a tick that
//! failed or found the lease held is the one case where the next wake is
//! load-bearing, so it sleeps [`RETRY_INTERVAL`] rather than assuming it
//! will wake soon. The handle is aborted, not awaited, on shutdown; a tick
//! cut short is safe because firing and expiry are idempotent per task.
//!
//! **A past event never means "run again now".** A due chore in a paused
//! project is skipped by the tick yet stays the earliest event, so an
//! earliest instant at or before `now` sleeps [`PAST_EVENT_SLEEP`] rather than
//! zero; otherwise the ticker would spin.

use std::time::Duration;

use anamnesis_app::{AppError, RecurrenceDeps, RecurrenceTick, run_recurrence_tick};
use anamnesis_core::Timestamp;

use crate::state::AppState;

/// The lease name the recurrence tick coordinates on.
pub const RECURRENCE_JOB: &str = "recurrence_tick";

/// Upper bound on any sleep.
pub const MAX_SLEEP: Duration = Duration::from_secs(15 * 60);

/// Sleep when the earliest event is already due: it is being skipped (paused
/// project) or is about to be handled by the next tick.
pub const PAST_EVENT_SLEEP: Duration = Duration::from_secs(60);

/// How long the recurrence lease is held; bounds only a crash, since the
/// ticker releases it as soon as the tick finishes.
///
/// A tick never renews it, so one slower than this can overlap another
/// instance's tick. That is safe: firing and expiry are idempotent per task
/// at a given `now` (two ticks that read the same row write the same
/// result, give or take a deadline chore's end-of-column position; tick
/// writes leave `last_touched_at` alone, so neither rejects the other), and
/// a user edit landing between a tick's read and write makes that write a
/// Conflict, which the tick skips.
const LEASE_TTL: Duration = Duration::from_secs(2 * 60);

/// One minute past [`LEASE_TTL`], so a lease held by a dead instance has
/// lapsed by the time a failed or deferred tick is retried.
const RETRY_INTERVAL: Duration = Duration::from_secs(LEASE_TTL.as_secs() + 60);

/// How long to sleep given the earliest upcoming recurrence event.
///
/// Until the event if it is in the future, never more than [`MAX_SLEEP`]
/// (also the answer when nothing is scheduled), and [`PAST_EVENT_SLEEP`] when
/// the event is at or before `now`.
pub fn sleep_for(now: Timestamp, earliest: Option<Timestamp>) -> Duration {
    let Some(earliest) = earliest else {
        return MAX_SLEEP;
    };
    let wait = earliest.unix_seconds() - now.unix_seconds();
    if wait <= 0 {
        return PAST_EVENT_SLEEP;
    }
    Duration::from_secs(wait.unsigned_abs()).min(MAX_SLEEP)
}

/// Whether the earliest event is at or before `now`, i.e. there may be work.
/// Nothing scheduled means nothing due.
fn is_due(now: Timestamp, earliest: Option<Timestamp>) -> bool {
    earliest.is_some_and(|e| e <= now)
}

/// What one [`tick_once`] resolved.
enum Tick {
    /// The tick ran; the next wake is chosen from the data.
    Ran,
    /// Another instance held the lease; retry shortly.
    Deferred,
}

async fn tick_once(state: &AppState, owner: &str) -> Result<Tick, AppError> {
    let now = state.clock.now();
    // Cheap pre-check, so a wake with nothing due never touches the lease
    // table or scans tasks. A failing read falls through to the tick.
    match state.tasks.earliest_recurrence_event().await {
        Ok(earliest) if !is_due(now, earliest) => return Ok(Tick::Ran),
        Ok(_) => {}
        Err(err) => tracing::warn!(
            error = %err,
            "recurrence ticker: could not read the next event; ticking anyway"
        ),
    }
    if !state
        .leases
        .try_acquire(RECURRENCE_JOB, owner, now, LEASE_TTL)
        .await?
    {
        tracing::debug!("recurrence ticker: another instance holds the lease");
        return Ok(Tick::Deferred);
    }
    let outcome = run_tick(state, now).await;
    if let Err(err) = state.leases.release(RECURRENCE_JOB, owner).await {
        tracing::warn!(
            error = %err,
            "recurrence ticker: could not release the lease; it will expire on its own"
        );
    }
    outcome.map(|tick| {
        log_tick(tick);
        Tick::Ran
    })
}

async fn run_tick(state: &AppState, now: Timestamp) -> Result<RecurrenceTick, AppError> {
    let deps = RecurrenceDeps {
        tasks: state.tasks.as_ref(),
        projects: state.projects.as_ref(),
        board: state.board.as_ref(),
        search: state.search_index.as_ref(),
        tz: state.timezone.as_ref(),
        zone: &state.timezone_name,
    };
    run_recurrence_tick(&deps, now).await
}

fn log_tick(tick: RecurrenceTick) {
    if tick.failed > 0 {
        tracing::warn!(
            failed = tick.failed,
            "recurrence tick: some chores could not be updated (each is logged); retrying them next tick"
        );
    }
    tracing::debug!(
        fired = tick.fired,
        expired = tick.expired,
        skipped_inactive = tick.skipped_inactive,
        failed = tick.failed,
        "recurrence tick ran"
    );
}

/// Sleep after a tick that ran, from a fresh read of the earliest event.
async fn sleep_after_run(state: &AppState) -> Duration {
    match state.tasks.earliest_recurrence_event().await {
        Ok(earliest) => sleep_for(state.clock.now(), earliest),
        Err(err) => {
            tracing::warn!(error = %err, "recurrence ticker: could not read the next event");
            RETRY_INTERVAL
        }
    }
}

/// Spawns the ticker as a detached task. Checks before its first sleep. Only
/// `main.rs` calls this (see the module doc).
pub fn spawn_ticker(state: AppState) -> tokio::task::JoinHandle<()> {
    let owner = state.id_gen.next().to_string();
    tokio::spawn(async move {
        loop {
            let next = match tick_once(&state, &owner).await {
                Ok(Tick::Ran) => sleep_after_run(&state).await,
                Ok(Tick::Deferred) => RETRY_INTERVAL,
                Err(err) => {
                    tracing::warn!(error = %err, "recurrence ticker: tick failed");
                    RETRY_INTERVAL
                }
            };
            tokio::time::sleep(next).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(seconds: i64) -> Timestamp {
        Timestamp::from_unix_seconds(seconds).unwrap()
    }

    #[test]
    fn due_only_when_earliest_event_has_arrived() {
        let now = ts(1_000_000);
        assert!(!is_due(now, None));
        assert!(!is_due(now, Some(ts(1_000_001))));
        assert!(is_due(now, Some(now)));
        assert!(is_due(now, Some(ts(10))));
    }

    #[test]
    fn sleeps_until_earliest_event() {
        let now = ts(1_000_000);
        assert_eq!(
            sleep_for(now, Some(ts(1_000_000 + 300))),
            Duration::from_secs(300)
        );
    }

    #[test]
    fn caps_at_fifteen_minutes() {
        let now = ts(1_000_000);
        assert_eq!(sleep_for(now, Some(ts(1_000_000 + 3 * 3600))), MAX_SLEEP);
        assert_eq!(sleep_for(now, None), MAX_SLEEP);
    }

    #[test]
    fn past_event_sleeps_one_minute() {
        let now = ts(1_000_000);
        assert_eq!(sleep_for(now, Some(ts(999_000))), Duration::from_secs(60));
        assert_eq!(sleep_for(now, Some(now)), Duration::from_secs(60));
    }
}
