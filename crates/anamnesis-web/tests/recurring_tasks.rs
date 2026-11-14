//! `tower::ServiceExt::oneshot` coverage for the task page's "Repeats"
//! editor: a typed phrase and a dropdown builder for the same `Schedule`,
//! with the server as the only interpreter of either — plus the card's
//! repeat glyph and the "due again" suggestion copy.

mod support;

use axum::http::StatusCode;
use time::{Date, Duration, OffsetDateTime, Weekday};

use anamnesis_app::{BoardQuery, TaskRepository};
use anamnesis_core::TaskId;
use support::{DEV_CSRF_TOKEN, TestApp, body_text, location_of};

/// The next three Tuesdays on or after today (UTC is the test zone), as the
/// preview formats them for an `at 6pm` schedule.
fn next_three_tuesdays_at_6pm() -> Vec<String> {
    let mut day: Date = OffsetDateTime::now_utc().date();
    while day.weekday() != Weekday::Tuesday {
        day += Duration::DAY;
    }
    (0..3)
        .map(|week| {
            let d = day + Duration::weeks(week);
            format!("Tue {} {} 6:00pm", &d.month().to_string()[..3], d.day())
        })
        .collect()
}

/// An active project with one task; returns the app, the task's path and id.
async fn setup() -> (TestApp, String, TaskId) {
    let app = TestApp::new(true).await;
    let (_, project_path) = support::new_active_project(&app, "Home", "Chores", None).await;
    let path = support::new_task(&app, &project_path, "Take out the trash", None).await;
    let id = TaskId::new(
        uuid::Uuid::parse_str(path.trim_start_matches("/tasks/")).expect("a task path"),
    );
    (app, path, id)
}

fn phrase_form(phrase: &str) -> Vec<(&str, &str)> {
    vec![
        ("csrf_token", DEV_CSRF_TOKEN),
        ("source", "phrase"),
        ("phrase", phrase),
    ]
}

/// The builder fields for "the last Friday of every month".
fn builder_form() -> Vec<(&'static str, &'static str)> {
    vec![
        ("csrf_token", DEV_CSRF_TOKEN),
        ("source", "builder"),
        ("phrase", ""),
        ("every_n", "1"),
        ("unit", "months"),
        ("anchor", "calendar"),
        ("month_mode", "nth"),
        ("nth", "last"),
        ("nth_weekday", "fri"),
    ]
}

fn unescape(body: &str) -> String {
    body.replace("&#x27;", "'")
}

async fn preview(app: &TestApp, path: &str, form: &[(&str, &str)]) -> String {
    let response = app
        .post_form_hx(&format!("{path}/schedule/preview"), form, None)
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    body_text(response).await
}

#[tokio::test]
async fn preview_from_phrase_shows_canonical_phrase_and_three_dates() {
    let (app, path, _) = setup().await;
    let body = preview(&app, &path, &phrase_form("Every Tuesday at 18:00 until 08:00")).await;
    assert!(body.contains("every tue at 6pm until 8am"), "{body}");
    for date in next_three_tuesdays_at_6pm() {
        assert!(body.contains(&date), "missing {date} in {body}");
    }
}

#[tokio::test]
async fn preview_from_builder_fills_phrase() {
    let (app, path, _) = setup().await;
    let body = preview(&app, &path, &builder_form()).await;
    assert!(body.contains("every month on the last fri"), "{body}");
}

#[tokio::test]
async fn builder_and_phrase_agree_on_round_trip() {
    let (app, path, _) = setup().await;
    let phrase = "every tue, thu at 6pm until 8am";
    let from_phrase = preview(&app, &path, &phrase_form(phrase)).await;

    let builder = [
        ("csrf_token", DEV_CSRF_TOKEN),
        ("source", "builder"),
        ("phrase", ""),
        ("every_n", "1"),
        ("unit", "weeks"),
        ("anchor", "calendar"),
        ("weekdays", "tue"),
        ("weekdays", "thu"),
        ("pop_in", "on"),
        ("pop_in_hour", "6"),
        ("pop_in_minute", "0"),
        ("pop_in_ampm", "pm"),
        ("until", "on"),
        ("until_hour", "8"),
        ("until_minute", "0"),
        ("until_ampm", "am"),
    ];
    let from_builder = preview(&app, &path, &builder).await;
    assert!(from_builder.contains(phrase), "{from_builder}");
    assert_eq!(
        from_phrase, from_builder,
        "either editor yields the very same section"
    );
}

