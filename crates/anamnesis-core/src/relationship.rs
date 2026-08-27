//! [`Relationship`] and [`RelationshipKind`]: standalone edges between tasks,
//! living outside any project (`docs/DOMAIN.md` §3).
//!
//! Edges live outside projects because real blockers cross domains
//! constantly — this is precisely why `Board` could not remain the
//! aggregate. `RelationshipKind` supplies the vocabulary for an edge:
//! built-in (`project_id: None`, available everywhere) or project-local
//! custom vocabulary (`project_id: Some(_)`).
//!
//! **Relationship cycles are allowed, never rejected** — contrast with
//! containment (`crate::task::set_parent`), which is enforced acyclic. "The
//! system needs to store what's in the user's head, and sometimes that
//! means storing a mess for a bit." (`docs/DOMAIN.md` §4). Detecting such a
//! cycle and surfacing it as a Tangle is Phase B's job (Tarjan SCC over the
//! blocking graph); this module performs no such detection and rejects
//! nothing on the basis of what the graph already contains.

use serde::{Deserialize, Serialize};

use crate::error::DomainError;
use crate::ids::{KindId, ProjectId, RelationshipId, TaskId, Timestamp};
use crate::title::Title;

/// A standalone edge between two tasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relationship {
    pub id: RelationshipId,
    pub from_task_id: TaskId,
    pub to_task_id: TaskId,
    pub kind_id: KindId,
    pub created_at: Timestamp,
}

/// The vocabulary for a [`Relationship`]: `project_id: None` is built-in and
/// available everywhere; `project_id: Some(_)` is a project's own custom
/// label.
///
/// **Only the built-in `blocks` kind ([`KindId::BUILTIN_BLOCKS`]) carries
/// blocking meaning.** Custom kinds — and the other two built-ins,
/// `relates to` and `duplicates` — are labels for how a user describes a
/// link ("inspired by", "same shop trip") and carry no scheduling semantics.
/// This keeps the suggestion engine and tangle detection reading one
/// well-defined edge type instead of guessing intent from free text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelationshipKind {
    pub id: KindId,
    pub project_id: Option<ProjectId>,
    pub forward_label: Title,
    pub reverse_label: Title,
}

impl RelationshipKind {
    /// True for a built-in kind (`project_id: None`), available on any edge
    /// regardless of which projects its two tasks belong to.
    pub fn is_builtin(&self) -> bool {
        self.project_id.is_none()
    }
}

/// Well-known, fixed ids for the three built-in relationship kinds. These
/// are stable across the whole system (seeded once, at bootstrap) rather
/// than generated — core generates no ids, and "the" `blocks` kind must be
/// recognisable without a lookup.
impl KindId {
    pub const BUILTIN_BLOCKS: KindId = KindId::from_u128(0xA1A5_0000_0000_0000_0000_0000_0000_0001);
    pub const BUILTIN_RELATES_TO: KindId =
        KindId::from_u128(0xA1A5_0000_0000_0000_0000_0000_0000_0002);
    pub const BUILTIN_DUPLICATES: KindId =
        KindId::from_u128(0xA1A5_0000_0000_0000_0000_0000_0000_0003);
}

/// The built-in `blocks` / `blocked by` kind — the *only* kind that gates
/// availability (see [`is_blocking`]).
pub fn builtin_blocks() -> RelationshipKind {
    RelationshipKind {
        id: KindId::BUILTIN_BLOCKS,
        project_id: None,
        forward_label: Title::new("blocks").expect("builtin label is valid"),
        reverse_label: Title::new("blocked by").expect("builtin label is valid"),
    }
}

/// The built-in `relates to` kind. Reciprocal: the same label both ways.
pub fn builtin_relates_to() -> RelationshipKind {
    RelationshipKind {
        id: KindId::BUILTIN_RELATES_TO,
        project_id: None,
        forward_label: Title::new("relates to").expect("builtin label is valid"),
        reverse_label: Title::new("relates to").expect("builtin label is valid"),
    }
}

/// The built-in `duplicates` / `duplicated by` kind.
pub fn builtin_duplicates() -> RelationshipKind {
    RelationshipKind {
        id: KindId::BUILTIN_DUPLICATES,
        project_id: None,
        forward_label: Title::new("duplicates").expect("builtin label is valid"),
        reverse_label: Title::new("duplicated by").expect("builtin label is valid"),
    }
}

/// True only for the one built-in kind that gates task availability
/// (`docs/DOMAIN.md` §3). Every other kind — including the other two
/// built-ins — is a label with no scheduling meaning.
pub fn is_blocking(kind: &RelationshipKind) -> bool {
    kind.id == KindId::BUILTIN_BLOCKS
}

/// Creates a new project-local (custom) relationship kind.
pub fn create_relationship_kind(
    id: KindId,
    project_id: ProjectId,
    forward_label: impl AsRef<str>,
    reverse_label: impl AsRef<str>,
) -> Result<RelationshipKind, DomainError> {
    Ok(RelationshipKind {
        id,
        project_id: Some(project_id),
        forward_label: Title::new(forward_label)?,
        reverse_label: Title::new(reverse_label)?,
    })
}

