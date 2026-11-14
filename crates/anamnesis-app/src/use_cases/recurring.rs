//! Recurring tasks (household chores) in the app layer: setting a schedule,
//! stamping completion, and the tick that fires and expires due chores
//! (`docs/superpowers/specs/2026-11-12-recurring-tasks-design.md`,
//! "Lifecycle").
//!
//! `anamnesis_core::recurring` decides what a task's fields become; this
//! module supplies what it cannot know: the local calendar (through
//! [`TimezoneResolver`]), the board's columns, and the project's status.
//!
//! **"Up" is decided by placement, so "done" is decided by the placement's
//! column.** The archive sweep leaves an archived task's placement at
//! `OnBoard { column: Done }`, and the board query leaves archived tasks out
//! of its item lists, so a task's done-ness is always read from its own
//! placement column's `is_done` flag ([`DoneColumns`]), never from which
//! item list it appears in.

use std::collections::HashSet;

use anamnesis_core::policy::Role;
use anamnesis_core::{
    self as core, Anchor, ColumnId, Firing, Placement, ProjectId, ProjectStatus, Schedule, Task,
    TaskId, TaskRecurrence, Timestamp, next_occurrence,
};
use time::{Date, Duration, PrimitiveDateTime, Time};

use crate::error::AppError;
use crate::policy::{Action, is_allowed};
use crate::ports::{
    BoardColumn, BoardQuery, Clock, ProjectRepository, SearchIndex, TaskRepository,
    TaskUpdateError, TimezoneResolver,
};

use super::indexing::log_index_failure;

/// The ports one recurrence tick needs together: tasks to fire, their
/// projects' status, the board's columns, the search index (firing can
/// un-archive a task the sweep took out of plain search), and the local
/// calendar.
pub struct RecurrenceDeps<'a> {
    pub tasks: &'a dyn TaskRepository,
    pub projects: &'a dyn ProjectRepository,
    pub board: &'a dyn BoardQuery,
    pub search: &'a dyn SearchIndex,
    pub tz: &'a dyn TimezoneResolver,
    pub zone: &'a str,
}

/// What one [`run_recurrence_tick`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecurrenceTick {
    pub fired: u32,
    pub expired: u32,
    /// Due, but left pending because the task's project is not Active.
    pub skipped_inactive: u32,
    /// Due, but an error stopped this task (logged with its id); left as
    /// stored for the next tick. Never stops the other tasks.
    pub failed: u32,
}

/// The firing timing for the first occurrence of `schedule` strictly after
/// local date `after` (or, for an after-completion schedule, the one counted
/// from `last_completed`): the pop-in instant (local midnight for an
/// interval chore) and, if the schedule has an `until`, the window's end —
/// on the next day when `until` is not after the pop-in time.
///
/// A pop-in time that falls in a spring-forward gap is shifted forward by an
/// hour, to the first wall-clock time after the gap (2:30am on the
/// changeover day fires at 3:30am). An unknown zone still errors.
pub fn next_due_instant(
    schedule: &Schedule,
    after: Date,
    last_completed: Option<Date>,
    tz: &dyn TimezoneResolver,
    zone: &str,
) -> Result<Firing, AppError> {
    let date = next_occurrence(schedule, after, last_completed);
    Ok(Firing {
        next_due_at: pop_in_instant(schedule, date, tz, zone)?,
        expires_at: window_end(schedule, date, tz, zone)?,
    })
}

/// When the occurrence on `date` pops in.
fn pop_in_instant(
    schedule: &Schedule,
    date: Date,
    tz: &dyn TimezoneResolver,
    zone: &str,
) -> Result<Timestamp, AppError> {
    let at = schedule.pop_in.map_or(Time::MIDNIGHT, |p| p.at);
    local_instant(tz, zone, date, at)
}

