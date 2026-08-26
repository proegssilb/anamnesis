//! `DomainError`: every way a rule (not the world) can reject a transition.

use crate::title::TitleError;
use crate::{CardId, ColumnId};

/// Every way a core transition can be rejected because a rule was broken.
///
/// Contrast with the shell's `RepoError`/`AppError`: those exist because the
/// world can fail (a database is down, a token expired). `DomainError`
/// exists because the *rules* forbid what was asked, and rules are decided
/// entirely within this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DomainError {
    /// No column with this id exists on the board.
    #[error("column not found: {0}")]
    ColumnNotFound(ColumnId),
    /// No card with this id exists on the board.
    #[error("card not found: {0}")]
    CardNotFound(CardId),
    /// A freshly supplied id collides with one already present on the board.
    #[error("duplicate id")]
    DuplicateId,
    /// A candidate title failed validation.
    #[error("invalid title: {0}")]
    InvalidTitle(#[from] TitleError),
    /// The move would push a column past its WIP limit.
    #[error("WIP limit exceeded")]
    WipLimitExceeded,
    /// `remove_column` was called on a column that still has cards.
    #[error("column is not empty")]
    ColumnNotEmpty,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_display_a_useful_message() {
        assert_eq!(
            DomainError::ColumnNotFound(ColumnId::new(uuid::Uuid::nil())).to_string(),
            format!("column not found: {}", ColumnId::new(uuid::Uuid::nil()))
        );
        assert_eq!(
            DomainError::CardNotFound(CardId::new(uuid::Uuid::nil())).to_string(),
            format!("card not found: {}", CardId::new(uuid::Uuid::nil()))
        );
        assert_eq!(DomainError::DuplicateId.to_string(), "duplicate id");
        assert_eq!(
            DomainError::WipLimitExceeded.to_string(),
            "WIP limit exceeded"
        );
        assert_eq!(
            DomainError::ColumnNotEmpty.to_string(),
            "column is not empty"
        );
    }

    #[test]
    fn invalid_title_wraps_the_title_error() {
        let err: DomainError = TitleError::Empty.into();
        assert_eq!(err, DomainError::InvalidTitle(TitleError::Empty));
        assert_eq!(err.to_string(), "invalid title: title must not be empty");
    }
}
