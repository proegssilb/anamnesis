//! [`Recurrence`] and sweeping (`docs/DOMAIN.md` §6): the first genuinely
//! time-triggered behaviour in the system, still kept pure.
//!
//! Two pure functions, no I/O — `now`/`from` and the timezone always arrive
//! as parameters:
//! - [`next_run`] — when the next sweep is due, or `None` for
//!   [`Recurrence::Never`].
//! - [`sweep_done`] — which tasks a sweep (scheduled *or* the manual
//!   "Archive all" button) should archive.
//!
//! ## Resolved ambiguities
//!
//! `docs/DOMAIN.md` §6 gives `Recurrence` no time-of-day field — only a
//! weekday or a day-of-month. This module resolves that by scheduling every
//! sweep at **local midnight (00:00:00)** on the due date: "Archive day" is
//! naturally the start of the day, and it gives the DST tests below a real
//! transition to straddle without inventing a field the design doesn't have.
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
//! ## Timezone and DST — what this crate can and cannot do
//!
//! `anamnesis-core`'s dependency list is deliberately just `serde`,
//! `thiserror`, `time`, `uuid`. The bare `time` crate carries no IANA time
//! zone database — genuine "America/New_York" lookups need an additional
//! crate (`time-tz` + `tzdb`, or similar) to embed that data. Pulling one in
//! was judged out of proportion for this phase (`docs/DOMAIN.md` §10 scopes
//! Phase C to the two pure functions plus timezone handling, not a general
//! tz-database integration), so **no new dependency was added.**
//!
//! Instead, [`Timezone`] models a timezone as data this crate already has
//! the primitives for: a standard [`UtcOffset`] plus an optional [`DstRule`]
//! describing exactly when a saving applies, expressed as "the Nth (or last)
//! weekday of a given month, at a given local time" — precisely how real
//! DST rules such as the current US and EU ones are phrased. This is
//! **genuine, tested DST handling for whatever rule the caller configures**
//! (the tests below use the real current US rule: second Sunday in March,
//! first Sunday in November), not a single fixed offset masquerading as one.
//! It is not a substitute for a full tzdb: it does not know historical rule
//! changes, and it does not track *which* IANA zone the caller means — the
//! caller (eventually, `Settings.timezone`) supplies the standard offset and
//! rule directly. That mapping from an IANA zone name to a `Timezone` value
//! is a shell/adapter concern for a later phase, not this one.
//!
//! One further simplification, also documented rather than silently assumed:
//! the day-of-week comparison used to decide whether a given local moment
//! falls inside the DST window is a **naive (offset-free) comparison** —
//! exactly the two-step algorithm real timezone libraries use. It is exact
//! everywhere except *inside* the transition's own skipped hour (spring) or
//! repeated hour (fall), which this phase does not need to disambiguate:
//! sweeps run at local midnight, and neither the US nor EU rule transitions
//! at midnight, so a scheduled sweep never itself lands in that hour.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset, Weekday};

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

/// Which occurrence of a weekday within its month a DST transition falls on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WeekOfMonth {
    First,
    Second,
    Third,
    Fourth,
    Last,
}

/// One DST transition, phrased the way real rules are: "the Nth (or last)
/// weekday of a month, at a given local time".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DstTransition {
    pub month: Month,
    pub week: WeekOfMonth,
    pub weekday: Weekday,
    pub local_time: Time,
}

impl DstTransition {
    fn naive_for_year(&self, year: i32) -> PrimitiveDateTime {
        let date = match self.week {
            WeekOfMonth::First => nth_weekday_of_month(year, self.month, self.weekday, 1),
            WeekOfMonth::Second => nth_weekday_of_month(year, self.month, self.weekday, 2),
            WeekOfMonth::Third => nth_weekday_of_month(year, self.month, self.weekday, 3),
            WeekOfMonth::Fourth => nth_weekday_of_month(year, self.month, self.weekday, 4),
            WeekOfMonth::Last => last_weekday_of_month(year, self.month, self.weekday),
        };
        PrimitiveDateTime::new(date, self.local_time)
    }
}

