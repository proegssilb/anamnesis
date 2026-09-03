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

/// The one field of each kind that `every_field_kind_can_be_set_through_its_form_and_read_back`
/// defines on its task, threaded through the test's later phases so each
/// phase can address the field it cares about by name instead of position.
struct SevenFields {
    price: anamnesis_core::FieldId,
    sqft: anamnesis_core::FieldId,
    viewing: anamnesis_core::FieldId,
    open_house: anamnesis_core::FieldId,
    offer_deadline: anamnesis_core::FieldId,
    agent: anamnesis_core::FieldId,
    notes: anamnesis_core::FieldId,
}

/// Defines one field definition of every `FieldKind`, matching the
/// house-hunting vocabulary this suite uses throughout.
async fn define_every_field_kind(
    app: &TestApp,
    project_id: anamnesis_core::ProjectId,
) -> SevenFields {
    SevenFields {
        price: add_field(app, project_id, "Price", FieldKind::Currency, 0, false).await,
        sqft: add_field(
            app,
            project_id,
            "Square footage",
            FieldKind::Number,
            1,
            false,
        )
        .await,
        viewing: add_field(app, project_id, "Viewing date", FieldKind::Date, 2, false).await,
        open_house: add_field(
            app,
            project_id,
            "Open house time",
            FieldKind::Time,
            3,
            false,
        )
        .await,
        offer_deadline: add_field(
            app,
            project_id,
            "Offer deadline",
            FieldKind::DateTime,
            4,
            false,
        )
        .await,
        agent: add_field(app, project_id, "Agent", FieldKind::Line, 5, false).await,
        notes: add_field(app, project_id, "Notes", FieldKind::Block, 6, false).await,
    }
}

/// Submits one value through the task's field form for every field kind --
/// a currency (dollars, cents and ISO code), a scaled-integer number, a
/// date, a time, a local-wall-clock datetime (resolved through the app's
/// configured timezone -- UTC in tests, see `support::TestApp`), a
/// single-line, and a multi-line value.
async fn submit_a_value_for_every_field_kind(
    app: &TestApp,
    task_path: &str,
    fields: &SevenFields,
    cookie: Option<&str>,
) {
    let posts: [(anamnesis_core::FieldId, &[(&str, &str)]); 7] = [
        (fields.price, &[("value", "419999.99"), ("currency", "usd")]),
        (fields.sqft, &[("value", "1850.5")]),
        (fields.viewing, &[("value", "2026-09-14")]),
        (fields.open_house, &[("value", "14:30")]),
        (fields.offer_deadline, &[("value", "2026-09-20T17:00")]),
        (fields.agent, &[("value", "Jamie Rivera")]),
        (
            fields.notes,
            &[("value", "Needs a new roof.\nAsk about the well.")],
        ),
    ];
    for (field_id, extra) in posts {
        let mut form = vec![("csrf_token", support::DEV_CSRF_TOKEN)];
        form.extend_from_slice(extra);
        let r = app
            .post_form(&format!("{task_path}/fields/{field_id}"), &form, cookie)
            .await;
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
    }
}

/// Reads every value back through the rendered task page.
async fn assert_every_value_renders_on_the_task_page(
    app: &TestApp,
    task_path: &str,
    cookie: Option<&str>,
) {
    let body = body_text(app.get(task_path, cookie).await).await;
    assert!(body.contains("419,999.99") || body.contains("419999.99"));
    assert!(body.contains("USD"));
    assert!(body.contains("1850.5"));
    assert!(body.contains("2026-09-14"));
    assert!(body.contains("14:30"));
    assert!(body.contains("Jamie Rivera"));
    assert!(body.contains("Needs a new roof."));
}

