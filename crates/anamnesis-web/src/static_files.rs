//! The handful of static assets the app serves: the stylesheet, the PWA
//! manifest, and its icon. All three are embedded at compile time — no
//! filesystem dependency at runtime, and nothing here is user-supplied so a
//! plain `include_str!` is all `GET /static/*` and `GET /manifest.webmanifest`
//! need.

use axum::http::header;
use axum::response::IntoResponse;

pub async fn app_css() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../static/app.css"),
    )
}

pub async fn icon() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "image/svg+xml")],
        include_str!("../static/icon.svg"),
    )
}

pub async fn manifest() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/manifest+json")],
        include_str!("../static/manifest.webmanifest"),
    )
}
