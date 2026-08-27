//! `AppError`: every way a use case can fail, plus the port-level error types
//! it wraps.
//!
//! Contrast with `anamnesis_core::DomainError`: that exists because a *rule*
//! forbade what was asked. `AppError` additionally covers what the use-case
//! orchestration itself decides (`NotFound`, `Forbidden`) and what the world
//! reported back through a port (`RepoError`, `IdentityError`).

use std::error::Error as StdError;

use anamnesis_core::legacy::DomainError;

/// Every way a use case can fail.
#[derive(Debug, PartialEq, thiserror::Error)]
pub enum AppError {
    /// The requested aggregate does not exist.
    #[error("not found")]
    NotFound,
    /// The current user is not permitted to view or act on this aggregate.
    #[error("forbidden")]
    Forbidden,
    /// A core transition rejected the request because a rule was broken.
    #[error(transparent)]
    Domain(#[from] DomainError),
    /// A port reported that the world failed (I/O, a database, a network
    /// call).
    #[error(transparent)]
    Repo(#[from] RepoError),
}

/// An opaque error from a [`crate::ports::BoardRepository`] implementation.
///
/// `anamnesis-app` never depends on a concrete storage crate, so this type
/// carries only a human-readable message plus an optional boxed cause —
/// enough for the shell to log the cause without leaking it to a page, and
/// without `app` needing to know what a `sqlx::Error` is.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct RepoError {
    message: String,
    #[source]
    source: Option<Box<dyn StdError + Send + Sync + 'static>>,
}

impl RepoError {
    /// Builds a `RepoError` with no further cause attached.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            source: None,
        }
    }

    /// Wraps an adapter-specific error (e.g. `sqlx::Error`) as a `RepoError`,
    /// retaining it as the `source` for logging.
    pub fn from_source(
        message: impl Into<String>,
        source: impl StdError + Send + Sync + 'static,
    ) -> Self {
        Self {
            message: message.into(),
            source: Some(Box::new(source)),
        }
    }
}

/// An opaque error from an [`crate::ports::IdentityProvider`] implementation.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct IdentityError {
    message: String,
    #[source]
    source: Option<Box<dyn StdError + Send + Sync + 'static>>,
}

impl IdentityError {
    /// Builds an `IdentityError` with no further cause attached.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            source: None,
        }
    }

    /// Wraps an adapter-specific error, retaining it as the `source`.
    pub fn from_source(
        message: impl Into<String>,
        source: impl StdError + Send + Sync + 'static,
    ) -> Self {
        Self {
            message: message.into(),
            source: Some(Box::new(source)),
        }
    }
}

// Manual `PartialEq` impls so tests can assert on `AppError`/`RepoError`
// shape without needing the boxed source (which is not `PartialEq`) to
// participate. Two `RepoError`s are equal when their messages match.
impl PartialEq for RepoError {
    fn eq(&self, other: &Self) -> bool {
        self.message == other.message
    }
}

impl PartialEq for IdentityError {
    fn eq(&self, other: &Self) -> bool {
        self.message == other.message
    }
}
