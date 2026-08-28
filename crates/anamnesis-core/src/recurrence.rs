//! [`Recurrence`] and sweeping (`docs/DOMAIN.md` §6): the first genuinely
//! time-triggered behaviour in the system, still kept pure.
//!
//! Two pure functions, no I/O:
//! - [`next_run`] — the next *local calendar date* a sweep is due, strictly
//!   after `from`, or `None` for [`Recurrence::Never`].
//! - [`sweep_done`] — which tasks a sweep (scheduled *or* the manual
//!   "Archive all" button) should archive.
//!
//! ## Resolved ambiguities
//!
//! `docs/DOMAIN.md` §6 gives `Recurrence` no time-of-day field — only a
//! weekday or a day-of-month. This module resolves that by scheduling every
//! sweep at **local midnight (00:00:00)** on the due date: "Archive day" is
//! naturally the start of the day. (Turning that local midnight into a UTC
//! instant is not this module's job — see "Timezones are not this crate's
//! job" below.)
//!
//! **`EveryNWeeks` anchor.** "Every other Monday" is ambiguous on its own —
//! which Monday is the on-cycle one? This resolves it to a fixed epoch,
//! independent of `from`: the first occurrence of `weekday` on or after the
//! Unix epoch (1970-01-01, a Thursday). Valid run-dates are
//! `anchor + k * n` weeks for `k = 0, 1, 2, ...`; `next_run` returns the
//! smallest one strictly after `from`. Because the sequence is fixed rather
//! than derived from whatever `from` happens to be, feeding one call's result
//! back in as the next call's `from` always steps by exactly `n` weeks, and
//! two different `from` values in the same cycle resolve to the same next
//! occurrence — it does not collapse to "every week" no matter how often the
//! ticker asks.
//!
//! **`DayOfMonth` clamping.** A day past the end of a short month (the 31st
//! in April, the 29th–31st in February) clamps to that month's actual last
//! day, per [`time::Month::length`] (leap-aware) — the sane reading of "the
//! 15th" for a month that has no such day, rather than rejecting the
//! recurrence or skipping the month.
//!
//! ## Timezones are not this crate's job
//!
//! An earlier version of this module modeled `Timezone`/`DstRule` itself —
//! a standard offset plus a hand-rolled "Nth weekday of the month" DST rule
//! — and had `next_run` take a `Timestamp` and convert through it. That was
//! a defect, not a simplification: real DST rules change by government
//! decree (Brazil abolished DST in 2019, Mexico and Iran dropped most of it
//! in 2022, Jordan and Syria moved permanently, Chile shifts its dates —
//! all with as little as weeks of notice), a hand-curated rule silently
//! goes stale, and a rule reapplied to a historical timestamp gives that
//! timestamp *today's* rule instead of whatever was in force then. No
//! amount of careful hand-rolling fixes that; it needs a real IANA tzdb.
//!
//! `anamnesis-core`'s dependency list stays exactly `serde`, `thiserror`,
//! `time`, `uuid` — no tzdb crate belongs in the pure domain model. So this
//! module now works **purely in local calendar terms**: [`next_run`] takes
//! and returns a plain [`Date`], with no timezone or offset anywhere in
//! sight. Converting a UTC instant to "what local date is it" (to get a
//! `from`), and converting the resulting local date (at local midnight) back
//! to a UTC instant (to actually schedule the sweep), is the caller's job —
//! `anamnesis_app::ports::TimezoneResolver`, backed by a real tzdb in
//! `anamnesis-adapters`.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use time::{Date, Month, Weekday};

use crate::column::Column;
use crate::ids::{ColumnId, TaskId, Timestamp};
use crate::placement::Placement;
use crate::task::Task;

/// How a scheduled sweep repeats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Recurrence {
    /// "Every other Monday": every `n`-th occurrence of `weekday`, anchored
    /// to a fixed epoch so the cadence never drifts (see the module doc
    /// comment and [`next_run`]).
    EveryNWeeks { n: u8, weekday: Weekday },
    /// "The 15th": a fixed day of the month, clamped to the month's actual
    /// last day when it runs short.
    DayOfMonth { day: u8 },
    /// No schedule at all. [`next_run`] always yields `None`.
    Never,
}

/// `day`, clamped to the last real day of `year`/`month`.
fn clamp_day_of_month(year: i32, month: Month, day: u8) -> u8 {
    day.min(month.length(year))
}

/// The fixed anchor date for `EveryNWeeks`: the first occurrence of
/// `weekday` on or after the Unix epoch (1970-01-01, a Thursday).
fn epoch_anchor(weekday: Weekday) -> Date {
    let epoch = Date::from_calendar_date(1970, Month::January, 1).expect("valid date");
    let delta = (i64::from(weekday.number_days_from_monday())
        - i64::from(epoch.weekday().number_days_from_monday()))
    .rem_euclid(7);
    epoch + time::Duration::days(delta)
}

