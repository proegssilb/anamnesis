//! Fragment-vs-full-page content negotiation (`docs/DOMAIN.md` §8): "one
//! endpoint, two representations" rather than a forked route table. A
//! request carrying `HX-Request: true` came from htmx (a boosted navigation,
//! a form submit, or `htmx.ajax` — see `static/app.js`'s drag handler) and
//! wants just the fragment it is about to swap in; anything else — a plain
//! browser navigation, a no-JS form POST's redirect target — wants the full
//! page.

use axum::http::HeaderMap;

/// Whether `headers` carries htmx's `HX-Request: true` marker.
pub fn is_hx_request(headers: &HeaderMap) -> bool {
    headers
        .get("HX-Request")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("true"))
}
