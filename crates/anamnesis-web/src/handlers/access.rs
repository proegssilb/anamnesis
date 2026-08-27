//! Resolves "what role does this user hold here" for a handler to pass into
//! `anamnesis_app`'s use cases — the caller-side lookup
//! `crate::ports::MembershipQuery`'s module doc comment says core cannot do
//! itself.

use anamnesis_core::policy::Role;
use anamnesis_core::{AreaId, ProjectId, UserId};

use crate::error::WebError;
use crate::state::AppState;

pub async fn is_system_admin(state: &AppState, user: &UserId) -> Result<bool, WebError> {
    Ok(state.membership.is_system_admin(user).await?)
}

pub async fn area_role(
    state: &AppState,
    user: &UserId,
    area: AreaId,
) -> Result<Option<Role>, WebError> {
    Ok(state.membership.effective_area_role(user, area).await?)
}

pub async fn project_role(
    state: &AppState,
    user: &UserId,
    project: ProjectId,
    area: AreaId,
) -> Result<Option<Role>, WebError> {
    Ok(state.membership.effective_role(user, project, area).await?)
}
