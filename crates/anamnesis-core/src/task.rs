//! [`Task`]: the unit of work (`docs/DOMAIN.md` §3).
//!
//! Checklists are containment: a task's checklist items *are* tasks, related
//! via `parent_task_id`. A checklist item can be raised above the horizon
//! independently of its parent — containment and [`Placement`] are
//! orthogonal, which is why `set_parent` never touches `placement` and the
//! placement transitions never touch `parent_task_id`.
//!
//! `bounce_count`, `last_bounced_at` and `last_offered_at` are present as
//! fields here (per `docs/DOMAIN.md` §3) but this module does not mutate
//! them: accounting for bounces and offers is explicitly Phase B's job
//! ("bounce + cooldown accounting", `docs/DOMAIN.md` §10), wired through the
//! suggestion engine. Phase A only establishes that the fields exist and
//! start at their zero value.

use serde::{Deserialize, Serialize};

use crate::error::DomainError;
use crate::ids::{ProjectId, TaskId, Timestamp};
use crate::placement::Placement;
use crate::title::Title;

/// A single unit of work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub project_id: ProjectId,
    pub title: Title,
    pub description: String,
    pub placement: Placement,
    pub parent_task_id: Option<TaskId>,
    pub checklist_position: u32,
    pub created_at: Timestamp,
    pub last_touched_at: Timestamp,
    pub archived_at: Option<Timestamp>,
    pub bounce_count: u32,
    pub last_bounced_at: Option<Timestamp>,
    pub last_offered_at: Option<Timestamp>,
}

/// Creates a new task, below the horizon, with no parent.
pub fn create_task(
    id: TaskId,
    project_id: ProjectId,
    title: impl AsRef<str>,
    description: impl Into<String>,
    now: Timestamp,
) -> Result<Task, DomainError> {
    let title = Title::new(title)?;
    Ok(Task {
        id,
        project_id,
        title,
        description: description.into(),
        placement: Placement::Below,
        parent_task_id: None,
        checklist_position: 0,
        created_at: now,
        last_touched_at: now,
        archived_at: None,
        bounce_count: 0,
        last_bounced_at: None,
        last_offered_at: None,
    })
}

/// Replaces a task's title and description, stamping `last_touched_at`.
pub fn edit_task(
    task: &Task,
    title: impl AsRef<str>,
    description: impl Into<String>,
    now: Timestamp,
) -> Result<Task, DomainError> {
    let title = Title::new(title)?;
    Ok(Task {
        title,
        description: description.into(),
        last_touched_at: now,
        ..task.clone()
    })
}

/// Moves a task to a new [`Placement`] (below the horizon, or onto a board
/// column/position), stamping `last_touched_at`.
///
/// This is deliberately generic over the direction of the move — it does
/// not increment `bounce_count`. That accounting is Phase B's concern (see
/// the module doc comment); a Phase B function is expected to wrap this one
/// for the `OnBoard -> Below` direction specifically.
pub fn move_placement(
    task: &Task,
    placement: Placement,
    now: Timestamp,
) -> Result<Task, DomainError> {
    if task.archived_at.is_some() {
        return Err(DomainError::AlreadyArchived);
    }
    Ok(Task {
        placement,
        last_touched_at: now,
        ..task.clone()
    })
}

/// Archives a task. Rejects an already-archived task.
pub fn archive_task(task: &Task, now: Timestamp) -> Result<Task, DomainError> {
    if task.archived_at.is_some() {
        return Err(DomainError::AlreadyArchived);
    }
    Ok(Task {
        archived_at: Some(now),
        last_touched_at: now,
        ..task.clone()
    })
}

/// Restores an archived task. Rejects a task that is not archived.
pub fn unarchive_task(task: &Task, now: Timestamp) -> Result<Task, DomainError> {
    if task.archived_at.is_none() {
        return Err(DomainError::NotArchived);
    }
    Ok(Task {
        archived_at: None,
        last_touched_at: now,
        ..task.clone()
    })
}

/// Sets (or clears) a task's parent, enforcing that containment stays
/// acyclic.
///
/// `new_parent`'s full ancestor chain (its parent, its parent's parent, and
/// so on up to the root) must be supplied by the caller as
/// `new_parent_ancestors` — core loads no graph, so it cannot walk the chain
/// itself. Rejected when:
/// - `new_parent == Some(task.id)` (a task cannot be its own parent), or
/// - `task.id` appears in `new_parent_ancestors` (the task would become an
///   ancestor of its own ancestor, i.e. a containment cycle).
///
/// Containment cycles are rejected — contrast with `Relationship` edges,
/// where cycles are *allowed* by design (`docs/DOMAIN.md` §4): a task
/// containing its own ancestor breaks checklist rendering and roll-up with
/// no user-facing meaning, so it is enforced acyclic here in core, whereas a
/// relationship cycle is just the system faithfully storing a tangle that
/// exists in the user's head, which the product wants to represent, surface
/// and eventually help untangle rather than silently refuse to store.
pub fn set_parent(
    task: &Task,
    new_parent: Option<TaskId>,
    new_parent_ancestors: &[TaskId],
    now: Timestamp,
) -> Result<Task, DomainError> {
    if let Some(parent_id) = new_parent
        && (parent_id == task.id || new_parent_ancestors.contains(&task.id))
    {
        return Err(DomainError::ContainmentCycle);
    }
    Ok(Task {
        parent_task_id: new_parent,
        last_touched_at: now,
        ..task.clone()
    })
}

