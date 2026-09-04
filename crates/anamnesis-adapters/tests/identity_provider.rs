//! `OidcIdentityProvider` exercised end-to-end (discovery, PKCE, full
//! ID-token validation) against a mock OIDC provider running on loopback via
//! `wiremock`. Entirely offline: no real network access is needed or used,
//! only a local TCP socket.

use anamnesis_adapters::OidcIdentityProvider;
use anamnesis_app::{IdentityError, IdentityProvider, LoginCallback};
use chrono::{Duration, Utc};
use openidconnect::core::{
    CoreIdToken, CoreIdTokenClaims, CoreIdTokenFields, CoreJsonWebKeySet, CoreJwsSigningAlgorithm,
    CoreProviderMetadata, CoreResponseType, CoreRsaPrivateSigningKey, CoreSubjectIdentifierType,
    CoreTokenResponse, CoreTokenType,
};
use openidconnect::{
    AccessToken, Audience, AuthUrl, EmptyAdditionalClaims, EmptyAdditionalProviderMetadata,
    EmptyExtraTokenFields, IssuerUrl, JsonWebKeySetUrl, Nonce, PrivateSigningKey, ResponseTypes,
    StandardClaims, SubjectIdentifier, TokenUrl,
};
use rsa::pkcs1::EncodeRsaPrivateKey;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const CLIENT_ID: &str = "anamnesis-test-client";
const SUBJECT: &str = "user-0f1e2d3c";

/// A private RSA key plus the discovery document and JWKS that advertise its
/// public half, ready to be served from a mock provider.
struct MockProvider {
    server: MockServer,
    signing_key: CoreRsaPrivateSigningKey,
    provider_metadata: CoreProviderMetadata,
}

impl MockProvider {
    async fn start() -> Self {
        let server = MockServer::start().await;
        let issuer = server.uri();

        let mut rng = rsa::rand_core::OsRng;
        let private_key =
            rsa::RsaPrivateKey::new(&mut rng, 2048).expect("generate RSA key for test signing");
        let pem = private_key
            .to_pkcs1_pem(rsa::pkcs1::LineEnding::LF)
            .expect("encode RSA private key as PEM");
        let signing_key = CoreRsaPrivateSigningKey::from_pem(&pem, None)
            .expect("build a CoreRsaPrivateSigningKey from the generated PEM");

        let provider_metadata = CoreProviderMetadata::new(
            IssuerUrl::new(issuer.clone()).unwrap(),
            AuthUrl::new(format!("{issuer}/authorize")).unwrap(),
            JsonWebKeySetUrl::new(format!("{issuer}/jwks")).unwrap(),
            vec![ResponseTypes::new(vec![CoreResponseType::Code])],
            vec![CoreSubjectIdentifierType::Public],
            vec![CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256],
            EmptyAdditionalProviderMetadata {},
        )
        .set_token_endpoint(Some(TokenUrl::new(format!("{issuer}/token")).unwrap()));

        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&provider_metadata))
            .mount(&server)
            .await;

        let jwks = CoreJsonWebKeySet::new(vec![signing_key.as_verification_key()]);
        Mock::given(method("GET"))
            .and(path("/jwks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&jwks))
            .mount(&server)
            .await;

        Self {
            server,
            signing_key,
            provider_metadata,
        }
    }

    fn issuer_url(&self) -> String {
        self.server.uri()
    }

    /// Registers a `/token` response carrying a validly signed ID token for
    /// `nonce`. Kept separate from `start` because the nonce is only known
    /// after `begin_login` has generated one.
    async fn respond_to_token_exchange_with_nonce(&self, nonce: &Nonce) {
        let now = Utc::now();
        let claims = CoreIdTokenClaims::new(
            self.provider_metadata.issuer().clone(),
            vec![Audience::new(CLIENT_ID.to_string())],
            now + Duration::seconds(300),
            now,
            StandardClaims::new(SubjectIdentifier::new(SUBJECT.to_string())),
            EmptyAdditionalClaims {},
        )
        .set_nonce(Some(nonce.clone()));

        let access_token = AccessToken::new("test-access-token".to_string());
        let id_token = CoreIdToken::new(
            claims,
            &self.signing_key,
            CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
            Some(&access_token),
            None,
        )
        .expect("sign test ID token");

        let token_response = CoreTokenResponse::new(
            access_token,
            CoreTokenType::Bearer,
            CoreIdTokenFields::new(Some(id_token), EmptyExtraTokenFields {}),
        );

        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&token_response))
            .mount(&self.server)
            .await;
    }
}

async fn discover(provider: &MockProvider) -> OidcIdentityProvider {
    discover_with_ca_bundle(provider, None)
        .await
        .expect("discovery against the mock provider should succeed")
}

/// As [`discover`], but with an explicit `ANAMNESIS_TLS_CA_BUNDLE` path and
/// the error surfaced rather than unwrapped.
async fn discover_with_ca_bundle(
    provider: &MockProvider,
    tls_ca_bundle: Option<&str>,
) -> Result<OidcIdentityProvider, IdentityError> {
    OidcIdentityProvider::discover(
        &provider.issuer_url(),
        CLIENT_ID.to_string(),
        Some("test-client-secret".to_string()),
        "https://anamnesis.example/auth/callback".to_string(),
        vec!["openid".to_string(), "profile".to_string()],
        tls_ca_bundle,
    )
    .await
}

