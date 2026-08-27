//! Viewing and editing the singleton [`crate::Settings`] row
//! (`docs/DOMAIN.md` §3). Both directions are gated identically —
//! `Action::ManageSystemSettings`, System Admin only, on the read path as
//! well as the write path. Every other read-only use case in this crate
//! (`view_area`, `view_project`, `view_task`, ...) is open to any assigned
//! role via `Action::View*`; settings are the one exception, because they
//! are not scoped to anything a Member could hold a role on — there is no
//! "view your own project's settings," only the one global row, and it
//! carries values (the active-project limit, the sweep schedule) that are
//! meaningless without the authority to change them. So unlike the
//! Area/Project/Task ports, no `Action::ViewSettings` exists at all: reading
//! and writing are the same capability.

use anamnesis_core::policy::Role;

use crate::error::AppError;
use crate::policy::{Action, is_allowed};
use crate::ports::SettingsRepository;
use crate::settings::Settings;

/// Loads the current settings. `Err(AppError::Forbidden)` for anyone but a
/// System Admin.
pub async fn view_settings(
    repo: &dyn SettingsRepository,
    role: Option<Role>,
) -> Result<Settings, AppError> {
    if !is_allowed(role, Action::ManageSystemSettings) {
        return Err(AppError::Forbidden);
    }
    Ok(repo.load().await?)
}

/// Replaces `active_project_limit`, `suggestion`, and `sweep_recurrence`
/// with the given values (never `last_swept_at` — see
/// [`crate::ports::SettingsRepository::update`]'s doc comment). Returns the
/// settings actually stored, reloaded from the repository rather than
/// echoing `new_settings` back verbatim — the caller passes no meaningful
/// `last_swept_at` for a value it never edits, and reloading is what keeps
/// the returned value truthful instead of clobbering the caller's view of
/// that field with a placeholder.
pub async fn update_settings(
    repo: &dyn SettingsRepository,
    role: Option<Role>,
    new_settings: Settings,
) -> Result<Settings, AppError> {
    if !is_allowed(role, Action::ManageSystemSettings) {
        return Err(AppError::Forbidden);
    }
    repo.update(&new_settings).await?;
    Ok(repo.load().await?)
}
