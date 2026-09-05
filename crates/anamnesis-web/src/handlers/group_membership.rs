//! Mapping OIDC groups to roles through the UI — the group-dimension twin of
//! [`crate::handlers::membership`], over
//! `anamnesis_app::use_cases::group_membership`. The same three surfaces:
//!
//! - An Area's "Groups" block, beside its members list.
//! - A Project's "Groups" block, symmetric.
//! - `/users`, where a System Admin maps a group to System Admin itself.
//!
//! Every handler resolves the actor's role exactly as its per-user
//! counterpart does (`crate::handlers::access`, scoped to the specific Area
//! or Project) and hands it to the use case unchanged; the use case is what
//! refuses an escalation attempt. This layer adds only the web furniture —
//! CSRF check, form parsing, the redirect back to the page posted from — and
//! reuses [`crate::handlers::membership::parse_grantable_role`], so
//! `role=system_admin` is unspellable through an Area or Project form here
//! for exactly the same reason it is there.
//!
//! ## The panel, and why the UI is data-gated
//!
//! [`AccessPanel`] is what an Area or Project page's "who can act here"
//! section renders: its members, its group mappings, and the group names an
//! admin may pick from. The three are fetched together, gated together
//! (`can_manage`), and rendered together as one block, so they travel as one
//! value rather than as three more parameters through the four call sites
//! that rebuild those pages.
//!
//! [`AccessPanel::show_groups`] is the whole of the "an unconfigured
//! deployment sees no new UI" rule, and it is derived from *data* rather
//! than from configuration: `list_known_groups` is empty exactly when no
//! user has ever presented a group and no mapping exists, which is precisely
//! the state of a deployment that never set `ANAMNESIS_OIDC_GROUPS_CLAIM`.
//! Reading config here instead would put a startup value back into the
//! request path for no gain — and would get the answer wrong for a
//! deployment that switched the claim off while old mappings still stood.

use axum::Form;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};

use anamnesis_app::{
    AppError, grant_admin_group, grant_area_group_role, grant_project_group_role, list_area_groups,
    list_known_groups, list_project_groups, revoke_admin_group, revoke_area_group_role,
    revoke_project_group_role,
};
use anamnesis_core::policy::Role;
use anamnesis_core::{AreaId, ProjectId, UserId};

use crate::error::WebError;
use crate::session::csrf_tokens_match;
use crate::state::AppState;

use super::access;
use super::forms::{
    GrantAdminGroupForm, GrantAreaGroupRoleForm, GrantProjectGroupRoleForm, RevokeAdminGroupForm,
    RevokeAreaGroupRoleForm, RevokeProjectGroupRoleForm,
};
use super::membership::{format_role, parse_grantable_role};

/// The contents of an Area's or Project's access section: who holds a role
/// here directly, which groups hold one, and every group name known to the
/// deployment (for the picker behind the group input).
///
/// Empty in every field when the viewer may not manage this scope — see
/// [`Self::hidden`].
#[derive(Debug, Default)]
pub(super) struct AccessPanel {
    pub can_manage: bool,
    pub members: Vec<(UserId, Role)>,
    pub groups: Vec<(String, Role)>,
    pub known_groups: Vec<String>,
}

impl AccessPanel {
    /// The panel a viewer who may not manage this scope gets: nothing. A
    /// plain Member has no business seeing who else holds a role here — the
    /// same gate the `list_*` use cases enforce themselves, so this only
    /// avoids calls that would come back `Forbidden`.
    pub(super) fn hidden() -> Self {
        Self::default()
    }

    /// Whether to render the group half of the panel at all. See the module
    /// doc comment: derived from data, never from configuration.
    pub(super) fn show_groups(&self) -> bool {
        !self.known_groups.is_empty()
    }

    /// `members` and `groups` shaped for the `area.html` / `project.html`
    /// templates — identical mapping in both, so it lives here once instead
    /// of twice.
    pub(super) fn member_and_group_context(
        &self,
    ) -> (Vec<minijinja::Value>, Vec<minijinja::Value>) {
        let members = self
            .members
            .iter()
            .map(|(user, role)| {
                minijinja::context! { user_id => user.to_string(), role => format_role(*role) }
            })
            .collect();
        let groups = self
            .groups
            .iter()
            .map(|(group, role)| minijinja::context! { group => group, role => format_role(*role) })
            .collect();
        (members, groups)
    }
}

/// Reads an Area's whole access panel, or [`AccessPanel::hidden`] when the
/// viewer may not manage it.
pub(super) async fn area_panel(
    state: &AppState,
    role: Option<Role>,
    area_id: AreaId,
    can_manage: bool,
) -> Result<AccessPanel, WebError> {
    if !can_manage {
        return Ok(AccessPanel::hidden());
    }
    Ok(AccessPanel {
        can_manage,
        members: anamnesis_app::list_area_members(state.membership.as_ref(), role, area_id).await?,
        groups: list_area_groups(state.group_membership.as_ref(), role, area_id).await?,
        known_groups: list_known_groups(state.group_membership.as_ref(), role).await?,
    })
}

/// The [`area_panel`] sibling for a Project.
pub(super) async fn project_panel(
    state: &AppState,
    role: Option<Role>,
    project_id: ProjectId,
    can_manage: bool,
) -> Result<AccessPanel, WebError> {
    if !can_manage {
        return Ok(AccessPanel::hidden());
    }
    Ok(AccessPanel {
        can_manage,
        members: anamnesis_app::list_project_members(state.membership.as_ref(), role, project_id)
            .await?,
        groups: list_project_groups(state.group_membership.as_ref(), role, project_id).await?,
        // Gated at `Action::ManageArea`, which a Project Admin satisfies —
        // see `anamnesis_app::list_known_groups`.
        known_groups: list_known_groups(state.group_membership.as_ref(), role).await?,
    })
}

