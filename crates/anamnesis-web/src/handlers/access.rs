//! Resolves "what role does this user hold here" for a handler to pass into
//! `anamnesis_app`'s use cases — the caller-side lookup
//! `crate::ports::MembershipQuery`'s module doc comment says core cannot do
//! itself.
//!
//! Each function here delegates straight to `anamnesis_app::access`, which
//! joins the two grant dimensions: the user's own grants, and whatever their
//! OIDC groups hold. That join belongs in the app layer, not here — this
//! module's job is only to pull the two ports out of [`AppState`] and map
//! the error. Handlers must go through these rather than reaching for
//! `state.membership` directly, or a user whose whole access comes through a
//! group would be wrongly denied.

use anamnesis_core::policy::Role;
use anamnesis_core::{AreaId, ProjectId, UserId};

use crate::error::WebError;
use crate::state::AppState;

pub async fn is_system_admin(state: &AppState, user: &UserId) -> Result<bool, WebError> {
    Ok(anamnesis_app::access::is_system_admin(
        state.membership.as_ref(),
        state.group_membership.as_ref(),
        user,
    )
    .await?)
}

pub async fn area_role(
    state: &AppState,
    user: &UserId,
    area: AreaId,
) -> Result<Option<Role>, WebError> {
    Ok(anamnesis_app::access::effective_area_role(
        state.membership.as_ref(),
        state.group_membership.as_ref(),
        user,
        area,
    )
    .await?)
}

pub async fn project_role(
    state: &AppState,
    user: &UserId,
    project: ProjectId,
    area: AreaId,
) -> Result<Option<Role>, WebError> {
    Ok(anamnesis_app::access::effective_role(
        state.membership.as_ref(),
        state.group_membership.as_ref(),
        user,
        project,
        area,
    )
    .await?)
}
