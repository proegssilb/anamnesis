//! The task page's "Repeats" section: one `Schedule`, two editors.
//!
//! A typed phrase ("every tue at 6pm until 8am") and a click-through
//! builder of dropdowns edit the same schedule, and the **server is the
//! only interpreter** of either: a change in one posts the whole form to
//! [`preview_schedule_handler`], which builds the `Schedule` (with
//! [`anamnesis_core::parse`] for the phrase, or [`schedule_from_builder`]
//! for the dropdowns) and re-renders the whole section — canonical phrase,
//! dropdowns set to match, the next three dates — or an inline error. There
//! is no client-side parser, so the two editors cannot disagree.
//! [`save_schedule_handler`] stores it; an empty phrase clears it.
//!
//! Without JavaScript the form still posts to the save route, and the
//! phrase field is the no-JS path: the builder's rows only show and hide
//! through the preview round-trip, so the dropdowns alone cannot create a
//! schedule there. (A post whose phrase was left as rendered reads the
//! dropdowns, so visible fields of a saved schedule can still be adjusted.)

use std::collections::HashMap;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use minijinja::context;
use minijinja::value::Serde;
use serde::Serialize;
use time::{Date, Duration, Time, Weekday};

use anamnesis_app::policy::{Action, is_allowed};
use anamnesis_app::{AppError, TimezoneResolver, set_task_schedule, upcoming_due_date};
use anamnesis_core::policy::Role;
use anamnesis_core::{
    Anchor, MonthDay, Nth, ParseError, PopIn, Rule, Schedule, Task, TaskId, Timestamp, WeekdaySet,
    describe, first_start, parse, upcoming,
};

use crate::auth::CurrentUser;
use crate::error::WebError;
use crate::hx::is_hx_request;
use crate::session::csrf_tokens_match;
use crate::state::AppState;

use super::page::render_task_page;
use super::role_for_task;

/// How many upcoming dates the "Next:" line shows.
const PREVIEW_COUNT: usize = 3;

const WEEKDAY_CODES: [(&str, Weekday); 7] = [
    ("mon", Weekday::Monday),
    ("tue", Weekday::Tuesday),
    ("wed", Weekday::Wednesday),
    ("thu", Weekday::Thursday),
    ("fri", Weekday::Friday),
    ("sat", Weekday::Saturday),
    ("sun", Weekday::Sunday),
];

const NTH_CODES: [(&str, Nth); 5] = [
    ("first", Nth::First),
    ("second", Nth::Second),
    ("third", Nth::Third),
    ("fourth", Nth::Fourth),
    ("last", Nth::Last),
];

/// Every field of the Repeats form, as posted and as rendered back. All
/// text, because a half-filled builder has to be echoed exactly as typed.
#[derive(Debug, Clone, Serialize)]
pub(super) struct ScheduleForm {
    csrf_token: String,
    /// `"phrase"` or `"builder"`: which editor the post came from.
    source: String,
    phrase: String,
    /// The canonical phrase this form was rendered with. A post whose
    /// `phrase` still equals it was not edited in the phrase box, so (with
    /// no JavaScript to say otherwise) the dropdowns are what changed.
    original_phrase: String,
    every_n: String,
    /// `"days"`, `"weeks"` or `"months"`.
    unit: String,
    /// `"calendar"` or `"after_done"`.
    anchor: String,
    /// `"mon"`..`"sun"`.
    weekdays: Vec<String>,
    /// `"day"`, `"last_day"` or `"nth"`.
    month_mode: String,
    month_day: String,
    /// `"first"`..`"fourth"` or `"last"`.
    nth: String,
    nth_weekday: String,
    pop_in: bool,
    pop_in_hour: String,
    pop_in_minute: String,
    pop_in_ampm: String,
    until: bool,
    until_hour: String,
    until_minute: String,
    until_ampm: String,
}

