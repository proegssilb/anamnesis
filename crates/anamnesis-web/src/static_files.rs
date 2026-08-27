//! The handful of static assets the app serves: the stylesheet, the PWA
//! manifest, its icon, and the two vendored client-side libraries
//! `docs/DOMAIN.md` §8 calls for (`htmx` for transport, `SortableJS` for
//! drag mechanics — see that section's "Sortable drags, htmx persists"),
//! plus this app's own small glue script. All are embedded at compile time
//! — no filesystem dependency at runtime, and nothing here is user-supplied
//! so a plain `include_str!` is all `GET /static/*` and
//! `GET /manifest.webmanifest` need.
//!
//! **Vendored, not CDN-loaded**: this is self-hosted software and must not
//! depend on a third party being reachable at runtime — `htmx.min.js` and
//! `sortable.min.js` are copied verbatim from their upstream npm packages
//! (`htmx.org`, `sortablejs`) into `static/`, not fetched from a CDN at
//! request time or build time.

use axum::http::header;
use axum::response::IntoResponse;

pub async fn app_css() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../static/app.css"),
    )
}

pub async fn app_js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/javascript; charset=utf-8")],
        include_str!("../static/app.js"),
    )
}

/// Vendored `htmx` (`docs/DOMAIN.md` §8) — see the module doc comment.
pub async fn htmx_js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/javascript; charset=utf-8")],
        include_str!("../static/htmx.min.js"),
    )
}

/// Vendored `SortableJS` (`docs/DOMAIN.md` §8) — see the module doc comment.
pub async fn sortable_js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/javascript; charset=utf-8")],
        include_str!("../static/sortable.min.js"),
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