#[tokio::test]
async fn bad_phrase_shows_inline_error_and_keeps_input() {
    let (app, path, _) = setup().await;
    let body = unescape(&preview(&app, &path, &phrase_form("every fortnight")).await);
    assert!(body.contains("didn't understand 'fortnight'"), "{body}");
    assert!(body.contains(r#"value="every fortnight""#), "{body}");
}

#[tokio::test]
async fn save_sets_schedule_and_empty_phrase_clears_it() {
    let (app, path, id) = setup().await;
    let save = format!("{path}/schedule");

    let response = app
        .post_form(&save, &phrase_form("every tue at 6pm until 8am"), None)
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(location_of(&response), path);
    assert!(app.store.load(id).await.unwrap().unwrap().task.recurrence.is_some());

    let page = body_text(app.get(&path, None).await).await;
    assert!(page.contains("every tue at 6pm until 8am"), "{page}");

    let response = app.post_form(&save, &phrase_form(""), None).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(app.store.load(id).await.unwrap().unwrap().task.recurrence.is_none());
}

#[tokio::test]
async fn save_from_builder_stores_the_built_schedule() {
    let (app, path, id) = setup().await;
    let response = app
        .post_form(&format!("{path}/schedule"), &builder_form(), None)
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let stored = app.store.load(id).await.unwrap().unwrap().task;
    let phrase = anamnesis_core::describe(&stored.recurrence.unwrap().schedule);
    assert_eq!(phrase, "every month on the last fri");
}

#[tokio::test]
async fn save_of_a_bad_phrase_changes_nothing_and_explains() {
    let (app, path, id) = setup().await;
    let response = app
        .post_form(
            &format!("{path}/schedule"),
            &phrase_form("every fortnight"),
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(unescape(&body_text(response).await).contains("didn't understand 'fortnight'"));
    assert!(app.store.load(id).await.unwrap().unwrap().task.recurrence.is_none());
}

#[tokio::test]
async fn save_requires_edit_permission() {
    let (app, task_path, stranger_cookie) = support::setup_task_as_admin().await;
    let response = app
        .post_form(
            &format!("{task_path}/schedule"),
            &[
                ("csrf_token", "stranger-token"),
                ("source", "phrase"),
                ("phrase", "every day"),
            ],
            Some(&stranger_cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

/// Raises the task onto the board's entry column.
async fn raise(app: &TestApp, path: &str) {
    let column = app.store.columns_with_items().await.unwrap()[0].column.id;
    let response = app
        .post_form(
            &format!("{path}/raise"),
            &[
                ("csrf_token", DEV_CSRF_TOKEN),
                ("column_id", &column.to_string()),
            ],
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn recurring_card_shows_repeat_glyph() {
    let (app, path, _) = setup().await;
    app.post_form(&format!("{path}/schedule"), &phrase_form("every tue"), None)
        .await;
    raise(&app, &path).await;

    let board = body_text(app.get("/board", None).await).await;
    assert!(board.contains("↻"), "{board}");
    assert!(board.contains(r#"title="Repeats: every tue""#), "{board}");
}

#[tokio::test]
async fn a_task_without_a_schedule_has_no_glyph() {
    let (app, path, _) = setup().await;
    raise(&app, &path).await;
    let board = body_text(app.get("/board", None).await).await;
    assert!(board.contains("Take out the trash"));
    assert!(!board.contains("↻"));
}

#[tokio::test]
async fn due_suggestion_reads_due_again() {
    let (app, path, id) = setup().await;
    app.post_form(&format!("{path}/schedule"), &phrase_form("every day"), None)
        .await;
    // Always give the due chore the first suggestion.
    let settings = app
        .post_form(
            "/settings",
            &[
                ("csrf_token", DEV_CSRF_TOKEN),
                ("active_project_limit", "5"),
                ("cooldown_seconds", "259200"),
                ("high_bounce_threshold", "3"),
                ("suggestion_due_slot_percent", "100"),
                ("sweep_kind", "never"),
            ],
            None,
        )
        .await;
    assert_eq!(settings.status(), StatusCode::SEE_OTHER);

    // Come due: an interval chore waits below the board with `due_since` set.
    let mut task = app.store.load(id).await.unwrap().unwrap().task;
    let touched = task.last_touched_at;
    task.recurrence.as_mut().unwrap().due_since = Some(app.state.clock.now());
    app.store.update(&task, touched).await.unwrap();

    let board = body_text(app.get("/board", None).await).await;
    assert!(board.contains("Take out the trash"), "{board}");
    assert!(board.contains("due again"), "{board}");
}
