//! End-to-end through real HTTP: create a board, add a column, add a card,
//! and confirm the card really shows up on the rendered page. Then move it
//! across columns and confirm the `303` and the follow-up `GET` agree.

mod support;

use axum::http::StatusCode;

/// Runs a mutating POST, asserts it 303s, and returns the `Location` it
/// redirected to.
async fn post_and_follow_redirect(
    app: &support::TestApp,
    path: &str,
    form: &[(&str, &str)],
) -> String {
    let response = app.post_form(path, form, None).await;
    assert_eq!(
        response.status(),
        StatusCode::SEE_OTHER,
        "expected a 303 from POST {path}"
    );
    support::location_of(&response).to_string()
}

#[tokio::test]
async fn create_board_add_column_add_card_renders_the_card() {
    let app = support::TestApp::new(true).await;
    let csrf = support::DEV_CSRF_TOKEN;

    let board_location = post_and_follow_redirect(
        &app,
        "/boards",
        &[("csrf_token", csrf), ("title", "Launch Plan")],
    )
    .await;
    let board_path = board_location.split('#').next().unwrap().to_string();

    let after_column = post_and_follow_redirect(
        &app,
        &format!("{board_path}/columns"),
        &[("csrf_token", csrf), ("title", "Todo"), ("wip_limit", "")],
    )
    .await;
    // "/boards/{id}#column-{cid}"
    let column_id = after_column.split("#column-").nth(1).unwrap().to_string();

    let after_card = post_and_follow_redirect(
        &app,
        &format!("{board_path}/columns/{column_id}/cards"),
        &[
            ("csrf_token", csrf),
            ("title", "Write the launch email"),
            ("body", "Draft, then send to the list."),
        ],
    )
    .await;
    assert!(after_card.starts_with(&format!("{board_path}#card-")));
    let card_id = after_card.split("#card-").nth(1).unwrap().to_string();

    let board_page = app.get(&board_path, None).await;
    assert_eq!(board_page.status(), StatusCode::OK);
    let body = support::body_text(board_page).await;

    assert!(body.contains("Launch Plan"), "board title should render");
    assert!(body.contains("Todo"), "column title should render");
    assert!(
        body.contains("Write the launch email"),
        "card title should render"
    );
    assert!(
        body.contains("Draft, then send to the list."),
        "card body should render"
    );
    assert!(
        body.contains(&format!(r#"id="card-{card_id}""#)),
        "card should carry its stable #card-{{uuid}} hook"
    );
}

#[tokio::test]
async fn a_move_post_redirects_and_the_card_appears_in_its_new_column() {
    let app = support::TestApp::new(true).await;
    let csrf = support::DEV_CSRF_TOKEN;

    let board_location =
        post_and_follow_redirect(&app, "/boards", &[("csrf_token", csrf), ("title", "Board")])
            .await;
    let board_path = board_location.split('#').next().unwrap().to_string();

    let after_todo = post_and_follow_redirect(
        &app,
        &format!("{board_path}/columns"),
        &[("csrf_token", csrf), ("title", "Todo"), ("wip_limit", "")],
    )
    .await;
    let todo_id = after_todo.split("#column-").nth(1).unwrap().to_string();

    let after_doing = post_and_follow_redirect(
        &app,
        &format!("{board_path}/columns"),
        &[("csrf_token", csrf), ("title", "Doing"), ("wip_limit", "")],
    )
    .await;
    let doing_id = after_doing.split("#column-").nth(1).unwrap().to_string();

    let after_card = post_and_follow_redirect(
        &app,
        &format!("{board_path}/columns/{todo_id}/cards"),
        &[
            ("csrf_token", csrf),
            ("title", "Movable card"),
            ("body", ""),
        ],
    )
    .await;
    let card_id = after_card.split("#card-").nth(1).unwrap().to_string();

    // Sanity: it starts out in Todo.
    let before = support::body_text(app.get(&board_path, None).await).await;
    let todo_section = section_between(
        &before,
        &format!("column-{todo_id}"),
        &format!("column-{doing_id}"),
    );
    assert!(todo_section.contains("Movable card"));

    let move_redirect = post_and_follow_redirect(
        &app,
        &format!("{board_path}/cards/{card_id}/move"),
        &[
            ("csrf_token", csrf),
            ("to_column", &doing_id),
            ("to_index", "0"),
        ],
    )
    .await;
    assert_eq!(move_redirect, format!("{board_path}#card-{card_id}"));

    let after = support::body_text(app.get(&board_path, None).await).await;
    let doing_section = section_after(&after, &format!("column-{doing_id}"));
    assert!(
        doing_section.contains("Movable card"),
        "card should now render inside the Doing column"
    );
    let todo_section_after = section_between(
        &after,
        &format!("column-{todo_id}"),
        &format!("column-{doing_id}"),
    );
    assert!(
        !todo_section_after.contains("Movable card"),
        "card should no longer render inside the Todo column"
    );
}

fn section_after<'a>(body: &'a str, marker: &str) -> &'a str {
    let start = body.find(marker).expect("marker present");
    &body[start..]
}

fn section_between<'a>(body: &'a str, start_marker: &str, end_marker: &str) -> &'a str {
    let start = body.find(start_marker).expect("start marker present");
    let rest = &body[start..];
    match rest.find(end_marker) {
        Some(end) => &rest[..end],
        None => rest,
    }
}
