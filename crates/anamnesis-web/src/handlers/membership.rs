//! Granting and revoking roles through the UI — the web layer's thin
//! transport over `anamnesis_app::use_cases::membership` (`docs/DOMAIN.md`
//! §3, §12). Three surfaces:
//!
//! - An Area's "members" section (`view_area_handler`'s page), visible only
//!   to whoever already administers that Area.
//! - A Project's "members" section (`view_project_handler`'s page),
//!   symmetric, visible only to whoever already administers that Project.
//! - `/users`: the one System-Admin-only place in the whole UI that can
//!   grant (or revoke) System Admin itself.
//!
//! Every handler here resolves the actor's role exactly the way every other
//! mutating handler in this crate does (`crate::handlers::access`, scoped to
//! the *specific* Area or Project being acted on) and hands it to the use
//! case unchanged — the use case is what actually refuses an escalation
//! attempt (`anamnesis_app::use_cases::membership`'s module doc comment);
//! this layer only ever adds the ordinary web furniture (CSRF check, form
//! parsing, the 303 redirect back to the page the form was submitted from).

use axum::Form;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use minijinja::context;

use anamnesis_app::{
    AppError, grant_area_role, grant_project_role, grant_system_admin, list_admin_groups,
    list_known_groups, list_system_admins, revoke_area_role, revoke_project_role,
    revoke_system_admin,
};
use anamnesis_core::policy::Role;
use anamnesis_core::{AreaId, ProjectId, UserId};

use crate::auth::CurrentUser;
use crate::error::WebError;
use crate::session::csrf_tokens_match;
use crate::state::AppState;

use super::access;
use super::forms::{
    GrantAreaRoleForm, GrantProjectRoleForm, GrantSystemAdminForm, RevokeAreaRoleForm,
    RevokeProjectRoleForm, RevokeSystemAdminForm,
};

/// `Role` -> the text an Area/Project "members" section renders next to a
/// name (`crate::handlers::areas`/`crate::handlers::projects`, which render
/// the same list this module's use cases produce). Kept here, next to
/// [`parse_grantable_role`], as the one place that owns the UI's role
/// vocabulary.
pub(super) fn format_role(role: Role) -> &'static str {
    match role {
        Role::SystemAdmin => "System Admin",
        Role::ProjectAdmin => "Project Admin",
        Role::Member => "Member",
    }
}

/// `"member"` | `"project_admin"` only — deliberately narrower than
/// `anamnesis_core::policy::Role` itself. There is no way to spell
/// `Role::SystemAdmin` through this parser at all, which is the transport
/// layer's own belt-and-suspenders on top of `grant_area_role`/
/// `grant_project_role`'s independent refusal (see that module's doc
/// comment) — even a hand-crafted POST with `role=system_admin` never
/// reaches the use case carrying that value.
pub(super) fn parse_grantable_role(raw: &str) -> Result<Role, WebError> {
    match raw {
        "member" => Ok(Role::Member),
        "project_admin" => Ok(Role::ProjectAdmin),
        other => Err(WebError::BadRequest(format!(
            "{other:?} is not a grantable role"
        ))),
    }
}

// --- Area members ---

