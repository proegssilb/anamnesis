use axum::response::{IntoResponse, Redirect};

/// Liveness probe. Deliberately requires no state and no auth.
pub async fn healthz_handler() -> impl IntoResponse {
    "ok"
}

/// `GET /` always sends the browser to `/areas`; `/areas` is what decides
/// whether that lands on the area grid or a `/login` redirect.
pub async fn root_handler() -> impl IntoResponse {
    Redirect::to("/areas")
}
