//! Shared plumbing for every "bulk add" use case (issue #34: areas,
//! projects, and tasks were each addable only one at a time — painful for
//! initial setup or transcribing an already-fully-formed project plan).
//!
//! A bulk create runs the same single-item use case once per title, exactly
//! as a caller pasting them in one at a time would, so a batch obeys the
//! same rules a lone creation does. The one thing it changes: a per-title
//! rule violation (an empty or too-long title) is collected as a partial
//! failure rather than aborting the rest of the batch — one bad line in a
//! pasted list shouldn't cost the good ones. Anything that isn't specific to
//! one title (an authorization failure, a repository error) still aborts the
//! whole batch immediately, exactly like the single-item use case would.

use anamnesis_core::DomainError;

use crate::error::AppError;

/// The result of a bulk create: everything that was made, and the titles
/// that were rejected (with why) instead.
pub struct BulkCreateOutcome<T> {
    pub created: Vec<T>,
    pub failures: Vec<(String, DomainError)>,
}

/// Calls `create_one` once per entry in `titles`, in order, routing a
/// per-item [`AppError::Rule`] into the outcome's `failures` instead of
/// aborting. `create_one` also receives the title's index in `titles`, for
/// callers (like [`super::area::bulk_create_areas`]) that need to derive a
/// per-item value — a display position, say — from it.
pub(super) async fn bulk_create<'a, T, F, Fut>(
    titles: &[&'a str],
    mut create_one: F,
) -> Result<BulkCreateOutcome<T>, AppError>
where
    F: FnMut(usize, &'a str) -> Fut,
    Fut: std::future::Future<Output = Result<T, AppError>>,
{
    let mut created = Vec::with_capacity(titles.len());
    let mut failures = Vec::new();
    for (index, &title) in titles.iter().enumerate() {
        match create_one(index, title).await {
            Ok(item) => created.push(item),
            Err(AppError::Rule(e)) => failures.push((title.to_string(), e)),
            Err(err) => return Err(err),
        }
    }
    Ok(BulkCreateOutcome { created, failures })
}