// --- Area group mappings ---

pub async fn grant_area_group_handler(
    State(state): State<AppState>,
    user: crate::auth::CurrentUser,
    Path(id): Path<uuid::Uuid>,
    Form(form): Form<GrantAreaGroupRoleForm>,
) -> Response {
    match grant_area_group_impl(&state, &user, AreaId::new(id), form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn grant_area_group_impl(
    state: &AppState,
    user: &crate::auth::CurrentUser,
    area_id: AreaId,
    form: GrantAreaGroupRoleForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let role = parse_grantable_role(&form.role)?;
    let group = form.group.trim();
    let actor_role = access::area_role(state, &user.user_id, area_id).await?;
    grant_area_group_role(
        state.group_membership_write.as_ref(),
        actor_role,
        area_id,
        group,
        role,
    )
    .await?;
    Ok(Redirect::to(&format!("/areas/{area_id}")).into_response())
}

pub async fn revoke_area_group_handler(
    State(state): State<AppState>,
    user: crate::auth::CurrentUser,
    Path(id): Path<uuid::Uuid>,
    Form(form): Form<RevokeAreaGroupRoleForm>,
) -> Response {
    match revoke_area_group_impl(&state, &user, AreaId::new(id), form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn revoke_area_group_impl(
    state: &AppState,
    user: &crate::auth::CurrentUser,
    area_id: AreaId,
    form: RevokeAreaGroupRoleForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let actor_role = access::area_role(state, &user.user_id, area_id).await?;
    revoke_area_group_role(
        state.group_membership_write.as_ref(),
        actor_role,
        area_id,
        form.group.trim(),
    )
    .await?;
    Ok(Redirect::to(&format!("/areas/{area_id}")).into_response())
}

// --- Project group mappings ---

pub async fn grant_project_group_handler(
    State(state): State<AppState>,
    user: crate::auth::CurrentUser,
    Path(id): Path<uuid::Uuid>,
    Form(form): Form<GrantProjectGroupRoleForm>,
) -> Response {
    match grant_project_group_impl(&state, &user, ProjectId::new(id), form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn grant_project_group_impl(
    state: &AppState,
    user: &crate::auth::CurrentUser,
    project_id: ProjectId,
    form: GrantProjectGroupRoleForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let role = parse_grantable_role(&form.role)?;
    let actor_role = project_actor_role(state, user, project_id).await?;
    grant_project_group_role(
        state.group_membership_write.as_ref(),
        actor_role,
        project_id,
        form.group.trim(),
        role,
    )
    .await?;
    Ok(Redirect::to(&format!("/projects/{project_id}")).into_response())
}

pub async fn revoke_project_group_handler(
    State(state): State<AppState>,
    user: crate::auth::CurrentUser,
    Path(id): Path<uuid::Uuid>,
    Form(form): Form<RevokeProjectGroupRoleForm>,
) -> Response {
    match revoke_project_group_impl(&state, &user, ProjectId::new(id), form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn revoke_project_group_impl(
    state: &AppState,
    user: &crate::auth::CurrentUser,
    project_id: ProjectId,
    form: RevokeProjectGroupRoleForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let actor_role = project_actor_role(state, user, project_id).await?;
    revoke_project_group_role(
        state.group_membership_write.as_ref(),
        actor_role,
        project_id,
        form.group.trim(),
    )
    .await?;
    Ok(Redirect::to(&format!("/projects/{project_id}")).into_response())
}

/// A project-scoped role needs the project's Area to resolve the inherited
/// half, and the Area is only reachable through the project itself — the
/// same two-step `crate::handlers::membership`'s project handlers take.
async fn project_actor_role(
    state: &AppState,
    user: &crate::auth::CurrentUser,
    project_id: ProjectId,
) -> Result<Option<Role>, WebError> {
    let area_id = state
        .projects
        .load(project_id)
        .await?
        .ok_or(AppError::NotFound)?
        .project
        .area_id;
    access::project_role(state, &user.user_id, project_id, area_id).await
}

// --- Admin groups (`/users`) ---

pub async fn grant_admin_group_handler(
    State(state): State<AppState>,
    user: crate::auth::CurrentUser,
    Form(form): Form<GrantAdminGroupForm>,
) -> Response {
    match grant_admin_group_impl(&state, &user, form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn grant_admin_group_impl(
    state: &AppState,
    user: &crate::auth::CurrentUser,
    form: GrantAdminGroupForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let role = access::is_system_admin(state, &user.user_id)
        .await?
        .then_some(Role::SystemAdmin);
    grant_admin_group(
        state.group_membership_write.as_ref(),
        role,
        form.group.trim(),
    )
    .await?;
    Ok(Redirect::to("/users").into_response())
}

pub async fn revoke_admin_group_handler(
    State(state): State<AppState>,
    user: crate::auth::CurrentUser,
    Form(form): Form<RevokeAdminGroupForm>,
) -> Response {
    match revoke_admin_group_impl(&state, &user, form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn revoke_admin_group_impl(
    state: &AppState,
    user: &crate::auth::CurrentUser,
    form: RevokeAdminGroupForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let role = access::is_system_admin(state, &user.user_id)
        .await?
        .then_some(Role::SystemAdmin);
    // No `AppError::LastSystemAdmin` arm, unlike
    // `crate::handlers::membership::revoke_system_admin_impl`: the use case
    // cannot raise it here, because an admin-group row is not evidence that
    // any user holds admin.
    revoke_admin_group(
        state.group_membership_write.as_ref(),
        role,
        form.group.trim(),
    )
    .await?;
    Ok(Redirect::to("/users").into_response())
}
