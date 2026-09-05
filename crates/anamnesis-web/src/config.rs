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
    /// Which OIDC claim supplies the stable identity anchor
    /// (`AuthenticatedIdentity::user_id`). `None` means unconfigured: the
    /// adapter always falls back to `sub`, which the OIDC spec guarantees is
    /// present. `Some(name)` means an operator explicitly chose `name`; if
    /// that claim can't be resolved, login fails rather than silently using
    /// `sub` instead — see `anamnesis_adapters::OidcIdentityProvider`.
    pub oidc_user_id_claim: Option<String>,
    /// Which OIDC claim supplies the human-readable label
    /// (`AuthenticatedIdentity::display_name`). `None` means unconfigured:
    /// the adapter tries `preferred_username`, falling back to `sub`, and
    /// never fails login over it. `Some(name)` carries the same
    /// resolve-or-error contract as [`Self::oidc_user_id_claim`].
    pub oidc_display_name_claim: Option<String>,
    /// Which OIDC claim carries the user's groups
    /// (`AuthenticatedIdentity::groups`). `None` — the default — means the
    /// whole group dimension of authorization is inert: no groups are read,
    /// no rows are recorded, and no mapping can match anything. `Some(name)`
    /// reads `name` as a list of group names at each login. Unlike the two
    /// claims above, an unresolvable groups claim is *not* a login failure;
    /// it yields no groups and a warning, so a typo cannot lock a working
    /// deployment out — see `anamnesis_adapters::OidcIdentityProvider`.
    pub oidc_groups_claim: Option<String>,
    /// A group whose members hold System Admin, seeded into
    /// `system_admin_groups` on every boot by [`crate::bootstrap`]. This
    /// exists for the same reason [`Self::bootstrap_admin`] does — the
    /// first-user problem — and is a seeding lever only: at request time the
    /// database is the single source of truth, and further admin groups are
    /// granted through the admin UI. Requires [`Self::oidc_groups_claim`].
    pub oidc_admin_group: Option<String>,
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
    /// Where file attachments live (`docs/DOMAIN.md` §3): an `s3://bucket`
    /// or `s3://bucket/prefix` URL selects
    /// `anamnesis_adapters::S3BlobStore`, and anything else is a local
    /// filesystem directory for `anamnesis_adapters::FsBlobStore` — the same
    /// dispatch-on-scheme `ANAMNESIS_DATABASE_URL` gets. Not security- or
    /// correctness-sensitive the way `ANAMNESIS_SESSION_SECRET` is, so —
    /// unlike every `require`d field above — it defaults rather than failing
    /// startup when unset, exactly like `ANAMNESIS_BIND_ADDR`.
    ///
    /// The name is historical: it was a directory before it could also be a
    /// bucket, and renaming the variable would break every existing
    /// deployment for no gain.
    pub blob_root: String,
    /// The object store's endpoint and credentials, resolved **only** when
    /// [`Self::blob_root`] is an `s3://` URL and `None` otherwise. Every
    /// filesystem deployment — which is every single-machine one — therefore
    /// never has to set an `ANAMNESIS_S3_*` variable, and an `s3://` root
    /// with no credentials fails at startup naming the missing one rather
    /// than on the first upload.
    pub s3: Option<S3Config>,
    /// An optional PEM bundle of extra certificate authorities to trust when
    /// talking to the OIDC provider, *in addition to* the public roots
    /// compiled in via `webpki-roots`.
    ///
    /// Nothing in this process reads the system trust store, so
    /// `SSL_CERT_FILE` and `/etc/ssl/certs` have no effect — an internally
    /// issued IdP certificate is unreachable without this. Postgres needs no
    /// equivalent: `sqlx` already accepts `?sslrootcert=` in the connection
    /// URL and honours `PGSSLROOTCERT`.
    pub tls_ca_bundle: Option<String>,
    /// The largest request body the server will accept, in bytes.
    ///
    /// This is a whole-request cap covering attachment uploads, so any proxy
    /// in front of Anamnesis must allow at least as much or its limit becomes
    /// the real one. Defaults to [`DEFAULT_MAX_BODY_BYTES`] rather than
    /// axum's own 2 MiB, which is far too small for the file attachments
    /// `docs/DOMAIN.md` §3 describes.
    pub max_body_bytes: usize,
}