fn next_every_n_weeks_date(n: u8, weekday: Weekday, from: Date) -> Date {
    let anchor = epoch_anchor(weekday);
    let step = i64::from(n.max(1)) * 7;
    let diff_days = (from - anchor).whole_days();
    let k = diff_days.div_euclid(step) + 1;
    anchor + time::Duration::days(step * k)
}

fn next_day_of_month_date(day: u8, from: Date) -> Date {
    let mut year = from.year();
    let mut month = from.month();
    loop {
        let clamped = clamp_day_of_month(year, month, day);
        let candidate =
            Date::from_calendar_date(year, month, clamped).expect("clamped day fits the month");
        if candidate > from {
            return candidate;
        }
        if month == Month::December {
            year += 1;
        }
        month = month.next();
    }
}

/// The next local calendar date a sweep is due, strictly after `from`.
/// `Recurrence::Never` never sweeps.
///
/// Every recurrence resolves to local midnight (00:00:00) on the returned
/// date — see the "Resolved ambiguities" section of the module doc comment.
/// Turning `from` (an instant) into a local date, and turning the returned
/// date back into an instant to actually schedule, is the caller's job: see
/// "Timezones are not this crate's job" above.
pub fn next_run(recurrence: Recurrence, from: Date) -> Option<Date> {
    match recurrence {
        Recurrence::Never => None,
        Recurrence::EveryNWeeks { n, weekday } => Some(next_every_n_weeks_date(n, weekday, from)),
        Recurrence::DayOfMonth { day } => Some(next_day_of_month_date(day, from)),
    }
}

/// Which tasks a sweep should archive: everything currently `OnBoard` in a
/// column whose `is_done` is true, excluding tasks already archived.
///
/// This serves **both** the scheduled sweep and the manual "Archive all"
/// button (`docs/DOMAIN.md` §6) — the manual path is just this same
/// function, called on demand instead of from a ticker. No second function
/// is needed: "which done-column tasks are archivable right now" is the
/// exact question both paths ask. A sweep with nothing to archive is not an
/// error — it returns an empty `Vec`.
///
/// `now` is accepted, not inspected: every other mutating transition in
/// this crate (`archive_task` included) takes `now` to stamp onto the
/// result, and keeping the same shape here means a caller can call this,
/// then `archive_task(&task, now)` for each id, with the one `now` it
/// already has. It also leaves room for a future time-gated rule (e.g. "only
/// sweep tasks that have sat in Done for at least a day") without changing
/// this function's signature — not in scope per `docs/DOMAIN.md` §6, which
/// defines "done" purely by column membership.
pub fn sweep_done(tasks: &[Task], columns: &[Column], _now: Timestamp) -> Vec<TaskId> {
    let done_columns: HashSet<ColumnId> = columns
        .iter()
        .filter(|column| column.is_done)
        .map(|column| column.id)
        .collect();

    tasks
        .iter()
        .filter(|task| task.archived_at.is_none())
        .filter_map(|task| match task.placement {
            Placement::OnBoard { column, .. } if done_columns.contains(&column) => Some(task.id),
            _ => None,
        })
        .collect()
}

