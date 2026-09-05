//! [`OidcIdentityProvider`]: the `IdentityProvider` port backed by any
//! standards-compliant OpenID Connect provider, via the `openidconnect`
//! crate. Discovery, PKCE, and ID-token validation (signature, issuer,
//! audience, nonce) all live here; nothing here names Authentik or any
//! other specific provider — a provider-specific branch would be a bug.

use std::collections::HashMap;
use std::ops::Deref;

use anamnesis_app::{
    AuthenticatedIdentity, IdentityError, IdentityProvider, LoginCallback, LoginRedirect,
};
use anamnesis_core::UserId;
use async_trait::async_trait;
use openidconnect::core::{
    CoreAuthenticationFlow, CoreClient, CoreGenderClaim, CoreIdTokenClaims, CoreProviderMetadata,
};
use openidconnect::{
    AccessToken, AdditionalClaims, AuthorizationCode, ClientId, ClientSecret, CsrfToken,
    EndpointMaybeSet, EndpointNotSet, EndpointSet, IssuerUrl, LocalizedClaim, Nonce,
    OAuth2TokenResponse, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope,
    SubjectIdentifier, TokenResponse as OidcTokenResponse, UserInfoClaims,
};
use serde::{Deserialize, Serialize};

/// The concrete client type produced by [`CoreClient::from_provider_metadata`]
/// once a redirect URI has been set: the auth endpoint is always present
/// after discovery (`EndpointSet`), the token endpoint is present on any
/// provider that supports the Authorization Code flow but is only checked at
/// call time (`EndpointMaybeSet`), and the userinfo endpoint is likewise
/// only checked at call time -- needed for [`OidcIdentityProvider::resolve_claim`]'s
/// fallback lookup of a custom (non-standard) configured claim. The
/// remaining optional endpoints (device auth, introspection, revocation)
/// are not used here at all (`EndpointNotSet`).
type DiscoveredClient = CoreClient<
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;

/// The OIDC standard claims this adapter knows how to read directly off an
/// already-verified ID token. A configured claim name outside this set is
/// resolved via an extra `/userinfo` request instead (see [`RawClaims`]).
const STANDARD_CLAIMS: &[&str] = &[
    "sub",
    "name",
    "given_name",
    "family_name",
    "middle_name",
    "nickname",
    "preferred_username",
    "email",
];

/// The custom-claims type used only for a `/userinfo` lookup, when a
/// configured claim name isn't one of [`STANDARD_CLAIMS`]. `openidconnect`
/// lets each call to `UserInfoRequest::request_async` pick its own claims
/// type independent of [`DiscoveredClient`]'s own `CoreIdTokenClaims`, so
/// this adds no generic parameters anywhere else in this module.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
struct RawClaims(HashMap<String, serde_json::Value>);

impl AdditionalClaims for RawClaims {}

/// An [`IdentityProvider`] backed by OAuth2 Authorization Code + PKCE
/// against a discovered OpenID Connect provider.
pub struct OidcIdentityProvider {
    client: DiscoveredClient,
    http_client: openidconnect::reqwest::Client,
    scopes: Vec<Scope>,
    /// Which claim supplies [`AuthenticatedIdentity::user_id`]. `None` means
    /// "not configured": always resolves to `sub`, and (since `sub` is
    /// always present per the OIDC spec) this never fails. `Some(name)`
    /// means an operator explicitly chose `name`; if it can't be resolved,
    /// `complete_login` fails loudly rather than silently falling back.
    user_id_claim: Option<String>,
    /// Which claim supplies [`AuthenticatedIdentity::display_name`]. `None`
    /// means "not configured": tries `preferred_username`, then falls back
    /// to `sub`, and never fails. `Some(name)` carries the same
    /// resolve-or-error contract as `user_id_claim`.
    display_name_claim: Option<String>,
}

impl OidcIdentityProvider {
    /// Discovers `issuer_url` (`GET {issuer}/.well-known/openid-configuration`
    /// plus its JWKS) and builds a provider ready to drive the Authorization
    /// Code + PKCE flow.
    ///
    /// `tls_ca_bundle` optionally names a PEM file of extra certificate
    /// authorities to trust — see [`build_http_client`]. `user_id_claim` and
    /// `display_name_claim` configure which OIDC claim resolves each field
    /// of the eventual [`AuthenticatedIdentity`]; see their doc comments on
    /// [`OidcIdentityProvider`] for the unconfigured-vs-explicit contract.
    #[allow(clippy::too_many_arguments)]
    pub async fn discover(
        issuer_url: &str,
        client_id: String,
        client_secret: Option<String>,
        redirect_url: String,
        scopes: Vec<String>,
        tls_ca_bundle: Option<&str>,
        user_id_claim: Option<String>,
        display_name_claim: Option<String>,
    ) -> Result<Self, IdentityError> {
        let http_client = build_http_client(tls_ca_bundle).await?;

        let issuer = IssuerUrl::new(issuer_url.to_string()).map_err(|e| {
            IdentityError::from_source(format!("invalid issuer URL {issuer_url:?}"), e)
        })?;
        let provider_metadata = CoreProviderMetadata::discover_async(issuer, &http_client)
            .await
            .map_err(|e| IdentityError::from_source("OIDC discovery failed", e))?;

        let redirect = RedirectUrl::new(redirect_url.clone()).map_err(|e| {
            IdentityError::from_source(format!("invalid redirect URL {redirect_url:?}"), e)
        })?;
        let client = CoreClient::from_provider_metadata(
            provider_metadata,
            ClientId::new(client_id),
            client_secret.map(ClientSecret::new),
        )
        .set_redirect_uri(redirect);

        Ok(Self {
            client,
            http_client,
            scopes: scopes.into_iter().map(Scope::new).collect(),
            user_id_claim,
            display_name_claim,
        })
    }