/// What an `s3://` blob root needs to actually reach its bucket.
///
/// Every value is under this application's own `ANAMNESIS_S3_*` prefix
/// rather than the ambient `AWS_*` names: configuration here is one
/// validated-at-startup surface (`docs/DEPLOYMENT.md` §2), and a credential
/// picked up from an inherited `AWS_ACCESS_KEY_ID` would be the one input
/// this process could not name, check, or report on.
///
/// `#[derive(Debug)]` is safe here, unlike on [`Config`], only because
/// [`Secret`] redacts itself.
#[derive(Debug, Clone)]
pub struct S3Config {
    /// `ANAMNESIS_S3_ENDPOINT` — required for Garage or MinIO, and omitted
    /// only when the bucket really is on AWS.
    pub endpoint: Option<String>,
    /// `ANAMNESIS_S3_REGION` — signed over even by servers that ignore it,
    /// so it has to match what the server expects. Unset leaves the S3
    /// client's own default.
    pub region: Option<String>,
    pub access_key_id: String,
    pub secret_access_key: Secret,
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
            .field("oidc_user_id_claim", &self.oidc_user_id_claim)
            .field("oidc_display_name_claim", &self.oidc_display_name_claim)
            .field("oidc_groups_claim", &self.oidc_groups_claim)
            .field("oidc_admin_group", &self.oidc_admin_group)
            .field("cookie_key", &"<redacted>")
            .field("dev_auth_bypass", &self.dev_auth_bypass)
            .field("timezone", &self.timezone)
            .field("bootstrap_admin", &self.bootstrap_admin)
            .field("blob_root", &self.blob_root)
            .field("s3", &self.s3)
            .field("tls_ca_bundle", &self.tls_ca_bundle)
            .field("max_body_bytes", &self.max_body_bytes)
            .finish()
    }
}

