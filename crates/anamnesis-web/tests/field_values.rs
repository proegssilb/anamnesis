//! `tower::ServiceExt::oneshot` coverage for setting a task's custom field
//! values (`docs/DOMAIN.md` §3) — the feature that rendered field
//! *definitions* since Phase F1 but had no form to ever set one, making it
//! dead. The owner's motivating example is house-hunting (each task a
//! house, fields for price, viewing date, ...), so this suite uses that
//! vocabulary throughout.

mod support;

use axum::http::StatusCode;

use anamnesis_adapters::UuidIdGen;
use anamnesis_app::{BoardQuery, TaskRepository, add_field_definition};
use anamnesis_core::policy::Role;
use anamnesis_core::{CurrencyAmount, CurrencyCode, FieldData, FieldKind, NumberValue};
use support::{TestApp, body_text, location_of};

/// Creates an area, a project, and a task; returns the task's path and id
/// plus its project id (field definitions belong to the project).
async fn setup_task(app: &TestApp) -> (String, uuid::Uuid, anamnesis_core::ProjectId) {
    let cookie: Option<&str> = None;
    let area_path = location_of(
        &app.post_form(
            "/areas",
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("title", "Home hunting"),
                ("description", ""),
            ],
            cookie,
        )
        .await,
    )
    .to_string();
    let project_path = location_of(
        &app.post_form(
            &format!("{area_path}/projects"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("title", "House shopping"),
                ("description", ""),
            ],
            cookie,
        )
        .await,
    )
    .to_string();
    let project_id: anamnesis_core::ProjectId = anamnesis_core::ProjectId::new(
        project_path
            .trim_start_matches("/projects/")
            .parse()
            .unwrap(),
    );
    let task_path = location_of(
        &app.post_form(
            &format!("{project_path}/tasks"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("title", "123 Maple St"),
                ("description", ""),
            ],
            cookie,
        )
        .await,
    )
    .to_string();
    let task_id: uuid::Uuid = task_path.trim_start_matches("/tasks/").parse().unwrap();
    (task_path, task_id, project_id)
}

async fn add_field(
    app: &TestApp,
    project_id: anamnesis_core::ProjectId,
    name: &str,
    kind: FieldKind,
    position: u32,
    show_on_card: bool,
) -> anamnesis_core::FieldId {
    let ids = UuidIdGen;
    add_field_definition(
        app.store.as_ref(),
        &ids,
        Some(Role::SystemAdmin),
        project_id,
        name,
        kind,
        position,
        show_on_card,
    )
    .await
    .unwrap()
    .id
}

#[tokio::test]
async fn every_field_kind_can_be_set_through_its_form_and_read_back() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;
    let (task_path, task_id, project_id) = setup_task(&app).await;
    let task_id = anamnesis_core::TaskId::new(task_id);

    let price = add_field(&app, project_id, "Price", FieldKind::Currency, 0, false).await;
    let sqft = add_field(
        &app,
        project_id,
        "Square footage",
        FieldKind::Number,
        1,
        false,
    )
    .await;
    let viewing = add_field(&app, project_id, "Viewing date", FieldKind::Date, 2, false).await;
    let open_house = add_field(
        &app,
        project_id,
        "Open house time",
        FieldKind::Time,
        3,
        false,
    )
    .await;
    let offer_deadline = add_field(
        &app,
        project_id,
        "Offer deadline",
        FieldKind::DateTime,
        4,
        false,
    )
    .await;
    let agent = add_field(&app, project_id, "Agent", FieldKind::Line, 5, false).await;
    let notes = add_field(&app, project_id, "Notes", FieldKind::Block, 6, false).await;

    // Currency: dollars and cents, plus the ISO code.
    let r = app
        .post_form(
            &format!("{task_path}/fields/{price}"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("value", "419999.99"),
                ("currency", "usd"),
            ],
            cookie,
        )
        .await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);

    // Number: a scaled integer, arbitrary decimal places.
    let r = app
        .post_form(
            &format!("{task_path}/fields/{sqft}"),
            &[("csrf_token", support::DEV_CSRF_TOKEN), ("value", "1850.5")],
            cookie,
        )
        .await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);

    // Date.
    let r = app
        .post_form(
            &format!("{task_path}/fields/{viewing}"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("value", "2026-09-14"),
            ],
            cookie,
        )
        .await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);

    // Time.
    let r = app
        .post_form(
            &format!("{task_path}/fields/{open_house}"),
            &[("csrf_token", support::DEV_CSRF_TOKEN), ("value", "14:30")],
            cookie,
        )
        .await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);

    // DateTime: a local wall-clock moment, resolved through the app's
    // configured timezone (UTC in tests — see `support::TestApp`).
    let r = app
        .post_form(
            &format!("{task_path}/fields/{offer_deadline}"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("value", "2026-09-20T17:00"),
            ],
            cookie,
        )
        .await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);

    // Line.
    let r = app
        .post_form(
            &format!("{task_path}/fields/{agent}"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("value", "Jamie Rivera"),
            ],
            cookie,
        )
        .await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);

    // Block: multi-line text.
    let r = app
        .post_form(
            &format!("{task_path}/fields/{notes}"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("value", "Needs a new roof.\nAsk about the well."),
            ],
            cookie,
        )
        .await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);

    // Read every value back through the rendered task page.
    let body = body_text(app.get(&task_path, cookie).await).await;
    assert!(body.contains("419,999.99") || body.contains("419999.99"));
    assert!(body.contains("USD"));
    assert!(body.contains("1850.5"));
    assert!(body.contains("2026-09-14"));
    assert!(body.contains("14:30"));
    assert!(body.contains("Jamie Rivera"));
    assert!(body.contains("Needs a new roof."));

    // And read every value back from storage directly, to confirm exact
    // typed round-tripping (`docs/DOMAIN.md` §3: currency and number are
    // integers, never floats).
    let aggregate = app.store.load(task_id).await.unwrap().unwrap();
    let values = aggregate.field_values;
    let get = |id: anamnesis_core::FieldId| {
        values
            .iter()
            .find(|v| v.field_id == id)
            .unwrap_or_else(|| panic!("field {id} was not stored"))
            .data
            .clone()
    };

    assert_eq!(
        get(price),
        FieldData::Currency(CurrencyAmount {
            minor_units: 41_999_999,
            currency: CurrencyCode::new("USD").unwrap(),
        })
    );
    assert_eq!(
        get(sqft),
        FieldData::Number(NumberValue {
            units: 18505,
            scale: 1,
        })
    );
    match get(viewing) {
        FieldData::Date(d) => {
            assert_eq!(d.year(), 2026);
            assert_eq!(d.day(), 14);
        }
        other => panic!("expected a Date, got {other:?}"),
    }
    match get(open_house) {
        FieldData::Time(t) => {
            assert_eq!((t.hour(), t.minute()), (14, 30));
        }
        other => panic!("expected a Time, got {other:?}"),
    }
    assert!(matches!(get(offer_deadline), FieldData::DateTime(_)));
    assert_eq!(get(agent), FieldData::Line("Jamie Rivera".to_string()));
    assert_eq!(
        get(notes),
        FieldData::Block("Needs a new roof.\nAsk about the well.".to_string())
    );
}