    /// Resolves the given claim `name` from the already-verified ID token
    /// when it's one of [`STANDARD_CLAIMS`], falling back to one `/userinfo`
    /// request (typed with [`RawClaims`]) otherwise. `Ok(None)` means the
    /// claim genuinely isn't present anywhere -- callers, not this function,
    /// decide whether that's acceptable for the field they're resolving.
    async fn resolve_claim(
        &self,
        claims: &CoreIdTokenClaims,
        access_token: &AccessToken,
        name: &str,
    ) -> Result<Option<String>, IdentityError> {
        if STANDARD_CLAIMS.contains(&name) {
            return Ok(standard_claim_value(claims, name));
        }
        let userinfo = self.fetch_userinfo(access_token, claims.subject()).await?;
        Ok(userinfo
            .0
            .get(name)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string))
    }

    /// Fetches `/userinfo`, typed as [`RawClaims`] rather than whatever the
    /// main client's `CoreIdTokenClaims` uses -- `openidconnect` supports
    /// this per-call (its own `examples/gitlab.rs` does the same), so no
    /// generic parameter of [`DiscoveredClient`] needs to change to support
    /// an arbitrary configured claim name.
    async fn fetch_userinfo(
        &self,
        access_token: &AccessToken,
        subject: &SubjectIdentifier,
    ) -> Result<RawClaims, IdentityError> {
        let request = self
            .client
            .user_info(access_token.clone(), Some(subject.clone()))
            .map_err(|e| IdentityError::from_source("provider has no userinfo endpoint", e))?;
        let claims: UserInfoClaims<RawClaims, CoreGenderClaim> = request
            .request_async(&self.http_client)
            .await
            .map_err(|e| IdentityError::from_source("userinfo request failed", e))?;
        Ok(claims.additional_claims().clone())
    }

    /// Resolves [`AuthenticatedIdentity::user_id`]: always resolve-or-error,
    /// whether `user_id_claim` was explicitly configured or defaulted to
    /// `sub`.
    async fn resolve_user_id(
        &self,
        claims: &CoreIdTokenClaims,
        access_token: &AccessToken,
    ) -> Result<UserId, IdentityError> {
        let name = self.user_id_claim.as_deref().unwrap_or("sub");
        let value = self
            .resolve_claim(claims, access_token, name)
            .await?
            .ok_or_else(|| {
                IdentityError::new(format!(
                    "the configured user-id claim {name:?} was not present in the ID token or \
                     userinfo response"
                ))
            })?;
        Ok(UserId::new(value))
    }

    /// Resolves [`AuthenticatedIdentity::display_name`]: an explicitly
    /// configured claim is resolve-or-error, same as the user id; left
    /// unconfigured, tries `preferred_username` and falls back to `sub`
    /// without ever failing or touching `/userinfo`.
    async fn resolve_display_name(
        &self,
        claims: &CoreIdTokenClaims,
        access_token: &AccessToken,
    ) -> Result<String, IdentityError> {
        let Some(name) = &self.display_name_claim else {
            return Ok(standard_claim_value(claims, "preferred_username")
                .unwrap_or_else(|| claims.subject().as_str().to_string()));
        };
        self.resolve_claim(claims, access_token, name)
            .await?
            .ok_or_else(|| {
                IdentityError::new(format!(
                    "the configured display-name claim {name:?} was not present in the ID \
                     token or userinfo response"
                ))
            })
    }
}

/// Reads `name` directly off an already-verified ID token. Only meaningful
/// for a `name` in [`STANDARD_CLAIMS`] -- any other name returns `None`
/// here regardless of what the token actually contains, because this
/// function has no way to look up an arbitrary claim by string.
fn standard_claim_value(claims: &CoreIdTokenClaims, name: &str) -> Option<String> {
    match name {
        "sub" => Some(claims.subject().as_str().to_string()),
        "preferred_username" => claims.preferred_username().map(|v| v.as_str().to_string()),
        "email" => claims.email().map(|v| v.as_str().to_string()),
        "name" => localized_value(claims.name()),
        "given_name" => localized_value(claims.given_name()),
        "family_name" => localized_value(claims.family_name()),
        "middle_name" => localized_value(claims.middle_name()),
        "nickname" => localized_value(claims.nickname()),
        _ => None,
    }
}