/// When the occurrence on `date`'s window closes, if it has an `until`.
fn window_end(
    schedule: &Schedule,
    date: Date,
    tz: &dyn TimezoneResolver,
    zone: &str,
) -> Result<Option<Timestamp>, AppError> {
    let Some((at, until)) = schedule.pop_in.and_then(|p| p.until.map(|u| (p.at, u))) else {
        return Ok(None);
    };
    let day = if until <= at {
        date.saturating_add(Duration::DAY)
    } else {
        date
    };
    local_instant(tz, zone, day, until).map(Some)
}

/// Local wall-clock `date` at `time` as a UTC instant. A time that does not
/// exist (a spring-forward gap) is retried an hour later; if that fails too
/// (an unknown zone) the original error is returned.
fn local_instant(
    tz: &dyn TimezoneResolver,
    zone: &str,
    date: Date,
    time: Time,
) -> Result<Timestamp, AppError> {
    tz.to_utc(zone, date, time).or_else(|err| {
        let later = PrimitiveDateTime::new(date, time).saturating_add(Duration::HOUR);
        tz.to_utc(zone, later.date(), later.time())
            .map_err(|_| AppError::from(err))
    })
}

/// The first firing of `schedule` whose pop-in is strictly after `now`.
///
/// For a calendar schedule that is today's occurrence if its pop-in time is
/// still ahead, otherwise the first one after today — so setting "every tue
/// at 6pm" on a Tuesday morning fires tonight, and an interval chore (which
/// pops in at midnight) is next due on a later day. An after-completion
/// schedule's date depends only on `last_completed`, so it may be in the
/// past (due at once).
fn upcoming_firing(
    schedule: &Schedule,
    now: Timestamp,
    last_completed: Option<Date>,
    tz: &dyn TimezoneResolver,
    zone: &str,
) -> Result<Firing, AppError> {
    let today = tz.local_date(zone, now)?;
    let from_today = next_due_instant(
        schedule,
        today.saturating_sub(Duration::DAY),
        last_completed,
        tz,
        zone,
    )?;
    if from_today.next_due_at > now || matches!(schedule.anchor, Anchor::AfterCompletion { .. }) {
        return Ok(from_today);
    }
    next_due_instant(schedule, today, last_completed, tz, zone)
}

/// `instant`'s local date, if any.
fn local_date_of(
    instant: Option<Timestamp>,
    tz: &dyn TimezoneResolver,
    zone: &str,
) -> Result<Option<Date>, AppError> {
    Ok(instant.map(|t| tz.local_date(zone, t)).transpose()?)
}

/// Sets (`Some`) or clears (`None`) a task's repeat schedule. Setting one
/// computes `next_due_at` as the first occurrence still ahead of now; an
/// open window or due-ness that still applies is kept
/// ([`core::set_schedule`]), and placement is untouched.
///
/// Saving the schedule the task already has writes nothing and returns the
/// task as stored: the editor posts the whole form on every save, and an
/// unchanged schedule must not move `next_due_at` or reset live state.
///
/// A real change is a user edit, so it stamps `last_touched_at` like
/// `edit_task` does. That stamp is the optimistic-concurrency token: a tick
/// that read the task before this edit then has its write rejected as a
/// conflict (and skips the task), instead of writing back the old schedule.
pub async fn set_task_schedule(
    repo: &dyn TaskRepository,
    clock: &dyn Clock,
    tz: &dyn TimezoneResolver,
    zone: &str,
    role: Option<Role>,
    task_id: TaskId,
    schedule: Option<Schedule>,
) -> Result<Task, AppError> {
    if !is_allowed(role, Action::EditTask) {
        return Err(AppError::Forbidden);
    }
    let aggregate = repo.load(task_id).await?.ok_or(AppError::NotFound)?;
    let task = &aggregate.task;
    if task.recurrence.as_ref().map(|r| &r.schedule) == schedule.as_ref() {
        return Ok(aggregate.task);
    }
    let now = clock.now();
    let rescheduled = match schedule {
        None => core::clear_schedule(task),
        Some(schedule) => {
            schedule
                .validate()
                .map_err(|e| AppError::Invalid(e.to_string()))?;
            let last = task.recurrence.as_ref().and_then(|r| r.last_completed_at);
            let last = local_date_of(last, tz, zone)?;
            let firing = upcoming_firing(&schedule, now, last, tz, zone)?;
            core::set_schedule(task, schedule, firing.next_due_at)
        }
    };
    let updated = Task {
        last_touched_at: now,
        ..rescheduled
    };
    repo.update(&updated, task.last_touched_at).await?;
    Ok(updated)
}