/// A daylight-saving rule: a `saving` applied between `starts` and `ends`,
/// recomputed for whichever year is in play.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DstRule {
    pub saving: UtcOffset,
    pub starts: DstTransition,
    pub ends: DstTransition,
}

/// A civil timezone: a standard offset, plus an optional [`DstRule`]. See
/// the module doc comment for exactly what this does and does not model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timezone {
    pub standard_offset: UtcOffset,
    pub dst: Option<DstRule>,
}

impl Timezone {
    /// A timezone with no daylight saving at all — a fixed offset year-round.
    pub const fn fixed(standard_offset: UtcOffset) -> Self {
        Self {
            standard_offset,
            dst: None,
        }
    }

    /// A timezone that observes the given DST rule.
    pub const fn with_dst(standard_offset: UtcOffset, dst: DstRule) -> Self {
        Self {
            standard_offset,
            dst: Some(dst),
        }
    }

    /// Which offset applies at a given naive (offset-free) local moment.
    fn offset_for_naive(&self, naive: PrimitiveDateTime) -> UtcOffset {
        let Some(rule) = &self.dst else {
            return self.standard_offset;
        };
        let year = naive.year();
        let start = rule.starts.naive_for_year(year);
        let end = rule.ends.naive_for_year(year);
        let in_dst = if start <= end {
            naive >= start && naive < end
        } else {
            // A Southern-hemisphere-style rule where DST spans the turn of
            // the year (starts late, ends early next year).
            naive >= start || naive < end
        };
        if in_dst {
            dst_offset(self.standard_offset, rule.saving)
        } else {
            self.standard_offset
        }
    }

    /// Converts a naive local wall-clock instant to UTC, choosing whichever
    /// offset applies to that local moment.
    pub fn to_utc(&self, naive: PrimitiveDateTime) -> OffsetDateTime {
        naive.assume_offset(self.offset_for_naive(naive))
    }

    /// Converts a UTC instant to the local wall-clock time it corresponds to.
    pub fn to_local(&self, instant: OffsetDateTime) -> PrimitiveDateTime {
        let approx = instant.to_offset(self.standard_offset);
        let approx_naive = PrimitiveDateTime::new(approx.date(), approx.time());
        let offset = self.offset_for_naive(approx_naive);
        let resolved = instant.to_offset(offset);
        PrimitiveDateTime::new(resolved.date(), resolved.time())
    }
}

fn dst_offset(standard: UtcOffset, saving: UtcOffset) -> UtcOffset {
    UtcOffset::from_whole_seconds(standard.whole_seconds() + saving.whole_seconds())
        .expect("a standard offset plus a DST saving stays within +/-24h")
}

/// The `n`th (1-based) occurrence of `weekday` in `year`/`month`. `n` must be
/// small enough to fit (1..=4 always does; every weekday occurs at least
/// four times in any month).
fn nth_weekday_of_month(year: i32, month: Month, weekday: Weekday, n: u8) -> Date {
    let first = Date::from_calendar_date(year, month, 1).expect("day 1 is always valid");
    let delta = (i64::from(weekday.number_days_from_monday())
        - i64::from(first.weekday().number_days_from_monday()))
    .rem_euclid(7);
    let day = 1 + delta + 7 * i64::from(n - 1);
    Date::from_calendar_date(year, month, day as u8)
        .expect("the nth occurrence of a weekday fits within a month for n <= 4")
}

