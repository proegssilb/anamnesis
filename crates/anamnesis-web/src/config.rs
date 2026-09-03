//! Typed configuration, resolved once at startup from environment variables.
//! Every required variable that is missing or malformed fails loudly,
//! naming the variable — nothing here silently falls back to a default that
//! was not explicitly documented as one.

use std::net::SocketAddr;
use std::str::FromStr;

use axum_extra::extract::cookie::Key;

/// The application's fully validated configuration.
///
/// Deliberately **not** `#[derive(Debug)]`: this struct is the one place
/// every secret the process holds passes through, and a derived `Debug`
/// would print them in full anywhere a `Config` reached a log line or a
/// panic message (CWE-312). The hand-written impl below redacts them.
#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub bind_addr: SocketAddr,
    pub base_url: String,
    pub oidc_issuer_url: Option<String>,
    pub oidc_client_id: Option<String>,
    pub oidc_client_secret: Option<Secret>,
    pub oidc_scopes: Vec<String>,
    /// The cookie signing key derived from `ANAMNESIS_SESSION_SECRET`. The
    /// raw secret is consumed by [`resolve_cookie_key`] and never stored, so
    /// there is no cleartext session credential anywhere in this struct.
    pub cookie_key: Key,
    pub dev_auth_bypass: bool,
    /// An IANA time zone name (e.g. `"America/New_York"`), required once
    /// scheduled sweeps exist (`docs/DOMAIN.md` §6: "'every other Monday' is
    /// meaningless without one"). Validated for real against a `TimezoneResolver`
    /// at startup in `main.rs`, not here — this crate has no tzdb of its own.
    pub timezone: String,
    /// The subject (OIDC `sub`, or the dev-bypass user id) granted System
    /// Admin on first boot of an empty database — see `crate::bootstrap`.
    pub bootstrap_admin: String,
    /// The local filesystem directory `anamnesis_adapters::FsBlobStore`
    /// roots file attachments under (`docs/DOMAIN.md` §3). Not security- or
    /// correctness-sensitive the way `ANAMNESIS_SESSION_SECRET` is, so —
    /// unlike every `require`d field above — it defaults rather than failing
    /// startup when unset, exactly like `ANAMNESIS_BIND_ADDR`.
    pub blob_root: String,
}

/// A credential that must stay a `String` because the API consuming it takes
/// one — `OidcIdentityProvider::discover`'s `client_secret` parameter.
///
/// The wrapper exists purely so the value cannot be printed by accident: the
/// only way to the cleartext is [`Secret::expose`], which is greppable and
/// reads as a deliberate act at the call site, while `Debug` renders a
/// placeholder.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: String) -> Self {
        Secret(value)
    }

    /// The cleartext credential. Every call site is a place a secret leaves
    /// the type's protection, so keep them few and obvious.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// Hand-written so neither credential can be printed. `cookie_key` is
/// rendered as a placeholder (`Key` has no `Debug` of its own, and its bytes
/// are the session signing key); `oidc_client_secret` redacts itself via
/// [`Secret`]'s own impl. Every other field is ordinary configuration and
/// prints normally, which is what makes this struct still debuggable.
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("database_url", &self.database_url)
            .field("bind_addr", &self.bind_addr)
            .field("base_url", &self.base_url)
            .field("oidc_issuer_url", &self.oidc_issuer_url)
            .field("oidc_client_id", &self.oidc_client_id)
            .field("oidc_client_secret", &self.oidc_client_secret)
            .field("oidc_scopes", &self.oidc_scopes)
            .field("cookie_key", &"<redacted>")
            .field("dev_auth_bypass", &self.dev_auth_bypass)
            .field("timezone", &self.timezone)
            .field("bootstrap_admin", &self.bootstrap_admin)
            .field("blob_root", &self.blob_root)
            .finish()
    }
}

const DEFAULT_BIND_ADDR: &str = "0.0.0.0:8080";
const DEFAULT_OIDC_SCOPES: &str = "openid profile email";
const DEFAULT_BLOB_ROOT: &str = "./data/blobs";
/// The floor `axum_extra`'s `Key::from` accepts without panicking. Enforced
/// on `ANAMNESIS_SESSION_SECRET` so a short secret is a named configuration
/// error at startup rather than a panic deep in cookie signing.
const MIN_SESSION_SECRET_BYTES: usize = 64;