/// The unlocalized (`locale: None`) value of a [`LocalizedClaim`] field.
fn localized_value<T>(claim: Option<&LocalizedClaim<T>>) -> Option<String>
where
    T: Deref<Target = String>,
{
    claim.and_then(|c| c.get(None)).map(|v| v.as_str().to_string())
}

/// Builds the HTTP client every OIDC request goes through, optionally
/// trusting extra certificate authorities from the PEM bundle at
/// `tls_ca_bundle`.
///
/// `reqwest` here is compiled against `webpki-roots` — a *bundled* copy of the
/// public root store — and reads nothing from the system trust store, so
/// neither `SSL_CERT_FILE` nor `/etc/ssl/certs` can make an internally issued
/// identity provider reachable. This parameter is the only way to trust one.
/// The roots it adds are *additional*: the public store keeps working, so a
/// deployment mixing an internal IdP with public endpoints needs no second
/// configuration.
///
/// Every failure names the path. A trust anchor that was silently skipped
/// would surface later as an unexplained TLS handshake failure against the
/// issuer, which is a much harder thing to diagnose than a startup error.
async fn build_http_client(
    tls_ca_bundle: Option<&str>,
) -> Result<openidconnect::reqwest::Client, IdentityError> {
    let mut builder = openidconnect::reqwest::ClientBuilder::new()
        // Following redirects on discovery/token requests opens an SSRF
        // hole; the browser is what follows the *authorize* redirect.
        .redirect(openidconnect::reqwest::redirect::Policy::none());

    if let Some(path) = tls_ca_bundle {
        let pem = tokio::fs::read(path).await.map_err(|e| {
            IdentityError::from_source(format!("failed to read the TLS CA bundle {path:?}"), e)
        })?;
        let certificates =
            openidconnect::reqwest::Certificate::from_pem_bundle(&pem).map_err(|e| {
                IdentityError::from_source(format!("failed to parse the TLS CA bundle {path:?}"), e)
            })?;
        // `from_pem_bundle` treats a file with no PEM blocks in it as an
        // empty bundle rather than an error, so a path pointing at the wrong
        // file (or at a DER certificate) would otherwise succeed here and add
        // no trust anchors at all -- precisely the silent failure this
        // function exists to avoid.
        if certificates.is_empty() {
            return Err(IdentityError::new(format!(
                "the TLS CA bundle {path:?} contains no PEM certificates"
            )));
        }
        for certificate in certificates {
            builder = builder.add_root_certificate(certificate);
        }
    }

    builder
        .build()
        .map_err(|e| IdentityError::from_source("failed to build OIDC HTTP client", e))
}

#[async_trait]
impl IdentityProvider for OidcIdentityProvider {
    async fn begin_login(&self) -> Result<LoginRedirect, IdentityError> {
        let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

        let mut request = self
            .client
            .authorize_url(
                CoreAuthenticationFlow::AuthorizationCode,
                CsrfToken::new_random,
                Nonce::new_random,
            )
            .set_pkce_challenge(pkce_challenge);
        for scope in &self.scopes {
            request = request.add_scope(scope.clone());
        }
        let (authorize_url, csrf_state, nonce) = request.url();

        Ok(LoginRedirect {
            authorize_url: authorize_url.to_string(),
            csrf_state: csrf_state.secret().clone(),
            pkce_verifier: pkce_verifier.secret().clone(),
            nonce: nonce.secret().clone(),
        })
    }

    async fn complete_login(
        &self,
        callback: LoginCallback,
    ) -> Result<AuthenticatedIdentity, IdentityError> {
        if callback.state != callback.expected_state {
            return Err(IdentityError::new(
                "CSRF state mismatch: the callback's state did not match the one issued at login",
            ));
        }

        let token_request = self
            .client
            .exchange_code(AuthorizationCode::new(callback.code))
            .map_err(|e| IdentityError::from_source("provider has no token endpoint", e))?
            .set_pkce_verifier(PkceCodeVerifier::new(callback.pkce_verifier));

        let token_response = token_request
            .request_async(&self.http_client)
            .await
            .map_err(|e| IdentityError::from_source("token exchange failed", e))?;

        let id_token = token_response
            .id_token()
            .ok_or_else(|| IdentityError::new("provider did not return an ID token"))?;

        let nonce = Nonce::new(callback.expected_nonce);
        let verifier = self.client.id_token_verifier();
        let claims = id_token
            .claims(&verifier, &nonce)
            .map_err(|e| IdentityError::from_source("ID token validation failed", e))?;

        let access_token = token_response.access_token();
        let user_id = self.resolve_user_id(claims, access_token).await?;
        let display_name = self.resolve_display_name(claims, access_token).await?;
        Ok(AuthenticatedIdentity {
            user_id,
            display_name,
        })
    }
}
