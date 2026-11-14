//! Per-task recurrence state and its pure lifecycle (`docs/superpowers/specs/
//! 2026-11-12-recurring-tasks-design.md`, "Lifecycle").
//!
//! Every function returns a new [`Task`]; none mutates. Firing, expiry and
//! checklist reset deliberately never touch `bounce_count`,
//! `last_bounced_at`, `last_offered_at` or `last_touched_at`: the system
//! putting a chore up or taking it down is not the user's doing.
//!
//! Timing arithmetic (next occurrence, window end) is computed by the caller
//! in local-calendar terms and passed in; this module only decides what the
//! task's fields become.

use serde::{Deserialize, Serialize};

use crate::ids::Timestamp;
use crate::placement::Placement;
use crate::schedule::Schedule;
use crate::task::Task;

/// A task's recurrence state.
///
/// `next_due_at == None` means paused (explicitly archived): the schedule is
/// kept so restoring the task can resume it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRecurrence {
    pub schedule: Schedule,
    pub next_due_at: Option<Timestamp>,
    pub expires_at: Option<Timestamp>,
    pub last_completed_at: Option<Timestamp>,
    pub due_since: Option<Timestamp>,
}

/// The caller-computed timing for the occurrence being fired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Firing {
    pub next_due_at: Timestamp,
    pub expires_at: Option<Timestamp>,
}

fn with_recurrence(task: &Task, change: impl FnOnce(&mut TaskRecurrence)) -> Task {
    let mut out = task.clone();
    if let Some(recurrence) = out.recurrence.as_mut() {
        change(recurrence);
    }
    out
}

/// Pauses recurrence: timing cleared, schedule kept so it can resume.
pub fn pause_recurrence(task: &Task) -> Task {
    with_recurrence(task, |r| {
        r.next_due_at = None;
        r.expires_at = None;
        r.due_since = None;
    })
}

/// Replaces the recurrence with a fresh one for `schedule`, due at
/// `next_due_at`. Keeps `last_completed_at` if one existed; placement is
/// untouched.
pub fn set_schedule(task: &Task, schedule: Schedule, next_due_at: Timestamp) -> Task {
    let last_completed_at = task.recurrence.as_ref().and_then(|r| r.last_completed_at);
    Task {
        recurrence: Some(TaskRecurrence {
            schedule,
            next_due_at: Some(next_due_at),
            expires_at: None,
            last_completed_at,
            due_since: None,
        }),
        ..task.clone()
    }
}

/// Removes recurrence entirely.
pub fn clear_schedule(task: &Task) -> Task {
    Task {
        recurrence: None,
        ..task.clone()
    }
}

/// Records a completion: stamps `last_completed_at`, clears `due_since` and
/// `expires_at`. `next_due_at` is replaced only when given (after-completion
/// schedules).
pub fn record_completion(task: &Task, now: Timestamp, next_due_at: Option<Timestamp>) -> Task {
    with_recurrence(task, |r| {
        r.last_completed_at = Some(now);
        r.due_since = None;
        r.expires_at = None;
        if next_due_at.is_some() {
            r.next_due_at = next_due_at;
        }
    })
}

fn is_up_and_unfinished(task: &Task, in_done_column: bool) -> bool {
    task.placement.is_on_board() && !in_done_column
}

/// Fires a due occurrence. `entry_end` is the end of the board's entry
/// column, computed by the caller; a deadline chore lands there regardless
/// of any WIP limit. A task without recurrence is returned unchanged.
pub fn fire(
    task: &Task,
    in_done_column: bool,
    entry_end: Placement,
    firing: Firing,
    now: Timestamp,
) -> Task {
    let Some(recurrence) = task.recurrence.as_ref() else {
        return task.clone();
    };
    let mut recurrence = recurrence.clone();
    recurrence.next_due_at = Some(firing.next_due_at);

    let up = is_up_and_unfinished(task, in_done_column);
    let mut placement = if up { task.placement } else { Placement::Below };
    if recurrence.schedule.is_deadline() {
        let stale = firing.expires_at.is_some_and(|e| e <= now);
        if !up && !stale {
            placement = entry_end;
        }
        // A stale window stays below with nothing left to expire.
        recurrence.expires_at = if stale { None } else { firing.expires_at };
    } else {
        recurrence.due_since = recurrence.due_since.or(Some(now));
    }

    Task {
        placement,
        archived_at: None,
        recurrence: Some(recurrence),
        ..task.clone()
    }
}

/// Expires a deadline chore's window: if it is still up and unfinished it
/// goes `Below` without bouncing. Always clears `expires_at`.
pub fn expire(task: &Task, in_done_column: bool, now: Timestamp) -> Task {
    let due = task
        .recurrence
        .as_ref()
        .and_then(|r| r.expires_at)
        .is_some_and(|e| e <= now);
    let mut out = with_recurrence(task, |r| r.expires_at = None);
    if due && is_up_and_unfinished(task, in_done_column) {
        out.placement = Placement::Below;
    }
    out
}

