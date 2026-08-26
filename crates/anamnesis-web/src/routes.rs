//! Builds the `axum::Router`: one resource-oriented route per job, exactly
//! the list in `docs/PLAN.md`'s Phase 4 section — so a future PWA client can
//! call the same URLs and content-negotiate JSON instead of HTML.

use axum::Router;
use axum::routing::{get, post};

use crate::handlers::{
    add_card_handler, add_column_handler, callback_handler, create_board_handler,
    delete_board_handler, delete_card_handler, healthz_handler, list_boards_handler, login_handler,
    logout_handler, move_card_handler, root_handler, view_board_handler,
};
use crate::state::AppState;
use crate::static_files;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz_handler))
        .route("/", get(root_handler))
        .route("/login", get(login_handler))
        .route("/auth/callback", get(callback_handler))
        .route("/logout", post(logout_handler))
        .route(
            "/boards",
            get(list_boards_handler).post(create_board_handler),
        )
        .route("/boards/{id}", get(view_board_handler))
        .route("/boards/{id}/delete", post(delete_board_handler))
        .route("/boards/{id}/columns", post(add_column_handler))
        .route("/boards/{id}/columns/{cid}/cards", post(add_card_handler))
        .route("/boards/{id}/cards/{card_id}/move", post(move_card_handler))
        .route(
            "/boards/{id}/cards/{card_id}/delete",
            post(delete_card_handler),
        )
        .route("/static/app.css", get(static_files::app_css))
        .route("/static/icon.svg", get(static_files::icon))
        .route("/manifest.webmanifest", get(static_files::manifest))
        .with_state(state)
}
