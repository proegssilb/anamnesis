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
use std::time::Duration;

use anamnesis_app::{AppError, TimezoneResolver, archive_done_tasks};

use crate::state::AppState;

/// How often the ticker wakes to check due-ness. Small deliberately — "a
/// small interval, a minute or so" — not a busy loop, but frequent enough
/// that a due sweep fires within a minute of becoming due.
pub const TICK_INTERVAL: Duration = Duration::from_secs(60);

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
/// [`is_due`], and — if due — runs the sweep (`anamnesis_app::archive_done_tasks`,
/// the same operation the manual "Archive all" button calls) and stamps
/// `last_swept_at` via `SettingsRepository::record_sweep` so the sweep does
/// not immediately re-fire on the ticker's very next wake.
async fn tick_once(state: &AppState) -> Result<(), AppError> {
    let settings = state.settings.load().await?;
    let now = state.clock.now();
    if !is_due(
        settings.sweep_recurrence,
        settings.last_swept_at,
        now,
        state.timezone.as_ref(),
        &state.timezone_name,
    )? {
        return Ok(());
    }

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
/// a full [`TICK_INTERVAL`] — see the module doc comment's "Catch-up on
/// startup" section.
///
/// See the module doc comment's "Kept out of the test harness" section for
/// why this function has exactly one call site in the whole workspace.
pub fn spawn_ticker(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if let Err(err) = tick_once(&state).await {
                tracing::error!(error = %err, "sweep ticker: failed to check or run a scheduled sweep");
            }
            tokio::time::sleep(TICK_INTERVAL).await;
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
