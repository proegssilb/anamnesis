//! Area CRUD (`docs/DOMAIN.md` §3). Gated by [`Action::ManageArea`] /
//! [`Action::ViewArea`] — see `crate::policy`'s module doc comment: both are
//! now Area-scoped (any assigned Area role for viewing, Project-Admin-tier
//! for managing), resolved by the caller via
//! [`crate::ports::MembershipQuery::effective_area_role`] and passed in.
//! `create_area` is the one exception: a brand-new Area has no scope to
//! resolve a role *in*, so in practice only a true System Admin can ever
//! construct a role value that satisfies [`Action::ManageArea`] here.

use anamnesis_core::policy::Role;
use anamnesis_core::{self as core, Area, AreaId};

use crate::error::AppError;
use crate::policy::{Action, is_allowed};
use crate::ports::{AreaRepository, Clock, IdGen, SearchIndex};

use super::indexing::log_index_failure;

/// Creates a new area, then indexes it for global search
/// (`docs/DOMAIN.md` §8). Indexing happens here, beside the repository
/// write, rather than in a caller — see `crate::use_cases::indexing`'s
/// module doc comment for why, and for what happens if the index write
/// itself fails (logged, non-fatal: the area was already created
/// successfully).
#[allow(clippy::too_many_arguments)]
pub async fn create_area(
    repo: &dyn AreaRepository,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    search: &dyn SearchIndex,
    role: Option<Role>,
    title: &str,
    description: &str,
    position: u32,
) -> Result<Area, AppError> {
    if !is_allowed(role, Action::ManageArea) {
        return Err(AppError::Forbidden);
    }
    let area = core::create_area(
        AreaId::new(ids.next()),
        title,
        description,
        position,
        clock.now(),
    )?;
    repo.insert(&area).await?;
    if let Err(err) = search.index_area(area.id, area.title.as_str()).await {
        log_index_failure("create_area", err);
    }
    Ok(area)
}

/// Returns a single area.
pub async fn view_area(
    repo: &dyn AreaRepository,
    role: Option<Role>,
    id: AreaId,
) -> Result<Area, AppError> {
    if !is_allowed(role, Action::ViewArea) {
        return Err(AppError::Forbidden);
    }
    repo.load(id).await?.ok_or(AppError::NotFound)
}

/// Lists every area (the area grid, `docs/DOMAIN.md` §3).
pub async fn list_areas(
    repo: &dyn AreaRepository,
    role: Option<Role>,
) -> Result<Vec<Area>, AppError> {
    if !is_allowed(role, Action::ViewArea) {
        return Err(AppError::Forbidden);
    }
    Ok(repo.list().await?)
}

/// Replaces an area's title and description, then re-indexes it — its
/// title, the only field global search sees, may have just changed. See
/// [`create_area`]'s doc comment for the indexing-failure policy.
pub async fn edit_area(
    repo: &dyn AreaRepository,
    clock: &dyn Clock,
    search: &dyn SearchIndex,
    role: Option<Role>,
    id: AreaId,
    title: &str,
    description: &str,
) -> Result<Area, AppError> {
    if !is_allowed(role, Action::ManageArea) {
        return Err(AppError::Forbidden);
    }
    let area = repo.load(id).await?.ok_or(AppError::NotFound)?;
    let edited = core::edit_area(&area, title, description, clock.now())?;
    repo.update(&edited).await?;
    if let Err(err) = search.index_area(edited.id, edited.title.as_str()).await {
        log_index_failure("edit_area", err);
    }
    Ok(edited)
}

/// Moves an area to a new position in the grid.
pub async fn reposition_area(
    repo: &dyn AreaRepository,
    role: Option<Role>,
    id: AreaId,
    position: u32,
) -> Result<Area, AppError> {
    if !is_allowed(role, Action::ManageArea) {
        return Err(AppError::Forbidden);
    }
    let area = repo.load(id).await?.ok_or(AppError::NotFound)?;
    let moved = core::reposition_area(&area, position);
    repo.update(&moved).await?;
    Ok(moved)
}