/// Discovers with `bundle` as the CA bundle path, asserts it failed, and
/// hands back the error message. `map(|_| ())` is only there because
/// `OidcIdentityProvider` has no `Debug` for `expect_err` to print.
async fn ca_bundle_error(provider: &MockProvider, bundle: &std::path::Path) -> String {
    discover_with_ca_bundle(provider, Some(bundle.to_str().unwrap()))
        .await
        .map(|_| ())
        .expect_err("an unusable CA bundle should fail discovery")
        .to_string()
}

/// A CA bundle that cannot be read is a startup failure naming the path, not
/// a silent fall-back to the built-in roots — which would turn a
/// misconfiguration into an unexplained TLS handshake failure later.
#[tokio::test]
async fn an_unreadable_ca_bundle_fails_naming_the_path() {
    let provider = MockProvider::start().await;
    let dir = tempfile::tempdir().expect("create temp dir");
    let missing = dir.path().join("no-such-ca.crt");

    let message = ca_bundle_error(&provider, &missing).await;

    assert!(
        message.contains("no-such-ca.crt"),
        "the error should name the bundle path, got: {message}"
    );
}

/// The same contract for a file that exists but holds no certificates. This
/// is the case that needs an explicit check: `Certificate::from_pem_bundle`
/// reads a file with no PEM blocks as an *empty* bundle, not an error, so
/// without one a bundle pointed at the wrong file would add no trust anchors
/// and say nothing about it.
#[tokio::test]
async fn a_ca_bundle_with_no_certificates_fails_naming_the_path() {
    let provider = MockProvider::start().await;
    let dir = tempfile::tempdir().expect("create temp dir");
    let bundle = dir.path().join("garbage-ca.crt");
    std::fs::write(&bundle, b"this is not a PEM certificate").expect("write the bundle");

    let message = ca_bundle_error(&provider, &bundle).await;

    assert!(
        message.contains("garbage-ca.crt"),
        "the error should name the bundle path, got: {message}"
    );
}

#[tokio::test]
async fn begin_login_builds_an_authorize_url_against_the_discovered_endpoint() {
    let provider = MockProvider::start().await;
    let identity = discover(&provider).await;

    let redirect = identity.begin_login().await.expect("begin_login");

    assert!(
        redirect
            .authorize_url
            .starts_with(&format!("{}/authorize", provider.issuer_url())),
        "authorize_url was: {}",
        redirect.authorize_url
    );
    assert!(redirect.authorize_url.contains("code_challenge="));
    assert!(
        redirect
            .authorize_url
            .contains(&format!("client_id={CLIENT_ID}"))
    );
    assert!(!redirect.csrf_state.is_empty());
    assert!(!redirect.pkce_verifier.is_empty());
    assert!(!redirect.nonce.is_empty());
}

#[tokio::test]
async fn complete_login_validates_the_id_token_and_returns_the_subject_as_user_id() {
    let provider = MockProvider::start().await;
    let identity = discover(&provider).await;

    let redirect = identity.begin_login().await.expect("begin_login");
    provider
        .respond_to_token_exchange_with_nonce(&Nonce::new(redirect.nonce.clone()))
        .await;

    let user = identity
        .complete_login(LoginCallback {
            code: "test-authorization-code".to_string(),
            state: redirect.csrf_state.clone(),
            expected_state: redirect.csrf_state.clone(),
            pkce_verifier: redirect.pkce_verifier.clone(),
            expected_nonce: redirect.nonce.clone(),
        })
        .await
        .expect("complete_login should validate the signed ID token");

    assert_eq!(user.as_str(), SUBJECT);
}

#[tokio::test]
async fn complete_login_rejects_a_state_mismatch_without_contacting_the_token_endpoint() {
    let provider = MockProvider::start().await;
    let identity = discover(&provider).await;

    let redirect = identity.begin_login().await.expect("begin_login");
    // Deliberately no `/token` mock registered: if the CSRF check didn't
    // short-circuit, this test would fail on a connection/mock-miss error
    // instead of the CSRF error we're asserting on.

    let err = identity
        .complete_login(LoginCallback {
            code: "test-authorization-code".to_string(),
            state: "attacker-supplied-state".to_string(),
            expected_state: redirect.csrf_state.clone(),
            pkce_verifier: redirect.pkce_verifier.clone(),
            expected_nonce: redirect.nonce.clone(),
        })
        .await
        .expect_err("mismatched state must be rejected");

    assert!(
        err.to_string().to_lowercase().contains("state"),
        "error was: {err}"
    );
}

#[tokio::test]
async fn complete_login_rejects_a_token_signed_for_the_wrong_nonce() {
    let provider = MockProvider::start().await;
    let identity = discover(&provider).await;

    let redirect = identity.begin_login().await.expect("begin_login");
    // Sign the ID token for a *different* nonce than the one begin_login
    // generated, simulating a replayed or forged token.
    provider
        .respond_to_token_exchange_with_nonce(&Nonce::new("a-different-nonce".to_string()))
        .await;

    let err: IdentityError = identity
        .complete_login(LoginCallback {
            code: "test-authorization-code".to_string(),
            state: redirect.csrf_state.clone(),
            expected_state: redirect.csrf_state.clone(),
            pkce_verifier: redirect.pkce_verifier.clone(),
            expected_nonce: redirect.nonce.clone(),
        })
        .await
        .expect_err("a token signed for the wrong nonce must be rejected");

    let _ = err; // the message's exact wording is the openidconnect crate's; only rejection matters.
}
