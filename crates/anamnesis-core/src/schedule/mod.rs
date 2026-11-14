//! [`Schedule`]: when a recurring chore comes due, as pure local-calendar
//! arithmetic (no timezone, no clock; callers pass `Date`s).
//!
//! Two anchors, two meanings of "next":
//! - [`Anchor::Calendar`]: a fixed cadence from a start date.
//!   [`next_occurrence`] is the first on-cycle date **strictly after** `after`
//!   (and never before `start`). Multi-week rules (`n > 1`) phase from the
//!   Monday-based week containing `start`; multi-month rules from the month
//!   containing `start`.
//! - [`Anchor::AfterCompletion`]: due `interval` after the last completion
//!   (or on `set_on` if never completed). `after` is ignored on purpose: an
//!   after-done chore stays due until it is completed.

mod grammar;

pub use grammar::{ParseError, describe, parse};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::{Date, Duration, Month, Time, Weekday};

/// A set of weekdays, one bit each (bit 0 = Monday).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WeekdaySet(u8);

impl WeekdaySet {
    pub fn new() -> Self {
        Self(0)
    }

    pub fn with(self, weekday: Weekday) -> Self {
        Self(self.0 | Self::bit(weekday))
    }

    pub fn contains(self, weekday: Weekday) -> bool {
        self.0 & Self::bit(weekday) != 0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Members in Monday-first order.
    pub fn iter(self) -> impl Iterator<Item = Weekday> {
        (0..7u8)
            .map(|i| Weekday::Monday.nth_next(i))
            .filter(move |w| self.contains(*w))
    }

    fn bit(weekday: Weekday) -> u8 {
        1 << weekday.number_days_from_monday()
    }
}

/// Which occurrence of a weekday within a month.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Nth {
    First,
    Second,
    Third,
    Fourth,
    Last,
}

/// Which day of the month a monthly rule lands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MonthDay {
    /// A fixed day, clamped to the month's last day when it runs short.
    Day(u8),
    LastDay,
    /// "Last Friday", "2nd Saturday".
    Nth {
        nth: Nth,
        weekday: Weekday,
    },
}

/// The repeat interval and, for calendar anchors, the days it lands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rule {
    Days { n: u16 },
    Weeks { n: u8, on: WeekdaySet },
    Months { n: u8, on: MonthDay },
}

/// What the cadence is measured from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Anchor {
    Calendar {
        start: Date,
    },
    /// `set_on` is the save date, used as "due now" when never completed.
    AfterCompletion {
        set_on: Date,
    },
}

