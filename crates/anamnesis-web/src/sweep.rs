//! The scheduled sweep ticker (`docs/DOMAIN.md` §6): "the shell owns a
//! ticker that asks whether a sweep is due and applies the result."
//!
//! **Kept out of the test harness, structurally.** [`spawn_ticker`] is
//! called from exactly one place in this whole workspace: `main.rs`. It is
//! never called from [`crate::routes::build_router`], from
//! [`crate::state::AppState`] construction, or from [`crate::bootstrap::run`]
//! — the three things every integration test in `tests/` (via
//! `tests/support::TestApp`) actually calls to stand up an app. A test
//! harness that never calls `spawn_ticker` can never cause a ticker to
//! spawn, full stop — this is not a timing assumption ("the test finishes
//! before the tick interval elapses") but a structural one: the code path
//! that would spawn the background task is simply not reachable from
//! anything `tests/support` builds. [`is_due`], the pure due-ness decision
//! the ticker's loop calls on each wake, is exported and unit-tested
//! directly below with no ticker involved at all.
//!
//! **Catch-up on startup.** [`is_due`] computes due-ness from
//! `Settings.last_swept_at` (real history persisted in the database), not
//! from process uptime or "has this process seen the scheduled instant
//! pass while running" — so a sweep whose scheduled time fell while the
//! server was down still reports due the moment the process starts back up
//! and the ticker's loop runs its first check (before the first sleep, not
//! after — see [`spawn_ticker`]), rather than being silently skipped until
//! the *next* cycle.
//!
//! **The loop never assumes it will wake again soon.** Its ordinary interval
//! is [`POLL_INTERVAL`], a whole day, which is only defensible because a tick
//! that leaves a due sweep unaccounted for — one that failed, or that found
//! another instance already holding the lease — says so ([`Tick`]) and gets
//! the much shorter [`RETRY_INTERVAL`]. Nothing recovers by being retried
//! implicitly on "the next ordinary wake". This is a property to preserve if
//! the loop grows another outcome: every new one has to answer "does this
//! leave work undone?" before it can pick an interval.
//!
//! **Graceful shutdown.** [`spawn_ticker`] returns a `JoinHandle` the
//! caller (`main.rs`) aborts, rather than awaits, once the server itself
//! has finished shutting down. `abort()` returns immediately — the ticker
//! never blocks or delays process exit. An abort mid-sweep is safe to
//! resume on the next boot: `archive_done_tasks` (via
//! `anamnesis_core::sweep_done`) only ever archives tasks that are not
//! already archived, so re-running it after a partial run is a no-op for
//! whatever it already finished.

use anamnesis_core::policy::Role;
use anamnesis_core::{Recurrence, Timestamp, next_run};
use std::sync::Arc;
use std::time::Duration;

use anamnesis_app::{AppError, JobLease, TimezoneResolver, archive_done_tasks};

use crate::state::AppState;

/// How long the ticker sleeps after a tick that settled the question — see
/// [`Tick::Settled`].
///
/// A day, because the thing being polled for changes at most daily.
/// `Recurrence`'s finest granularity is a weekday or a day of the month
/// (`docs/DOMAIN.md` §6), so a sweep becomes due at some local midnight and
/// stays due until it runs; asking more often than once a day cannot make it
/// fire on an earlier *date*, only at an earlier hour of the right one. And
/// because `next_run` anchors on fixed calendar dates rather than on the last
/// tick, a sweep that fires late does not push the following one late — the
/// lateness does not accumulate (`docs/DOMAIN.md`: the schedule "is
/// independent of how often the ticker happened to ask").
///
/// This is only the *upper* bound on going back to sleep. A tick that did not
/// settle the question sleeps [`RETRY_INTERVAL`] instead, and startup checks
/// before sleeping at all — so neither a failure nor a restart has to wait
/// out a day.
pub const POLL_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// How long the ticker sleeps after a tick that left a due sweep unaccounted
/// for: either it failed outright, or another instance had claimed it and
/// this one has no way to know that instance succeeded.
///
/// One minute past [`SWEEP_LEASE_TTL`], so that by the time the re-check
/// happens a lease held by an instance that has since died has certainly
/// lapsed and can be claimed. This is what lets [`POLL_INTERVAL`] be a whole
/// day: nothing in the loop assumes a failed or skipped tick will be picked
/// up by "the next ordinary wake, which is soon".
const RETRY_INTERVAL: Duration = Duration::from_secs(SWEEP_LEASE_TTL.as_secs() + 60);