/// Stamps a completion on a recurring task that has just been moved into a
/// done column. An after-completion schedule's next due date is counted from
/// today; a calendar schedule keeps its own (doing the vacuum early does not
/// move Saturday). A task without recurrence is returned unchanged.
pub(crate) fn record_recurring_completion(
    task: &Task,
    now: Timestamp,
    tz: &dyn TimezoneResolver,
    zone: &str,
) -> Result<Task, AppError> {
    let Some(recurrence) = task.recurrence.as_ref() else {
        return Ok(task.clone());
    };
    let next = match recurrence.schedule.anchor {
        Anchor::Calendar { .. } => None,
        Anchor::AfterCompletion { .. } => {
            let today = tz.local_date(zone, now)?;
            let firing = next_due_instant(&recurrence.schedule, today, Some(today), tz, zone)?;
            Some(firing.next_due_at)
        }
    };
    Ok(core::record_completion(task, now, next))
}

/// [`record_recurring_completion`] for a move from `before` to `after`, if
/// it took a recurring task from outside any done column into one. Reads
/// the board's columns only for a recurring task.
pub(crate) async fn complete_on_entering_done(
    board: &dyn BoardQuery,
    tz: &dyn TimezoneResolver,
    zone: &str,
    before: &Task,
    after: Task,
    now: Timestamp,
) -> Result<Task, AppError> {
    if after.recurrence.is_none() {
        return Ok(after);
    }
    let done = DoneColumns::from_board(&board.columns_with_items().await?);
    if done.holds(after.placement) && !done.holds(before.placement) {
        return record_recurring_completion(&after, now, tz, zone);
    }
    Ok(after)
}

/// Resumes a paused recurring task (one explicitly archived, so its
/// `next_due_at` was cleared) from now, keeping its schedule. A task that
/// is not recurring, or whose recurrence was never paused (the sweep
/// archived it), is returned unchanged.
pub(crate) fn resume_recurrence(
    task: &Task,
    now: Timestamp,
    tz: &dyn TimezoneResolver,
    zone: &str,
) -> Result<Task, AppError> {
    let Some(recurrence) = task.recurrence.as_ref() else {
        return Ok(task.clone());
    };
    if recurrence.next_due_at.is_some() {
        return Ok(task.clone());
    }
    let last = local_date_of(recurrence.last_completed_at, tz, zone)?;
    let firing = upcoming_firing(&recurrence.schedule, now, last, tz, zone)?;
    Ok(core::set_schedule(
        task,
        recurrence.schedule,
        firing.next_due_at,
    ))
}

/// Which board columns are `is_done`, plus the entry column (the first one)
/// a deadline chore lands in.
pub(crate) struct DoneColumns {
    done: HashSet<ColumnId>,
    entry: Option<ColumnId>,
}

impl DoneColumns {
    pub(crate) fn from_board(columns: &[BoardColumn]) -> Self {
        Self {
            done: columns
                .iter()
                .filter(|bc| bc.column.is_done)
                .map(|bc| bc.column.id)
                .collect(),
            entry: columns.first().map(|bc| bc.column.id),
        }
    }

    /// Whether `placement` is in a done column.
    pub(crate) fn holds(&self, placement: Placement) -> bool {
        matches!(placement, Placement::OnBoard { column, .. } if self.done.contains(&column))
    }

