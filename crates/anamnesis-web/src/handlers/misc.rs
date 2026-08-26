use axum::response::{IntoResponse, Redirect};

/// Liveness probe. Deliberately requires no state and no auth.
pub async fn healthz_handler() -> impl IntoResponse {
    "ok"
}

/// `GET /` always sends the browser to `/boards`; `/boards` is what decides
/// whether that lands on the board list or a `/login` redirect.
pub async fn root_handler() -> impl IntoResponse {
    Redirect::to("/boards")
}