/// A time-of-day window that turns a chore into a deadline chore.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PopIn {
    pub at: Time,
    pub until: Option<Time>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule {
    pub rule: Rule,
    pub anchor: Anchor,
    pub pop_in: Option<PopIn>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ScheduleError {
    #[error("the interval must be at least 1")]
    ZeroInterval,
    #[error("a weekly schedule needs at least one weekday")]
    NoWeekdays,
    #[error("day of month {0} is out of range (1-31)")]
    DayOutOfRange(u8),
}

impl Schedule {
    pub fn validate(&self) -> Result<(), ScheduleError> {
        match self.rule {
            Rule::Days { n } => check_interval(u32::from(n)),
            Rule::Weeks { n, on } => {
                check_interval(u32::from(n))?;
                if on.is_empty() && matches!(self.anchor, Anchor::Calendar { .. }) {
                    return Err(ScheduleError::NoWeekdays);
                }
                Ok(())
            }
            Rule::Months { n, on } => {
                check_interval(u32::from(n))?;
                check_month_day(on)
            }
        }
    }

    /// Deadline chores (those with a pop-in time) are the only ones allowed
    /// to exceed a WIP limit.
    pub fn is_deadline(&self) -> bool {
        self.pop_in.is_some()
    }
}

fn check_interval(n: u32) -> Result<(), ScheduleError> {
    if n == 0 {
        Err(ScheduleError::ZeroInterval)
    } else {
        Ok(())
    }
}

fn check_month_day(on: MonthDay) -> Result<(), ScheduleError> {
    match on {
        MonthDay::Day(d) if d == 0 || d > 31 => Err(ScheduleError::DayOutOfRange(d)),
        _ => Ok(()),
    }
}

/// The first date on or after `today` that matches `rule`: the
/// `Calendar { start }` value to store for a new schedule. The interval phase
/// is anchored *at* the returned date, so `n` is ignored when searching.
pub fn first_start(rule: &Rule, today: Date) -> Date {
    let one_cycle = match *rule {
        Rule::Days { .. } => return today,
        Rule::Weeks { on, .. } => Rule::Weeks { n: 1, on },
        Rule::Months { on, .. } => Rule::Months { n: 1, on },
    };
    next_calendar(&one_cycle, today, today - Duration::days(1))
}

/// The next due date. For `Calendar` it is the first on-cycle date strictly
/// after `after` (never before `start`). For `AfterCompletion` it is
/// `last_completed + interval` (or `set_on` if never completed), regardless
/// of `after`.
pub fn next_occurrence(schedule: &Schedule, after: Date, last_completed: Option<Date>) -> Date {
    match schedule.anchor {
        Anchor::Calendar { start } => next_calendar(&schedule.rule, start, after),
        Anchor::AfterCompletion { set_on } => match last_completed {
            Some(done) => add_interval(&schedule.rule, done),
            None => set_on,
        },
    }
}

/// Up to `count` upcoming due dates, strictly increasing. An after-completion
/// schedule only ever has one: the next date depends on when it is done.
pub fn upcoming(
    schedule: &Schedule,
    after: Date,
    last_completed: Option<Date>,
    count: usize,
) -> Vec<Date> {
    if count == 0 {
        return Vec::new();
    }
    let mut dates = vec![next_occurrence(schedule, after, last_completed)];
    if matches!(schedule.anchor, Anchor::AfterCompletion { .. }) {
        return dates;
    }
    while dates.len() < count {
        let last = dates[dates.len() - 1];
        dates.push(next_occurrence(schedule, last, last_completed));
    }
    dates
}

fn add_interval(rule: &Rule, from: Date) -> Date {
    match *rule {
        Rule::Days { n } => from + Duration::days(i64::from(n)),
        Rule::Weeks { n, .. } => from + Duration::weeks(i64::from(n)),
        Rule::Months { n, .. } => add_months(from, i32::from(n)),
    }
}

fn next_calendar(rule: &Rule, start: Date, after: Date) -> Date {
    let from = (after + Duration::days(1)).max(start);
    match *rule {
        Rule::Days { n } => next_days(n, start, from),
        Rule::Weeks { n, on } => next_weeks(n, on, start, from),
        Rule::Months { n, on } => next_months(n, on, start, from),
    }
}

/// First `start + k * n` days on or after `from` (`from >= start`).
fn next_days(n: u16, start: Date, from: Date) -> Date {
    let step = i64::from(n.max(1));
    let diff = (from - start).whole_days();
    start + Duration::days((diff + step - 1) / step * step)
}

fn monday_of(date: Date) -> Date {
    date - Duration::days(i64::from(date.weekday().number_days_from_monday()))
}

/// First date on or after `from` whose weekday is in `on` and whose
/// Monday-based week is an on-cycle week counted from `start`'s week. A
/// schedule with no weekdays is invalid; it degrades to `from` here.
fn next_weeks(n: u8, on: WeekdaySet, start: Date, from: Date) -> Date {
    let n = i64::from(n.max(1));
    let origin = monday_of(start);
    (0..=7 * n + 7)
        .map(|i| from + Duration::days(i))
        .find(|date| {
            let week = (*date - origin).whole_days().div_euclid(7);
            week % n == 0 && on.contains(date.weekday())
        })
        .unwrap_or(from)
}

/// First date on or after `from` that falls on `on` in an on-cycle month
/// counted from `start`'s month.
fn next_months(n: u8, on: MonthDay, start: Date, from: Date) -> Date {
    let n = i32::from(n.max(1));
    let start_index = month_index(start);
    let from_index = month_index(from);
    (0..=2 * n)
        .map(|step| from_index + step)
        .filter(|index| (index - start_index).rem_euclid(n) == 0)
        .map(|index| month_date(index, on))
        .find(|date| *date >= from)
        .expect("an on-cycle month matches within two cycles")
}

/// Months since year 0, so month arithmetic is plain integer arithmetic.
fn month_index(date: Date) -> i32 {
    date.year() * 12 + i32::from(u8::from(date.month())) - 1
}

fn year_month(index: i32) -> (i32, Month) {
    let month = Month::try_from(index.rem_euclid(12) as u8 + 1).expect("1..=12");
    (index.div_euclid(12), month)
}

fn make_date(year: i32, month: Month, day: u8) -> Date {
    let day = day.clamp(1, month.length(year));
    Date::from_calendar_date(year, month, day).expect("clamped day fits the month")
}

/// `date` plus `n` months, the day clamped to the target month's length.
fn add_months(date: Date, n: i32) -> Date {
    let (year, month) = year_month(month_index(date) + n);
    make_date(year, month, date.day())
}

fn month_date(index: i32, on: MonthDay) -> Date {
    let (year, month) = year_month(index);
    let day = match on {
        MonthDay::Day(day) => day,
        MonthDay::LastDay => month.length(year),
        MonthDay::Nth { nth, weekday } => nth_weekday_day(year, month, nth, weekday),
    };
    make_date(year, month, day)
}

/// Days from `from` forward to the next `to` (0 if the same).
fn weekday_gap(from: Weekday, to: Weekday) -> u8 {
    let gap = i16::from(to.number_days_from_monday()) - i16::from(from.number_days_from_monday());
    gap.rem_euclid(7) as u8
}

/// Day of month of the `nth` `weekday` in the month.
fn nth_weekday_day(year: i32, month: Month, nth: Nth, weekday: Weekday) -> u8 {
    let length = month.length(year);
    let k = match nth {
        Nth::Last => {
            let last_weekday = make_date(year, month, length).weekday();
            return length - weekday_gap(weekday, last_weekday);
        }
        Nth::First => 0,
        Nth::Second => 1,
        Nth::Third => 2,
        Nth::Fourth => 3,
    };
    1 + weekday_gap(make_date(year, month, 1).weekday(), weekday) + 7 * k
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use time::Weekday::{
        Friday as Fri, Monday as Mon, Saturday as Sat, Thursday as Thu, Tuesday as Tue,
    };

    fn d(y: i32, m: u8, day: u8) -> Date {
        Date::from_calendar_date(y, Month::try_from(m).unwrap(), day).unwrap()
    }

    fn cal(rule: Rule, start: Date) -> Schedule {
        Schedule {
            rule,
            anchor: Anchor::Calendar { start },
            pop_in: None,
        }
    }

    fn weekly(n: u8, on: &[Weekday], start: Date) -> Schedule {
        let on = on.iter().fold(WeekdaySet::new(), |set, w| set.with(*w));
        cal(Rule::Weeks { n, on }, start)
    }

    fn days(n: u16, start: Date) -> Schedule {
        cal(Rule::Days { n }, start)
    }

    fn monthly(n: u8, on: MonthDay, start: Date) -> Schedule {
        cal(Rule::Months { n, on }, start)
    }

    fn nth(nth: Nth, weekday: Weekday) -> MonthDay {
        MonthDay::Nth { nth, weekday }
    }

    fn months(n: u8) -> Rule {
        Rule::Months {
            n,
            on: MonthDay::LastDay,
        }
    }

    fn weeks(n: u8) -> Rule {
        Rule::Weeks {
            n,
            on: WeekdaySet::new(),
        }
    }

    fn after(rule: Rule) -> Schedule {
        Schedule {
            rule,
            anchor: Anchor::AfterCompletion {
                set_on: d(2026, 11, 13),
            },
            pop_in: None,
        }
    }

    #[rstest]
    // Calendar weekly, n=1, Tue: strictly after a Tuesday is the next Tuesday.
    #[case(weekly(1, &[Tue], d(2026,11,17)), d(2026,11,17), None, d(2026,11,24))]
    // n=2 phases from the week containing `start`, Monday-based weeks.
    #[case(weekly(2, &[Sat], d(2026,11,21)), d(2026,11,22), None, d(2026,12,5))]
    // Multi-day set.
    #[case(weekly(1, &[Mon, Thu], d(2026,11,16)), d(2026,11,16), None, d(2026,11,19))]
    // Every 10 days from start.
    #[case(days(10, d(2026, 11, 1)), d(2026, 11, 5), None, d(2026, 11, 11))]
    // Day 31 clamps in short months, leap-aware.
    #[case(
        monthly(1, MonthDay::Day(31), d(2026, 1, 31)),
        d(2026, 1, 31),
        None,
        d(2026, 2, 28)
    )]
    #[case(
        monthly(1, MonthDay::Day(31), d(2028, 1, 31)),
        d(2028, 1, 31),
        None,
        d(2028, 2, 29)
    )]
    #[case(
        monthly(1, MonthDay::LastDay, d(2026, 4, 30)),
        d(2026, 4, 30),
        None,
        d(2026, 5, 31)
    )]
    // Last Friday / 2nd Saturday.
    #[case(
        monthly(1, nth(Nth::Last, Fri), d(2026, 11, 27)),
        d(2026, 11, 27),
        None,
        d(2026, 12, 25)
    )]
    #[case(
        monthly(1, nth(Nth::Second, Sat), d(2026, 11, 14)),
        d(2026, 11, 14),
        None,
        d(2026, 12, 12)
    )]
    // Every 3 months on the 15th from a Nov start skips Dec, Jan.
    #[case(
        monthly(3, MonthDay::Day(15), d(2026, 11, 15)),
        d(2026, 11, 15),
        None,
        d(2027, 2, 15)
    )]
    // After completion: 3 months after done, month-end clamped.
    #[case(
        after(months(3)),
        d(2026, 11, 30),
        Some(d(2026, 11, 30)),
        d(2027, 2, 28)
    )]
    #[case(
        after(weeks(1)),
        d(2026, 11, 13),
        Some(d(2026, 11, 10)),
        d(2026, 11, 17)
    )]
    fn next_occurrence_cases(
        #[case] s: Schedule,
        #[case] after: Date,
        #[case] done: Option<Date>,
        #[case] want: Date,
    ) {
        assert_eq!(next_occurrence(&s, after, done), want);
    }

    #[test]
    fn after_completion_never_completed_is_due_on_set_on() {
        let s = after(Rule::Days { n: 3 });
        assert_eq!(next_occurrence(&s, d(2026, 11, 12), None), d(2026, 11, 13));
    }

    #[test]
    fn after_completion_stays_due_regardless_of_after() {
        let s = after(Rule::Days { n: 3 });
        let done = Some(d(2026, 11, 1));
        assert_eq!(next_occurrence(&s, d(2026, 12, 25), done), d(2026, 11, 4));
    }

    #[test]
    fn calendar_before_start_returns_start() {
        let s = days(10, d(2026, 11, 20));
        assert_eq!(next_occurrence(&s, d(2026, 11, 1), None), d(2026, 11, 20));
    }

    #[test]
    fn feeding_result_back_steps_exactly_one_period() {
        let s = weekly(2, &[Sat], d(2026, 11, 21));
        let mut cur = d(2026, 11, 21);
        for _ in 0..5 {
            let next = next_occurrence(&s, cur, None);
            assert_eq!((next - cur).whole_days(), 14);
            cur = next;
        }
    }

    #[test]
    fn upcoming_returns_count_strictly_increasing() {
        let s = weekly(1, &[Mon, Thu], d(2026, 11, 16));
        let got = upcoming(&s, d(2026, 11, 16), None, 3);
        assert_eq!(got.len(), 3);
        assert!(got.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn upcoming_after_completion_is_a_single_date() {
        let s = after(Rule::Days { n: 3 });
        assert_eq!(
            upcoming(&s, d(2026, 11, 13), None, 3),
            vec![d(2026, 11, 13)]
        );
    }

    #[rstest]
    #[case(days(0, d(2026, 11, 13)), ScheduleError::ZeroInterval)]
    #[case(weekly(1, &[], d(2026,11,13)), ScheduleError::NoWeekdays)]
    #[case(
        monthly(1, MonthDay::Day(32), d(2026, 11, 13)),
        ScheduleError::DayOutOfRange(32)
    )]
    #[case(
        monthly(1, MonthDay::Day(0), d(2026, 11, 13)),
        ScheduleError::DayOutOfRange(0)
    )]
    #[case(
        monthly(0, MonthDay::LastDay, d(2026, 11, 13)),
        ScheduleError::ZeroInterval
    )]
    fn validate_rejects(#[case] s: Schedule, #[case] want: ScheduleError) {
        assert_eq!(s.validate(), Err(want));
    }

    #[test]
    fn validate_accepts_a_good_schedule_and_is_deadline_follows_pop_in() {
        let mut s = weekly(1, &[Tue], d(2026, 11, 17));
        assert_eq!(s.validate(), Ok(()));
        assert!(!s.is_deadline());
        s.pop_in = Some(PopIn {
            at: time::Time::from_hms(9, 0, 0).unwrap(),
            until: None,
        });
        assert!(s.is_deadline());
    }

    #[test]
    fn first_start_finds_next_matching_date() {
        let rule = Rule::Weeks {
            n: 1,
            on: WeekdaySet::new().with(Tue),
        };
        assert_eq!(first_start(&rule, d(2026, 11, 13)), d(2026, 11, 17));
        // On the day itself counts.
        assert_eq!(first_start(&rule, d(2026, 11, 17)), d(2026, 11, 17));
    }

    #[test]
    fn first_start_ignores_the_interval_phase() {
        let rule = Rule::Weeks {
            n: 2,
            on: WeekdaySet::new().with(Mon),
        };
        // Friday: the next Monday, not two weeks of phase away.
        assert_eq!(first_start(&rule, d(2026, 11, 13)), d(2026, 11, 16));
    }

    #[test]
    fn weekday_set_basics() {
        let set = WeekdaySet::new().with(Mon).with(Sat);
        assert!(set.contains(Mon) && set.contains(Sat) && !set.contains(Tue));
        assert!(!set.is_empty() && WeekdaySet::new().is_empty());
        assert_eq!(set.iter().collect::<Vec<_>>(), vec![Mon, Sat]);
    }
}