/// Reorders a task among its checklist siblings. Infallible: any `u32` is a
/// valid position, resolving collisions among siblings is a display concern
/// for the caller.
pub fn set_checklist_position(task: &Task, position: u32) -> Task {
    Task {
        checklist_position: position,
        ..task.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn tid(n: u128) -> TaskId {
        TaskId::new(Uuid::from_u128(n))
    }

    fn pid(n: u128) -> ProjectId {
        ProjectId::new(Uuid::from_u128(n))
    }

    fn ts(secs: i64) -> Timestamp {
        Timestamp::from_unix_seconds(secs).unwrap()
    }

    fn task() -> Task {
        create_task(tid(1), pid(1), "Regrout the shower", "", ts(0)).unwrap()
    }

    #[test]
    fn create_task_starts_below_with_no_parent_and_zeroed_bounce_state() {
        let t = task();
        assert_eq!(t.placement, Placement::Below);
        assert_eq!(t.parent_task_id, None);
        assert_eq!(t.checklist_position, 0);
        assert_eq!(t.bounce_count, 0);
        assert_eq!(t.last_bounced_at, None);
        assert_eq!(t.last_offered_at, None);
        assert_eq!(t.archived_at, None);
    }

    #[test]
    fn create_task_rejects_an_invalid_title() {
        let result = create_task(tid(1), pid(1), "   ", "", ts(0));
        assert!(matches!(result, Err(DomainError::InvalidTitle(_))));
    }

    #[test]
    fn edit_task_replaces_title_and_description_and_touches() {
        let t = task();
        let edited = edit_task(&t, "New title", "desc", ts(5)).unwrap();
        assert_eq!(edited.title.as_str(), "New title");
        assert_eq!(edited.description, "desc");
        assert_eq!(edited.last_touched_at, ts(5));
    }

    #[test]
    fn move_placement_updates_placement_and_touches() {
        let t = task();
        let column = crate::ids::ColumnId::new(Uuid::from_u128(500));
        let moved = move_placement(
            &t,
            Placement::OnBoard {
                column,
                position: 0,
            },
            ts(7),
        )
        .unwrap();
        assert_eq!(
            moved.placement,
            Placement::OnBoard {
                column,
                position: 0
            }
        );
        assert_eq!(moved.last_touched_at, ts(7));
    }

    #[test]
    fn move_placement_rejects_an_archived_task() {
        let t = task();
        let archived = archive_task(&t, ts(1)).unwrap();
        let result = move_placement(&archived, Placement::Below, ts(2));
        assert_eq!(result, Err(DomainError::AlreadyArchived));
    }

    #[test]
    fn archive_task_stamps_archived_at_and_touches() {
        let t = task();
        let archived = archive_task(&t, ts(9)).unwrap();
        assert_eq!(archived.archived_at, Some(ts(9)));
        assert_eq!(archived.last_touched_at, ts(9));
    }

    #[test]
    fn archive_task_rejects_an_already_archived_task() {
        let t = task();
        let archived = archive_task(&t, ts(9)).unwrap();
        let result = archive_task(&archived, ts(10));
        assert_eq!(result, Err(DomainError::AlreadyArchived));
    }

    #[test]
    fn unarchive_task_clears_archived_at() {
        let t = task();
        let archived = archive_task(&t, ts(9)).unwrap();
        let restored = unarchive_task(&archived, ts(10)).unwrap();
        assert_eq!(restored.archived_at, None);
    }

    #[test]
    fn unarchive_task_rejects_a_task_that_is_not_archived() {
        let t = task();
        let result = unarchive_task(&t, ts(10));
        assert_eq!(result, Err(DomainError::NotArchived));
    }

    // --- set_parent / containment: the required cycle-rejection tests. ---

    #[test]
    fn set_parent_attaches_a_checklist_item_to_its_parent() {
        let t = task();
        let parent = tid(2);
        let result = set_parent(&t, Some(parent), &[], ts(3)).unwrap();
        assert_eq!(result.parent_task_id, Some(parent));
        assert_eq!(result.last_touched_at, ts(3));
    }

    #[test]
    fn set_parent_clears_the_parent_when_given_none() {
        let t = task();
        let with_parent = set_parent(&t, Some(tid(2)), &[], ts(1)).unwrap();
        let cleared = set_parent(&with_parent, None, &[], ts(2)).unwrap();
        assert_eq!(cleared.parent_task_id, None);
    }

    #[test]
    fn set_parent_rejects_a_task_becoming_its_own_parent() {
        let t = task();
        let result = set_parent(&t, Some(t.id), &[], ts(3));
        assert_eq!(result, Err(DomainError::ContainmentCycle));
    }

    #[test]
    fn set_parent_rejects_a_task_becoming_an_ancestor_of_its_own_ancestor() {
        // A -> B -> C exists (C's parent is B, B's parent is A). Attempting
        // to reparent A under C would make A both an ancestor and a
        // descendant of itself. C's ancestor chain, as the caller would
        // compute it, is [B, A]; A appears in it, so this must be rejected.
        let a = task();
        let result = set_parent(&a, Some(tid(3)), &[tid(2), a.id], ts(3));
        assert_eq!(result, Err(DomainError::ContainmentCycle));
    }

    #[test]
    fn set_parent_allows_reparenting_to_an_unrelated_task() {
        let t = task();
        let unrelated_ancestors = [tid(50), tid(51)];
        let result = set_parent(&t, Some(tid(2)), &unrelated_ancestors, ts(3)).unwrap();
        assert_eq!(result.parent_task_id, Some(tid(2)));
    }

    #[test]
    fn set_checklist_position_reorders_without_touching_other_fields() {
        let t = task();
        let reordered = set_checklist_position(&t, 4);
        assert_eq!(reordered.checklist_position, 4);
        assert_eq!(reordered.last_touched_at, t.last_touched_at);
    }
}
