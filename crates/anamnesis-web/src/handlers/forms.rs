//! Form bodies for every mutating route.

use serde::Deserialize;

/// Shared by every mutating route that needs nothing but the CSRF token:
/// logging out, running "archive all".
#[derive(Debug, Deserialize)]
pub struct CsrfOnlyForm {
    pub csrf_token: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateAreaForm {
    pub csrf_token: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
}

/// Replaces an area's title and description (`anamnesis_app::edit_area`).
#[derive(Debug, Deserialize)]
pub struct EditAreaForm {
    pub csrf_token: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateProjectForm {
    pub csrf_token: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct TransitionProjectStatusForm {
    pub csrf_token: String,
    /// `"pending"` | `"active"` | `"complete"` — the same text spelling the
    /// SQL adapter stores (`anamnesis_adapters::sql::project_status_to_text`),
    /// so a `<select>` option's value round-trips without translation.
    pub status: String,
}

/// Grants a role on an Area (`crate::handlers::membership`). `role` is
/// `"member"` | `"project_admin"` only — deliberately no `"system_admin"`
/// option exists in the `<select>` this form is posted from, and
/// `crate::handlers::membership::parse_grantable_role` refuses anything else
/// server-side too (belt-and-suspenders on top of `anamnesis_app::
/// grant_area_role`'s own refusal — see that use case's module doc comment
/// on why System Admin can only ever be granted through the dedicated
/// System Admin form).
#[derive(Debug, Deserialize)]
pub struct GrantAreaRoleForm {
    pub csrf_token: String,
    pub user_id: String,
    pub role: String,
}

/// Revokes a user's role on an Area entirely.
#[derive(Debug, Deserialize)]
pub struct RevokeAreaRoleForm {
    pub csrf_token: String,
    pub user_id: String,
}

/// Grants a role on a Project — the [`GrantAreaRoleForm`] sibling.
#[derive(Debug, Deserialize)]
pub struct GrantProjectRoleForm {
    pub csrf_token: String,
    pub user_id: String,
    pub role: String,
}

/// Revokes a user's role on a Project entirely.
#[derive(Debug, Deserialize)]
pub struct RevokeProjectRoleForm {
    pub csrf_token: String,
    pub user_id: String,
}

/// Grants System Admin (`crate::handlers::membership`) — the one
/// System-Admin-only place in the UI that can mint another System Admin.
#[derive(Debug, Deserialize)]
pub struct GrantSystemAdminForm {
    pub csrf_token: String,
    pub user_id: String,
}

/// Revokes System Admin. Refused by `anamnesis_app::revoke_system_admin`
/// when `user_id` names the last remaining System Admin
/// (`AppError::LastSystemAdmin`).
#[derive(Debug, Deserialize)]
pub struct RevokeSystemAdminForm {
    pub csrf_token: String,
    pub user_id: String,
}

/// Defines a new custom field on a project (`docs/DOMAIN.md` §3) — the
/// owner's motivating house-hunting example (price, viewing date, ...) needs
/// this to be reachable from the UI at all, not just by hand-writing SQL.
/// `kind` is the lowercase spelling `crate::handlers::format::format_field_kind`
/// produces and `crate::handlers::field_form::parse_field_kind` consumes.
///
/// `show_on_card` is an HTML checkbox: a browser omits an unchecked box's
/// name from the submitted body entirely (rather than sending `"false"`), so
/// this is a plain `String` (present-and-non-empty when checked, absent when
/// not) rather than a `bool` — `serde`'s `bool` deserializer only accepts the
/// literal tokens `true`/`false`, which a checkbox never sends either way.
/// `crate::handlers::projects::add_field_definition_impl` converts it with
/// `!form.show_on_card.is_empty()`.
#[derive(Debug, Deserialize)]
pub struct AddFieldDefinitionForm {
    pub csrf_token: String,
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub show_on_card: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateTaskForm {
    pub csrf_token: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
}

/// Title and description are edited independently on the task detail page
/// (separate `<details>` blocks so the status pills can sit between them);
/// each handler loads the untouched field itself rather than trusting a
/// hidden input for it, so these forms carry only the field being edited.
#[derive(Debug, Deserialize)]
pub struct EditTaskTitleForm {
    pub csrf_token: String,
    pub title: String,
}

#[derive(Debug, Deserialize)]
pub struct EditTaskDescriptionForm {
    pub csrf_token: String,
    #[serde(default)]
    pub description: String,
}

/// Raises a task onto the board, or moves it to a different column — the
/// same form and the same handler serve both (`raise_task` handles either
/// direction identically; see `docs/DOMAIN.md` §2).
#[derive(Debug, Deserialize)]
pub struct RaiseTaskForm {
    pub csrf_token: String,
    pub column_id: uuid::Uuid,
}

#[derive(Debug, Deserialize)]
pub struct AddCommentForm {
    pub csrf_token: String,
    pub body: String,
}

#[derive(Debug, Deserialize)]
pub struct AddLinkAttachmentForm {
    pub csrf_token: String,
    pub url: String,
}

/// Sets or clears a task's checklist parent. An empty `parent_task_id`
/// clears it — the web-form equivalent of `set_task_parent`'s
/// `Option<TaskId>`, since an HTML form field cannot itself be absent.
#[derive(Debug, Deserialize)]
pub struct SetParentForm {
    pub csrf_token: String,
    #[serde(default)]
    pub parent_task_id: String,
}

/// Quick-adds a checklist item: creates a new task in the same project and
/// sets its parent to the task the form was posted to, in one request
/// (`crate::handlers::tasks::add_checklist_item_impl`) — the no-fuss
/// alternative to hand-carrying an existing task's id into [`SetParentForm`].
#[derive(Debug, Deserialize)]
pub struct AddChecklistItemForm {
    pub csrf_token: String,
    pub title: String,
}

/// Creates a relationship edge from the task the form was submitted on to
/// `to_task_id`, using one of the three built-in kinds (`docs/DOMAIN.md` §3:
/// cross-project edges may only use a built-in kind, and this phase's UI has
/// no project-local custom-kind picker yet).
#[derive(Debug, Deserialize)]
pub struct CreateRelationshipForm {
    pub csrf_token: String,
    pub to_task_id: uuid::Uuid,
    /// `"blocks"` | `"relates_to"` | `"duplicates"`.
    pub kind: String,
}

/// Accepts one suggested task from the board's suggestion prompt, raising it
/// straight onto the entry column (`docs/DOMAIN.md` §5).
#[derive(Debug, Deserialize)]
pub struct AcceptSuggestionForm {
    pub csrf_token: String,
    pub task_id: uuid::Uuid,
}

/// Accepts one suggested tangle from the board's suggestion prompt, placing
/// it on the entry column (`docs/DOMAIN.md`'s Tangle section: "accepting the
/// offer places it").
#[derive(Debug, Deserialize)]
pub struct AcceptTangleForm {
    pub csrf_token: String,
    pub tangle_id: uuid::Uuid,
}

/// Sets a task's value for one of its project's custom fields
/// (`docs/DOMAIN.md` §3). One shape for every [`anamnesis_core::FieldKind`]:
/// `value` is the raw text of whichever single input that kind's edit form
/// renders (a decimal string, an `<input type="date">`'s value, ...);
/// `currency` is used only for `Currency` fields (see
/// `crate::handlers::field_form::parse_field_data`).
#[derive(Debug, Deserialize)]
pub struct SetFieldValueForm {
    pub csrf_token: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub currency: String,
}

/// Replaces the runtime-editable [`anamnesis_app::Settings`] row
/// (`docs/DOMAIN.md` §3) — `crate::handlers::settings`'s one write, System
/// Admin only. `sweep_kind` selects which of `sweep_n`/`sweep_weekday`/
/// `sweep_day` actually apply (`crate::handlers::settings::parse_recurrence`
/// consumes them); the unused ones are sent blank by the `<select>`-driven
/// form but never required to be.
#[derive(Debug, Deserialize)]
pub struct UpdateSettingsForm {
    pub csrf_token: String,
    pub active_project_limit: String,
    pub cooldown_seconds: String,
    pub high_bounce_threshold: String,
    /// `"never"` | `"every_n_weeks"` | `"day_of_month"`.
    pub sweep_kind: String,
    #[serde(default)]
    pub sweep_n: String,
    /// `"monday"`..`"sunday"`, lowercase — the same spelling
    /// `crate::handlers::settings::format_weekday`/`parse_weekday` use.
    #[serde(default)]
    pub sweep_weekday: String,
    #[serde(default)]
    pub sweep_day: String,
}

/// Moves a board card (a task or a placed tangle) to `column_id`/`position`
/// (`docs/DOMAIN.md` §8) — posted either by `static/app.js`'s drag handler
/// (via `htmx.ajax`) or by `_reposition_form.html`'s plain-form fallback,
/// both against the exact same endpoint and field names.
#[derive(Debug, Deserialize)]
pub struct RepositionForm {
    pub csrf_token: String,
    /// `"task"` | `"tangle"`.
    pub item_kind: String,
    pub item_id: uuid::Uuid,
    pub column_id: uuid::Uuid,
    pub position: u32,
}
