//! [`SqlBoardRepository`]: the `BoardRepository` port backed by SQLite or
//! Postgres, selected at runtime from the connection string's scheme.
//!
//! Two deliberate choices carried over from `docs/ARCHITECTURE.md`:
//!
//! - Runtime `sqlx::query`, never the `query!` macros — the macros need a
//!   compile-time database connection and bind the binary to one backend.
//! - UUIDs are stored as `TEXT` in SQLite and native `UUID` in Postgres, so
//!   the row-mapping code is backend-specific; the two are kept from
//!   drifting by sharing a single `BoardRepository` contract test instead of
//!   sharing SQL.

use std::collections::HashMap;

use anamnesis_app::{Board, BoardRepository, BoardSummary, RepoError};
use anamnesis_core::legacy::{Card, Column};
use anamnesis_core::{BoardId, CardId, ColumnId, Timestamp, Title, UserId};
use async_trait::async_trait;
use sqlx::{PgPool, Row, SqlitePool};
use uuid::Uuid;

static SQLITE_MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("migrations/sqlite");
static POSTGRES_MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("migrations/postgres");

/// The two backends a connection string can select. Kept private: nothing
/// outside this module ever matches on it.
#[derive(Debug)]
enum Backend {
    Sqlite(SqlitePool),
    Postgres(PgPool),
}

/// A [`BoardRepository`] backed by SQLite or Postgres.
///
/// The backend is chosen once, at [`SqlBoardRepository::connect`], from the
/// connection string's scheme: `sqlite://` selects SQLite, `postgres://` or
/// `postgresql://` selects Postgres. Any other scheme is a startup error
/// naming both supported forms.
#[derive(Debug)]
pub struct SqlBoardRepository {
    backend: Backend,
}

impl SqlBoardRepository {
    /// Connects to `database_url`, running that backend's migrations before
    /// returning. The scheme prefix decides the backend; anything else is
    /// rejected up front so a typo fails at startup, not on first query.
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

/// A reconstructed row's-worth of column data, cards filled in afterward.
struct ColumnRow {
    id: ColumnId,
    title: Title,
    wip_limit: Option<u16>,
}

fn wip_limit_from_i64(raw: Option<i64>) -> Result<Option<u16>, RepoError> {
    raw.map(|n| {
        u16::try_from(n).map_err(|_| RepoError::new(format!("wip_limit {n} out of range for u16")))
    })
    .transpose()
}

fn title_from_text(raw: String) -> Result<Title, RepoError> {
    Title::new(&raw).map_err(|e| RepoError::from_source(format!("invalid stored title {raw:?}"), e))
}

fn assemble_board(
    id: BoardId,
    owner: UserId,
    title: Title,
    column_rows: Vec<ColumnRow>,
    mut cards_by_column: HashMap<ColumnId, Vec<Card>>,
) -> Board {
    let columns = column_rows
        .into_iter()
        .map(|row| Column {
            cards: cards_by_column.remove(&row.id).unwrap_or_default(),
            id: row.id,
            title: row.title,
            wip_limit: row.wip_limit,
        })
        .collect();
    Board {
        id,
        owner,
        title,
        columns,
    }
}

mod sqlite_impl {
    use super::*;