/// The lease name the scheduled sweep coordinates on.
pub const SWEEP_JOB: &str = "archive_sweep";

/// How long the sweep lease is held for.
///
/// The ticker releases it the moment the sweep finishes, so this number only
/// ever bounds a *crash*: long enough that a slow sweep over a large board
/// is not overtaken by another instance, short enough that an instance killed
/// mid-sweep does not block the next scheduled one for long.
///
/// Never renewed, though `JobLease` supports it. A sweep's runtime is bounded
/// by how much is sitting in the Done column, so one generous TTL covers the
/// worst case that a heartbeat would — and if an overlong sweep were overtaken
/// anyway, the second instance reloads each task and re-asks `sweep_done`, so
/// it skips whatever the first already archived. Revisit if that runtime ever
/// becomes genuinely unbounded; the fix then is a heartbeat, not a bigger
/// number.
const SWEEP_LEASE_TTL: Duration = Duration::from_secs(300);

/// What one [`tick_once`] resolved — which is the whole of what decides when
/// the ticker next wakes.
///
/// The ticker deliberately does not have a single interval any more. "When
/// should I look again?" has two genuinely different answers depending on
/// whether this tick left anything hanging, and collapsing them into one
/// number is what would force that number to be small.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tick {
    /// The schedule is satisfied: either nothing was due, or a sweep was due
    /// and this instance ran it and stamped `last_swept_at`. Nothing further
    /// can happen until the next scheduled occurrence, which is at soonest
    /// the next local midnight.
    Settled,
    /// A sweep was due and this instance did not run it, because another
    /// instance held the lease. That instance has very probably finished it —
    /// but "very probably" is not a thing to encode as a day of silence, and
    /// an instance that died mid-sweep leaves nothing else to notice. Ask
    /// again shortly instead.
    Deferred,
}

/// Whether a sweep is due right now, given the configured `recurrence`,
/// when the last sweep actually ran (`None` if never), the current instant,
/// and the timezone every local-date calculation in this system is resolved
/// against.
///
/// `Recurrence::Never` is never due. Otherwise: due whenever the next
/// scheduled occurrence *after* the last sweep (or, if the sweep has never
/// run, after the Unix epoch — so a schedule whose first occurrence already
/// lies in the past catches up immediately on a fresh install rather than
/// waiting a full cycle) has already arrived. This is what makes catch-up
/// work across downtime: it does not matter how many scheduled occurrences
/// were missed while the server was down, only whether *a* due occurrence
/// exists before `now` — running the sweep once catches up on all of them
/// at once, since `sweep_done` just archives whatever is currently sitting
/// in a done column regardless of how long it has been there.
pub fn is_due(
    recurrence: Recurrence,
    last_swept_at: Option<Timestamp>,
    now: Timestamp,
    timezone: &dyn TimezoneResolver,
    timezone_name: &str,
) -> Result<bool, AppError> {
    if recurrence == Recurrence::Never {
        return Ok(false);
    }
    let from_date = match last_swept_at {
        Some(ts) => timezone.local_date(timezone_name, ts)?,
        None => time::Date::from_calendar_date(1970, time::Month::January, 1)
            .expect("1970-01-01 is a valid calendar date"),
    };
    let Some(next_date) = next_run(recurrence, from_date) else {
        return Ok(false);
    };
    let next_instant = timezone.to_utc(timezone_name, next_date, time::Time::MIDNIGHT)?;
    Ok(next_instant <= now)
}