impl Default for ScheduleForm {
    fn default() -> Self {
        let text = |s: &str| s.to_owned();
        Self {
            csrf_token: String::new(),
            source: text("phrase"),
            phrase: String::new(),
            original_phrase: String::new(),
            every_n: text("1"),
            unit: text("weeks"),
            anchor: text("calendar"),
            weekdays: Vec::new(),
            month_mode: text("day"),
            month_day: text("1"),
            nth: text("first"),
            nth_weekday: text("mon"),
            pop_in: false,
            pop_in_hour: text("6"),
            pop_in_minute: text("0"),
            pop_in_ampm: text("pm"),
            until: false,
            until_hour: text("8"),
            until_minute: text("0"),
            until_ampm: text("am"),
        }
    }
}

impl ScheduleForm {
    /// Reads a urlencoded body by hand: `weekdays` repeats, which axum's
    /// `Form` cannot collect, and absent fields take the defaults a fresh
    /// form shows.
    fn from_body(body: &[u8]) -> Self {
        let mut values: HashMap<String, String> = HashMap::new();
        let mut weekdays = Vec::new();
        for (key, value) in form_urlencoded::parse(body) {
            if key == "weekdays" {
                weekdays.push(value.into_owned());
            } else {
                values.insert(key.into_owned(), value.into_owned());
            }
        }
        let defaults = Self::default();
        let (pop_in, until) = (values.contains_key("pop_in"), values.contains_key("until"));
        let mut text =
            |key: &str, default: &str| values.remove(key).unwrap_or_else(|| default.to_owned());
        Self {
            csrf_token: text("csrf_token", ""),
            source: text("source", &defaults.source),
            phrase: text("phrase", ""),
            original_phrase: text("original_phrase", ""),
            every_n: text("every_n", &defaults.every_n),
            unit: text("unit", &defaults.unit),
            anchor: text("anchor", &defaults.anchor),
            weekdays,
            month_mode: text("month_mode", &defaults.month_mode),
            month_day: text("month_day", &defaults.month_day),
            nth: text("nth", &defaults.nth),
            nth_weekday: text("nth_weekday", &defaults.nth_weekday),
            pop_in,
            pop_in_hour: text("pop_in_hour", &defaults.pop_in_hour),
            pop_in_minute: text("pop_in_minute", &defaults.pop_in_minute),
            pop_in_ampm: text("pop_in_ampm", &defaults.pop_in_ampm),
            until,
            until_hour: text("until_hour", &defaults.until_hour),
            until_minute: text("until_minute", &defaults.until_minute),
            until_ampm: text("until_ampm", &defaults.until_ampm),
        }
    }

    /// The form that shows `schedule`: canonical phrase in the text box and
    /// every dropdown set to match.
    fn from_schedule(schedule: &Schedule) -> Self {
        let phrase = describe(schedule);
        let mut form = Self {
            original_phrase: phrase.clone(),
            phrase,
            anchor: match schedule.anchor {
                Anchor::Calendar { .. } => "calendar",
                Anchor::AfterCompletion { .. } => "after_done",
            }
            .to_owned(),
            ..Self::default()
        };
        form.apply_rule(&schedule.rule);
        form.apply_pop_in(schedule.pop_in);
        form
    }

    fn apply_rule(&mut self, rule: &Rule) {
        match *rule {
            Rule::Days { n } => {
                self.unit = "days".to_owned();
                self.every_n = n.to_string();
            }
            Rule::Weeks { n, on } => {
                self.unit = "weeks".to_owned();
                self.every_n = n.to_string();
                self.weekdays = on
                    .iter()
                    .map(|day| code_of(&WEEKDAY_CODES, day).to_owned())
                    .collect();
            }
            Rule::Months { n, on } => {
                self.unit = "months".to_owned();
                self.every_n = n.to_string();
                self.apply_month_day(on);
            }
        }
    }

    fn apply_month_day(&mut self, on: MonthDay) {
        match on {
            MonthDay::Day(day) => {
                self.month_mode = "day".to_owned();
                self.month_day = day.to_string();
            }
            MonthDay::LastDay => self.month_mode = "last_day".to_owned(),
            MonthDay::Nth { nth, weekday } => {
                self.month_mode = "nth".to_owned();
                self.nth = code_of(&NTH_CODES, nth).to_owned();
                self.nth_weekday = code_of(&WEEKDAY_CODES, weekday).to_owned();
            }
        }
    }