pub async fn grant_area_member_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
    Form(form): Form<GrantAreaRoleForm>,
) -> Response {
    match grant_area_member_impl(&state, &user, AreaId::new(id), form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn grant_area_member_impl(
    state: &AppState,
    user: &CurrentUser,
    area_id: AreaId,
    form: GrantAreaRoleForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let role = parse_grantable_role(&form.role)?;
    let target = UserId::new(form.user_id.trim());
    let actor_role = access::area_role(state, &user.user_id, area_id).await?;
    grant_area_role(
        state.membership_write.as_ref(),
        actor_role,
        area_id,
        &target,
        role,
    )
    .await?;
    Ok(Redirect::to(&format!("/areas/{area_id}")).into_response())
}

pub async fn revoke_area_member_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
    Form(form): Form<RevokeAreaRoleForm>,
) -> Response {
    match revoke_area_member_impl(&state, &user, AreaId::new(id), form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn revoke_area_member_impl(
    state: &AppState,
    user: &CurrentUser,
    area_id: AreaId,
    form: RevokeAreaRoleForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let target = UserId::new(form.user_id.trim());
    let actor_role = access::area_role(state, &user.user_id, area_id).await?;
    revoke_area_role(
        state.membership_write.as_ref(),
        actor_role,
        area_id,
        &target,
    )
    .await?;
    Ok(Redirect::to(&format!("/areas/{area_id}")).into_response())
}

// --- Project members ---

pub async fn grant_project_member_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
    Form(form): Form<GrantProjectRoleForm>,
) -> Response {
    match grant_project_member_impl(&state, &user, ProjectId::new(id), form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn grant_project_member_impl(
    state: &AppState,
    user: &CurrentUser,
    project_id: ProjectId,
    form: GrantProjectRoleForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let role = parse_grantable_role(&form.role)?;
    let target = UserId::new(form.user_id.trim());
    let area_id = state
        .projects
        .load(project_id)
        .await?
        .ok_or(AppError::NotFound)?
        .project
        .area_id;
    let actor_role = access::project_role(state, &user.user_id, project_id, area_id).await?;
    grant_project_role(
        state.membership_write.as_ref(),
        actor_role,
        project_id,
        &target,
        role,
    )
    .await?;
    Ok(Redirect::to(&format!("/projects/{project_id}")).into_response())
}

pub async fn revoke_project_member_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
    Form(form): Form<RevokeProjectRoleForm>,
) -> Response {
    match revoke_project_member_impl(&state, &user, ProjectId::new(id), form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn revoke_project_member_impl(
    state: &AppState,
    user: &CurrentUser,
    project_id: ProjectId,
    form: RevokeProjectRoleForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let target = UserId::new(form.user_id.trim());
    let area_id = state
        .projects
        .load(project_id)
        .await?
        .ok_or(AppError::NotFound)?
        .project
        .area_id;
    let actor_role = access::project_role(state, &user.user_id, project_id, area_id).await?;
    revoke_project_role(
        state.membership_write.as_ref(),
        actor_role,
        project_id,
        &target,
    )
    .await?;
    Ok(Redirect::to(&format!("/projects/{project_id}")).into_response())
}

// --- System Admins (`/users`) ---

pub async fn view_users_handler(State(state): State<AppState>, user: CurrentUser) -> Response {
    match view_users_impl(&state, &user, None, StatusCode::OK).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

/// Everything `/users` lists: the System Admins themselves, the groups
/// mapped to System Admin, and every group name the deployment has ever
/// seen (the picker behind the group input). One value rather than three
/// more parameters, for the same reason
/// [`crate::handlers::group_membership::AccessPanel`] is one — they are read
/// under one gate, rendered as one page, and never used apart.
struct UsersPage {
    admins: Vec<UserId>,
    admin_groups: Vec<String>,
    known_groups: Vec<String>,
}

impl UsersPage {
    /// Whether to render the admin-groups half of the page. Derived from
    /// data, not configuration — see
    /// `crate::handlers::group_membership`'s module doc comment.
    fn show_groups(&self) -> bool {
        !self.known_groups.is_empty()
    }
}

async fn view_users_impl(
    state: &AppState,
    user: &CurrentUser,
    error: Option<&str>,
    status: StatusCode,
) -> Result<Response, WebError> {
    let admin = access::is_system_admin(state, &user.user_id).await?;
    let role = admin.then_some(Role::SystemAdmin);
    let page = UsersPage {
        admins: list_system_admins(state.membership.as_ref(), role).await?,
        admin_groups: list_admin_groups(state.group_membership.as_ref(), role).await?,
        known_groups: list_known_groups(state.group_membership.as_ref(), role).await?,
    };
    render_users_page(state, user, &page, error, status)
}

pub async fn grant_system_admin_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Form(form): Form<GrantSystemAdminForm>,
) -> Response {
    match grant_system_admin_impl(&state, &user, form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn grant_system_admin_impl(
    state: &AppState,
    user: &CurrentUser,
    form: GrantSystemAdminForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let admin = access::is_system_admin(state, &user.user_id).await?;
    let role = admin.then_some(Role::SystemAdmin);
    let target = UserId::new(form.user_id.trim());
    grant_system_admin(state.membership_write.as_ref(), role, &target).await?;
    Ok(Redirect::to("/users").into_response())
}

pub async fn revoke_system_admin_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Form(form): Form<RevokeSystemAdminForm>,
) -> Response {
    match revoke_system_admin_impl(&state, &user, form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn revoke_system_admin_impl(
    state: &AppState,
    user: &CurrentUser,
    form: RevokeSystemAdminForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let admin = access::is_system_admin(state, &user.user_id).await?;
    let role = admin.then_some(Role::SystemAdmin);
    let target = UserId::new(form.user_id.trim());
    match revoke_system_admin(
        state.membership.as_ref(),
        state.membership_write.as_ref(),
        role,
        &target,
    )
    .await
    {
        Ok(_) => Ok(Redirect::to("/users").into_response()),
        Err(AppError::LastSystemAdmin) => {
            view_users_impl(
                state,
                user,
                Some("That is the last System Admin — grant someone else System Admin first."),
                StatusCode::UNPROCESSABLE_ENTITY,
            )
            .await
        }
        Err(other) => Err(WebError::from(other)),
    }
}

fn render_users_page(
    state: &AppState,
    user: &CurrentUser,
    page: &UsersPage,
    error: Option<&str>,
    status: StatusCode,
) -> Result<Response, WebError> {
    let admin_names: Vec<String> = page.admins.iter().map(|u| u.to_string()).collect();
    let tmpl = state
        .templates
        .get_template("users.html")
        .map_err(WebError::template)?;
    let body = tmpl
        .render(context! {
            admins => admin_names,
            admin_groups => page.admin_groups,
            known_groups => page.known_groups,
            show_groups => page.show_groups(),
            csrf_token => user.csrf_token,
            current_user => user.display_name,
            is_system_admin => true,
            error => error,
        })
        .map_err(WebError::template)?;
    Ok((status, Html(body)).into_response())
}
