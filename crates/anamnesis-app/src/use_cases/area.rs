//! Area CRUD (`docs/DOMAIN.md` §3). Gated by [`Action::ManageArea`] /
//! [`Action::ViewArea`] — see `crate::policy`'s module doc comment for why
//! areas are treated as System Admin territory.

use anamnesis_core::policy::Role;
use anamnesis_core::{self as core, Area, AreaId};

use crate::error::AppError;
use crate::policy::{Action, is_allowed};
use crate::ports::{AreaRepository, Clock, IdGen};

/// Creates a new area.
pub async fn create_area(
    repo: &dyn AreaRepository,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    role: Option<Role>,
    title: &str,
    description: &str,
    position: u32,
) -> Result<Area, AppError> {
    if !is_allowed(role, Action::ManageArea) {
        return Err(AppError::Forbidden);
    }
    let area = core::create_area(AreaId::new(ids.next()), title, description, position, clock.now())?;
    repo.insert(&area).await?;
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
pub async fn list_areas(repo: &dyn AreaRepository, role: Option<Role>) -> Result<Vec<Area>, AppError> {
    if !is_allowed(role, Action::ViewArea) {
        return Err(AppError::Forbidden);
    }
    Ok(repo.list().await?)
}

/// Replaces an area's title and description.
pub async fn edit_area(
    repo: &dyn AreaRepository,
    clock: &dyn Clock,
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
