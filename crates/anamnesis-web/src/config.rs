//! Typed configuration, resolved once at startup from environment variables.
//! Every required variable that is missing or malformed fails loudly,
//! naming the variable — nothing here silently falls back to a default that
//! was not explicitly documented as one.

use std::net::SocketAddr;
use std::str::FromStr;

/// The application's fully validated configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub bind_addr: SocketAddr,
    pub base_url: String,
    pub oidc_issuer_url: Option<String>,
    pub oidc_client_id: Option<String>,
    pub oidc_client_secret: Option<String>,
    pub oidc_scopes: Vec<String>,
    pub session_secret: String,
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

const DEFAULT_BIND_ADDR: &str = "0.0.0.0:8080";
const DEFAULT_OIDC_SCOPES: &str = "openid profile email";
const DEFAULT_BLOB_ROOT: &str = "./data/blobs";
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
        let session_secret = resolve_session_secret(&get)?;
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
            session_secret,
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

/// `ANAMNESIS_SESSION_SECRET`, required and enforced to be at least
/// [`MIN_SESSION_SECRET_BYTES`] long.
fn resolve_session_secret(get: &impl Fn(&str) -> Option<String>) -> Result<String, ConfigError> {
    let session_secret = require(get, "ANAMNESIS_SESSION_SECRET")?;
    if session_secret.len() < MIN_SESSION_SECRET_BYTES {
        return Err(ConfigError::Invalid {
            name: "ANAMNESIS_SESSION_SECRET",
            reason: format!(
                "must be at least {MIN_SESSION_SECRET_BYTES} bytes, got {}",
                session_secret.len()
            ),
        });
    }
    Ok(session_secret)
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
type OidcCredentials = (Option<String>, Option<String>, Option<String>);

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
            get("ANAMNESIS_OIDC_CLIENT_SECRET"),
        ));
    }
    Ok((
        Some(require(get, "ANAMNESIS_OIDC_ISSUER_URL")?),
        Some(require(get, "ANAMNESIS_OIDC_CLIENT_ID")?),
        Some(require(get, "ANAMNESIS_OIDC_CLIENT_SECRET")?),
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
        assert_eq!(cfg.oidc_client_secret.as_deref(), Some("secret"));
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