    fn apply_pop_in(&mut self, pop_in: Option<PopIn>) {
        let Some(pop_in) = pop_in else { return };
        self.pop_in = true;
        (self.pop_in_hour, self.pop_in_minute, self.pop_in_ampm) = clock_parts(pop_in.at);
        if let Some(until) = pop_in.until {
            self.until = true;
            (self.until_hour, self.until_minute, self.until_ampm) = clock_parts(until);
        }
    }
}

/// A time as the three dropdowns show it: 12-hour hour, minute, am/pm.
fn clock_parts(time: Time) -> (String, String, String) {
    let hour12 = if time.hour().is_multiple_of(12) {
        12
    } else {
        time.hour() % 12
    };
    let ampm = if time.hour() < 12 { "am" } else { "pm" };
    (
        hour12.to_string(),
        time.minute().to_string(),
        ampm.to_owned(),
    )
}

/// The code a dropdown uses for `value`.
fn code_of<T: Copy + PartialEq>(table: &[(&'static str, T)], value: T) -> &'static str {
    table
        .iter()
        .find(|(_, candidate)| *candidate == value)
        .map_or("", |(code, _)| code)
}

/// The value a dropdown `code` stands for.
fn value_of<T: Copy>(table: &[(&str, T)], code: &str, what: &str) -> Result<T, ParseError> {
    table
        .iter()
        .find(|(candidate, _)| *candidate == code)
        .map(|(_, value)| *value)
        .ok_or_else(|| builder_error(&format!("pick {what}")))
}

/// An error from the dropdowns, which have no single offending word.
fn builder_error(hint: &str) -> ParseError {
    ParseError {
        word: None,
        hint: hint.to_owned(),
    }
}

fn whole_number(text: &str, what: &str) -> Result<u16, ParseError> {
    text.trim()
        .parse()
        .map_err(|_| builder_error(&format!("{what} must be a whole number")))
}

/// Builds the schedule the dropdowns describe. `today` supplies what the
/// dropdowns leave out, exactly as it does for a typed phrase.
pub(super) fn schedule_from_builder(
    form: &ScheduleForm,
    today: Date,
) -> Result<Schedule, ParseError> {
    let rule = rule_from_builder(form)?;
    let anchor = if form.anchor == "after_done" {
        Anchor::AfterCompletion { set_on: today }
    } else {
        Anchor::Calendar {
            start: first_start(&rule, today),
        }
    };
    let schedule = Schedule {
        rule,
        anchor,
        pop_in: pop_in_from_builder(form)?,
    };
    schedule
        .validate()
        .map_err(|e| builder_error(&e.to_string()))?;
    Ok(schedule)
}

/// The repeat interval and the days it lands on. After-done schedules have
/// no days: the cadence is counted from the completion.
fn rule_from_builder(form: &ScheduleForm) -> Result<Rule, ParseError> {
    let n = whole_number(&form.every_n, "how often")?;
    let after_done = form.anchor == "after_done";
    match form.unit.as_str() {
        "days" => Ok(Rule::Days { n }),
        "weeks" => {
            let n = u8::try_from(n).map_err(|_| builder_error("every can be 255 at most"))?;
            // `on` is ignored for after-done: the grammar's constant, no days.
            let on = if after_done {
                WeekdaySet::new()
            } else {
                weekdays_from_builder(form)?
            };
            Ok(Rule::Weeks { n, on })
        }
        "months" => {
            let n = u8::try_from(n).map_err(|_| builder_error("every can be 255 at most"))?;
            // `on` is ignored for after-done (counted from the completion),
            // so it is the same constant the grammar uses, never today's day:
            // otherwise re-saving an unchanged schedule would differ.
            let on = if after_done {
                MonthDay::Day(1)
            } else {
                month_day_from_builder(form)?
            };
            Ok(Rule::Months { n, on })
        }
        _ => Err(builder_error("pick days, weeks or months")),
    }
}

fn weekdays_from_builder(form: &ScheduleForm) -> Result<WeekdaySet, ParseError> {
    form.weekdays
        .iter()
        .try_fold(WeekdaySet::new(), |set, code| {
            Ok(set.with(value_of(&WEEKDAY_CODES, code, "a day of the week")?))
        })
}

fn month_day_from_builder(form: &ScheduleForm) -> Result<MonthDay, ParseError> {
    match form.month_mode.as_str() {
        "last_day" => Ok(MonthDay::LastDay),
        "nth" => Ok(MonthDay::Nth {
            nth: value_of(&NTH_CODES, &form.nth, "which one of the month")?,
            weekday: value_of(&WEEKDAY_CODES, &form.nth_weekday, "a day of the week")?,
        }),
        _ => {
            let day = whole_number(&form.month_day, "the day of the month")?;
            let day = u8::try_from(day).map_err(|_| builder_error("pick a day from 1 to 31"))?;
            Ok(MonthDay::Day(day))
        }
    }
}

/// The time-of-day window, if the form asks for one.
fn pop_in_from_builder(form: &ScheduleForm) -> Result<Option<PopIn>, ParseError> {
    if !form.pop_in {
        return Ok(None);
    }
    let at = time_from_builder(&form.pop_in_hour, &form.pop_in_minute, &form.pop_in_ampm)?;
    let until = if form.until {
        Some(time_from_builder(
            &form.until_hour,
            &form.until_minute,
            &form.until_ampm,
        )?)
    } else {
        None
    };
    Ok(Some(PopIn { at, until }))
}

fn time_from_builder(hour: &str, minute: &str, ampm: &str) -> Result<Time, ParseError> {
    let hour = u8::try_from(whole_number(hour, "the hour")?)
        .ok()
        .filter(|h| (1..=12).contains(h))
        .ok_or_else(|| builder_error("pick an hour from 1 to 12"))?;
    let minute = u8::try_from(whole_number(minute, "the minute")?)
        .map_err(|_| builder_error("pick a minute from 0 to 59"))?;
    let hour24 = hour % 12 + if ampm == "pm" { 12 } else { 0 };
    Time::from_hms(hour24, minute, 0).map_err(|_| builder_error("pick a minute from 0 to 59"))
}

/// The schedule the form asks for: `None` for an empty phrase (clear it).
///
/// The builder is read when it was the editor that changed — `source` says
/// so — or, without JavaScript, when the phrase box was left as rendered.
/// Otherwise the phrase wins.
fn schedule_from_form(
    form: &ScheduleForm,
    today: Date,
    stored: Option<&Schedule>,
) -> Result<Option<Schedule>, ParseError> {
    let phrase_untouched = !form.original_phrase.is_empty() && form.phrase == form.original_phrase;
    let schedule = if form.source == "builder" || phrase_untouched {
        schedule_from_builder(form, today)?
    } else if form.phrase.trim().is_empty() {
        return Ok(None);
    } else {
        parse(&form.phrase, today)?
    };
    Ok(Some(keep_stored_anchor(schedule, stored)))
}

/// Saving a schedule that has not changed must not move it: the start date
/// phases a multi-week or multi-month cadence, and `set_on` is the due date
/// of an after-done chore nobody has finished yet.
fn keep_stored_anchor(mut schedule: Schedule, stored: Option<&Schedule>) -> Schedule {
    let Some(stored) = stored else {
        return schedule;
    };
    let same_kind = matches!(
        (stored.anchor, schedule.anchor),
        (Anchor::Calendar { .. }, Anchor::Calendar { .. })
            | (
                Anchor::AfterCompletion { .. },
                Anchor::AfterCompletion { .. }
            )
    );
    if same_kind && stored.rule == schedule.rule {
        schedule.anchor = stored.anchor;
    }
    schedule
}

/// Everything `_schedule_editor.html` renders.
#[derive(Debug, Serialize)]
pub(super) struct EditorView {
    task_id: String,
    csrf_token: String,
    form: ScheduleForm,
    /// Inline error under the phrase box.
    error: Option<String>,
    /// "Tue Nov 17 6:00pm", up to three of them.
    next: Vec<String>,
    /// The canonical phrase for what the form currently says, shown beside
    /// (not in) the phrase box while the user is typing in it.
    reads_as: Option<String>,
    saved: bool,
    at_minutes: Vec<u8>,
    until_minutes: Vec<u8>,
}

impl EditorView {
    fn new(
        user: &CurrentUser,
        task_id: TaskId,
        mut form: ScheduleForm,
        error: Option<String>,
        next: Vec<String>,
    ) -> Self {
        form.csrf_token.clone_from(&user.csrf_token);
        Self {
            task_id: task_id.to_string(),
            csrf_token: user.csrf_token.clone(),
            at_minutes: minute_options(&form.pop_in_minute),
            until_minutes: minute_options(&form.until_minute),
            form,
            error,
            next,
            reads_as: None,
            saved: false,
        }
    }
}

/// Five-minute steps, plus the minute a typed phrase asked for if it is not
/// on that grid ("9:07am") — so the dropdown can always show what is stored.
fn minute_options(current: &str) -> Vec<u8> {
    let mut minutes: Vec<u8> = (0..60).step_by(5).collect();
    if let Ok(minute) = current.trim().parse::<u8>()
        && minute < 60
        && !minutes.contains(&minute)
    {
        minutes.push(minute);
        minutes.sort_unstable();
    }
    minutes
}

/// `Tue Nov 17 6:00pm`; an interval chore with no pop-in time is just
/// `Tue Nov 17`.
fn format_occurrence(date: Date, at: Option<Time>) -> String {
    let day = format!(
        "{} {} {}",
        &date.weekday().to_string()[..3],
        &date.month().to_string()[..3],
        date.day()
    );
    match at {
        Some(time) => {
            let (hour, minute, ampm) = clock_parts(time);
            format!("{day} {hour}:{minute:0>2}{ampm}")
        }
        None => day,
    }
}

/// The dates `schedule` comes due, starting with `first` (itself an
/// occurrence: the date it will next fire on).
fn preview_dates(schedule: &Schedule, first: Date, last_completed: Option<Date>) -> Vec<String> {
    let at = schedule.pop_in.map(|p| p.at);
    upcoming(
        schedule,
        first - Duration::DAY,
        last_completed,
        PREVIEW_COUNT,
    )
    .into_iter()
    .map(|date| format_occurrence(date, at))
    .collect()
}

/// The preview for `schedule` saved at `now`, starting from the date
/// `set_task_schedule` would make it next due on — never tonight's pop-in
/// once it has passed.
fn preview_from_now(
    schedule: &Schedule,
    now: Timestamp,
    last_completed: Option<Date>,
    tz: &dyn TimezoneResolver,
    zone: &str,
) -> Result<Vec<String>, AppError> {
    let first = upcoming_due_date(schedule, now, last_completed, tz, zone)?;
    Ok(preview_dates(schedule, first, last_completed))
}

/// The "Next:" dates for `schedule` on `task`. The task's own schedule
/// starts from its stored `next_due_at` (which an unchanged save keeps);
/// anything else, or a paused one, from the first pop-in after now.
fn preview_for_task(
    state: &AppState,
    task: &Task,
    schedule: &Schedule,
) -> Result<Vec<String>, WebError> {
    let last = last_completed_date(state, task)?;
    let stored_next = task
        .recurrence
        .as_ref()
        .filter(|r| r.schedule == *schedule)
        .and_then(|r| r.next_due_at);
    let zone = &state.timezone_name;
    if let Some(next) = stored_next {
        let first = state.timezone.local_date(zone, next)?;
        return Ok(preview_dates(schedule, first, last));
    }
    let now = state.clock.now();
    Ok(preview_from_now(
        schedule,
        now,
        last,
        state.timezone.as_ref(),
        zone,
    )?)
}

fn local_today(state: &AppState) -> Result<Date, WebError> {
    let now = state.clock.now();
    Ok(state.timezone.local_date(&state.timezone_name, now)?)
}

/// The local date a task was last completed, which an after-done schedule
/// counts from.
fn last_completed_date(state: &AppState, task: &Task) -> Result<Option<Date>, WebError> {
    let Some(done) = task.recurrence.as_ref().and_then(|r| r.last_completed_at) else {
        return Ok(None);
    };
    Ok(Some(state.timezone.local_date(&state.timezone_name, done)?))
}

/// The editor as it stands for a task's stored schedule — what the task page
/// and a successful save show.
pub(super) fn editor_for_task(
    state: &AppState,
    user: &CurrentUser,
    task: &Task,
) -> Result<EditorView, WebError> {
    let (form, next) = match task.recurrence.as_ref() {
        Some(recurrence) => (
            ScheduleForm::from_schedule(&recurrence.schedule),
            preview_for_task(state, task, &recurrence.schedule)?,
        ),
        None => (ScheduleForm::default(), Vec::new()),
    };
    Ok(EditorView::new(user, task.id, form, None, next))
}

/// Checks the CSRF token and that the caller may edit the task, then loads
/// it — the shared opening of both handlers.
async fn authorize(
    state: &AppState,
    user: &CurrentUser,
    task_id: TaskId,
    form: &ScheduleForm,
) -> Result<(Option<Role>, Task), WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let (_, role) = role_for_task(state, &user.user_id, task_id).await?;
    if !is_allowed(role, Action::EditTask) {
        return Err(AppError::Forbidden.into());
    }
    let aggregate = state.tasks.load(task_id).await?.ok_or(AppError::NotFound)?;
    Ok((role, aggregate.task))
}

fn render_editor(state: &AppState, view: &EditorView) -> Result<Response, WebError> {
    let tmpl = state
        .templates
        .get_template("_schedule_editor.html")
        .map_err(WebError::template)?;
    let body = tmpl
        .render(context! { schedule_editor => Serde(view) })
        .map_err(WebError::template)?;
    Ok(Html(body).into_response())
}

/// `POST /tasks/{id}/schedule/preview`: re-renders the section for whatever
/// the form now says, without saving anything.
pub async fn preview_schedule_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
    body: Bytes,
) -> Response {
    match preview_impl(&state, &user, TaskId::new(id), &body).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn preview_impl(
    state: &AppState,
    user: &CurrentUser,
    task_id: TaskId,
    body: &[u8],
) -> Result<Response, WebError> {
    let form = ScheduleForm::from_body(body);
    let (_, task) = authorize(state, user, task_id, &form).await?;
    let today = local_today(state)?;
    let stored = task.recurrence.as_ref().map(|r| &r.schedule);
    let view = match schedule_from_form(&form, today, stored) {
        Ok(Some(schedule)) => {
            let next = preview_for_task(state, &task, &schedule)?;
            let mut shown = ScheduleForm::from_schedule(&schedule);
            let reads_as = shown.phrase.clone();
            // Never rewrite what the user is typing: the canonical reading
            // goes in its own line, and replaces the text only when the
            // builder drove the change.
            if form.source != "builder" {
                shown.phrase.clone_from(&form.phrase);
            }
            let mut view = EditorView::new(user, task_id, shown, None, next);
            view.reads_as = Some(reads_as);
            view
        }
        Ok(None) => EditorView::new(user, task_id, form, None, Vec::new()),
        Err(err) => EditorView::new(user, task_id, form, Some(err.to_string()), Vec::new()),
    };
    render_editor(state, &view)
}

/// `POST /tasks/{id}/schedule`: saves the schedule (or clears it, for an
/// empty phrase). A plain form post redirects back to the task page; an
/// htmx post gets the section back.
pub async fn save_schedule_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let hx = is_hx_request(&headers);
    match save_impl(&state, &user, TaskId::new(id), &body, hx).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn save_impl(
    state: &AppState,
    user: &CurrentUser,
    task_id: TaskId,
    body: &[u8],
    hx: bool,
) -> Result<Response, WebError> {
    let form = ScheduleForm::from_body(body);
    let (role, task) = authorize(state, user, task_id, &form).await?;
    let today = local_today(state)?;
    let stored = task.recurrence.as_ref().map(|r| &r.schedule);
    let schedule = match schedule_from_form(&form, today, stored) {
        Ok(schedule) => schedule,
        Err(err) => return refuse_schedule(state, user, &task, form, &err, hx).await,
    };
    let saved = set_task_schedule(
        state.tasks.as_ref(),
        state.clock.as_ref(),
        state.timezone.as_ref(),
        &state.timezone_name,
        role,
        task_id,
        schedule,
    )
    .await?;
    if !hx {
        return Ok(Redirect::to(&format!("/tasks/{task_id}")).into_response());
    }
    let mut view = editor_for_task(state, user, &saved)?;
    view.saved = true;
    render_editor(state, &view)
}

/// A schedule that could not be built: the section with its inline error for
/// htmx, the whole task page with a `422` otherwise.
async fn refuse_schedule(
    state: &AppState,
    user: &CurrentUser,
    task: &Task,
    form: ScheduleForm,
    err: &ParseError,
    hx: bool,
) -> Result<Response, WebError> {
    if hx {
        let view = EditorView::new(user, task.id, form, Some(err.to_string()), Vec::new());
        return render_editor(state, &view);
    }
    render_task_page(
        state,
        user,
        task.id,
        task,
        Some(&err.to_string()),
        None,
        None,
        StatusCode::UNPROCESSABLE_ENTITY,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    const FRIDAY: Date = date!(2026 - 11 - 13);
    const LATER: Date = date!(2026 - 11 - 26);

    fn after_done_months() -> ScheduleForm {
        ScheduleForm {
            source: "builder".to_owned(),
            unit: "months".to_owned(),
            anchor: "after_done".to_owned(),
            ..ScheduleForm::default()
        }
    }

    #[test]
    fn builder_after_done_rule_does_not_depend_on_today() {
        let form = after_done_months();
        let first = schedule_from_builder(&form, FRIDAY).unwrap();
        let later = schedule_from_builder(&form, LATER).unwrap();
        assert_eq!(first.rule, later.rule);
    }

    #[test]
    fn re_saving_an_unchanged_after_done_monthly_keeps_the_stored_schedule() {
        let stored = parse("every month after done", FRIDAY).unwrap();
        let phrase = phrase_form("every month after done");
        let from_phrase = schedule_from_form(&phrase, LATER, Some(&stored)).unwrap();
        assert_eq!(from_phrase, Some(stored));
        let from_builder = schedule_from_form(&after_done_months(), LATER, Some(&stored)).unwrap();
        assert_eq!(from_builder, Some(stored));
    }

    /// `date` at `hh:00` UTC.
    fn utc(date: Date, hour: u8) -> anamnesis_core::Timestamp {
        let at = date.with_hms(hour, 0, 0).unwrap().assume_utc();
        anamnesis_core::Timestamp::from_unix_seconds(at.unix_timestamp()).unwrap()
    }

    #[test]
    fn preview_after_tonights_pop_in_starts_next_week() {
        let tuesday = date!(2026 - 11 - 17);
        let schedule = parse("every tue at 6pm", tuesday).unwrap();
        let tz = anamnesis_adapters::TzTimezoneResolver::new();
        let evening = preview_from_now(&schedule, utc(tuesday, 20), None, &tz, "UTC").unwrap();
        assert_eq!(
            evening,
            ["Tue Nov 24 6:00pm", "Tue Dec 1 6:00pm", "Tue Dec 8 6:00pm"]
        );
        let morning = preview_from_now(&schedule, utc(tuesday, 9), None, &tz, "UTC").unwrap();
        assert_eq!(morning[0], "Tue Nov 17 6:00pm");
    }

    #[test]
    fn a_saved_schedules_preview_starts_at_its_stored_next_due_date() {
        let schedule = parse("every tue at 6pm", date!(2026 - 11 - 17)).unwrap();
        let next = preview_dates(&schedule, date!(2026 - 12 - 1), None);
        assert_eq!(next[0], "Tue Dec 1 6:00pm");
    }

    fn phrase_form(phrase: &str) -> ScheduleForm {
        ScheduleForm {
            phrase: phrase.to_owned(),
            ..ScheduleForm::default()
        }
    }
}