const DEFAULT_BIND_ADDR: &str = "0.0.0.0:8080";
const DEFAULT_OIDC_SCOPES: &str = "openid profile email";
const DEFAULT_BLOB_ROOT: &str = "./data/blobs";
/// 40 MiB. Chosen as a deliberate ceiling for file attachments rather than
/// inherited from axum's 2 MiB `DefaultBodyLimit`, which rejects most real
/// documents. Raising it raises peak memory too: an upload is read fully
/// into a `Vec<u8>` (`Multipart::bytes`, then `BlobStore::put`), so the
/// worst case is roughly this figure times the number of concurrent uploads.
const DEFAULT_MAX_BODY_BYTES: usize = 40 * 1024 * 1024;
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
        let oidc_user_id_claim = get("ANAMNESIS_OIDC_USER_ID_CLAIM");
        let oidc_display_name_claim = get("ANAMNESIS_OIDC_DISPLAY_NAME_CLAIM");
        let (oidc_groups_claim, oidc_admin_group) = resolve_oidc_groups(&get)?;
        let timezone = require(&get, "ANAMNESIS_TIMEZONE")?;
        let bootstrap_admin = require(&get, "ANAMNESIS_BOOTSTRAP_ADMIN")?;
        let blob_root = get("ANAMNESIS_BLOB_ROOT").unwrap_or_else(|| DEFAULT_BLOB_ROOT.to_string());
        let s3 = resolve_s3(&get, &blob_root)?;
        let tls_ca_bundle = get("ANAMNESIS_TLS_CA_BUNDLE").filter(|v| !v.is_empty());
        let max_body_bytes = resolve_max_body_bytes(&get)?;

        Ok(Config {
            database_url,
            bind_addr,
            base_url,
            oidc_issuer_url,
            oidc_client_id,
            oidc_client_secret,
            oidc_scopes,
            oidc_user_id_claim,
            oidc_display_name_claim,
            oidc_groups_claim,
            oidc_admin_group,
            cookie_key,
            dev_auth_bypass,
            timezone,
            bootstrap_admin,
            blob_root,
            s3,
            tls_ca_bundle,
            max_body_bytes,
        })
    }

    /// Just the listen address, resolved from the environment without
    /// requiring any of the other variables.
    ///
    /// The `--health-check` probe (`crate::health`) needs to know which port
    /// to talk to and nothing else — it is a bare HTTP GET against an
    /// already-running server. Requiring a database URL and an OIDC client
    /// secret before it could run would make a container's `HEALTHCHECK`
    /// report unhealthy for reasons unrelated to the server's health.
    pub fn bind_addr_from_env() -> Result<SocketAddr, ConfigError> {
        resolve_bind_addr(&|key| std::env::var(key).ok())
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

/// `ANAMNESIS_MAX_BODY_BYTES`, defaulting to [`DEFAULT_MAX_BODY_BYTES`].
///
/// A plain byte count, not a human-readable size like `40MB`: every other
/// variable here is a plain value, and a size-string parser would be a new
/// dependency and a new class of malformed input for the sake of one knob.
/// Zero is rejected rather than accepted as "unlimited" — a limit of nothing
/// would reject every request, which is never what someone setting it meant.
fn resolve_max_body_bytes(get: &impl Fn(&str) -> Option<String>) -> Result<usize, ConfigError> {
    let Some(raw) = get("ANAMNESIS_MAX_BODY_BYTES").filter(|v| !v.is_empty()) else {
        return Ok(DEFAULT_MAX_BODY_BYTES);
    };
    let invalid = |reason: String| ConfigError::Invalid {
        name: "ANAMNESIS_MAX_BODY_BYTES",
        reason,
    };
    match raw.trim().parse::<usize>() {
        Ok(0) => Err(invalid("must be greater than zero".to_string())),
        Ok(bytes) => Ok(bytes),
        Err(e) => Err(invalid(format!("expected a byte count: {e}"))),
    }
}

/// The `ANAMNESIS_S3_*` family, but only when `blob_root` is an `s3://` URL
/// — on a filesystem blob root there is nothing to configure and any of
/// these that happen to be set are ignored.
///
/// The two credentials are `require`d, so an object-store deployment cannot
/// get as far as binding a socket and then fail its first upload with a
/// permissions error that looks like the bucket's fault. Endpoint and region
/// stay optional: unset means "AWS's own", which is exactly right when the
/// bucket really is on AWS and obviously wrong (and immediately visible) when
/// it is not.
fn resolve_s3(
    get: &impl Fn(&str) -> Option<String>,
    blob_root: &str,
) -> Result<Option<S3Config>, ConfigError> {
    if !blob_root.starts_with("s3://") {
        return Ok(None);
    }
    Ok(Some(S3Config {
        endpoint: get("ANAMNESIS_S3_ENDPOINT").filter(|v| !v.is_empty()),
        region: get("ANAMNESIS_S3_REGION").filter(|v| !v.is_empty()),
        access_key_id: require(get, "ANAMNESIS_S3_ACCESS_KEY_ID")?,
        secret_access_key: Secret::new(require(get, "ANAMNESIS_S3_SECRET_ACCESS_KEY")?),
    }))
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

/// `(oidc_groups_claim, oidc_admin_group)`, both `None` unless the operator
/// opted into the group dimension.
///
/// An admin group named without a groups claim is a hard error rather than a
/// silent no-op: it can never match anything, so the deployment would look
/// configured while granting nobody anything — precisely the failure mode
/// this module exists to prevent. The reverse is fine and expected: a groups
/// claim alone records groups and lets an existing System Admin map them
/// from the UI, which is how everything past the first admin is meant to
/// work.
fn resolve_oidc_groups(
    get: &impl Fn(&str) -> Option<String>,
) -> Result<(Option<String>, Option<String>), ConfigError> {
    let groups_claim = get("ANAMNESIS_OIDC_GROUPS_CLAIM").filter(|v| !v.is_empty());
    let admin_group = get("ANAMNESIS_OIDC_ADMIN_GROUP").filter(|v| !v.is_empty());
    if admin_group.is_some() && groups_claim.is_none() {
        return Err(ConfigError::Invalid {
            name: "ANAMNESIS_OIDC_ADMIN_GROUP",
            reason: "set without ANAMNESIS_OIDC_GROUPS_CLAIM, so no login could ever match it"
                .to_string(),
        });
    }
    Ok((groups_claim, admin_group))
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

    /// The env pairs an `s3://` blob root additionally needs.
    fn s3_env() -> Vec<(&'static str, &'static str)> {
        let mut pairs = full_valid_env();
        pairs.push(("ANAMNESIS_BLOB_ROOT", "s3://anamnesis/blobs"));
        pairs.push(("ANAMNESIS_S3_ACCESS_KEY_ID", "GK31c2f218a2e44f485b94239e"));
        pairs.push(("ANAMNESIS_S3_SECRET_ACCESS_KEY", S3_CANARY));
        pairs
    }

    const S3_CANARY: &str = "canary-s3-secret-access-key";

    #[test]
    fn a_filesystem_blob_root_needs_no_s3_settings() {
        let cfg = Config::from_source(env(&full_valid_env())).unwrap();
        assert!(cfg.s3.is_none());
    }

    #[test]
    fn stray_s3_settings_are_ignored_on_a_filesystem_blob_root() {
        // Not an error: an operator moving a deployment back to local disk
        // should not have to unset every variable to boot.
        let mut pairs = full_valid_env();
        pairs.push(("ANAMNESIS_S3_ENDPOINT", "https://garage.example.com:3900"));
        let cfg = Config::from_source(env(&pairs)).unwrap();
        assert!(cfg.s3.is_none());
    }

    #[test]
    fn an_s3_blob_root_resolves_its_endpoint_and_credentials() {
        let mut pairs = s3_env();
        pairs.push(("ANAMNESIS_S3_ENDPOINT", "https://garage.example.com:3900"));
        pairs.push(("ANAMNESIS_S3_REGION", "garage"));

        let cfg = Config::from_source(env(&pairs)).unwrap();
        let s3 = cfg.s3.expect("an s3:// blob root resolves S3 settings");
        assert_eq!(cfg.blob_root, "s3://anamnesis/blobs");
        assert_eq!(
            s3.endpoint.as_deref(),
            Some("https://garage.example.com:3900")
        );
        assert_eq!(s3.region.as_deref(), Some("garage"));
        assert_eq!(s3.access_key_id, "GK31c2f218a2e44f485b94239e");
        assert_eq!(s3.secret_access_key.expose(), S3_CANARY);
    }

    #[test]
    fn an_s3_blob_root_leaves_endpoint_and_region_unset_when_they_are() {
        let cfg = Config::from_source(env(&s3_env())).unwrap();
        let s3 = cfg.s3.unwrap();
        assert_eq!(s3.endpoint, None);
        assert_eq!(s3.region, None);
    }

    #[test]
    fn an_s3_blob_root_without_credentials_names_the_missing_variable() {
        let mut pairs = s3_env();
        pairs.retain(|(k, _)| *k != "ANAMNESIS_S3_ACCESS_KEY_ID");
        let err = Config::from_source(env(&pairs)).unwrap_err();
        assert_eq!(err, ConfigError::Missing("ANAMNESIS_S3_ACCESS_KEY_ID"));

        let mut pairs = s3_env();
        pairs.retain(|(k, _)| *k != "ANAMNESIS_S3_SECRET_ACCESS_KEY");
        let err = Config::from_source(env(&pairs)).unwrap_err();
        assert_eq!(err, ConfigError::Missing("ANAMNESIS_S3_SECRET_ACCESS_KEY"));
    }

    #[test]
    fn debug_does_not_render_the_s3_secret_key() {
        let cfg = Config::from_source(env(&s3_env())).unwrap();
        let rendered = format!("{cfg:?}");
        assert!(
            !rendered.contains(S3_CANARY),
            "the S3 secret key must never reach a Debug rendering: {rendered}"
        );
        // The non-secret half of the credential still prints, which is what
        // makes a misconfigured key debuggable at all.
        assert!(
            rendered.contains("GK31c2f218a2e44f485b94239e"),
            "{rendered}"
        );
    }

    #[test]
    fn tls_ca_bundle_defaults_to_none() {
        let cfg = Config::from_source(env(&full_valid_env())).unwrap();
        assert_eq!(cfg.tls_ca_bundle, None);
    }

    #[test]
    fn tls_ca_bundle_is_overridable() {
        let mut pairs = full_valid_env();
        pairs.push(("ANAMNESIS_TLS_CA_BUNDLE", "/etc/anamnesis/ca.crt"));
        let cfg = Config::from_source(env(&pairs)).unwrap();
        assert_eq!(cfg.tls_ca_bundle.as_deref(), Some("/etc/anamnesis/ca.crt"));
    }

    #[test]
    fn max_body_bytes_defaults_to_40_mib() {
        let cfg = Config::from_source(env(&full_valid_env())).unwrap();
        assert_eq!(cfg.max_body_bytes, 40 * 1024 * 1024);
    }

    #[test]
    fn max_body_bytes_is_overridable() {
        let mut pairs = full_valid_env();
        pairs.push(("ANAMNESIS_MAX_BODY_BYTES", "1048576"));
        let cfg = Config::from_source(env(&pairs)).unwrap();
        assert_eq!(cfg.max_body_bytes, 1024 * 1024);
    }

    #[test]
    fn non_numeric_max_body_bytes_is_rejected_by_name() {
        let mut pairs = full_valid_env();
        pairs.push(("ANAMNESIS_MAX_BODY_BYTES", "40MB"));
        let err = Config::from_source(env(&pairs)).unwrap_err();
        assert!(matches!(
            err,
            ConfigError::Invalid {
                name: "ANAMNESIS_MAX_BODY_BYTES",
                ..
            }
        ));
    }

    /// Zero is the one numerically valid value that cannot mean what whoever
    /// set it intended: a body limit of nothing rejects every request.
    #[test]
    fn zero_max_body_bytes_is_rejected_by_name() {
        let mut pairs = full_valid_env();
        pairs.push(("ANAMNESIS_MAX_BODY_BYTES", "0"));
        let err = Config::from_source(env(&pairs)).unwrap_err();
        assert_eq!(
            err,
            ConfigError::Invalid {
                name: "ANAMNESIS_MAX_BODY_BYTES",
                reason: "must be greater than zero".to_string(),
            }
        );
    }

    #[test]
    fn oidc_claim_names_default_to_unset() {
        let cfg = Config::from_source(env(&full_valid_env())).unwrap();
        assert_eq!(cfg.oidc_user_id_claim, None);
        assert_eq!(cfg.oidc_display_name_claim, None);
    }

    #[test]
    fn oidc_claim_names_are_overridable() {
        let mut pairs = full_valid_env();
        pairs.push(("ANAMNESIS_OIDC_USER_ID_CLAIM", "employee_id"));
        pairs.push(("ANAMNESIS_OIDC_DISPLAY_NAME_CLAIM", "nickname"));
        let cfg = Config::from_source(env(&pairs)).unwrap();
        assert_eq!(cfg.oidc_user_id_claim.as_deref(), Some("employee_id"));
        assert_eq!(cfg.oidc_display_name_claim.as_deref(), Some("nickname"));
    }

    /// The group dimension is off unless explicitly configured.
    #[test]
    fn oidc_group_settings_default_to_unset() {
        let cfg = Config::from_source(env(&full_valid_env())).unwrap();
        assert_eq!(cfg.oidc_groups_claim, None);
        assert_eq!(cfg.oidc_admin_group, None);
    }

    #[test]
    fn oidc_group_settings_are_overridable() {
        let mut pairs = full_valid_env();
        pairs.push(("ANAMNESIS_OIDC_GROUPS_CLAIM", "groups"));
        pairs.push(("ANAMNESIS_OIDC_ADMIN_GROUP", "anamnesis-admins"));
        let cfg = Config::from_source(env(&pairs)).unwrap();
        assert_eq!(cfg.oidc_groups_claim.as_deref(), Some("groups"));
        assert_eq!(cfg.oidc_admin_group.as_deref(), Some("anamnesis-admins"));
    }

    /// A groups claim on its own is the expected shape for every deployment
    /// past the first admin: groups are recorded, and an existing System
    /// Admin maps them from the UI.
    #[test]
    fn a_groups_claim_without_an_admin_group_is_fine() {
        let mut pairs = full_valid_env();
        pairs.push(("ANAMNESIS_OIDC_GROUPS_CLAIM", "groups"));
        let cfg = Config::from_source(env(&pairs)).unwrap();
        assert_eq!(cfg.oidc_groups_claim.as_deref(), Some("groups"));
        assert_eq!(cfg.oidc_admin_group, None);
    }

    /// The reverse is not: an admin group with no claim to read groups from
    /// can never match anything, so the deployment would look configured
    /// while granting nobody anything.
    #[test]
    fn an_admin_group_without_a_groups_claim_is_rejected() {
        let mut pairs = full_valid_env();
        pairs.push(("ANAMNESIS_OIDC_ADMIN_GROUP", "anamnesis-admins"));
        let err = Config::from_source(env(&pairs)).unwrap_err();
        assert!(
            matches!(
                err,
                ConfigError::Invalid {
                    name: "ANAMNESIS_OIDC_ADMIN_GROUP",
                    ..
                }
            ),
            "got: {err}"
        );
        assert!(err.to_string().contains("ANAMNESIS_OIDC_GROUPS_CLAIM"));
    }

    /// Neither is a secret, so both print in full — the redaction canary
    /// above covers the two fields that must not.
    #[test]
    fn debug_shows_the_group_settings() {
        let mut pairs = full_valid_env();
        pairs.push(("ANAMNESIS_OIDC_GROUPS_CLAIM", "groups"));
        pairs.push(("ANAMNESIS_OIDC_ADMIN_GROUP", "anamnesis-admins"));
        let rendered = format!("{:?}", Config::from_source(env(&pairs)).unwrap());
        assert!(rendered.contains("groups"));
        assert!(rendered.contains("anamnesis-admins"));
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