/// Resets a recurring chore's checklist item: `Some` (un-archived, `Below`)
/// only if the item is archived or sits in a done column.
pub fn reset_checklist_item(item: &Task, in_done_column: bool) -> Option<Task> {
    if item.archived_at.is_none() && !(item.placement.is_on_board() && in_done_column) {
        return None;
    }
    Some(Task {
        placement: Placement::Below,
        archived_at: None,
        ..item.clone()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{ColumnId, ProjectId, TaskId};
    use crate::schedule::{Anchor, PopIn, Rule};
    use crate::task::{archive_task, create_task};
    use time::macros::{date, time};
    use uuid::Uuid;

    fn ts(secs: i64) -> Timestamp {
        Timestamp::from_unix_seconds(secs).unwrap()
    }

    fn col() -> ColumnId {
        ColumnId::new(Uuid::from_u128(500))
    }

    fn entry_end() -> Placement {
        Placement::OnBoard {
            column: col(),
            position: 3,
        }
    }

    fn schedule(deadline: bool) -> Schedule {
        Schedule {
            rule: Rule::Days { n: 1 },
            anchor: Anchor::Calendar {
                start: date!(2026 - 11 - 13),
            },
            pop_in: deadline.then_some(PopIn {
                at: time!(18:00),
                until: None,
            }),
        }
    }

    fn recurring(deadline: bool) -> Task {
        let mut t = create_task(
            TaskId::new(Uuid::from_u128(1)),
            ProjectId::new(Uuid::from_u128(1)),
            "Take out bins",
            "",
            ts(0),
        )
        .unwrap();
        t.bounce_count = 2;
        t.last_bounced_at = Some(ts(11));
        t.last_offered_at = Some(ts(12));
        t.last_touched_at = ts(13);
        t.recurrence = Some(TaskRecurrence {
            schedule: schedule(deadline),
            next_due_at: Some(ts(100)),
            expires_at: None,
            last_completed_at: None,
            due_since: None,
        });
        t
    }

    fn rec(t: &Task) -> &TaskRecurrence {
        t.recurrence.as_ref().unwrap()
    }

    fn firing(expires: Option<i64>) -> Firing {
        Firing {
            next_due_at: ts(1000),
            expires_at: expires.map(ts),
        }
    }

    fn on_board(mut t: Task) -> Task {
        t.placement = Placement::OnBoard {
            column: col(),
            position: 0,
        };
        t
    }

    #[test]
    fn fire_deadline_from_done_lands_at_entry_end_with_expiry() {
        let t = on_board(recurring(true));
        let fired = fire(&t, true, entry_end(), firing(Some(900)), ts(200));
        assert_eq!(fired.placement, entry_end());
        assert_eq!(rec(&fired).expires_at, Some(ts(900)));
        assert_eq!(rec(&fired).next_due_at, Some(ts(1000)));
        assert_eq!(fired.bounce_count, 2);
    }

    #[test]
    fn fire_deadline_from_archive_unarchives_and_raises() {
        let t = archive_task(&recurring(true), ts(50)).unwrap();
        let fired = fire(&t, false, entry_end(), firing(None), ts(200));
        assert_eq!(fired.archived_at, None);
        assert_eq!(fired.placement, entry_end());
    }

    #[test]
    fn fire_deadline_with_stale_window_goes_below_not_board() {
        let t = on_board(recurring(true));
        let fired = fire(&t, true, entry_end(), firing(Some(150)), ts(200));
        assert_eq!(fired.placement, Placement::Below);
        assert_eq!(rec(&fired).expires_at, None);
        assert_eq!(rec(&fired).next_due_at, Some(ts(1000)));
    }

    #[test]
    fn fire_leaves_task_already_up_and_unfinished_where_it_is() {
        let t = on_board(recurring(true));
        let fired = fire(&t, false, entry_end(), firing(Some(900)), ts(200));
        assert_eq!(fired.placement, t.placement);
        assert_eq!(rec(&fired).expires_at, Some(ts(900)));
    }

    #[test]
    fn fire_interval_goes_below_and_sets_due_since_once() {
        let t = on_board(recurring(false));
        let first = fire(&t, true, entry_end(), firing(None), ts(200));
        assert_eq!(first.placement, Placement::Below);
        assert_eq!(rec(&first).due_since, Some(ts(200)));
        let second = fire(&first, false, entry_end(), firing(None), ts(300));
        assert_eq!(rec(&second).due_since, Some(ts(200)));
    }

    #[test]
    fn fire_never_touches_bounce_offer_or_touch_fields() {
        for deadline in [true, false] {
            let t = archive_task(&recurring(deadline), ts(50)).unwrap();
            let fired = fire(&t, false, entry_end(), firing(Some(900)), ts(200));
            assert_eq!(fired.bounce_count, t.bounce_count);
            assert_eq!(fired.last_bounced_at, t.last_bounced_at);
            assert_eq!(fired.last_offered_at, t.last_offered_at);
            assert_eq!(fired.last_touched_at, t.last_touched_at);
        }
    }

    #[test]
    fn expire_lowers_without_bounce() {
        let mut t = on_board(recurring(true));
        t.recurrence.as_mut().unwrap().expires_at = Some(ts(100));
        let out = expire(&t, false, ts(100));
        assert_eq!(out.placement, Placement::Below);
        assert_eq!(rec(&out).expires_at, None);
        assert_eq!(out.bounce_count, 2);
        assert_eq!(out.last_bounced_at, Some(ts(11)));
    }

    #[test]
    fn expire_after_user_already_dropped_it_is_a_noop_on_placement() {
        let mut t = recurring(true);
        t.recurrence.as_mut().unwrap().expires_at = Some(ts(100));
        let out = expire(&t, false, ts(500));
        assert_eq!(out.placement, Placement::Below);
        assert_eq!(out.bounce_count, 2);
        assert_eq!(rec(&out).expires_at, None);
    }

    #[test]
    fn expire_leaves_a_finished_chore_in_done() {
        let mut t = on_board(recurring(true));
        t.recurrence.as_mut().unwrap().expires_at = Some(ts(100));
        let out = expire(&t, true, ts(500));
        assert_eq!(out.placement, t.placement);
        assert_eq!(rec(&out).expires_at, None);
    }

    #[test]
    fn record_completion_clears_due_and_expiry_and_only_replaces_next_due_when_given() {
        let mut t = recurring(false);
        {
            let r = t.recurrence.as_mut().unwrap();
            r.due_since = Some(ts(60));
            r.expires_at = Some(ts(70));
        }
        let kept = record_completion(&t, ts(200), None);
        assert_eq!(rec(&kept).last_completed_at, Some(ts(200)));
        assert_eq!(rec(&kept).due_since, None);
        assert_eq!(rec(&kept).expires_at, None);
        assert_eq!(rec(&kept).next_due_at, Some(ts(100)));
        let replaced = record_completion(&t, ts(200), Some(ts(5000)));
        assert_eq!(rec(&replaced).next_due_at, Some(ts(5000)));
    }

    #[test]
    fn set_schedule_keeps_last_completed_and_clears_due_since() {
        let mut t = on_board(recurring(false));
        {
            let r = t.recurrence.as_mut().unwrap();
            r.last_completed_at = Some(ts(40));
            r.due_since = Some(ts(60));
            r.expires_at = Some(ts(70));
        }
        let out = set_schedule(&t, schedule(true), ts(777));
        assert_eq!(rec(&out).schedule, schedule(true));
        assert_eq!(rec(&out).next_due_at, Some(ts(777)));
        assert_eq!(rec(&out).last_completed_at, Some(ts(40)));
        assert_eq!(rec(&out).due_since, None);
        assert_eq!(rec(&out).expires_at, None);
        assert_eq!(out.placement, t.placement);
        let fresh = set_schedule(&recurring(false).clone(), schedule(false), ts(1));
        assert_eq!(rec(&fresh).last_completed_at, None);
        assert_eq!(clear_schedule(&out).recurrence, None);
    }

    #[test]
    fn reset_checklist_item_only_touches_done_or_archived_items() {
        let item = recurring(false);
        assert_eq!(reset_checklist_item(&item, false), None);
        assert_eq!(reset_checklist_item(&on_board(item.clone()), false), None);
        let done = reset_checklist_item(&on_board(item.clone()), true).unwrap();
        assert_eq!(done.placement, Placement::Below);
        let archived = archive_task(&on_board(item.clone()), ts(5)).unwrap();
        let reset = reset_checklist_item(&archived, false).unwrap();
        assert_eq!(reset.archived_at, None);
        assert_eq!(reset.placement, Placement::Below);
        assert_eq!(reset.last_touched_at, archived.last_touched_at);
    }

    #[test]
    fn pause_keeps_schedule_and_clears_timing() {
        let mut t = recurring(true);
        {
            let r = t.recurrence.as_mut().unwrap();
            r.expires_at = Some(ts(70));
            r.due_since = Some(ts(60));
            r.last_completed_at = Some(ts(40));
        }
        let out = pause_recurrence(&t);
        assert_eq!(rec(&out).schedule, schedule(true));
        assert_eq!(rec(&out).next_due_at, None);
        assert_eq!(rec(&out).expires_at, None);
        assert_eq!(rec(&out).due_since, None);
        assert_eq!(rec(&out).last_completed_at, Some(ts(40)));
    }
}