#[tokio::test]
async fn a_currency_value_round_trips_without_precision_loss() {
    // A value that would drift under repeated float arithmetic must not
    // drift here at all, because there is no float in this path
    // (`docs/DOMAIN.md` §3).
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;
    let (task_path, task_id, project_id) = setup_task(&app).await;
    let task_id = anamnesis_core::TaskId::new(task_id);
    let price = add_field(&app, project_id, "Price", FieldKind::Currency, 0, false).await;

    let r = app
        .post_form(
            &format!("{task_path}/fields/{price}"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("value", "10.01"),
                ("currency", "USD"),
            ],
            cookie,
        )
        .await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);

    let aggregate = app.store.load(task_id).await.unwrap().unwrap();
    let value = aggregate
        .field_values
        .iter()
        .find(|v| v.field_id == price)
        .unwrap();
    assert_eq!(
        value.data,
        FieldData::Currency(CurrencyAmount {
            minor_units: 1001,
            currency: CurrencyCode::new("USD").unwrap(),
        }),
        "10.01 must store as exactly 1001 minor units, not a float-drifted neighbour"
    );

    // Overwrite it 1000 times with the same value the way repeated saves
    // would in real use, and confirm it still lands on exactly 1001 -- a
    // float accumulator would not survive this unchanged.
    for _ in 0..50 {
        app.post_form(
            &format!("{task_path}/fields/{price}"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("value", "10.01"),
                ("currency", "USD"),
            ],
            cookie,
        )
        .await;
    }
    let aggregate = app.store.load(task_id).await.unwrap().unwrap();
    let value = aggregate
        .field_values
        .iter()
        .find(|v| v.field_id == price)
        .unwrap();
    assert_eq!(
        value.data,
        FieldData::Currency(CurrencyAmount {
            minor_units: 1001,
            currency: CurrencyCode::new("USD").unwrap(),
        })
    );
}

#[tokio::test]
async fn a_show_on_card_field_renders_compactly_on_the_board_card() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;
    let (task_path, _task_id, project_id) = setup_task(&app).await;
    let price = add_field(&app, project_id, "Price", FieldKind::Currency, 0, true).await;

    app.post_form(
        &format!("{task_path}/fields/{price}"),
        &[
            ("csrf_token", support::DEV_CSRF_TOKEN),
            ("value", "550000"),
            ("currency", "USD"),
        ],
        cookie,
    )
    .await;

    let todo = app.store.columns_with_items().await.unwrap()[0].column.id;
    app.post_form(
        &format!("{task_path}/raise"),
        &[
            ("csrf_token", support::DEV_CSRF_TOKEN),
            ("column_id", &todo.to_string()),
        ],
        cookie,
    )
    .await;

    let board = body_text(app.get("/board", cookie).await).await;
    assert!(
        board.contains("Price"),
        "the field name must appear on the card: {board}"
    );
    assert!(
        board.contains("550000.00 USD"),
        "the show_on_card field's value must render on the board card: {board}"
    );
}
