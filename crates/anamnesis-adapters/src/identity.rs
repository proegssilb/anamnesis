//! [`OidcIdentityProvider`]: the `IdentityProvider` port backed by any
//! standards-compliant OpenID Connect provider, via the `openidconnect`
//! crate. Discovery, PKCE, and ID-token validation (signature, issuer,
//! audience, nonce) all live here; nothing here names Authentik or any
//! other specific provider — a provider-specific branch would be a bug.

use anamnesis_app::{IdentityError, IdentityProvider, LoginCallback, LoginRedirect};
use anamnesis_core::UserId;
use async_trait::async_trait;
use openidconnect::core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata};
use openidconnect::{
    AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointMaybeSet, EndpointNotSet,
    EndpointSet, IssuerUrl, Nonce, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope,
    TokenResponse as OidcTokenResponse,
};

/// The concrete client type produced by [`CoreClient::from_provider_metadata`]
/// once a redirect URI has been set: the auth endpoint is always present
/// after discovery (`EndpointSet`), the token endpoint is present on any
/// provider that supports the Authorization Code flow but is only checked at
/// call time (`EndpointMaybeSet`), and the remaining optional endpoints
/// (device auth, introspection, revocation) are not used here at all
/// (`EndpointNotSet`).
type DiscoveredClient = CoreClient<
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;

/// An [`IdentityProvider`] backed by OAuth2 Authorization Code + PKCE
/// against a discovered OpenID Connect provider.
pub struct OidcIdentityProvider {
    client: DiscoveredClient,
    http_client: openidconnect::reqwest::Client,
    scopes: Vec<Scope>,
}

impl OidcIdentityProvider {
    /// Discovers `issuer_url` (`GET {issuer}/.well-known/openid-configuration`
    /// plus its JWKS) and builds a provider ready to drive the Authorization
    /// Code + PKCE flow.
    ///
    /// `tls_ca_bundle` optionally names a PEM file of extra certificate
    /// authorities to trust — see [`build_http_client`].
    pub async fn discover(
        issuer_url: &str,
        client_id: String,
        client_secret: Option<String>,
        redirect_url: String,
        scopes: Vec<String>,
        tls_ca_bundle: Option<&str>,
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
        })
    }
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

    async fn complete_login(&self, callback: LoginCallback) -> Result<UserId, IdentityError> {
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

        Ok(UserId::new(claims.subject().as_str()))
    }
}