    /// Whether a move from `from` (`None`: below the horizon) to `to`
    /// entered a done column from outside one — what counts as completing.
    pub(crate) fn entered(&self, from: Option<ColumnId>, to: ColumnId) -> bool {
        self.done.contains(&to) && !from.is_some_and(|c| self.done.contains(&c))
    }
}

/// One pass over every recurring task that has something due at `now`:
/// expire closed windows, fire due occurrences, reset fired chores'
/// checklists. A task whose project is not Active is skipped with its
/// `next_due_at` left pending, so it fires once on the first tick after
/// reactivation. A task someone else wrote to mid-tick is left for the next
/// tick.
///
/// **One task's error never stops the others.** A task that fails (a
/// storage error, a pop-in its zone cannot place) is logged with its id,
/// counted in [`RecurrenceTick::failed`], and left as stored, so it is
/// retried next tick while every other chore still fires. Only the reads
/// the whole pass depends on (the board's columns, the actionable list)
/// fail the tick.
pub async fn run_recurrence_tick(
    deps: &RecurrenceDeps<'_>,
    now: Timestamp,
) -> Result<RecurrenceTick, AppError> {
    let columns = DoneColumns::from_board(&deps.board.columns_with_items().await?);
    let mut tick = RecurrenceTick::default();
    for task in deps.tasks.list_recurrence_actionable(now).await? {
        match tick_task(deps, &columns, &task, now).await {
            Ok(step) => tick.record(step),
            Err(err) => {
                log_tick_failure(task.id, &err);
                tick.failed += 1;
            }
        }
    }
    Ok(tick)
}

/// What the tick did with one actionable task.
enum TaskStep {
    SkippedInactive,
    /// Someone else wrote to it first; left for the next tick.
    Conflicted,
    Advanced(Advanced),
}

impl RecurrenceTick {
    fn record(&mut self, step: TaskStep) {
        match step {
            TaskStep::SkippedInactive => self.skipped_inactive += 1,
            TaskStep::Conflicted => {}
            TaskStep::Advanced(advanced) => {
                self.expired += u32::from(advanced.expired);
                self.fired += u32::from(advanced.fired);
            }
        }
    }
}

async fn tick_task(
    deps: &RecurrenceDeps<'_>,
    columns: &DoneColumns,
    task: &Task,
    now: Timestamp,
) -> Result<TaskStep, AppError> {
    if !project_is_active(deps.projects, task.project_id).await? {
        return Ok(TaskStep::SkippedInactive);
    }
    let advanced = advance_task(deps, columns, task, now).await?;
    Ok(advanced.map_or(TaskStep::Conflicted, TaskStep::Advanced))
}

/// `anamnesis-app` has no logging crate (see
/// `super::indexing::log_index_failure`), so this goes to stderr.
fn log_tick_failure(task_id: TaskId, err: &AppError) {
    eprintln!("anamnesis: recurrence tick could not update task {task_id}: {err}");
}

async fn project_is_active(
    projects: &dyn ProjectRepository,
    project_id: ProjectId,
) -> Result<bool, AppError> {
    Ok(projects.load(project_id).await?.is_some_and(|agg| {
        agg.project.status == ProjectStatus::Active && agg.project.archived_at.is_none()
    }))
}

/// What [`advance_task`] did to one task.
struct Advanced {
    expired: bool,
    fired: bool,
}

/// Expires, then fires, one actionable task, and writes it back once.
/// `None` if a concurrent write got there first.
async fn advance_task(
    deps: &RecurrenceDeps<'_>,
    columns: &DoneColumns,
    task: &Task,
    now: Timestamp,
) -> Result<Option<Advanced>, AppError> {
    let expired = recurrence_event_due(task, |r| r.expires_at, now);
    let mut next = if expired {
        core::expire(task, columns.holds(task.placement), now)
    } else {
        task.clone()
    };
    let fired = recurrence_event_due(&next, |r| r.next_due_at, now);
    if fired {
        next = fire_task(deps, columns, &next, now).await?;
    }
    if !write_back(deps.tasks, &next, task.last_touched_at).await? {
        return Ok(None);
    }
    if fired {
        reindex_if_restored(deps.search, task, &next).await;
        reset_checklist(deps, columns, next.id).await;
    }
    Ok(Some(Advanced { expired, fired }))
}