/// Why configuration could not be resolved. The `Display` message always
/// names the offending environment variable.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("missing required environment variable {0}")]
    Missing(&'static str),
    #[error("invalid value for environment variable {name}: {reason}")]
    Invalid { name: &'static str, reason: String },
}

impl Config {
    /// Resolves configuration from the process environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_source(|key| std::env::var(key).ok())
    }

    /// Resolves configuration from an arbitrary key lookup function. Kept
    /// separate from [`Config::from_env`] so validation can be unit tested
    /// without touching real process environment (which is global, mutable,
    /// and shared across test threads).
    pub fn from_source(get: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let dev_auth_bypass = match get("ANAMNESIS_DEV_AUTH_BYPASS").as_deref() {
            None => false,
            Some(raw) => parse_bool(raw),
        };

        let database_url = require(&get, "ANAMNESIS_DATABASE_URL")?;
        let base_url = require(&get, "ANAMNESIS_BASE_URL")?;
        let bind_addr = resolve_bind_addr(&get)?;
        let cookie_key = resolve_cookie_key(&get)?;
        let oidc_scopes = resolve_oidc_scopes(&get);
        let (oidc_issuer_url, oidc_client_id, oidc_client_secret) =
            resolve_oidc_credentials(&get, dev_auth_bypass)?;
        let timezone = require(&get, "ANAMNESIS_TIMEZONE")?;
        let bootstrap_admin = require(&get, "ANAMNESIS_BOOTSTRAP_ADMIN")?;
        let blob_root = get("ANAMNESIS_BLOB_ROOT").unwrap_or_else(|| DEFAULT_BLOB_ROOT.to_string());

        Ok(Config {
            database_url,
            bind_addr,
            base_url,
            oidc_issuer_url,
            oidc_client_id,
            oidc_client_secret,
            oidc_scopes,
            cookie_key,
            dev_auth_bypass,
            timezone,
            bootstrap_admin,
            blob_root,
        })
    }

    /// Whether [`Config::base_url`] is an `https://` URL — decides whether
    /// the session cookie gets the `Secure` attribute.
    pub fn base_url_is_https(&self) -> bool {
        self.base_url.starts_with("https://")
    }
}

fn require(
    get: &impl Fn(&str) -> Option<String>,
    name: &'static str,
) -> Result<String, ConfigError> {
    get(name)
        .filter(|v| !v.is_empty())
        .ok_or(ConfigError::Missing(name))
}

/// `ANAMNESIS_BIND_ADDR`, defaulting to [`DEFAULT_BIND_ADDR`] when unset.
fn resolve_bind_addr(get: &impl Fn(&str) -> Option<String>) -> Result<SocketAddr, ConfigError> {
    match get("ANAMNESIS_BIND_ADDR") {
        Some(raw) => SocketAddr::from_str(&raw).map_err(|e| ConfigError::Invalid {
            name: "ANAMNESIS_BIND_ADDR",
            reason: e.to_string(),
        }),
        None => Ok(SocketAddr::from_str(DEFAULT_BIND_ADDR).expect("default bind addr is valid")),
    }
}

/// `ANAMNESIS_SESSION_SECRET`, validated to at least
/// [`MIN_SESSION_SECRET_BYTES`] and immediately derived into the cookie
/// signing key.
///
/// The raw secret is a local that dies with this call, so no cleartext
/// session credential is ever stored in [`Config`] or reachable through its
/// `Debug`. Deriving here also puts the length check next to the `Key::from`
/// it exists to protect — that call panics below the same floor, and it used
/// to sit in `main.rs`, a whole module away from the check guarding it.
fn resolve_cookie_key(get: &impl Fn(&str) -> Option<String>) -> Result<Key, ConfigError> {
    let secret = require(get, "ANAMNESIS_SESSION_SECRET")?;
    if secret.len() < MIN_SESSION_SECRET_BYTES {
        return Err(ConfigError::Invalid {
            name: "ANAMNESIS_SESSION_SECRET",
            reason: format!(
                "must be at least {MIN_SESSION_SECRET_BYTES} bytes, got {}",
                secret.len()
            ),
        });
    }
    Ok(Key::from(secret.as_bytes()))
}