/// The exact-equality half of the round-trip check: fields whose stored
/// value should match the submitted one bit-for-bit, no interpretation
/// needed.
fn assert_scalar_fields_round_trip_exactly(
    get: &impl Fn(anamnesis_core::FieldId) -> FieldData,
    fields: &SevenFields,
) {
    assert_eq!(
        get(fields.price),
        FieldData::Currency(CurrencyAmount {
            minor_units: 41_999_999,
            currency: CurrencyCode::new("USD").unwrap(),
        })
    );
    assert_eq!(
        get(fields.sqft),
        FieldData::Number(NumberValue {
            units: 18505,
            scale: 1,
        })
    );
    assert_eq!(
        get(fields.agent),
        FieldData::Line("Jamie Rivera".to_string())
    );
    assert_eq!(
        get(fields.notes),
        FieldData::Block("Needs a new roof.\nAsk about the well.".to_string())
    );
}

/// The date/time half of the round-trip check: fields verified by their
/// individual components (or, for the timezone-resolved datetime, just its
/// variant) rather than by exact equality.
fn assert_temporal_fields_round_trip_their_components(
    get: &impl Fn(anamnesis_core::FieldId) -> FieldData,
    fields: &SevenFields,
) {
    match get(fields.viewing) {
        FieldData::Date(d) => {
            assert_eq!(d.year(), 2026);
            assert_eq!(d.day(), 14);
        }
        other => panic!("expected a Date, got {other:?}"),
    }
    match get(fields.open_house) {
        FieldData::Time(t) => {
            assert_eq!((t.hour(), t.minute()), (14, 30));
        }
        other => panic!("expected a Time, got {other:?}"),
    }
    assert!(matches!(get(fields.offer_deadline), FieldData::DateTime(_)));
}

/// Reads every value back from storage directly, to confirm exact typed
/// round-tripping (`docs/DOMAIN.md` §3: currency and number are integers,
/// never floats).
async fn assert_every_value_round_trips_with_exact_types(
    app: &TestApp,
    task_id: anamnesis_core::TaskId,
    fields: &SevenFields,
) {
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

    assert_scalar_fields_round_trip_exactly(&get, fields);
    assert_temporal_fields_round_trip_their_components(&get, fields);
}

#[tokio::test]
async fn every_field_kind_can_be_set_through_its_form_and_read_back() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;
    let (task_path, task_id, project_id) = setup_task(&app).await;
    let task_id = anamnesis_core::TaskId::new(task_id);

    let fields = define_every_field_kind(&app, project_id).await;
    submit_a_value_for_every_field_kind(&app, &task_path, &fields, cookie).await;
    assert_every_value_renders_on_the_task_page(&app, &task_path, cookie).await;
    assert_every_value_round_trips_with_exact_types(&app, task_id, &fields).await;
}

/// Submits the fixed "10.01 USD" value onto `price` through its form, the
/// way a repeated real-world save would.
async fn submit_ten_and_a_cent(
    app: &TestApp,
    task_path: &str,
    price: anamnesis_core::FieldId,
    cookie: Option<&str>,
) -> StatusCode {
    app.post_form(
        &format!("{task_path}/fields/{price}"),
        &[
            ("csrf_token", support::DEV_CSRF_TOKEN),
            ("value", "10.01"),
            ("currency", "USD"),
        ],
        cookie,
    )
    .await
    .status()
}

/// Loads the task fresh from storage and returns `price`'s stored data.
async fn stored_price(
    app: &TestApp,
    task_id: anamnesis_core::TaskId,
    price: anamnesis_core::FieldId,
) -> FieldData {
    app.store
        .load(task_id)
        .await
        .unwrap()
        .unwrap()
        .field_values
        .iter()
        .find(|v| v.field_id == price)
        .unwrap()
        .data
        .clone()
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
    let expected = FieldData::Currency(CurrencyAmount {
        minor_units: 1001,
        currency: CurrencyCode::new("USD").unwrap(),
    });

    let status = submit_ten_and_a_cent(&app, &task_path, price, cookie).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        stored_price(&app, task_id, price).await,
        expected,
        "10.01 must store as exactly 1001 minor units, not a float-drifted neighbour"
    );

    // Overwrite it 50 times with the same value the way repeated saves
    // would in real use, and confirm it still lands on exactly 1001 -- a
    // float accumulator would not survive this unchanged.
    for _ in 0..50 {
        submit_ten_and_a_cent(&app, &task_path, price, cookie).await;
    }
    assert_eq!(stored_price(&app, task_id, price).await, expected);
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