/// One pass of the ticker: loads current settings, decides due-ness via
/// [`is_due`], claims the [`SWEEP_JOB`] lease, and — if it gets the lease —
/// runs the sweep.
///
/// The lease goes *inside* the due branch rather than around the whole tick,
/// so instances that agree there is nothing to do never touch the lease table
/// at all. [`is_due`] itself stays untouched: it is pure and correct, and the
/// race was never in it. It reads `last_swept_at`, which every instance reads
/// and only the sweeping instance writes, so with N instances running all N
/// see the same sweep become due at the same moment — the lease is what makes
/// exactly one of them act on it.
async fn tick_once(state: &AppState, leases: &dyn JobLease, owner: &str) -> Result<Tick, AppError> {
    let settings = state.settings.load().await?;
    let now = state.clock.now();
    if !is_due(
        settings.sweep_recurrence,
        settings.last_swept_at,
        now,
        state.timezone.as_ref(),
        &state.timezone_name,
    )? {
        return Ok(Tick::Settled);
    }

    if !leases
        .try_acquire(SWEEP_JOB, owner, now, SWEEP_LEASE_TTL)
        .await?
    {
        tracing::debug!("sweep ticker: a sweep is due, but another instance is running it");
        return Ok(Tick::Deferred);
    }

    let outcome = run_sweep(state, now).await;

    // Released as soon as the sweep finishes rather than held for the whole
    // TTL: `record_sweep` has already moved `last_swept_at`, so no instance
    // will find this sweep due again anyway, and an early release means a
    // genuinely *later* sweep is never blocked by this one's stale claim.
    // Best-effort on purpose — a failed release costs the rest of the TTL,
    // which is not worth failing an otherwise successful sweep over.
    if let Err(err) = leases.release(SWEEP_JOB, owner).await {
        tracing::warn!(
            error = %err,
            "sweep ticker: could not release the sweep lease; it will expire on its own"
        );
    }
    outcome.map(|()| Tick::Settled)
}

/// The sweep itself: `anamnesis_app::archive_done_tasks` (the same operation
/// the manual "Archive all" button calls), then a `last_swept_at` stamp via
/// `SettingsRepository::record_sweep` so it does not immediately re-fire on
/// the ticker's very next wake.
///
/// The lease decides *who* sweeps; `record_sweep` records *that* one
/// happened. They answer different questions, so both stay.
async fn run_sweep(state: &AppState, now: Timestamp) -> Result<(), AppError> {
    let archived = archive_done_tasks(
        state.board.as_ref(),
        state.tasks.as_ref(),
        state.tangles.as_ref(),
        state.clock.as_ref(),
        state.search_index.as_ref(),
        Some(Role::SystemAdmin),
    )
    .await?;

    state.settings.record_sweep(now).await?;

    tracing::info!(
        archived_task_count = archived.archived_task_ids.len(),
        archived_task_ids = ?archived.archived_task_ids,
        archived_tangle_count = archived.archived_tangle_ids.len(),
        archived_tangle_ids = ?archived.archived_tangle_ids,
        "scheduled sweep ran"
    );
    Ok(())
}