    pub(super) async fn load(pool: &SqlitePool, id: BoardId) -> Result<Option<Board>, RepoError> {
        let id_text = id.as_uuid().to_string();

        let Some(board_row) = sqlx::query("SELECT owner, title FROM boards WHERE id = ?")
            .bind(&id_text)
            .fetch_optional(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to load board", e))?
        else {
            return Ok(None);
        };
        let owner = UserId::new(board_row.get::<String, _>("owner"));
        let title = title_from_text(board_row.get::<String, _>("title"))?;

        let column_rows = sqlx::query(
            "SELECT id, title, wip_limit FROM columns WHERE board_id = ? ORDER BY position",
        )
        .bind(&id_text)
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to load columns", e))?;
        let columns = column_rows
            .into_iter()
            .map(|row| -> Result<ColumnRow, RepoError> {
                let col_id: String = row.get("id");
                let col_id = Uuid::parse_str(&col_id)
                    .map_err(|e| RepoError::from_source("invalid stored column id", e))?;
                Ok(ColumnRow {
                    id: ColumnId::new(col_id),
                    title: title_from_text(row.get::<String, _>("title"))?,
                    wip_limit: wip_limit_from_i64(row.get::<Option<i64>, _>("wip_limit"))?,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        let card_rows = sqlx::query(
            "SELECT cards.id AS card_id, cards.column_id AS column_id, cards.title AS title, \
             cards.body AS body, cards.created_at AS created_at \
             FROM cards JOIN columns ON cards.column_id = columns.id \
             WHERE columns.board_id = ? \
             ORDER BY columns.position, cards.position",
        )
        .bind(&id_text)
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to load cards", e))?;

        let mut cards_by_column: HashMap<ColumnId, Vec<Card>> = HashMap::new();
        for row in card_rows {
            let card_id: String = row.get("card_id");
            let card_id = Uuid::parse_str(&card_id)
                .map_err(|e| RepoError::from_source("invalid stored card id", e))?;
            let column_id: String = row.get("column_id");
            let column_id = Uuid::parse_str(&column_id)
                .map_err(|e| RepoError::from_source("invalid stored card's column id", e))?;
            let created_at = Timestamp::from_unix_seconds(row.get::<i64, _>("created_at"))
                .map_err(|e| RepoError::from_source("invalid stored created_at", e))?;
            cards_by_column
                .entry(ColumnId::new(column_id))
                .or_default()
                .push(Card {
                    id: CardId::new(card_id),
                    title: title_from_text(row.get::<String, _>("title"))?,
                    body: row.get("body"),
                    created_at,
                });
        }

        Ok(Some(assemble_board(
            id,
            owner,
            title,
            columns,
            cards_by_column,
        )))
    }

    pub(super) async fn save(pool: &SqlitePool, board: &Board) -> Result<(), RepoError> {
        let mut tx = pool
            .begin()
            .await
            .map_err(|e| RepoError::from_source("failed to start transaction", e))?;
        let id_text = board.id.as_uuid().to_string();

        sqlx::query(
            "INSERT INTO boards (id, owner, title) VALUES (?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET owner = excluded.owner, title = excluded.title",
        )
        .bind(&id_text)
        .bind(board.owner.as_str())
        .bind(board.title.as_str())
        .execute(&mut *tx)
        .await
        .map_err(|e| RepoError::from_source("failed to upsert board", e))?;

        sqlx::query(
            "DELETE FROM cards WHERE column_id IN (SELECT id FROM columns WHERE board_id = ?)",
        )
        .bind(&id_text)
        .execute(&mut *tx)
        .await
        .map_err(|e| RepoError::from_source("failed to clear old cards", e))?;
        sqlx::query("DELETE FROM columns WHERE board_id = ?")
            .bind(&id_text)
            .execute(&mut *tx)
            .await
            .map_err(|e| RepoError::from_source("failed to clear old columns", e))?;

        for (col_index, column) in board.columns.iter().enumerate() {
            let col_id_text = column.id.as_uuid().to_string();
            let position = i64::try_from(col_index).expect("column index fits in i64");
            let wip_limit = column.wip_limit.map(i64::from);
            sqlx::query(
                "INSERT INTO columns (id, board_id, title, wip_limit, position) \
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(&col_id_text)
            .bind(&id_text)
            .bind(column.title.as_str())
            .bind(wip_limit)
            .bind(position)
            .execute(&mut *tx)
            .await
            .map_err(|e| RepoError::from_source("failed to insert column", e))?;

            for (card_index, card) in column.cards.iter().enumerate() {
                let card_position = i64::try_from(card_index).expect("card index fits in i64");
                sqlx::query(
                    "INSERT INTO cards (id, column_id, title, body, created_at, position) \
                     VALUES (?, ?, ?, ?, ?, ?)",
                )
                .bind(card.id.as_uuid().to_string())
                .bind(&col_id_text)
                .bind(card.title.as_str())
                .bind(&card.body)
                .bind(card.created_at.unix_seconds())
                .bind(card_position)
                .execute(&mut *tx)
                .await
                .map_err(|e| RepoError::from_source("failed to insert card", e))?;
            }
        }

        tx.commit()
            .await
            .map_err(|e| RepoError::from_source("failed to commit save transaction", e))
    }

    pub(super) async fn list_for_owner(
        pool: &SqlitePool,
        owner: &UserId,
    ) -> Result<Vec<BoardSummary>, RepoError> {
        let rows = sqlx::query("SELECT id, title FROM boards WHERE owner = ? ORDER BY title")
            .bind(owner.as_str())
            .fetch_all(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to list boards for owner", e))?;
        rows.into_iter()
            .map(|row| -> Result<BoardSummary, RepoError> {
                let id: String = row.get("id");
                let id = Uuid::parse_str(&id)
                    .map_err(|e| RepoError::from_source("invalid stored board id", e))?;
                Ok(BoardSummary {
                    id: BoardId::new(id),
                    title: title_from_text(row.get::<String, _>("title"))?,
                })
            })
            .collect()
    }

    pub(super) async fn delete(pool: &SqlitePool, id: BoardId) -> Result<(), RepoError> {
        let mut tx = pool
            .begin()
            .await
            .map_err(|e| RepoError::from_source("failed to start transaction", e))?;
        let id_text = id.as_uuid().to_string();

        sqlx::query(
            "DELETE FROM cards WHERE column_id IN (SELECT id FROM columns WHERE board_id = ?)",
        )
        .bind(&id_text)
        .execute(&mut *tx)
        .await
        .map_err(|e| RepoError::from_source("failed to delete cards", e))?;
        sqlx::query("DELETE FROM columns WHERE board_id = ?")
            .bind(&id_text)
            .execute(&mut *tx)
            .await
            .map_err(|e| RepoError::from_source("failed to delete columns", e))?;
        sqlx::query("DELETE FROM boards WHERE id = ?")
            .bind(&id_text)
            .execute(&mut *tx)
            .await
            .map_err(|e| RepoError::from_source("failed to delete board", e))?;

        tx.commit()
            .await
            .map_err(|e| RepoError::from_source("failed to commit delete transaction", e))
    }
}

mod postgres_impl {
    use super::*;

    pub(super) async fn load(pool: &PgPool, id: BoardId) -> Result<Option<Board>, RepoError> {
        let Some(board_row) = sqlx::query("SELECT owner, title FROM boards WHERE id = $1")
            .bind(id.as_uuid())
            .fetch_optional(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to load board", e))?
        else {
            return Ok(None);
        };
        let owner = UserId::new(board_row.get::<String, _>("owner"));
        let title = title_from_text(board_row.get::<String, _>("title"))?;

        let column_rows = sqlx::query(
            "SELECT id, title, wip_limit FROM columns WHERE board_id = $1 ORDER BY position",
        )
        .bind(id.as_uuid())
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to load columns", e))?;
        let columns = column_rows
            .into_iter()
            .map(|row| -> Result<ColumnRow, RepoError> {
                Ok(ColumnRow {
                    id: ColumnId::new(row.get::<Uuid, _>("id")),
                    title: title_from_text(row.get::<String, _>("title"))?,
                    wip_limit: wip_limit_from_i64(
                        row.get::<Option<i32>, _>("wip_limit").map(i64::from),
                    )?,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        let card_rows = sqlx::query(
            "SELECT cards.id AS card_id, cards.column_id AS column_id, cards.title AS title, \
             cards.body AS body, cards.created_at AS created_at \
             FROM cards JOIN columns ON cards.column_id = columns.id \
             WHERE columns.board_id = $1 \
             ORDER BY columns.position, cards.position",
        )
        .bind(id.as_uuid())
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to load cards", e))?;

        let mut cards_by_column: HashMap<ColumnId, Vec<Card>> = HashMap::new();
        for row in card_rows {
            let created_at = Timestamp::from_unix_seconds(row.get::<i64, _>("created_at"))
                .map_err(|e| RepoError::from_source("invalid stored created_at", e))?;
            cards_by_column
                .entry(ColumnId::new(row.get::<Uuid, _>("column_id")))
                .or_default()
                .push(Card {
                    id: CardId::new(row.get::<Uuid, _>("card_id")),
                    title: title_from_text(row.get::<String, _>("title"))?,
                    body: row.get("body"),
                    created_at,
                });
        }

        Ok(Some(assemble_board(
            id,
            owner,
            title,
            columns,
            cards_by_column,
        )))
    }

    pub(super) async fn save(pool: &PgPool, board: &Board) -> Result<(), RepoError> {
        let mut tx = pool
            .begin()
            .await
            .map_err(|e| RepoError::from_source("failed to start transaction", e))?;

        sqlx::query(
            "INSERT INTO boards (id, owner, title) VALUES ($1, $2, $3) \
             ON CONFLICT (id) DO UPDATE SET owner = excluded.owner, title = excluded.title",
        )
        .bind(board.id.as_uuid())
        .bind(board.owner.as_str())
        .bind(board.title.as_str())
        .execute(&mut *tx)
        .await
        .map_err(|e| RepoError::from_source("failed to upsert board", e))?;

        sqlx::query(
            "DELETE FROM cards WHERE column_id IN (SELECT id FROM columns WHERE board_id = $1)",
        )
        .bind(board.id.as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(|e| RepoError::from_source("failed to clear old cards", e))?;
        sqlx::query("DELETE FROM columns WHERE board_id = $1")
            .bind(board.id.as_uuid())
            .execute(&mut *tx)
            .await
            .map_err(|e| RepoError::from_source("failed to clear old columns", e))?;

        for (col_index, column) in board.columns.iter().enumerate() {
            let position = i32::try_from(col_index)
                .map_err(|e| RepoError::from_source("column index out of range", e))?;
            let wip_limit = column
                .wip_limit
                .map(i32::try_from)
                .transpose()
                .map_err(|e| RepoError::from_source("wip_limit out of range for i32", e))?;
            sqlx::query(
                "INSERT INTO columns (id, board_id, title, wip_limit, position) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(column.id.as_uuid())
            .bind(board.id.as_uuid())
            .bind(column.title.as_str())
            .bind(wip_limit)
            .bind(position)
            .execute(&mut *tx)
            .await
            .map_err(|e| RepoError::from_source("failed to insert column", e))?;

            for (card_index, card) in column.cards.iter().enumerate() {
                let card_position = i32::try_from(card_index)
                    .map_err(|e| RepoError::from_source("card index out of range", e))?;
                sqlx::query(
                    "INSERT INTO cards (id, column_id, title, body, created_at, position) \
                     VALUES ($1, $2, $3, $4, $5, $6)",
                )
                .bind(card.id.as_uuid())
                .bind(column.id.as_uuid())
                .bind(card.title.as_str())
                .bind(&card.body)
                .bind(card.created_at.unix_seconds())
                .bind(card_position)
                .execute(&mut *tx)
                .await
                .map_err(|e| RepoError::from_source("failed to insert card", e))?;
            }
        }

        tx.commit()
            .await
            .map_err(|e| RepoError::from_source("failed to commit save transaction", e))
    }

    pub(super) async fn list_for_owner(
        pool: &PgPool,
        owner: &UserId,
    ) -> Result<Vec<BoardSummary>, RepoError> {
        let rows = sqlx::query("SELECT id, title FROM boards WHERE owner = $1 ORDER BY title")
            .bind(owner.as_str())
            .fetch_all(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to list boards for owner", e))?;
        rows.into_iter()
            .map(|row| -> Result<BoardSummary, RepoError> {
                Ok(BoardSummary {
                    id: BoardId::new(row.get::<Uuid, _>("id")),
                    title: title_from_text(row.get::<String, _>("title"))?,
                })
            })
            .collect()
    }

    pub(super) async fn delete(pool: &PgPool, id: BoardId) -> Result<(), RepoError> {
        let mut tx = pool
            .begin()
            .await
            .map_err(|e| RepoError::from_source("failed to start transaction", e))?;

        sqlx::query(
            "DELETE FROM cards WHERE column_id IN (SELECT id FROM columns WHERE board_id = $1)",
        )
        .bind(id.as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(|e| RepoError::from_source("failed to delete cards", e))?;
        sqlx::query("DELETE FROM columns WHERE board_id = $1")
            .bind(id.as_uuid())
            .execute(&mut *tx)
            .await
            .map_err(|e| RepoError::from_source("failed to delete columns", e))?;
        sqlx::query("DELETE FROM boards WHERE id = $1")
            .bind(id.as_uuid())
            .execute(&mut *tx)
            .await
            .map_err(|e| RepoError::from_source("failed to delete board", e))?;

        tx.commit()
            .await
            .map_err(|e| RepoError::from_source("failed to commit delete transaction", e))
    }
}

#[async_trait]
impl BoardRepository for SqlBoardRepository {
    async fn load(&self, id: BoardId) -> Result<Option<Board>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::load(pool, id).await,
            Backend::Postgres(pool) => postgres_impl::load(pool, id).await,
        }
    }

    async fn save(&self, board: &Board) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::save(pool, board).await,
            Backend::Postgres(pool) => postgres_impl::save(pool, board).await,
        }
    }

    async fn list_for_owner(&self, owner: &UserId) -> Result<Vec<BoardSummary>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::list_for_owner(pool, owner).await,
            Backend::Postgres(pool) => postgres_impl::list_for_owner(pool, owner).await,
        }
    }

    async fn delete(&self, id: BoardId) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::delete(pool, id).await,
            Backend::Postgres(pool) => postgres_impl::delete(pool, id).await,
        }
    }
}