/// Creates a relationship edge between two tasks.
///
/// `from_project_id`/`to_project_id` are the owning projects of the two
/// tasks — supplied by the caller, since core loads no tasks here. Enforces
/// the one rule from `docs/DOMAIN.md` §3 governing which kind an edge may
/// use:
///
/// - a built-in `kind` (`project_id: None`) may be used on any edge, same-
///   project or cross-project;
/// - a custom `kind` (`project_id: Some(p)`) may only be used when *both*
///   tasks belong to `p` — which is exactly why a custom kind can never
///   cross projects: there is no `p` that both a foreign `from` and `to`
///   could belong to.
///
/// Also rejects a task relating to itself, which no `RelationshipKind`
/// (blocking or otherwise) can give sensible meaning to.
///
/// No cycle check is performed, deliberately: see the module doc comment.
pub fn create_relationship(
    id: RelationshipId,
    from_task_id: TaskId,
    from_project_id: ProjectId,
    to_task_id: TaskId,
    to_project_id: ProjectId,
    kind: &RelationshipKind,
    now: Timestamp,
) -> Result<Relationship, DomainError> {
    if from_task_id == to_task_id {
        return Err(DomainError::SelfRelationship);
    }
    if let Some(kind_project) = kind.project_id
        && (kind_project != from_project_id || kind_project != to_project_id)
    {
        return Err(DomainError::RelationshipKindNotAllowed);
    }
    Ok(Relationship {
        id,
        from_task_id,
        to_task_id,
        kind_id: kind.id,
        created_at: now,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn rid(n: u128) -> RelationshipId {
        RelationshipId::new(Uuid::from_u128(n))
    }

    fn tid(n: u128) -> TaskId {
        TaskId::new(Uuid::from_u128(n))
    }

    fn pid(n: u128) -> ProjectId {
        ProjectId::new(Uuid::from_u128(n))
    }

    fn ts(secs: i64) -> Timestamp {
        Timestamp::from_unix_seconds(secs).unwrap()
    }

    #[test]
    fn builtin_kinds_are_builtin_and_carry_the_documented_labels() {
        let blocks = builtin_blocks();
        assert!(blocks.is_builtin());
        assert_eq!(blocks.forward_label.as_str(), "blocks");
        assert_eq!(blocks.reverse_label.as_str(), "blocked by");

        let relates = builtin_relates_to();
        assert!(relates.is_builtin());
        assert_eq!(relates.forward_label.as_str(), "relates to");

        let dup = builtin_duplicates();
        assert!(dup.is_builtin());
        assert_eq!(dup.forward_label.as_str(), "duplicates");
        assert_eq!(dup.reverse_label.as_str(), "duplicated by");
    }

    #[test]
    fn only_the_builtin_blocks_kind_is_blocking() {
        assert!(is_blocking(&builtin_blocks()));
        assert!(!is_blocking(&builtin_relates_to()));
        assert!(!is_blocking(&builtin_duplicates()));

        let custom = create_relationship_kind(
            KindId::new(Uuid::from_u128(900)),
            pid(1),
            "inspired by",
            "inspired",
        )
        .unwrap();
        assert!(!is_blocking(&custom));
    }

    #[test]
    fn create_relationship_kind_rejects_an_invalid_label() {
        let result = create_relationship_kind(KindId::new(Uuid::from_u128(1)), pid(1), "", "x");
        assert!(matches!(result, Err(DomainError::InvalidTitle(_))));
    }

    // --- create_relationship: cross-project + builtin-only rule. ---

    #[test]
    fn builtin_kind_is_allowed_across_projects() {
        let result = create_relationship(
            rid(1),
            tid(1),
            pid(1),
            tid(2),
            pid(2),
            &builtin_blocks(),
            ts(0),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn custom_kind_is_allowed_when_both_tasks_share_its_own_project() {
        let kind =
            create_relationship_kind(KindId::new(Uuid::from_u128(9)), pid(1), "a", "b").unwrap();
        let result = create_relationship(rid(1), tid(1), pid(1), tid(2), pid(1), &kind, ts(0));
        assert!(result.is_ok());
    }

    #[test]
    fn custom_kind_is_rejected_across_projects() {
        let kind =
            create_relationship_kind(KindId::new(Uuid::from_u128(9)), pid(1), "a", "b").unwrap();
        let result = create_relationship(rid(1), tid(1), pid(1), tid(2), pid(2), &kind, ts(0));
        assert_eq!(result, Err(DomainError::RelationshipKindNotAllowed));
    }

    #[test]
    fn custom_kind_is_rejected_even_same_project_pair_if_kind_belongs_elsewhere() {
        // The kind belongs to project 3; both tasks happen to live in
        // project 1 together, but that is not the kind's own project.
        let kind =
            create_relationship_kind(KindId::new(Uuid::from_u128(9)), pid(3), "a", "b").unwrap();
        let result = create_relationship(rid(1), tid(1), pid(1), tid(2), pid(1), &kind, ts(0));
        assert_eq!(result, Err(DomainError::RelationshipKindNotAllowed));
    }

    #[test]
    fn create_relationship_rejects_a_task_relating_to_itself() {
        let result = create_relationship(
            rid(1),
            tid(1),
            pid(1),
            tid(1),
            pid(1),
            &builtin_blocks(),
            ts(0),
        );
        assert_eq!(result, Err(DomainError::SelfRelationship));
    }

    // --- the deliberate asymmetry: relationship cycles are allowed. ---

    #[test]
    fn a_mutual_pair_of_relationships_is_allowed_a_relationship_cycle_is_never_rejected() {
        // A blocks B, and B (also) blocks A. This is a 2-cycle in the
        // blocking graph. Unlike `Task::set_parent`'s containment check,
        // `create_relationship` has no graph to consult and no cycle check
        // to run — both edges are created successfully. Detecting and
        // surfacing this as a Tangle is Phase B's job.
        let a_blocks_b = create_relationship(
            rid(1),
            tid(1),
            pid(1),
            tid(2),
            pid(1),
            &builtin_blocks(),
            ts(0),
        );
        let b_blocks_a = create_relationship(
            rid(2),
            tid(2),
            pid(1),
            tid(1),
            pid(1),
            &builtin_blocks(),
            ts(0),
        );
        assert!(a_blocks_b.is_ok());
        assert!(b_blocks_a.is_ok());
    }
}
