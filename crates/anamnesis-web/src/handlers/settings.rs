//! `GET`/`POST /settings`: the System-Admin-only page for the
//! runtime-editable [`anamnesis_app::Settings`] row (`docs/DOMAIN.md` §3)
//! — the active-project limit, the suggestion engine's cooldown and
//! high-bounce threshold, and the scheduled sweep's recurrence.
//!
//! Both directions delegate straight to `anamnesis_app::view_settings`/
//! `update_settings`, which already gate on `Some(Role::SystemAdmin)` vs.
//! anything else (including `None`) — this handler's only job is resolving
//! whether the caller *is* a System Admin and passing that role in, exactly
//! as `crate::handlers::areas::create_area_handler` does for `ManageArea`.
//! There is deliberately no separate "can view" check here beyond that: a
//! non-admin gets exactly the same `AppError::Forbidden` -> 403 on `GET` as
//! on `POST`.

use axum::Form;
use axum::extract::State;
use axum::response::{Html, IntoResponse, Redirect, Response};
use minijinja::context;

use anamnesis_app::{AppError, Settings, update_settings, view_settings};
use anamnesis_core::policy::Role;
use anamnesis_core::{Recurrence, SuggestionSettings};

use crate::auth::CurrentUser;
use crate::error::WebError;
use crate::session::csrf_tokens_match;
use crate::state::AppState;

use super::access;
use super::forms::UpdateSettingsForm;

fn bad(message: impl Into<String>) -> WebError {
    WebError::BadRequest(message.into())
}

/// `time::Weekday` <-> the lowercase text a `<select>` option's value
/// carries. Kept local to the web layer (not shared with
/// `anamnesis_adapters::sql::weekday_to_text`/`weekday_from_text`, which
/// exist for exactly the same reason one layer down) — nothing about form
/// parsing belongs in the SQL adapter, and nothing about a stored column
/// belongs here.
fn format_weekday(weekday: time::Weekday) -> &'static str {
    use time::Weekday::*;
    match weekday {
        Monday => "monday",
        Tuesday => "tuesday",
        Wednesday => "wednesday",
        Thursday => "thursday",
        Friday => "friday",
        Saturday => "saturday",
        Sunday => "sunday",
    }
}

fn parse_weekday(raw: &str) -> Result<time::Weekday, WebError> {
    use time::Weekday::*;
    match raw {
        "monday" => Ok(Monday),
        "tuesday" => Ok(Tuesday),
        "wednesday" => Ok(Wednesday),
        "thursday" => Ok(Thursday),
        "friday" => Ok(Friday),
        "saturday" => Ok(Saturday),
        "sunday" => Ok(Sunday),
        other => Err(bad(format!("{other:?} is not a day of the week"))),
    }
}

fn parse_u32(raw: &str, field: &str) -> Result<u32, WebError> {
    raw.trim()
        .parse::<u32>()
        .map_err(|_| bad(format!("{field} must be a whole number")))
}

fn parse_recurrence(form: &UpdateSettingsForm) -> Result<Recurrence, WebError> {
    match form.sweep_kind.as_str() {
        "never" => Ok(Recurrence::Never),
        "every_n_weeks" => {
            let n = form
                .sweep_n
                .trim()
                .parse::<u8>()
                .map_err(|_| bad("the recurrence interval must be a whole number of weeks"))?;
            if n == 0 {
                return Err(bad("the recurrence interval must be at least 1 week"));
            }
            let weekday = parse_weekday(form.sweep_weekday.trim())?;
            Ok(Recurrence::EveryNWeeks { n, weekday })
        }
        "day_of_month" => {
            let day = form
                .sweep_day
                .trim()
                .parse::<u8>()
                .map_err(|_| bad("the day of the month must be a whole number"))?;
            if !(1..=31).contains(&day) {
                return Err(bad("the day of the month must be between 1 and 31"));
            }
            Ok(Recurrence::DayOfMonth { day })
        }
        other => Err(bad(format!("{other:?} is not a recognised sweep schedule"))),
    }
}

pub async fn view_settings_handler(State(state): State<AppState>, user: CurrentUser) -> Response {
    match view_settings_impl(&state, &user).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn view_settings_impl(state: &AppState, user: &CurrentUser) -> Result<Response, WebError> {
    let admin = access::is_system_admin(state, &user.user_id).await?;
    let role = admin.then_some(Role::SystemAdmin);
    let settings = view_settings(state.settings.as_ref(), role).await?;
    render_settings_page(state, user, &settings, None)
}

pub async fn update_settings_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Form(form): Form<UpdateSettingsForm>,
) -> Response {
    match update_settings_impl(&state, &user, form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn update_settings_impl(
    state: &AppState,
    user: &CurrentUser,
    form: UpdateSettingsForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let admin = access::is_system_admin(state, &user.user_id).await?;
    let role = admin.then_some(Role::SystemAdmin);

    let new_settings = Settings {
        active_project_limit: parse_u32(&form.active_project_limit, "the active project limit")?,
        suggestion: SuggestionSettings {
            cooldown_seconds: parse_u32(&form.cooldown_seconds, "the suggestion cooldown")?.into(),
            high_bounce_threshold: parse_u32(
                &form.high_bounce_threshold,
                "the high-bounce threshold",
            )?,
        },
        sweep_recurrence: parse_recurrence(&form)?,
        // Ignored by `SettingsRepository::update` (see its doc comment) --
        // there is no meaningful value to send here, since this form never
        // edits it.
        last_swept_at: None,
    };

    match update_settings(state.settings.as_ref(), role, new_settings).await {
        Ok(_) => Ok(Redirect::to("/settings").into_response()),
        Err(AppError::Forbidden) => Err(WebError::App(AppError::Forbidden)),
        Err(other) => Err(WebError::from(other)),
    }
}

fn render_settings_page(
    state: &AppState,
    user: &CurrentUser,
    settings: &Settings,
    error: Option<&str>,
) -> Result<Response, WebError> {
    let (sweep_kind, sweep_n, sweep_weekday, sweep_day) = match settings.sweep_recurrence {
        Recurrence::Never => ("never", None, None, None),
        Recurrence::EveryNWeeks { n, weekday } => (
            "every_n_weeks",
            Some(n),
            Some(format_weekday(weekday)),
            None,
        ),
        Recurrence::DayOfMonth { day } => ("day_of_month", None, None, Some(day)),
    };

    let tmpl = state
        .templates
        .get_template("settings.html")
        .map_err(WebError::template)?;
    let body = tmpl
        .render(context! {
            csrf_token => user.csrf_token,
            current_user => user.display_name,
            is_system_admin => true,
            active_project_limit => settings.active_project_limit,
            cooldown_seconds => settings.suggestion.cooldown_seconds,
            high_bounce_threshold => settings.suggestion.high_bounce_threshold,
            sweep_kind => sweep_kind,
            sweep_n => sweep_n,
            sweep_weekday => sweep_weekday,
            sweep_day => sweep_day,
            last_swept_at => settings.last_swept_at.map(|t| t.unix_seconds()),
            error => error,
        })
        .map_err(WebError::template)?;
    Ok(Html(body).into_response())
}
