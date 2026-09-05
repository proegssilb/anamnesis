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
    EmptyExtraTokenFields, EndUserUsername, IssuerUrl, JsonWebKeySetUrl, Nonce, PrivateSigningKey,
    ResponseTypes, StandardClaims, SubjectIdentifier, TokenUrl, UserInfoUrl,
};
use rsa::pkcs1::EncodeRsaPrivateKey;
use serde_json::json;
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
        .set_token_endpoint(Some(TokenUrl::new(format!("{issuer}/token")).unwrap()))
        .set_userinfo_endpoint(Some(
            UserInfoUrl::new(format!("{issuer}/userinfo")).unwrap(),
        ));

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
        self.respond_to_token_exchange(nonce, None).await;
    }

    /// As [`Self::respond_to_token_exchange_with_nonce`], but also setting
    /// the `preferred_username` standard claim on the signed ID token when
    /// given one — for exercising the display-name default chain.
    async fn respond_to_token_exchange(&self, nonce: &Nonce, preferred_username: Option<&str>) {
        let now = Utc::now();
        let mut standard_claims = StandardClaims::new(SubjectIdentifier::new(SUBJECT.to_string()));
        if let Some(name) = preferred_username {
            standard_claims = standard_claims
                .set_preferred_username(Some(EndUserUsername::new(name.to_string())));
        }
        let claims = CoreIdTokenClaims::new(
            self.provider_metadata.issuer().clone(),
            vec![Audience::new(CLIENT_ID.to_string())],
            now + Duration::seconds(300),
            now,
            standard_claims,
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

    /// Registers a `/userinfo` response carrying `sub` plus one extra custom
    /// claim -- exercises the fallback path `ClaimSource` takes for a
    /// configured claim name outside the recognized standard set.
    async fn respond_to_userinfo(&self, claim_name: &str, claim_value: &str) {
        self.respond_to_userinfo_json(json!({
            "sub": SUBJECT,
            claim_name: claim_value,
        }))
        .await;
    }

    /// As [`MockProvider::respond_to_userinfo`], but with the whole response
    /// body given explicitly -- for claims whose value is not a string.
    async fn respond_to_userinfo_json(&self, body: serde_json::Value) {
        Mock::given(method("GET"))
            .and(path("/userinfo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&body))
            .mount(&self.server)
            .await;
    }

    /// How many `/userinfo` requests this provider has actually served.
    async fn userinfo_request_count(&self) -> usize {
        self.server
            .received_requests()
            .await
            .expect("the mock server records requests")
            .iter()
            .filter(|request| request.url.path() == "/userinfo")
            .count()
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
    discover_with_claims(provider, tls_ca_bundle, None, None, None).await
}

/// As [`discover`], but with an explicitly configured groups claim name and
/// the default user-id/display-name resolution.
async fn discover_with_groups_claim(
    provider: &MockProvider,
    groups_claim: &str,
) -> OidcIdentityProvider {
    discover_with_claims(provider, None, None, None, Some(groups_claim))
        .await
        .expect("discovery should succeed")
}

/// As [`discover`], but with explicitly configured user-id/display-name/
/// groups claim names -- for exercising the "explicitly configured" half of
/// the resolve-or-error contract.
async fn discover_with_claims(
    provider: &MockProvider,
    tls_ca_bundle: Option<&str>,
    user_id_claim: Option<&str>,
    display_name_claim: Option<&str>,
    groups_claim: Option<&str>,
) -> Result<OidcIdentityProvider, IdentityError> {
    OidcIdentityProvider::discover(
        &provider.issuer_url(),
        CLIENT_ID.to_string(),
        Some("test-client-secret".to_string()),
        "https://anamnesis.example/auth/callback".to_string(),
        vec!["openid".to_string(), "profile".to_string()],
        tls_ca_bundle,
        user_id_claim.map(str::to_string),
        display_name_claim.map(str::to_string),
        groups_claim.map(str::to_string),
    )
    .await
}

/// Drives a full login against `provider` with `identity`, from
/// `begin_login` through a matching `/token` response to `complete_login`.
async fn login(
    provider: &MockProvider,
    identity: &OidcIdentityProvider,
) -> anamnesis_app::AuthenticatedIdentity {
    let redirect = identity.begin_login().await.expect("begin_login");
    provider
        .respond_to_token_exchange_with_nonce(&Nonce::new(redirect.nonce.clone()))
        .await;
    identity
        .complete_login(LoginCallback {
            code: "test-authorization-code".to_string(),
            state: redirect.csrf_state.clone(),
            expected_state: redirect.csrf_state.clone(),
            pkce_verifier: redirect.pkce_verifier.clone(),
            expected_nonce: redirect.nonce,
        })
        .await
        .expect("complete_login should succeed")
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

    let identity_result = identity
        .complete_login(LoginCallback {
            code: "test-authorization-code".to_string(),
            state: redirect.csrf_state.clone(),
            expected_state: redirect.csrf_state.clone(),
            pkce_verifier: redirect.pkce_verifier.clone(),
            expected_nonce: redirect.nonce.clone(),
        })
        .await
        .expect("complete_login should validate the signed ID token");

    assert_eq!(identity_result.user_id.as_str(), SUBJECT);
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

#[tokio::test]
async fn unconfigured_display_name_falls_back_to_preferred_username_when_present() {
    let provider = MockProvider::start().await;
    let identity = discover(&provider).await;

    let redirect = identity.begin_login().await.expect("begin_login");
    provider
        .respond_to_token_exchange(&Nonce::new(redirect.nonce.clone()), Some("alice.example"))
        .await;

    let identity_result = identity
        .complete_login(LoginCallback {
            code: "test-authorization-code".to_string(),
            state: redirect.csrf_state.clone(),
            expected_state: redirect.csrf_state.clone(),
            pkce_verifier: redirect.pkce_verifier.clone(),
            expected_nonce: redirect.nonce.clone(),
        })
        .await
        .expect("complete_login should succeed");

    assert_eq!(identity_result.display_name, "alice.example");
}

#[tokio::test]
async fn unconfigured_display_name_falls_back_to_subject_when_preferred_username_absent() {
    let provider = MockProvider::start().await;
    let identity = discover(&provider).await;

    let redirect = identity.begin_login().await.expect("begin_login");
    provider
        .respond_to_token_exchange_with_nonce(&Nonce::new(redirect.nonce.clone()))
        .await;

    let identity_result = identity
        .complete_login(LoginCallback {
            code: "test-authorization-code".to_string(),
            state: redirect.csrf_state.clone(),
            expected_state: redirect.csrf_state.clone(),
            pkce_verifier: redirect.pkce_verifier.clone(),
            expected_nonce: redirect.nonce.clone(),
        })
        .await
        .expect("complete_login should succeed");

    assert_eq!(identity_result.display_name, SUBJECT);
}

#[tokio::test]
async fn configured_display_name_claim_resolves_a_standard_claim() {
    let provider = MockProvider::start().await;
    let identity = discover_with_claims(&provider, None, None, Some("preferred_username"), None)
        .await
        .expect("discovery should succeed");

    let redirect = identity.begin_login().await.expect("begin_login");
    provider
        .respond_to_token_exchange(&Nonce::new(redirect.nonce.clone()), Some("configured-name"))
        .await;

    let identity_result = identity
        .complete_login(LoginCallback {
            code: "test-authorization-code".to_string(),
            state: redirect.csrf_state.clone(),
            expected_state: redirect.csrf_state.clone(),
            pkce_verifier: redirect.pkce_verifier.clone(),
            expected_nonce: redirect.nonce.clone(),
        })
        .await
        .expect("complete_login should succeed");

    assert_eq!(identity_result.display_name, "configured-name");
}

#[tokio::test]
async fn configured_display_name_claim_resolves_a_custom_claim_via_userinfo() {
    let provider = MockProvider::start().await;
    let identity = discover_with_claims(&provider, None, None, Some("department"), None)
        .await
        .expect("discovery should succeed");

    let redirect = identity.begin_login().await.expect("begin_login");
    provider
        .respond_to_token_exchange_with_nonce(&Nonce::new(redirect.nonce.clone()))
        .await;
    provider
        .respond_to_userinfo("department", "Engineering")
        .await;

    let identity_result = identity
        .complete_login(LoginCallback {
            code: "test-authorization-code".to_string(),
            state: redirect.csrf_state.clone(),
            expected_state: redirect.csrf_state.clone(),
            pkce_verifier: redirect.pkce_verifier.clone(),
            expected_nonce: redirect.nonce.clone(),
        })
        .await
        .expect("complete_login should succeed, falling back to /userinfo for the custom claim");

    assert_eq!(identity_result.display_name, "Engineering");
}

#[tokio::test]
async fn configured_user_id_claim_resolves_a_custom_claim_via_userinfo() {
    let provider = MockProvider::start().await;
    let identity = discover_with_claims(&provider, None, Some("employee_id"), None, None)
        .await
        .expect("discovery should succeed");

    let redirect = identity.begin_login().await.expect("begin_login");
    provider
        .respond_to_token_exchange_with_nonce(&Nonce::new(redirect.nonce.clone()))
        .await;
    provider.respond_to_userinfo("employee_id", "E-42").await;

    let identity_result = identity
        .complete_login(LoginCallback {
            code: "test-authorization-code".to_string(),
            state: redirect.csrf_state.clone(),
            expected_state: redirect.csrf_state.clone(),
            pkce_verifier: redirect.pkce_verifier.clone(),
            expected_nonce: redirect.nonce.clone(),
        })
        .await
        .expect("complete_login should succeed, falling back to /userinfo for the custom claim");

    assert_eq!(identity_result.user_id.as_str(), "E-42");
}

#[tokio::test]
async fn configured_display_name_claim_that_cannot_be_resolved_is_a_hard_error() {
    let provider = MockProvider::start().await;
    let identity = discover_with_claims(&provider, None, None, Some("department"), None)
        .await
        .expect("discovery should succeed");

    let redirect = identity.begin_login().await.expect("begin_login");
    provider
        .respond_to_token_exchange_with_nonce(&Nonce::new(redirect.nonce.clone()))
        .await;
    // No /userinfo mock registered carrying "department" -- it comes back
    // with only "sub", so the configured claim can't be resolved anywhere.
    provider
        .respond_to_userinfo("unrelated_claim", "value")
        .await;

    let err = identity
        .complete_login(LoginCallback {
            code: "test-authorization-code".to_string(),
            state: redirect.csrf_state.clone(),
            expected_state: redirect.csrf_state.clone(),
            pkce_verifier: redirect.pkce_verifier.clone(),
            expected_nonce: redirect.nonce.clone(),
        })
        .await
        .expect_err("an explicitly configured, unresolvable claim must be a hard error");

    assert!(
        err.to_string().contains("department"),
        "error should name the unresolved claim, got: {err}"
    );
}

#[tokio::test]
async fn configured_user_id_claim_that_cannot_be_resolved_is_a_hard_error() {
    let provider = MockProvider::start().await;
    let identity = discover_with_claims(&provider, None, Some("employee_id"), None, None)
        .await
        .expect("discovery should succeed");

    let redirect = identity.begin_login().await.expect("begin_login");
    provider
        .respond_to_token_exchange_with_nonce(&Nonce::new(redirect.nonce.clone()))
        .await;
    provider
        .respond_to_userinfo("unrelated_claim", "value")
        .await;

    let err = identity
        .complete_login(LoginCallback {
            code: "test-authorization-code".to_string(),
            state: redirect.csrf_state.clone(),
            expected_state: redirect.csrf_state.clone(),
            pkce_verifier: redirect.pkce_verifier.clone(),
            expected_nonce: redirect.nonce.clone(),
        })
        .await
        .expect_err("an explicitly configured, unresolvable claim must be a hard error");

    assert!(
        err.to_string().contains("employee_id"),
        "error should name the unresolved claim, got: {err}"
    );
}

/// The regression that motivated gathering every claim into one
/// `ClaimSource`: resolving each configured claim on its own made one
/// byte-identical `/userinfo` request *per* non-standard claim name.
#[tokio::test]
async fn a_login_makes_at_most_one_userinfo_request_however_many_claims_are_custom() {
    let provider = MockProvider::start().await;
    let identity = discover_with_claims(
        &provider,
        None,
        Some("employee_id"),
        Some("department"),
        Some("groups"),
    )
    .await
    .expect("discovery should succeed");
    provider
        .respond_to_userinfo_json(json!({
            "sub": SUBJECT,
            "employee_id": "E-42",
            "department": "Engineering",
            "groups": ["anamnesis-admins"],
        }))
        .await;

    let result = login(&provider, &identity).await;

    assert_eq!(result.user_id.as_str(), "E-42");
    assert_eq!(result.display_name, "Engineering");
    assert_eq!(result.groups, vec!["anamnesis-admins".to_string()]);
    assert_eq!(
        provider.userinfo_request_count().await,
        1,
        "three custom claims must still cost one /userinfo request"
    );
}

/// The default. No groups claim configured means the whole group dimension
/// is inert, and — since nothing else needs it — `/userinfo` is never asked
/// for at all.
#[tokio::test]
async fn an_unconfigured_groups_claim_yields_no_groups_and_no_userinfo_request() {
    let provider = MockProvider::start().await;
    let identity = discover(&provider).await;

    let result = login(&provider, &identity).await;

    assert!(result.groups.is_empty());
    assert_eq!(provider.userinfo_request_count().await, 0);
}

#[tokio::test]
async fn a_groups_claim_holding_an_array_yields_its_string_elements() {
    let provider = MockProvider::start().await;
    let identity = discover_with_groups_claim(&provider, "groups").await;
    provider
        .respond_to_userinfo_json(json!({
            "sub": SUBJECT,
            // The middle element is not a string: skipped, not fatal.
            "groups": ["authentik Read-only", 17, "anamnesis-admins"],
        }))
        .await;

    let result = login(&provider, &identity).await;

    assert_eq!(
        result.groups,
        vec![
            "authentik Read-only".to_string(),
            "anamnesis-admins".to_string()
        ]
    );
}

/// Some providers emit a single-valued claim as a bare string rather than a
/// one-element array.
#[tokio::test]
async fn a_groups_claim_holding_a_bare_string_yields_one_group() {
    let provider = MockProvider::start().await;
    let identity = discover_with_groups_claim(&provider, "groups").await;
    provider
        .respond_to_userinfo("groups", "anamnesis-admins")
        .await;

    let result = login(&provider, &identity).await;

    assert_eq!(result.groups, vec!["anamnesis-admins".to_string()]);
}

/// Deliberately unlike the user-id and display-name claims, which are
/// resolve-or-error: being in no groups is a legitimate state, so a typo in
/// the configured claim name degrades to "nobody gets group grants" rather
/// than locking every user out of a working deployment.
#[tokio::test]
async fn an_absent_groups_claim_is_empty_rather_than_an_error() {
    let provider = MockProvider::start().await;
    let identity = discover_with_groups_claim(&provider, "groups").await;
    provider
        .respond_to_userinfo("unrelated_claim", "value")
        .await;

    let result = login(&provider, &identity).await;

    assert!(result.groups.is_empty());
    assert_eq!(
        result.user_id.as_str(),
        SUBJECT,
        "the rest of the login is unaffected"
    );
}
