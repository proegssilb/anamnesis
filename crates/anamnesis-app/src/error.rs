//! `AppError`: every way a use case can fail, plus the port-level error types
//! it wraps.
//!
//! Contrast with `anamnesis_core::DomainError`: that exists because a *rule*
//! forbade what was asked. `AppError` additionally covers what the use-case
//! orchestration itself decides (`NotFound`, `Forbidden`) and what the world
//! reported back through a port (`RepoError`, `IdentityError`).

use std::error::Error as StdError;

/// Every way a use case can fail.
#[derive(Debug, PartialEq, thiserror::Error)]
pub enum AppError {
    /// The requested aggregate does not exist.
    #[error("not found")]
    NotFound,
    /// The current user is not permitted to view or act on this aggregate.
    #[error("forbidden")]
    Forbidden,
    /// A real-domain-model transition (`anamnesis_core`, per `docs/DOMAIN.md`)
    /// rejected the request because a rule was broken.
    #[error(transparent)]
    Rule(#[from] anamnesis_core::DomainError),
    /// A candidate value failed validation at the application layer, for a
    /// type that carries no rule of its own in `anamnesis_core` (comments
    /// and attachments — see `crate::entities`).
    #[error("invalid input: {0}")]
    Invalid(String),
    /// A [`crate::ports::TaskRepository::update`] optimistic-concurrency
    /// check failed: the task was modified by someone else between the
    /// caller's load and this save (`docs/DOMAIN.md` §7 — "with finer-grained
    /// edits and plausible multi-device use, last-write-wins is no longer
    /// acceptable"). The caller should reload the task, re-apply its
    /// intended change on top of the fresh state, and retry.
    #[error("task was concurrently modified by someone else — reload and retry")]
    Conflict,
    /// A project transitioned (or tried to transition) to `Active` while the
    /// system was already at `Settings.active_project_limit`.
    #[error("active project limit reached")]
    ActiveProjectLimitExceeded,
    /// A column's WIP limit would have been exceeded by placing a task on it.
    #[error("column is at its work-in-progress limit")]
    WipLimitExceeded,
    /// A caller tried to revoke System Admin from the last user who holds
    /// it. Refused rather than honoured: on a self-hosted deployment, a
    /// database with zero System Admins is unrecoverable without direct
    /// database access, so the sane behaviour is to make this
    /// structurally unreachable through the application layer rather than
    /// something an admin can accidentally lock themselves out with.
    #[error("cannot revoke System Admin from the last user who holds it")]
    LastSystemAdmin,
    /// A port reported that the world failed (I/O, a database, a network
    /// call).
    #[error(transparent)]
    Repo(#[from] RepoError),
}

impl From<crate::ports::TaskUpdateError> for AppError {
    fn from(err: crate::ports::TaskUpdateError) -> Self {
        match err {
            crate::ports::TaskUpdateError::Conflict => AppError::Conflict,
            crate::ports::TaskUpdateError::Repo(e) => AppError::Repo(e),
        }
    }
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
