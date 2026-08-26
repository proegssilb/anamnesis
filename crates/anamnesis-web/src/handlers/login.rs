//! `GET /login`, `GET /auth/callback`, `POST /logout` — the OIDC
//! Authorization Code + PKCE round trip, orchestrated against the
//! `IdentityProvider` port. See `docs/ARCHITECTURE.md`'s "Authentication"
//! section.

use axum::Form;
use axum::extract::{Query, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum_extra::extract::cookie::{Key, SignedCookieJar};
use minijinja::context;
use serde::Deserialize;

use anamnesis_app::{IdentityError, LoginCallback};

use crate::auth::CurrentUser;
use crate::error::WebError;
use crate::session::{
    self, PENDING_LOGIN_COOKIE_NAME, PendingLogin, SessionData, csrf_tokens_match,
    parse_pending_login, pending_login_cookie, removal_cookie, session_cookie,
};
use crate::state::AppState;

use super::forms::CsrfOnlyForm;

pub async fn login_handler(State(state): State<AppState>, jar: SignedCookieJar<Key>) -> Response {
    if state.dev_auth_bypass {
        // Dev bypass already authenticates every request as the fixed user;
        // there is nothing for a real login round trip to do.
        return Redirect::to("/boards").into_response();
    }

    let Some(identity) = &state.identity else {
        return WebError::LoginFailed(IdentityError::new("no identity provider is configured"))
            .into_response_with(&state.templates);
    };

    match identity.begin_login().await {
        Ok(redirect) => {
            let pending = PendingLogin {
                csrf_state: redirect.csrf_state,
                pkce_verifier: redirect.pkce_verifier,
                nonce: redirect.nonce,
            };
            let jar = jar.add(pending_login_cookie(&pending, state.secure_cookies));

            match render_login_page(&state, &redirect.authorize_url) {
                Ok(body) => (jar, body).into_response(),
                Err(err) => err.into_response_with(&state.templates),
            }
        }
        Err(err) => WebError::LoginFailed(err).into_response_with(&state.templates),
    }
}

fn render_login_page(state: &AppState, authorize_url: &str) -> Result<Html<String>, WebError> {
    let tmpl = state
        .templates
        .get_template("login.html")
        .map_err(WebError::template)?;
    let body = tmpl
        .render(context! { authorize_url => authorize_url })
        .map_err(WebError::template)?;
    Ok(Html(body))
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
}

pub async fn callback_handler(
    State(state): State<AppState>,
    jar: SignedCookieJar<Key>,
    Query(query): Query<CallbackQuery>,
) -> Response {
    if state.dev_auth_bypass {
        return Redirect::to("/boards").into_response();
    }

    let Some(identity) = &state.identity else {
        return WebError::LoginFailed(IdentityError::new("no identity provider is configured"))
            .into_response_with(&state.templates);
    };

    let Some(pending) = jar
        .get(PENDING_LOGIN_COOKIE_NAME)
        .and_then(|cookie| parse_pending_login(cookie.value()))
    else {
        return WebError::LoginFailed(IdentityError::new(
            "no pending login found — the login attempt may have expired or this callback was \
             not reached from /login",
        ))
        .into_response_with(&state.templates);
    };
    // The pending-login cookie is single-use regardless of how this
    // callback resolves.
    let jar = jar.remove(removal_cookie(PENDING_LOGIN_COOKIE_NAME));

    let (Some(code), Some(returned_state)) = (query.code, query.state) else {
        return (
            jar,
            WebError::BadRequest(
                "the identity provider's callback was missing code or state".into(),
            )
            .into_response_with(&state.templates),
        )
            .into_response();
    };

    let callback = LoginCallback {
        code,
        state: returned_state,
        expected_state: pending.csrf_state,
        pkce_verifier: pending.pkce_verifier,
        expected_nonce: pending.nonce,
    };

    match identity.complete_login(callback).await {
        Ok(user_id) => {
            let session = SessionData {
                display_name: user_id.to_string(),
                user_id,
                csrf_token: session::generate_csrf_token(),
            };
            let jar = jar.add(session_cookie(&session, state.secure_cookies));
            (jar, Redirect::to("/boards")).into_response()
        }
        Err(err) => (
            jar,
            WebError::LoginFailed(err).into_response_with(&state.templates),
        )
            .into_response(),
    }
}

pub async fn logout_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    jar: SignedCookieJar<Key>,
    Form(form): Form<CsrfOnlyForm>,
) -> Response {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return WebError::CsrfMismatch.into_response_with(&state.templates);
    }
    let jar = jar.remove(removal_cookie(session::SESSION_COOKIE_NAME));
    (jar, Redirect::to("/login")).into_response()
}
