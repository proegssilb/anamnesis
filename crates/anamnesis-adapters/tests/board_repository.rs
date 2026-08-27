//! The `BoardRepository` contract, exercised once and run against both
//! backends so they cannot drift.
//!
//! SQLite runs against a temporary **file** (not `:memory:` — a pool opens
//! multiple connections, and each would otherwise get its own empty
//! in-memory database). Postgres runs when `ANAMNESIS_TEST_PG_URL` is set;
//! it is `#[ignore]`d otherwise so `cargo test` stays green without a live
//! Postgres server.

use anamnesis_adapters::SqlBoardRepository;
use anamnesis_app::{BoardRepository, BoardSummary};
use anamnesis_core::legacy::{Board, Card, Column};
use anamnesis_core::{BoardId, CardId, ColumnId, Timestamp, Title, UserId};
use uuid::Uuid;

fn title(raw: &str) -> Title {
    Title::new(raw).unwrap()
}

fn sample_board(owner: &UserId) -> Board {
    Board {
        id: BoardId::new(Uuid::new_v4()),
        owner: owner.clone(),
        title: title("Sprint Board"),
        columns: vec![
            Column {
                id: ColumnId::new(Uuid::new_v4()),
                title: title("Todo"),
                wip_limit: None,
                cards: vec![
                    Card {
                        id: CardId::new(Uuid::new_v4()),
                        title: title("First card"),
                        body: "first body".to_string(),
                        created_at: Timestamp::from_unix_seconds(1_700_000_000).unwrap(),
                    },
                    Card {
                        id: CardId::new(Uuid::new_v4()),
                        title: title("Second card"),
                        body: "second body".to_string(),
                        created_at: Timestamp::from_unix_seconds(1_700_000_100).unwrap(),
                    },
                    Card {
                        id: CardId::new(Uuid::new_v4()),
                        title: title("Third card"),
                        body: "third body".to_string(),
                        created_at: Timestamp::from_unix_seconds(1_700_000_200).unwrap(),
                    },
                ],
            },
            Column {
                id: ColumnId::new(Uuid::new_v4()),
                title: title("Doing"),
                wip_limit: Some(3),
                cards: vec![Card {
                    id: CardId::new(Uuid::new_v4()),
                    title: title("In progress card"),
                    body: "wip".to_string(),
                    created_at: Timestamp::from_unix_seconds(1_700_000_300).unwrap(),
                }],
            },
            Column {
                id: ColumnId::new(Uuid::new_v4()),
                title: title("Done"),
                wip_limit: None,
                cards: vec![],
            },
        ],
    }
}

/// The shared repository contract. Called once per backend so the two
/// implementations cannot drift apart.
async fn board_repository_contract(repo: &SqlBoardRepository) {
    let owner = UserId::new("alice");
    let other_owner = UserId::new("bob");

    // Loading a board that has never been saved is `None`, not an error.
    let missing = BoardId::new(Uuid::new_v4());
    assert_eq!(repo.load(missing).await.unwrap(), None);

    let board = sample_board(&owner);
    repo.save(&board).await.unwrap();

    let loaded = repo.load(board.id).await.unwrap().expect("board was saved");
    assert_eq!(
        loaded, board,
        "round-tripped board must equal the saved board, in order"
    );

    // A second, unrelated board for a different owner, to prove listing and
    // isolation both work.
    let other_board = sample_board(&other_owner);
    repo.save(&other_board).await.unwrap();

    let owner_boards = repo.list_for_owner(&owner).await.unwrap();
    assert_eq!(
        owner_boards,
        vec![BoardSummary {
            id: board.id,
            title: board.title.clone(),
        }]
    );

    // Re-saving with reordered/edited columns and cards must fully replace
    // the previous state (delete-and-reinsert), not merge with it.
    let mut edited = board.clone();
    edited.columns.remove(0); // drop "Todo" and its three cards entirely
    edited.columns[0].title = title("Doing (renamed)");
    repo.save(&edited).await.unwrap();

    let reloaded = repo.load(board.id).await.unwrap().unwrap();
    assert_eq!(reloaded, edited);
    assert_eq!(
        reloaded.columns.len(),
        2,
        "the deleted column must not resurface"
    );

    repo.delete(board.id).await.unwrap();
    assert_eq!(repo.load(board.id).await.unwrap(), None);
    assert_eq!(repo.list_for_owner(&owner).await.unwrap(), vec![]);

    // Deleting an already-absent board is not an error.
    repo.delete(board.id).await.unwrap();

    // Clean up the second board so the contract leaves no residue behind.
    repo.delete(other_board.id).await.unwrap();
}

#[tokio::test]
async fn sqlite_repository_contract() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let db_path = dir.path().join("anamnesis-test.db");
    let url = format!("sqlite://{}?mode=rwc", db_path.display());

    let repo = SqlBoardRepository::connect(&url)
        .await
        .expect("connect + migrate sqlite repository");

    board_repository_contract(&repo).await;
}

#[tokio::test]
#[ignore = "requires a live Postgres server; set ANAMNESIS_TEST_PG_URL and pass --ignored"]
async fn postgres_repository_contract() {
    let Ok(url) = std::env::var("ANAMNESIS_TEST_PG_URL") else {
        eprintln!("skipping postgres_repository_contract: ANAMNESIS_TEST_PG_URL is not set");
        return;
    };

    let repo = SqlBoardRepository::connect(&url)
        .await
        .expect("connect + migrate postgres repository");

    board_repository_contract(&repo).await;
}

#[tokio::test]
async fn unknown_scheme_is_a_startup_error_naming_both_supported_forms() {
    let err = SqlBoardRepository::connect("mysql://localhost/db")
        .await
        .expect_err("unsupported scheme must be rejected");

    let message = err.to_string();
    assert!(message.contains("sqlite://"), "message was: {message}");
    assert!(
        message.contains("postgres://") || message.contains("postgresql://"),
        "message was: {message}"
    );
}