/// Which tangles a sweep should archive: the [`sweep_done`] sibling for
/// [`crate::Tangle`] (`docs/DOMAIN.md`'s Tangle section: "the archive sweep
/// then treats it like anything else").
///
/// A tangle qualifies only when it is **both** `OnBoard` in an `is_done`
/// column **and already resolved** (`resolved_at.is_some()`) — unlike a
/// task, whose column membership alone decides done-ness, a tangle can sit
/// in a Done column while still unresolved (a user is free to place one
/// there directly), and an unresolved knot still has real work left in it.
/// Archiving it anyway would silently make the card impossible to find
/// again while the knot is still tied. Already-archived tangles are
/// excluded, exactly as [`sweep_done`] excludes already-archived tasks.
pub fn sweep_done_tangles(
    tangles: &[crate::Tangle],
    columns: &[Column],
    _now: Timestamp,
) -> Vec<crate::TangleId> {
    let done_columns: HashSet<ColumnId> = columns
        .iter()
        .filter(|column| column.is_done)
        .map(|column| column.id)
        .collect();

    tangles
        .iter()
        .filter(|tangle| tangle.archived_at.is_none() && tangle.resolved_at.is_some())
        .filter_map(|tangle| match tangle.placement {
            Placement::OnBoard { column, .. } if done_columns.contains(&column) => Some(tangle.id),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use uuid::Uuid;

    use crate::column::create_column;
    use crate::ids::{ProjectId, TangleId};
    use crate::tangle::{Fingerprint, Tangle};
    use crate::task::{create_task, move_placement};

    fn ts(seconds: i64) -> Timestamp {
        Timestamp::from_unix_seconds(seconds).unwrap()
    }

    fn date(year: i32, month: Month, day: u8) -> Date {
        Date::from_calendar_date(year, month, day).unwrap()
    }

    // --- Never ---

    #[test]
    fn never_yields_no_next_run() {
        assert_eq!(
            next_run(Recurrence::Never, date(1970, Month::January, 1)),
            None
        );
        assert_eq!(
            next_run(Recurrence::Never, date(2026, Month::June, 1)),
            None
        );
    }

    // --- DayOfMonth: ordinary + month-end clamping ---

    #[test]
    fn day_of_month_returns_this_month_when_the_day_is_still_ahead() {
        let from = date(2026, Month::June, 1);
        let next = next_run(Recurrence::DayOfMonth { day: 15 }, from).unwrap();
        assert_eq!(next, date(2026, Month::June, 15));
    }

    #[test]
    fn day_of_month_rolls_to_next_month_once_the_day_has_passed() {
        let from = date(2026, Month::June, 20);
        let next = next_run(Recurrence::DayOfMonth { day: 15 }, from).unwrap();
        assert_eq!(next, date(2026, Month::July, 15));
    }

    #[test]
    fn day_of_month_31_clamps_to_the_last_day_of_a_30_day_month() {
        let from = date(2026, Month::March, 31); // day already passed for March
        let next = next_run(Recurrence::DayOfMonth { day: 31 }, from).unwrap();
        // April has only 30 days.
        assert_eq!(next, date(2026, Month::April, 30));
    }

    #[test]
    fn day_of_month_31_clamps_to_february_28_in_a_non_leap_year() {
        let from = date(2025, Month::January, 31); // 2025 is not a leap year
        let next = next_run(Recurrence::DayOfMonth { day: 31 }, from).unwrap();
        assert_eq!(next, date(2025, Month::February, 28));
    }

    #[test]
    fn day_of_month_31_clamps_to_february_29_in_a_leap_year() {
        let from = date(2024, Month::January, 31); // 2024 is a leap year
        let next = next_run(Recurrence::DayOfMonth { day: 31 }, from).unwrap();
        assert_eq!(next, date(2024, Month::February, 29));
    }

    #[test]
    fn day_of_month_rolls_over_the_year_boundary() {
        let from = date(2026, Month::December, 20);
        let next = next_run(Recurrence::DayOfMonth { day: 15 }, from).unwrap();
        assert_eq!(next, date(2027, Month::January, 15));
    }

    // --- EveryNWeeks: anchoring ---

    #[test]
    fn every_n_weeks_steps_by_exactly_n_weeks_across_consecutive_calls() {
        let recurrence = Recurrence::EveryNWeeks {
            n: 2,
            weekday: Weekday::Monday,
        };
        let first = next_run(recurrence, date(2026, Month::January, 1)).unwrap();
        let second = next_run(recurrence, first).unwrap();
        let third = next_run(recurrence, second).unwrap();

        assert_eq!((second - first).whole_days(), 14);
        assert_eq!((third - second).whole_days(), 14);
    }

    #[test]
    fn every_n_weeks_does_not_collapse_to_every_week() {
        // n = 2: an "off-cycle" Monday one week after the first on-cycle
        // Monday must NOT be a valid next run — the cadence must skip it.
        let recurrence = Recurrence::EveryNWeeks {
            n: 2,
            weekday: Weekday::Monday,
        };
        let first = next_run(recurrence, date(2026, Month::January, 1)).unwrap();
        // Asking again from a date a few days into the off-cycle week must
        // still skip forward to the on-cycle Monday two weeks after `first`,
        // not the Monday one week after it.
        let mid_week_after = first + time::Duration::days(3);
        let next = next_run(recurrence, mid_week_after).unwrap();
        assert_eq!((next - first).whole_days(), 14);
    }

    #[test]
    fn every_n_weeks_anchor_is_independent_of_from_within_a_cycle() {
        // Two different `from` values that both fall before the same
        // on-cycle Monday must resolve to that same date — the anchor is
        // fixed, not derived from whichever `from` happened to be passed.
        let recurrence = Recurrence::EveryNWeeks {
            n: 2,
            weekday: Weekday::Monday,
        };
        let from_a = date(2026, Month::January, 1);
        let from_b = date(2026, Month::January, 4);
        let next_a = next_run(recurrence, from_a).unwrap();
        let next_b = next_run(recurrence, from_b).unwrap();
        assert_eq!(next_a, next_b);
    }

    // --- sweep_done ---

    fn pid() -> ProjectId {
        ProjectId::new(Uuid::from_u128(1))
    }

    fn column(n: u128, is_done: bool) -> Column {
        create_column(
            ColumnId::new(Uuid::from_u128(n)),
            "Column",
            0,
            None,
            is_done,
        )
        .unwrap()
    }

    fn task_on(id: u128, column: ColumnId) -> Task {
        let t = create_task(TaskId::new(Uuid::from_u128(id)), pid(), "Task", "", ts(0)).unwrap();
        move_placement(
            &t,
            Placement::OnBoard {
                column,
                position: 0,
            },
            ts(0),
        )
        .unwrap()
    }

    fn task_below(id: u128) -> Task {
        create_task(TaskId::new(Uuid::from_u128(id)), pid(), "Task", "", ts(0)).unwrap()
    }

    #[test]
    fn sweep_done_archives_only_tasks_in_done_columns() {
        let done = column(1, true);
        let doing = column(2, false);

        let done_task = task_on(1, done.id);
        let doing_task = task_on(2, doing.id);
        let below_task = task_below(3);

        let result = sweep_done(
            &[done_task.clone(), doing_task, below_task],
            &[done.clone(), doing],
            ts(100),
        );

        assert_eq!(result, vec![done_task.id]);
    }

    #[test]
    fn sweep_done_excludes_already_archived_tasks() {
        let done = column(1, true);
        let done_task = task_on(1, done.id);
        let archived_task = crate::task::archive_task(&task_on(2, done.id), ts(50)).unwrap();

        let result = sweep_done(&[done_task.clone(), archived_task], &[done], ts(100));

        assert_eq!(result, vec![done_task.id]);
    }

    #[test]
    fn sweep_done_is_a_no_op_when_nothing_qualifies() {
        let doing = column(1, false);
        let doing_task = task_on(1, doing.id);
        let below_task = task_below(2);

        let result = sweep_done(&[doing_task, below_task], &[doing], ts(100));

        assert_eq!(result, Vec::<TaskId>::new());
    }

    #[test]
    fn sweep_done_ignores_columns_not_present_on_any_task() {
        // An empty task list is a no-op regardless of column configuration.
        let done = column(1, true);
        let result = sweep_done(&[], &[done], ts(100));
        assert_eq!(result, Vec::<TaskId>::new());
    }

    // --- sweep_done_tangles (gap 2: resolved tangles piling up in Done) ---

    fn tangle_ids(n: u128) -> BTreeSet<TaskId> {
        [TaskId::new(Uuid::from_u128(n))].into_iter().collect()
    }

    fn tangle_on(id: u128, column: ColumnId, resolved: bool) -> Tangle {
        let task_ids = tangle_ids(id);
        Tangle {
            id: TangleId::new(Uuid::from_u128(id)),
            fingerprint: Fingerprint::of(&task_ids),
            task_ids,
            placement: Placement::OnBoard {
                column,
                position: 0,
            },
            frozen: true,
            detected_at: ts(0),
            resolved_at: resolved.then_some(ts(50)),
            archived_at: None,
        }
    }

    fn tangle_below(id: u128) -> Tangle {
        let task_ids = tangle_ids(id);
        Tangle {
            id: TangleId::new(Uuid::from_u128(id)),
            fingerprint: Fingerprint::of(&task_ids),
            task_ids,
            placement: Placement::Below,
            frozen: false,
            detected_at: ts(0),
            resolved_at: None,
            archived_at: None,
        }
    }

    #[test]
    fn sweep_done_tangles_archives_only_resolved_tangles_in_done_columns() {
        let done = column(1, true);
        let doing = column(2, false);

        let resolved_in_done = tangle_on(1, done.id, true);
        let unresolved_in_done = tangle_on(2, done.id, false);
        let resolved_in_doing = tangle_on(3, doing.id, true);
        let resolved_below = tangle_below(4);

        let result = sweep_done_tangles(
            &[
                resolved_in_done.clone(),
                unresolved_in_done,
                resolved_in_doing,
                resolved_below,
            ],
            &[done, doing],
            ts(100),
        );

        assert_eq!(
            result,
            vec![resolved_in_done.id],
            "only a resolved tangle sitting in an is_done column is swept"
        );
    }

    #[test]
    fn sweep_done_tangles_excludes_already_archived_tangles() {
        let done = column(1, true);
        let resolved = tangle_on(1, done.id, true);
        let archived = crate::tangle::archive_tangle(&tangle_on(2, done.id, true), ts(50)).unwrap();

        let result = sweep_done_tangles(&[resolved.clone(), archived], &[done], ts(100));

        assert_eq!(result, vec![resolved.id]);
    }

    #[test]
    fn sweep_done_tangles_is_a_no_op_when_nothing_qualifies() {
        let doing = column(1, false);
        let unresolved = tangle_on(1, doing.id, false);
        let below = tangle_below(2);

        let result = sweep_done_tangles(&[unresolved, below], &[doing], ts(100));

        assert_eq!(result, Vec::<TangleId>::new());
    }
}