/// Spawns the background ticker as a detached `tokio` task and returns its
/// `JoinHandle`. The loop checks due-ness *before* its first sleep (not
/// after), so a catch-up sweep runs promptly on startup rather than waiting
/// a full [`POLL_INTERVAL`] — see the module doc comment's "Catch-up on
/// startup" section.
///
/// Each pass chooses its own sleep from what the tick actually resolved
/// ([`Tick`]), which is the whole reason [`POLL_INTERVAL`] can be a day: a
/// tick that failed, or that found a sweep due and left it to another
/// instance, is the one case where the loop's next wake is load-bearing, and
/// that case gets [`RETRY_INTERVAL`] instead. The long interval therefore
/// costs nothing but the hour of the day a sweep lands on.
///
/// See the module doc comment's "Kept out of the test harness" section for
/// why this function has exactly one call site in the whole workspace.
///
/// `leases` is a parameter rather than an `AppState` field because no
/// *handler* needs it — the ticker is the only thing in a running server that
/// coordinates with other instances, and `AppState` is defined as what a
/// handler needs.
pub fn spawn_ticker(state: AppState, leases: Arc<dyn JobLease>) -> tokio::task::JoinHandle<()> {
    // One identity for the whole life of the process. It has to be stable
    // across ticks: the lease's owner is what lets this instance renew and
    // release its own claim instead of contending with itself.
    let owner = state.id_gen.next().to_string();
    tokio::spawn(async move {
        loop {
            let next = match tick_once(&state, leases.as_ref(), &owner).await {
                Ok(Tick::Settled) => POLL_INTERVAL,
                Ok(Tick::Deferred) => RETRY_INTERVAL,
                Err(err) => {
                    tracing::error!(error = %err, "sweep ticker: failed to check or run a scheduled sweep");
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
    use anamnesis_adapters::TzTimezoneResolver;

    fn ts(seconds: i64) -> Timestamp {
        Timestamp::from_unix_seconds(seconds).unwrap()
    }

    fn tz() -> TzTimezoneResolver {
        TzTimezoneResolver::new()
    }

    const UTC: &str = "UTC";

    #[test]
    fn never_is_never_due() {
        let due = is_due(Recurrence::Never, None, ts(999_999_999), &tz(), UTC).unwrap();
        assert!(!due);
    }

    #[test]
    fn a_day_of_month_schedule_is_not_due_before_its_local_midnight() {
        // 2026-06-15 00:00:00 UTC.
        let scheduled_midnight = tz()
            .to_utc(
                UTC,
                time::Date::from_calendar_date(2026, time::Month::June, 15).unwrap(),
                time::Time::MIDNIGHT,
            )
            .unwrap();
        let just_before = ts(scheduled_midnight.unix_seconds() - 1);
        let last_swept_at = Some(ts(tz()
            .to_utc(
                UTC,
                time::Date::from_calendar_date(2026, time::Month::May, 15).unwrap(),
                time::Time::MIDNIGHT,
            )
            .unwrap()
            .unix_seconds()));

        let due = is_due(
            Recurrence::DayOfMonth { day: 15 },
            last_swept_at,
            just_before,
            &tz(),
            UTC,
        )
        .unwrap();
        assert!(
            !due,
            "must not be due one second before the scheduled instant"
        );
    }

    #[test]
    fn a_day_of_month_schedule_is_due_once_its_local_midnight_has_arrived() {
        let scheduled_midnight = tz()
            .to_utc(
                UTC,
                time::Date::from_calendar_date(2026, time::Month::June, 15).unwrap(),
                time::Time::MIDNIGHT,
            )
            .unwrap();
        let last_swept_at = Some(ts(tz()
            .to_utc(
                UTC,
                time::Date::from_calendar_date(2026, time::Month::May, 15).unwrap(),
                time::Time::MIDNIGHT,
            )
            .unwrap()
            .unix_seconds()));

        let due = is_due(
            Recurrence::DayOfMonth { day: 15 },
            last_swept_at,
            scheduled_midnight,
            &tz(),
            UTC,
        )
        .unwrap();
        assert!(due, "must be due exactly at the scheduled instant");

        let well_after = ts(scheduled_midnight.unix_seconds() + 3600);
        let due = is_due(
            Recurrence::DayOfMonth { day: 15 },
            last_swept_at,
            well_after,
            &tz(),
            UTC,
        )
        .unwrap();
        assert!(due, "must still be due well after the scheduled instant");
    }

    #[test]
    fn catch_up_fires_when_last_swept_at_is_older_than_a_missed_occurrence() {
        // Schedule: the 15th of every month. Never actually swept
        // (`last_swept_at: None`) is the sharpest catch-up case -- there is
        // no history at all, yet a long-past scheduled occurrence must
        // still be caught by treating "never swept" as swept at the epoch.
        let far_future = ts(tz()
            .to_utc(
                UTC,
                time::Date::from_calendar_date(2026, time::Month::June, 20).unwrap(),
                time::Time::MIDNIGHT,
            )
            .unwrap()
            .unix_seconds());
        let due = is_due(
            Recurrence::DayOfMonth { day: 15 },
            None,
            far_future,
            &tz(),
            UTC,
        )
        .unwrap();
        assert!(
            due,
            "a never-swept installation with a schedule already in the past must catch up"
        );

        // The sharper case the task calls out explicitly: swept once, a
        // long time ago (as if the server had been down across one or more
        // scheduled occurrences), and now well past the next one.
        let last_swept_at = Some(ts(tz()
            .to_utc(
                UTC,
                time::Date::from_calendar_date(2026, time::Month::January, 15).unwrap(),
                time::Time::MIDNIGHT,
            )
            .unwrap()
            .unix_seconds()));
        let due = is_due(
            Recurrence::DayOfMonth { day: 15 },
            last_swept_at,
            far_future,
            &tz(),
            UTC,
        )
        .unwrap();
        assert!(
            due,
            "a sweep last run months ago, now well past the next scheduled occurrence, must catch up"
        );
    }

    #[test]
    fn every_n_weeks_recurrence_is_supported_too() {
        let recurrence = Recurrence::EveryNWeeks {
            n: 2,
            weekday: time::Weekday::Monday,
        };
        // Anything far enough in the future is due for a never-swept
        // install; the exact anchor math is `anamnesis_core::next_run`'s
        // own, already-tested responsibility -- this just proves `is_due`
        // wires an `EveryNWeeks` recurrence through correctly.
        let far_future = ts(4_102_444_800); // 2100-01-01 UTC
        let due = is_due(recurrence, None, far_future, &tz(), UTC).unwrap();
        assert!(due);
    }
}