/// `ANAMNESIS_OIDC_SCOPES`, whitespace-split, defaulting to
/// [`DEFAULT_OIDC_SCOPES`] when unset.
fn resolve_oidc_scopes(get: &impl Fn(&str) -> Option<String>) -> Vec<String> {
    get("ANAMNESIS_OIDC_SCOPES")
        .unwrap_or_else(|| DEFAULT_OIDC_SCOPES.to_string())
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// `(oidc_issuer_url, oidc_client_id, oidc_client_secret)`, all `Some` once
/// [`resolve_oidc_credentials`] succeeds unless `dev_auth_bypass` left them
/// unset. Named so the signature below reads instead of forcing clippy's
/// `type_complexity` lint to spell the tuple out.
type OidcCredentials = (Option<String>, Option<String>, Option<Secret>);

/// The OIDC issuer/client-id/client-secret triple. Required when
/// `dev_auth_bypass` is off (a real deployment must be able to authenticate
/// against a real provider); merely read-through-if-present when it is on
/// (the dev bypass never needs them, but a caller may still want to see
/// what's configured).
fn resolve_oidc_credentials(
    get: &impl Fn(&str) -> Option<String>,
    dev_auth_bypass: bool,
) -> Result<OidcCredentials, ConfigError> {
    if dev_auth_bypass {
        return Ok((
            get("ANAMNESIS_OIDC_ISSUER_URL"),
            get("ANAMNESIS_OIDC_CLIENT_ID"),
            get("ANAMNESIS_OIDC_CLIENT_SECRET").map(Secret::new),
        ));
    }
    Ok((
        Some(require(get, "ANAMNESIS_OIDC_ISSUER_URL")?),
        Some(require(get, "ANAMNESIS_OIDC_CLIENT_ID")?),
        Some(Secret::new(require(get, "ANAMNESIS_OIDC_CLIENT_SECRET")?)),
    ))
}

fn parse_bool(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |key| map.get(key).cloned()
    }

    fn full_valid_env() -> Vec<(&'static str, &'static str)> {
        vec![
            ("ANAMNESIS_DATABASE_URL", "sqlite://test.db"),
            ("ANAMNESIS_BASE_URL", "http://localhost:8080"),
            ("ANAMNESIS_OIDC_ISSUER_URL", "https://idp.example.com"),
            ("ANAMNESIS_OIDC_CLIENT_ID", "client"),
            ("ANAMNESIS_OIDC_CLIENT_SECRET", "secret"),
            (
                "ANAMNESIS_SESSION_SECRET",
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            ),
            ("ANAMNESIS_TIMEZONE", "America/New_York"),
            ("ANAMNESIS_BOOTSTRAP_ADMIN", "alice"),
        ]
    }

    #[test]
    fn resolves_a_fully_specified_environment() {
        let cfg = Config::from_source(env(&full_valid_env())).unwrap();
        assert_eq!(cfg.database_url, "sqlite://test.db");
        assert_eq!(cfg.base_url, "http://localhost:8080");
        assert_eq!(
            cfg.oidc_issuer_url.as_deref(),
            Some("https://idp.example.com")
        );
        assert_eq!(cfg.oidc_client_id.as_deref(), Some("client"));
        assert_eq!(
            cfg.oidc_client_secret.as_ref().map(Secret::expose),
            Some("secret")
        );
        assert_eq!(cfg.oidc_scopes, vec!["openid", "profile", "email"]);
        assert_eq!(cfg.bind_addr, SocketAddr::from_str("0.0.0.0:8080").unwrap());
        assert!(!cfg.dev_auth_bypass);
    }

    #[test]
    fn missing_database_url_names_the_variable() {
        let mut pairs = full_valid_env();
        pairs.retain(|(k, _)| *k != "ANAMNESIS_DATABASE_URL");
        let err = Config::from_source(env(&pairs)).unwrap_err();
        assert_eq!(err, ConfigError::Missing("ANAMNESIS_DATABASE_URL"));
    }

    #[test]
    fn missing_oidc_settings_are_required_unless_dev_bypass() {
        let mut pairs = full_valid_env();
        pairs.retain(|(k, _)| *k != "ANAMNESIS_OIDC_ISSUER_URL");
        let err = Config::from_source(env(&pairs)).unwrap_err();
        assert_eq!(err, ConfigError::Missing("ANAMNESIS_OIDC_ISSUER_URL"));
    }

    #[test]
    fn dev_bypass_makes_oidc_settings_optional() {
        let mut pairs = full_valid_env();
        pairs.retain(|(k, _)| !k.starts_with("ANAMNESIS_OIDC"));
        pairs.push(("ANAMNESIS_DEV_AUTH_BYPASS", "1"));
        let cfg = Config::from_source(env(&pairs)).unwrap();
        assert!(cfg.dev_auth_bypass);
        assert_eq!(cfg.oidc_issuer_url, None);
    }

    #[test]
    fn session_secret_shorter_than_64_bytes_is_rejected() {
        let mut pairs = full_valid_env();
        pairs.retain(|(k, _)| *k != "ANAMNESIS_SESSION_SECRET");
        pairs.push(("ANAMNESIS_SESSION_SECRET", "too-short"));
        let err = Config::from_source(env(&pairs)).unwrap_err();
        assert_eq!(
            err,
            ConfigError::Invalid {
                name: "ANAMNESIS_SESSION_SECRET",
                reason: "must be at least 64 bytes, got 9".to_string(),
            }
        );
    }

    /// Distinctive, long-enough stand-ins for the two credentials, so a
    /// `Debug` leak shows up as an exact substring match rather than a
    /// coincidence.
    const SESSION_CANARY: &str =
        "canary-session-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const OIDC_CANARY: &str = "canary-oidc-client-secret";

    /// The hazard `Config`'s hand-written `Debug` exists to prevent: a
    /// derived one would print both credentials in full wherever a `Config`
    /// reached a log line or a panic message (CWE-312). Guards against a
    /// future `#[derive(Debug)]` silently reintroducing it.
    #[test]
    fn debug_renders_neither_credential() {
        let mut pairs = full_valid_env();
        pairs.retain(|(k, _)| {
            *k != "ANAMNESIS_SESSION_SECRET" && *k != "ANAMNESIS_OIDC_CLIENT_SECRET"
        });
        pairs.push(("ANAMNESIS_SESSION_SECRET", SESSION_CANARY));
        pairs.push(("ANAMNESIS_OIDC_CLIENT_SECRET", OIDC_CANARY));

        let cfg = Config::from_source(env(&pairs)).unwrap();
        let rendered = format!("{cfg:?}");

        assert!(
            !rendered.contains(SESSION_CANARY),
            "the session secret must never reach a Debug rendering: {rendered}"
        );
        assert!(
            !rendered.contains(OIDC_CANARY),
            "the OIDC client secret must never reach a Debug rendering: {rendered}"
        );
        // Still useful for debugging: the non-secret fields do print.
        assert!(rendered.contains("America/New_York"));
    }

    #[test]
    fn secret_debug_is_redacted_but_expose_returns_the_value() {
        let secret = Secret::new(OIDC_CANARY.to_string());
        assert_eq!(format!("{secret:?}"), "Secret(<redacted>)");
        assert_eq!(secret.expose(), OIDC_CANARY);
    }

    #[test]
    fn custom_bind_addr_is_parsed() {
        let mut pairs = full_valid_env();
        pairs.push(("ANAMNESIS_BIND_ADDR", "127.0.0.1:3000"));
        let cfg = Config::from_source(env(&pairs)).unwrap();
        assert_eq!(
            cfg.bind_addr,
            SocketAddr::from_str("127.0.0.1:3000").unwrap()
        );
    }

    #[test]
    fn invalid_bind_addr_is_rejected_by_name() {
        let mut pairs = full_valid_env();
        pairs.push(("ANAMNESIS_BIND_ADDR", "not-an-addr"));
        let err = Config::from_source(env(&pairs)).unwrap_err();
        assert!(matches!(
            err,
            ConfigError::Invalid {
                name: "ANAMNESIS_BIND_ADDR",
                ..
            }
        ));
    }

    #[test]
    fn resolves_the_timezone_and_bootstrap_admin() {
        let cfg = Config::from_source(env(&full_valid_env())).unwrap();
        assert_eq!(cfg.timezone, "America/New_York");
        assert_eq!(cfg.bootstrap_admin, "alice");
    }

    #[test]
    fn missing_timezone_names_the_variable() {
        let mut pairs = full_valid_env();
        pairs.retain(|(k, _)| *k != "ANAMNESIS_TIMEZONE");
        let err = Config::from_source(env(&pairs)).unwrap_err();
        assert_eq!(err, ConfigError::Missing("ANAMNESIS_TIMEZONE"));
    }

    #[test]
    fn missing_bootstrap_admin_names_the_variable() {
        let mut pairs = full_valid_env();
        pairs.retain(|(k, _)| *k != "ANAMNESIS_BOOTSTRAP_ADMIN");
        let err = Config::from_source(env(&pairs)).unwrap_err();
        assert_eq!(err, ConfigError::Missing("ANAMNESIS_BOOTSTRAP_ADMIN"));
    }

    #[test]
    fn blob_root_defaults_when_unset() {
        let cfg = Config::from_source(env(&full_valid_env())).unwrap();
        assert_eq!(cfg.blob_root, DEFAULT_BLOB_ROOT);
    }

    #[test]
    fn blob_root_is_overridable() {
        let mut pairs = full_valid_env();
        pairs.push(("ANAMNESIS_BLOB_ROOT", "/var/lib/anamnesis/blobs"));
        let cfg = Config::from_source(env(&pairs)).unwrap();
        assert_eq!(cfg.blob_root, "/var/lib/anamnesis/blobs");
    }

    #[test]
    fn https_base_url_is_detected() {
        let mut pairs = full_valid_env();
        pairs.retain(|(k, _)| *k != "ANAMNESIS_BASE_URL");
        pairs.push(("ANAMNESIS_BASE_URL", "https://anamnesis.example.com"));
        let cfg = Config::from_source(env(&pairs)).unwrap();
        assert!(cfg.base_url_is_https());
    }
}
