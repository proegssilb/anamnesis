//! `DomainError`: every way a rule (not the world) can reject a transition on
//! the real domain model (`docs/DOMAIN.md`).
//!
//! Contrast with the shell's `RepoError`/`AppError`: those exist because the
//! world can fail (a database is down, a token expired). `DomainError`
//! exists because the *rules* forbid what was asked, and rules are decided
//! entirely within this crate.
//!
//! For the placeholder kanban model's error type, see
//! [`crate::legacy::DomainError`].

use crate::field::FieldKind;
use crate::title::TitleError;

/// Every way a core transition on the real domain model can be rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DomainError {
    /// A candidate title failed validation.
    #[error("invalid title: {0}")]
    InvalidTitle(#[from] TitleError),
    /// A candidate currency code was not three uppercase ASCII letters.
    #[error("invalid currency code")]
    InvalidCurrencyCode,
    /// A [`crate::field::FieldValueData`] variant did not match its
    /// [`FieldDefinition`](crate::field::FieldDefinition)'s declared `kind`.
    #[error("field value does not match its definition's kind {0:?}")]
    FieldKindMismatch(FieldKind),
    /// Setting a task's parent would make the task its own ancestor.
    ///
    /// Containment (checklists, via `parent_task_id`) is enforced acyclic —
    /// contrast with `Relationship` edges, where cycles are allowed by
    /// design (see `docs/DOMAIN.md` §4): a task containing its own ancestor
    /// breaks rendering and roll-up with no user meaning, whereas a
    /// relationship cycle just means the user's own head is tangled, which
    /// the system must be able to represent rather than reject.
    #[error("a task cannot contain its own ancestor")]
    ContainmentCycle,
    /// A relationship's `from_task_id` and `to_task_id` were the same task.
    #[error("a task cannot relate to itself")]
    SelfRelationship,
    /// A relationship crossed projects using a custom (non-built-in) kind.
    ///
    /// Only built-in kinds (`project_id: None`) are valid for an edge whose
    /// two tasks do not share a project; a custom kind belongs to one
    /// project and using it where the far end lives elsewhere leaves its
    /// ownership and visibility ambiguous.
    #[error("a custom relationship kind may only be used within its own project")]
    RelationshipKindNotAllowed,
    /// A project's status transitioned to `Active` while
    /// `count(status == Active)` was already at `active_project_limit`.
    #[error("active project limit reached")]
    ActiveProjectLimitExceeded,
    /// An operation that requires an archived entity was applied to one that
    /// is not archived.
    #[error("entity is not archived")]
    NotArchived,
    /// An operation that requires a non-archived entity was applied to one
    /// that is already archived.
    #[error("entity is already archived")]
    AlreadyArchived,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_display_a_useful_message() {
        assert_eq!(
            DomainError::InvalidCurrencyCode.to_string(),
            "invalid currency code"
        );
        assert_eq!(
            DomainError::FieldKindMismatch(FieldKind::Number).to_string(),
            "field value does not match its definition's kind Number"
        );
        assert_eq!(
            DomainError::ContainmentCycle.to_string(),
            "a task cannot contain its own ancestor"
        );
        assert_eq!(
            DomainError::SelfRelationship.to_string(),
            "a task cannot relate to itself"
        );
        assert_eq!(
            DomainError::RelationshipKindNotAllowed.to_string(),
            "a custom relationship kind may only be used within its own project"
        );
        assert_eq!(
            DomainError::ActiveProjectLimitExceeded.to_string(),
            "active project limit reached"
        );
        assert_eq!(
            DomainError::NotArchived.to_string(),
            "entity is not archived"
        );
        assert_eq!(
            DomainError::AlreadyArchived.to_string(),
            "entity is already archived"
        );
    }

    #[test]
    fn invalid_title_wraps_the_title_error() {
        let err: DomainError = TitleError::Empty.into();
        assert_eq!(err, DomainError::InvalidTitle(TitleError::Empty));
        assert_eq!(err.to_string(), "invalid title: title must not be empty");
    }
}
