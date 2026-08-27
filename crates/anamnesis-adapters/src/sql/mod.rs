//! [`SqlStore`]: the dual SQLite/Postgres backend for every Phase E port
//! that talks to the relational schema (`docs/DOMAIN.md` §7) — every
//! per-entity repository, `BoardQuery`, `SearchQuery`/`SearchIndex`, and
//! `MembershipQuery`.
//!
//! One struct implements all of them, sharing one connection pool and one
//! migration run, for the same reason `tests/domain_fakes` is one `Fakes`
//! struct rather than a dozen disconnected mocks: several of these ports
//! must agree about the same underlying rows in the same transaction-free
//! reads (`BoardQuery` reading tasks a `TaskRepository` just wrote, a
//! `SearchQuery` reading what `SearchIndex` just indexed). Splitting them
//! into separate Rust types would not change the tables they share, only
//! add connection-pool bookkeeping.
//!
//! Each entity's SQL lives in its own submodule (`area`, `project`, `task`,
//! ...), following the legacy `board_repository`'s dual-backend pattern:
//! runtime `sqlx::query` (never `query!`), a private `sqlite_impl`/
//! `postgres_impl` pair per operation, and one shared contract test run
//! against both (`tests/`).

mod area;
mod attachment;
mod board_query;
mod comment;
mod membership;
mod project;
mod relationship;
mod search;
mod tangle;
mod task;

use anamnesis_app::RepoError;
use sqlx::{PgPool, SqlitePool};

static SQLITE_MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("migrations/sqlite");
static POSTGRES_MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("migrations/postgres");

/// The two backends a connection string can select, exactly as
/// `crate::board_repository::Backend` does for the legacy repository.
#[derive(Debug)]
pub(crate) enum Backend {
    Sqlite(SqlitePool),
    Postgres(PgPool),
}

/// Backs every Phase E port that talks to SQL: [`anamnesis_app::AreaRepository`],
/// [`anamnesis_app::ProjectRepository`], [`anamnesis_app::TaskRepository`],
/// [`anamnesis_app::RelationshipRepository`], [`anamnesis_app::TangleRepository`],
/// [`anamnesis_app::CommentRepository`], [`anamnesis_app::AttachmentRepository`],
/// [`anamnesis_app::BoardQuery`], [`anamnesis_app::SearchQuery`],
/// [`anamnesis_app::SearchIndex`], and [`anamnesis_app::MembershipQuery`].
#[derive(Debug)]
pub struct SqlStore {
    pub(crate) backend: Backend,
}

impl SqlStore {
    /// Connects to `database_url`, running that backend's migrations before
    /// returning. `sqlite://` selects SQLite, `postgres://`/`postgresql://`
    /// selects Postgres; anything else is a startup error naming both
    /// supported forms.
    pub async fn connect(database_url: &str) -> Result<Self, RepoError> {
        if database_url.starts_with("sqlite://") {
            let pool = SqlitePool::connect(database_url)
                .await
                .map_err(|e| RepoError::from_source("failed to connect to SQLite database", e))?;
            SQLITE_MIGRATOR
                .run(&pool)
                .await
                .map_err(|e| RepoError::from_source("failed to run SQLite migrations", e))?;
            Ok(Self {
                backend: Backend::Sqlite(pool),
            })
        } else if database_url.starts_with("postgres://")
            || database_url.starts_with("postgresql://")
        {
            let pool = PgPool::connect(database_url)
                .await
                .map_err(|e| RepoError::from_source("failed to connect to Postgres database", e))?;
            POSTGRES_MIGRATOR
                .run(&pool)
                .await
                .map_err(|e| RepoError::from_source("failed to run Postgres migrations", e))?;
            Ok(Self {
                backend: Backend::Postgres(pool),
            })
        } else {
            Err(RepoError::new(format!(
                "unsupported database URL {database_url:?}: expected a \
                 \"sqlite://\" URL or a \"postgres://\"/\"postgresql://\" URL"
            )))
        }
    }
}

/// Parses a stored SQLite `TEXT` id column back into a `Uuid`.
pub(crate) fn parse_uuid(raw: &str) -> Result<uuid::Uuid, RepoError> {
    uuid::Uuid::parse_str(raw).map_err(|e| RepoError::from_source("invalid stored id", e))
}

/// Builds a validated [`anamnesis_core::Title`] from stored text, wrapping a
/// validation failure as a [`RepoError`] — a stored row failing a rule the
/// core type enforces on construction indicates corrupt data, not a normal
/// "not found".
pub(crate) fn title_from_text(raw: String) -> Result<anamnesis_core::Title, RepoError> {
    anamnesis_core::Title::new(&raw)
        .map_err(|e| RepoError::from_source(format!("invalid stored title {raw:?}"), e))
}

pub(crate) fn timestamp_from_seconds(raw: i64) -> Result<anamnesis_core::Timestamp, RepoError> {
    anamnesis_core::Timestamp::from_unix_seconds(raw)
        .map_err(|e| RepoError::from_source("invalid stored timestamp", e))
}

/// `ProjectStatus` <-> its stored text representation.
pub(crate) fn project_status_to_text(status: anamnesis_core::ProjectStatus) -> &'static str {
    use anamnesis_core::ProjectStatus::*;
    match status {
        Pending => "pending",
        Active => "active",
        Complete => "complete",
    }
}

pub(crate) fn project_status_from_text(
    raw: &str,
) -> Result<anamnesis_core::ProjectStatus, RepoError> {
    use anamnesis_core::ProjectStatus::*;
    match raw {
        "pending" => Ok(Pending),
        "active" => Ok(Active),
        "complete" => Ok(Complete),
        other => Err(RepoError::new(format!(
            "invalid stored project status {other:?}"
        ))),
    }
}

/// `FieldKind` <-> its stored text representation.
pub(crate) fn field_kind_to_text(kind: anamnesis_core::FieldKind) -> &'static str {
    use anamnesis_core::FieldKind::*;
    match kind {
        Number => "number",
        Currency => "currency",
        Date => "date",
        Time => "time",
        DateTime => "datetime",
        Line => "line",
        Block => "block",
    }
}

pub(crate) fn field_kind_from_text(raw: &str) -> Result<anamnesis_core::FieldKind, RepoError> {
    use anamnesis_core::FieldKind::*;
    match raw {
        "number" => Ok(Number),
        "currency" => Ok(Currency),
        "date" => Ok(Date),
        "time" => Ok(Time),
        "datetime" => Ok(DateTime),
        "line" => Ok(Line),
        "block" => Ok(Block),
        other => Err(RepoError::new(format!(
            "invalid stored field kind {other:?}"
        ))),
    }
}