/// Whether the recurrence timestamp `pick` selects is at or before `now`.
fn recurrence_event_due(
    task: &Task,
    pick: impl Fn(&TaskRecurrence) -> Option<Timestamp>,
    now: Timestamp,
) -> bool {
    task.recurrence
        .as_ref()
        .and_then(pick)
        .is_some_and(|at| at <= now)
}

/// Fires `task`'s due occurrence. A deadline chore lands at the end of the
/// entry column **without** `check_wip_limit`: "Deadline chores always reach
/// the board, even over the WIP limit" (spec, "Decided by the user"; only
/// they may — an interval chore goes below the horizon instead).
async fn fire_task(
    deps: &RecurrenceDeps<'_>,
    columns: &DoneColumns,
    task: &Task,
    now: Timestamp,
) -> Result<Task, AppError> {
    let recurrence = task
        .recurrence
        .as_ref()
        .expect("only a recurring task is ever due to fire");
    let firing = tick_firing(recurrence, now, deps.tz, deps.zone)?;
    let entry_end = match columns.entry {
        Some(column) => Placement::OnBoard {
            column,
            position: deps.board.count_on_column(column).await?,
        },
        None => Placement::Below,
    };
    Ok(core::fire(
        task,
        columns.holds(task.placement),
        entry_end,
        firing,
        now,
    ))
}

/// The timing a tick fires `recurrence` with: the window end of the latest
/// occurrence that has popped in (so a stale one is skipped rather than
/// shown), and the next occurrence still ahead of now. An after-completion
/// chore that comes due without being done counts its next occurrence from
/// today, so it fires once per interval until done rather than on every
/// tick.
fn tick_firing(
    recurrence: &TaskRecurrence,
    now: Timestamp,
    tz: &dyn TimezoneResolver,
    zone: &str,
) -> Result<Firing, AppError> {
    let schedule = &recurrence.schedule;
    let today = tz.local_date(zone, now)?;
    let next = upcoming_firing(schedule, now, Some(today), tz, zone)?;
    let due_on = local_date_of(recurrence.next_due_at, tz, zone)?.unwrap_or(today);
    let occurrence = latest_popped_in(schedule, due_on, today, now, tz, zone)?;
    Ok(Firing {
        next_due_at: next.next_due_at,
        expires_at: window_end(schedule, occurrence, tz, zone)?,
    })
}

/// The latest occurrence on or after `due_on` whose pop-in is at or before
/// `now`. A window never outlasts the day after its occurrence, so only
/// today's or yesterday's can still be open; anything older collapses to
/// `due_on`, whose window is closed either way. An after-completion
/// schedule has exactly one occurrence until done: `due_on`.
fn latest_popped_in(
    schedule: &Schedule,
    due_on: Date,
    today: Date,
    now: Timestamp,
    tz: &dyn TimezoneResolver,
    zone: &str,
) -> Result<Date, AppError> {
    if matches!(schedule.anchor, Anchor::AfterCompletion { .. }) {
        return Ok(due_on);
    }
    for day in [today, today.saturating_sub(Duration::DAY)] {
        let occurs = next_occurrence(schedule, day.saturating_sub(Duration::DAY), None) == day;
        if day >= due_on && occurs && pop_in_instant(schedule, day, tz, zone)? <= now {
            return Ok(day);
        }
    }
    Ok(due_on)
}

/// Writes `task` back, reporting `false` (written nothing) if someone else
/// wrote to it since it was read.
async fn write_back(
    tasks: &dyn TaskRepository,
    task: &Task,
    expected: Timestamp,
) -> Result<bool, AppError> {
    match tasks.update(task, expected).await {
        Ok(()) => Ok(true),
        Err(TaskUpdateError::Conflict) => Ok(false),
        Err(TaskUpdateError::Repo(err)) => Err(err.into()),
    }
}

