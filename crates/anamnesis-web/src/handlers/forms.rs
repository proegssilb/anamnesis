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

#[derive(Debug, Deserialize)]
pub struct CreateTaskForm {
    pub csrf_token: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct EditTaskForm {
    pub csrf_token: String,
    pub title: String,
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