/// The last occurrence of `weekday` in `year`/`month`.
fn last_weekday_of_month(year: i32, month: Month, weekday: Weekday) -> Date {
    let days_in_month = month.length(year);
    let last =
        Date::from_calendar_date(year, month, days_in_month).expect("the last day is always valid");
    let delta = (i64::from(last.weekday().number_days_from_monday())
        - i64::from(weekday.number_days_from_monday()))
    .rem_euclid(7);
    let day = i64::from(days_in_month) - delta;
    Date::from_calendar_date(year, month, day as u8).expect("computed day stays within the month")
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

fn local_date_of(instant: Timestamp, tz: &Timezone) -> Date {
    let offset_instant = OffsetDateTime::from_unix_timestamp(instant.unix_seconds())
        .expect("Timestamp was validated at construction");
    tz.to_local(offset_instant).date()
}

fn next_every_n_weeks_date(n: u8, weekday: Weekday, from: Timestamp, tz: &Timezone) -> Date {
    let from_date = local_date_of(from, tz);
    let anchor = epoch_anchor(weekday);
    let step = i64::from(n.max(1)) * 7;
    let diff_days = (from_date - anchor).whole_days();
    let k = diff_days.div_euclid(step) + 1;
    anchor + time::Duration::days(step * k)
}

fn next_day_of_month_date(day: u8, from: Timestamp, tz: &Timezone) -> Date {
    let from_date = local_date_of(from, tz);
    let mut year = from_date.year();
    let mut month = from_date.month();
    loop {
        let clamped = clamp_day_of_month(year, month, day);
        let candidate =
            Date::from_calendar_date(year, month, clamped).expect("clamped day fits the month");
        if candidate > from_date {
            return candidate;
        }
        if month == Month::December {
            year += 1;
        }
        month = month.next();
    }
}

/// When the next sweep is due, strictly after `from`, in the given
/// timezone. `Recurrence::Never` never sweeps.
///
/// Every recurrence resolves to local midnight (00:00:00) on its due date —
/// see the "Resolved ambiguities" section of the module doc comment.
pub fn next_run(recurrence: Recurrence, from: Timestamp, tz: &Timezone) -> Option<Timestamp> {
    let target_date = match recurrence {
        Recurrence::Never => return None,
        Recurrence::EveryNWeeks { n, weekday } => next_every_n_weeks_date(n, weekday, from, tz),
        Recurrence::DayOfMonth { day } => next_day_of_month_date(day, from, tz),
    };
    let naive = PrimitiveDateTime::new(target_date, Time::MIDNIGHT);
    let instant = tz.to_utc(naive);
    Some(
        Timestamp::from_unix_seconds(instant.unix_timestamp())
            .expect("a computed sweep instant stays within the representable range"),
    )
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

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    use crate::column::create_column;
    use crate::ids::ProjectId;
    use crate::task::{create_task, move_placement};

    fn ts(seconds: i64) -> Timestamp {
        Timestamp::from_unix_seconds(seconds).unwrap()
    }

    fn utc_naive(year: i32, month: Month, day: u8, hour: u8, minute: u8) -> PrimitiveDateTime {
        PrimitiveDateTime::new(
            Date::from_calendar_date(year, month, day).unwrap(),
            Time::from_hms(hour, minute, 0).unwrap(),
        )
    }

    fn utc(year: i32, month: Month, day: u8, hour: u8, minute: u8) -> Timestamp {
        ts(utc_naive(year, month, day, hour, minute)
            .assume_utc()
            .unix_timestamp())
    }

    fn utc_tz() -> Timezone {
        Timezone::fixed(UtcOffset::UTC)
    }

    // A real-world DST rule: US Eastern time. Standard offset UTC-5;
    // clocks spring forward 1h at 02:00 (standard) on the 2nd Sunday of
    // March, and fall back at 02:00 (daylight) on the 1st Sunday of
    // November — the actual current US rule.
    fn us_eastern() -> Timezone {
        Timezone::with_dst(
            UtcOffset::from_hms(-5, 0, 0).unwrap(),
            DstRule {
                saving: UtcOffset::from_hms(1, 0, 0).unwrap(),
                starts: DstTransition {
                    month: Month::March,
                    week: WeekOfMonth::Second,
                    weekday: Weekday::Sunday,
                    local_time: Time::from_hms(2, 0, 0).unwrap(),
                },
                ends: DstTransition {
                    month: Month::November,
                    week: WeekOfMonth::First,
                    weekday: Weekday::Sunday,
                    local_time: Time::from_hms(2, 0, 0).unwrap(),
                },
            },
        )
    }

    // --- Never ---

    #[test]
    fn never_yields_no_next_run() {
        let tz = utc_tz();
        assert_eq!(next_run(Recurrence::Never, ts(0), &tz), None);
        assert_eq!(
            next_run(Recurrence::Never, utc(2026, Month::June, 1, 0, 0), &tz),
            None
        );
    }

    // --- DayOfMonth: ordinary + month-end clamping ---

    #[test]
    fn day_of_month_returns_this_month_when_the_day_is_still_ahead() {
        let tz = utc_tz();
        let from = utc(2026, Month::June, 1, 12, 0);
        let next = next_run(Recurrence::DayOfMonth { day: 15 }, from, &tz).unwrap();
        assert_eq!(next, utc(2026, Month::June, 15, 0, 0));
    }

    #[test]
    fn day_of_month_rolls_to_next_month_once_the_day_has_passed() {
        let tz = utc_tz();
        let from = utc(2026, Month::June, 20, 0, 0);
        let next = next_run(Recurrence::DayOfMonth { day: 15 }, from, &tz).unwrap();
        assert_eq!(next, utc(2026, Month::July, 15, 0, 0));
    }

    #[test]
    fn day_of_month_31_clamps_to_the_last_day_of_a_30_day_month() {
        let tz = utc_tz();
        let from = utc(2026, Month::March, 31, 0, 0); // day already passed for March
        let next = next_run(Recurrence::DayOfMonth { day: 31 }, from, &tz).unwrap();
        // April has only 30 days.
        assert_eq!(next, utc(2026, Month::April, 30, 0, 0));
    }

    #[test]
    fn day_of_month_31_clamps_to_february_28_in_a_non_leap_year() {
        let tz = utc_tz();
        let from = utc(2025, Month::January, 31, 0, 0); // 2025 is not a leap year
        let next = next_run(Recurrence::DayOfMonth { day: 31 }, from, &tz).unwrap();
        assert_eq!(next, utc(2025, Month::February, 28, 0, 0));
    }

    #[test]
    fn day_of_month_31_clamps_to_february_29_in_a_leap_year() {
        let tz = utc_tz();
        let from = utc(2024, Month::January, 31, 0, 0); // 2024 is a leap year
        let next = next_run(Recurrence::DayOfMonth { day: 31 }, from, &tz).unwrap();
        assert_eq!(next, utc(2024, Month::February, 29, 0, 0));
    }

    #[test]
    fn day_of_month_rolls_over_the_year_boundary() {
        let tz = utc_tz();
        let from = utc(2026, Month::December, 20, 0, 0);
        let next = next_run(Recurrence::DayOfMonth { day: 15 }, from, &tz).unwrap();
        assert_eq!(next, utc(2027, Month::January, 15, 0, 0));
    }

    // --- EveryNWeeks: anchoring ---

    #[test]
    fn every_n_weeks_steps_by_exactly_n_weeks_across_consecutive_calls() {
        let tz = utc_tz();
        let recurrence = Recurrence::EveryNWeeks {
            n: 2,
            weekday: Weekday::Monday,
        };
        let first = next_run(recurrence, utc(2026, Month::January, 1, 0, 0), &tz).unwrap();
        let second = next_run(recurrence, first, &tz).unwrap();
        let third = next_run(recurrence, second, &tz).unwrap();

        assert_eq!(second.unix_seconds() - first.unix_seconds(), 14 * 86_400);
        assert_eq!(third.unix_seconds() - second.unix_seconds(), 14 * 86_400);
    }

    #[test]
    fn every_n_weeks_does_not_collapse_to_every_week() {
        // n = 2: an "off-cycle" Monday one week after the first on-cycle
        // Monday must NOT be a valid next run — the cadence must skip it.
        let tz = utc_tz();
        let recurrence = Recurrence::EveryNWeeks {
            n: 2,
            weekday: Weekday::Monday,
        };
        let first = next_run(recurrence, utc(2026, Month::January, 1, 0, 0), &tz).unwrap();
        // Asking again from a moment in the very next (off-cycle) week must
        // still skip forward to the on-cycle Monday two weeks after `first`,
        // not the Monday one week after it.
        let mid_week_after = ts(first.unix_seconds() + 3 * 86_400); // a few days into the off week
        let next = next_run(recurrence, mid_week_after, &tz).unwrap();
        assert_eq!(next.unix_seconds() - first.unix_seconds(), 14 * 86_400);
    }

    #[test]
    fn every_n_weeks_anchor_is_independent_of_from_within_a_cycle() {
        // Two different `from` values that both fall before the same
        // on-cycle Monday must resolve to that same instant — the anchor is
        // fixed, not derived from whichever `from` happened to be passed.
        let tz = utc_tz();
        let recurrence = Recurrence::EveryNWeeks {
            n: 2,
            weekday: Weekday::Monday,
        };
        let from_a = utc(2026, Month::January, 1, 0, 0);
        let from_b = utc(2026, Month::January, 4, 18, 30);
        let next_a = next_run(recurrence, from_a, &tz).unwrap();
        let next_b = next_run(recurrence, from_b, &tz).unwrap();
        assert_eq!(next_a, next_b);
    }

    // --- DST: spring forward and fall back, both honouring local midnight ---

    #[test]
    fn dst_spring_forward_is_honoured_across_the_transition() {
        let tz = us_eastern();
        let recurrence = Recurrence::EveryNWeeks {
            n: 1,
            weekday: Weekday::Sunday,
        };

        // 2026: the 2nd Sunday of March is 2026-03-08. The clock springs
        // forward at 02:00 EST -> 03:00 EDT that day, but local midnight
        // that same day is still standard time (EST, UTC-5).
        let saturday_before = utc(2026, Month::March, 7, 12, 0);
        let transition_sunday_run = next_run(recurrence, saturday_before, &tz).unwrap();
        assert_eq!(
            transition_sunday_run,
            utc(2026, Month::March, 8, 5, 0), // 00:00 EST == 05:00 UTC
            "midnight on the transition day itself is still standard time"
        );

        // The following Sunday (03-15) is fully inside DST: its midnight is
        // EDT (UTC-4). If the offset had silently stayed at EST, this would
        // be off by exactly one hour (06:00 UTC instead of 05:00 UTC).
        let next_sunday_run = next_run(recurrence, transition_sunday_run, &tz).unwrap();
        assert_eq!(
            next_sunday_run,
            utc(2026, Month::March, 15, 4, 0), // 00:00 EDT == 04:00 UTC
            "the week after the transition must use the DST offset, not drift"
        );
    }

    #[test]
    fn dst_fall_back_is_honoured_across_the_transition() {
        let tz = us_eastern();
        let recurrence = Recurrence::EveryNWeeks {
            n: 1,
            weekday: Weekday::Sunday,
        };

        // 2026: the 1st Sunday of November is 2026-11-01. The clock falls
        // back at 02:00 EDT -> 01:00 EST that day, but local midnight that
        // same day is still daylight time (EDT, UTC-4).
        let saturday_before = utc(2026, Month::October, 31, 12, 0);
        let transition_sunday_run = next_run(recurrence, saturday_before, &tz).unwrap();
        assert_eq!(
            transition_sunday_run,
            utc(2026, Month::November, 1, 4, 0), // 00:00 EDT == 04:00 UTC
            "midnight on the transition day itself is still daylight time"
        );

        // The following Sunday (11-08) is back on standard time (EST,
        // UTC-5). A fixed-offset bug would keep this at 04:00 UTC instead.
        let next_sunday_run = next_run(recurrence, transition_sunday_run, &tz).unwrap();
        assert_eq!(
            next_sunday_run,
            utc(2026, Month::November, 8, 5, 0), // 00:00 EST == 05:00 UTC
            "the week after the transition must use the standard offset, not drift"
        );
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
}