/// Puts a task the system just un-archived back into plain search (see
/// `crate::use_cases::task::archive_task` for why archiving took it out).
async fn reindex_if_restored(search: &dyn SearchIndex, before: &Task, after: &Task) {
    if before.archived_at.is_some()
        && after.archived_at.is_none()
        && let Err(err) = search.index_task(after.id, after.title.as_str()).await
    {
        log_index_failure("run_recurrence_tick", err);
    }
}

/// Resets a fired chore's finished checklist: every item that is archived
/// or in a done column goes back below the horizon, un-archived. Unfinished
/// items stay where they are.
///
/// The chore itself has already been written by now, so an error here is
/// logged per item and the rest carry on; it cannot un-fire the chore.
async fn reset_checklist(deps: &RecurrenceDeps<'_>, columns: &DoneColumns, parent: TaskId) {
    let items = match deps.tasks.list_checklist_including_archived(parent).await {
        Ok(items) => items,
        Err(err) => return log_tick_failure(parent, &err.into()),
    };
    for item in items {
        let Some(reset) = core::reset_checklist_item(&item, columns.holds(item.placement)) else {
            continue;
        };
        match write_back(deps.tasks, &reset, item.last_touched_at).await {
            Ok(true) => reindex_if_restored(deps.search, &item, &reset).await,
            Ok(false) => {}
            Err(err) => log_tick_failure(item.id, &err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::RepoError;
    use anamnesis_core::{PopIn, Rule, WeekdaySet};
    use time::macros::{date, time};
    use time::{OffsetDateTime, UtcOffset, Weekday};

    /// A zone named "Test/Eastern" at UTC-5, which springs forward to UTC-4
    /// at 2:00am on 2027-03-14 (2:00-2:59am that day does not exist). Any
    /// other zone name is unknown.
    struct FakeZone;

    const ZONE: &str = "Test/Eastern";
    const SPRING_FORWARD: Date = date!(2027 - 03 - 14);

    impl FakeZone {
        fn offset(date: Date, time: Time) -> Result<UtcOffset, RepoError> {
            let hours = if date < SPRING_FORWARD || (date == SPRING_FORWARD && time.hour() < 2) {
                -5
            } else if date == SPRING_FORWARD && time.hour() == 2 {
                return Err(RepoError::new("falls in a DST gap"));
            } else {
                -4
            };
            Ok(UtcOffset::from_hms(hours, 0, 0).unwrap())
        }

        fn check(zone: &str) -> Result<(), RepoError> {
            if zone == ZONE {
                Ok(())
            } else {
                Err(RepoError::new(format!("unknown zone {zone}")))
            }
        }
    }

    impl TimezoneResolver for FakeZone {
        fn local_date(&self, zone: &str, instant: Timestamp) -> Result<Date, RepoError> {
            Self::check(zone)?;
            let utc = OffsetDateTime::from_unix_timestamp(instant.unix_seconds()).unwrap();
            Ok(utc.to_offset(UtcOffset::from_hms(-5, 0, 0).unwrap()).date())
        }

        fn local_time(&self, zone: &str, instant: Timestamp) -> Result<Time, RepoError> {
            Self::check(zone)?;
            let utc = OffsetDateTime::from_unix_timestamp(instant.unix_seconds()).unwrap();
            Ok(utc.to_offset(UtcOffset::from_hms(-5, 0, 0).unwrap()).time())
        }

        fn to_utc(&self, zone: &str, date: Date, time: Time) -> Result<Timestamp, RepoError> {
            Self::check(zone)?;
            let local = PrimitiveDateTime::new(date, time).assume_offset(Self::offset(date, time)?);
            Ok(Timestamp::from_unix_seconds(local.unix_timestamp()).unwrap())
        }
    }

    /// `date` at `time` at a fixed UTC offset of `hours`.
    fn at(date: Date, time: Time, hours: i8) -> Timestamp {
        let offset = UtcOffset::from_hms(hours, 0, 0).unwrap();
        let instant = PrimitiveDateTime::new(date, time).assume_offset(offset);
        Timestamp::from_unix_seconds(instant.unix_timestamp()).unwrap()
    }

    fn tuesdays(pop_in: Option<PopIn>) -> Schedule {
        Schedule {
            rule: Rule::Weeks {
                n: 1,
                on: WeekdaySet::new().with(Weekday::Tuesday),
            },
            anchor: Anchor::Calendar {
                start: date!(2026 - 11 - 17),
            },
            pop_in,
        }
    }

    fn daily_at(at: Time) -> Schedule {
        Schedule {
            rule: Rule::Days { n: 1 },
            anchor: Anchor::Calendar {
                start: date!(2027 - 03 - 01),
            },
            pop_in: Some(PopIn { at, until: None }),
        }
    }

    #[test]
    fn deadline_chore_fires_at_its_pop_in_time() {
        let schedule = tuesdays(Some(PopIn {
            at: time!(18:00),
            until: Some(time!(22:00)),
        }));
        let firing =
            next_due_instant(&schedule, date!(2026 - 11 - 16), None, &FakeZone, ZONE).unwrap();
        assert_eq!(
            firing.next_due_at,
            at(date!(2026 - 11 - 17), time!(18:00), -5)
        );
        assert_eq!(
            firing.expires_at,
            Some(at(date!(2026 - 11 - 17), time!(22:00), -5))
        );
    }

    #[test]
    fn interval_chore_is_due_at_local_midnight_with_no_window() {
        let firing = next_due_instant(
            &tuesdays(None),
            date!(2026 - 11 - 16),
            None,
            &FakeZone,
            ZONE,
        )
        .unwrap();
        assert_eq!(
            firing.next_due_at,
            at(date!(2026 - 11 - 17), time!(0:00), -5)
        );
        assert_eq!(firing.expires_at, None);
    }

    #[test]
    fn until_at_or_before_pop_in_rolls_over_to_the_next_day() {
        let schedule = tuesdays(Some(PopIn {
            at: time!(18:00),
            until: Some(time!(8:00)),
        }));
        let firing =
            next_due_instant(&schedule, date!(2026 - 11 - 16), None, &FakeZone, ZONE).unwrap();
        assert_eq!(
            firing.expires_at,
            Some(at(date!(2026 - 11 - 18), time!(8:00), -5))
        );
    }

    #[test]
    fn pop_in_inside_a_spring_forward_gap_shifts_forward_by_the_gap() {
        let firing = next_due_instant(
            &daily_at(time!(2:30)),
            date!(2027 - 03 - 13),
            None,
            &FakeZone,
            ZONE,
        )
        .unwrap();
        assert_eq!(firing.next_due_at, at(SPRING_FORWARD, time!(3:30), -4));
    }

    #[test]
    fn an_unknown_zone_still_errors() {
        let result = next_due_instant(
            &daily_at(time!(2:30)),
            date!(2027 - 03 - 13),
            None,
            &FakeZone,
            "Nowhere/Special",
        );
        assert!(matches!(result, Err(AppError::Repo(_))), "got {result:?}");
    }

    #[test]
    fn upcoming_firing_takes_todays_occurrence_only_while_still_ahead() {
        let schedule = tuesdays(Some(PopIn {
            at: time!(18:00),
            until: None,
        }));
        let tuesday = date!(2026 - 11 - 17);
        let morning = at(tuesday, time!(9:00), -5);
        let evening = at(tuesday, time!(18:00), -5);
        let tonight = upcoming_firing(&schedule, morning, None, &FakeZone, ZONE).unwrap();
        assert_eq!(tonight.next_due_at, evening);
        let next_week = upcoming_firing(&schedule, evening, None, &FakeZone, ZONE).unwrap();
        assert_eq!(
            next_week.next_due_at,
            at(date!(2026 - 11 - 24), time!(18:00), -5)
        );
    }
}
