//! Unit tests for the `anamnesis-app` use cases, driven against the
//! in-memory test doubles in `support`.

mod support;

use anamnesis_app::{AppError, BoardRepository};
use anamnesis_core::UserId;
use support::{FixedClock, InMemoryBoardRepository, SequentialIdGen};

fn alice() -> UserId {
    UserId::new("alice")
}

fn mallory() -> UserId {
    UserId::new("mallory")
}

// --- create_board ---

#[tokio::test]
async fn create_board_persists_a_new_board_owned_by_the_caller() {
    let repo = InMemoryBoardRepository::new();
    let ids = SequentialIdGen::new();

    let board = anamnesis_app::create_board(&repo, &ids, &alice(), "My Board")
        .await
        .unwrap();

    assert_eq!(board.owner, alice());
    assert_eq!(board.title.as_str(), "My Board");
    assert!(board.columns.is_empty());

    let reloaded = repo.load(board.id).await.unwrap().unwrap();
    assert_eq!(reloaded, board);
}

#[tokio::test]
async fn create_board_rejects_an_invalid_title() {
    let repo = InMemoryBoardRepository::new();
    let ids = SequentialIdGen::new();

    let result = anamnesis_app::create_board(&repo, &ids, &alice(), "   ").await;

    assert!(matches!(result, Err(AppError::Domain(_))));
}

// --- list_boards ---

#[tokio::test]
async fn list_boards_returns_only_the_callers_boards() {
    let repo = InMemoryBoardRepository::new();
    let ids = SequentialIdGen::new();

    anamnesis_app::create_board(&repo, &ids, &alice(), "Alice's board")
        .await
        .unwrap();
    anamnesis_app::create_board(&repo, &ids, &mallory(), "Mallory's board")
        .await
        .unwrap();

    let summaries = anamnesis_app::list_boards(&repo, &alice()).await.unwrap();

    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].title.as_str(), "Alice's board");
}

// --- view_board ---

#[tokio::test]
async fn view_board_returns_the_board_for_its_owner() {
    let repo = InMemoryBoardRepository::new();
    let ids = SequentialIdGen::new();
    let board = anamnesis_app::create_board(&repo, &ids, &alice(), "My Board")
        .await
        .unwrap();

    let viewed = anamnesis_app::view_board(&repo, board.id, &alice())
        .await
        .unwrap();

    assert_eq!(viewed, board);
}

#[tokio::test]
async fn view_board_is_forbidden_for_a_non_owner() {
    let repo = InMemoryBoardRepository::new();
    let ids = SequentialIdGen::new();
    let board = anamnesis_app::create_board(&repo, &ids, &alice(), "My Board")
        .await
        .unwrap();

    let result = anamnesis_app::view_board(&repo, board.id, &mallory()).await;

    assert!(matches!(result, Err(AppError::Forbidden)));
}

#[tokio::test]
async fn view_board_is_not_found_when_it_does_not_exist() {
    let repo = InMemoryBoardRepository::new();
    let missing = anamnesis_core::BoardId::new(uuid::Uuid::from_u128(999));

    let result = anamnesis_app::view_board(&repo, missing, &alice()).await;

    assert!(matches!(result, Err(AppError::NotFound)));
}

// --- add_column ---

#[tokio::test]
async fn add_column_appends_a_column_to_the_owners_board() {
    let repo = InMemoryBoardRepository::new();
    let ids = SequentialIdGen::new();
    let board = anamnesis_app::create_board(&repo, &ids, &alice(), "My Board")
        .await
        .unwrap();

    let updated = anamnesis_app::add_column(&repo, &ids, board.id, &alice(), "Todo", None)
        .await
        .unwrap();

    assert_eq!(updated.columns.len(), 1);
    assert_eq!(updated.columns[0].title.as_str(), "Todo");
}

#[tokio::test]
async fn add_column_is_forbidden_for_a_non_owner() {
    let repo = InMemoryBoardRepository::new();
    let ids = SequentialIdGen::new();
    let board = anamnesis_app::create_board(&repo, &ids, &alice(), "My Board")
        .await
        .unwrap();

    let result = anamnesis_app::add_column(&repo, &ids, board.id, &mallory(), "Todo", None).await;

    assert!(matches!(result, Err(AppError::Forbidden)));
}

// --- add_card ---

#[tokio::test]
async fn add_card_appends_a_card_to_the_named_column() {
    let repo = InMemoryBoardRepository::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(1_000);
    let board = anamnesis_app::create_board(&repo, &ids, &alice(), "My Board")
        .await
        .unwrap();
    let board = anamnesis_app::add_column(&repo, &ids, board.id, &alice(), "Todo", None)
        .await
        .unwrap();
    let column_id = board.columns[0].id;

    let updated = anamnesis_app::add_card(
        &repo,
        &ids,
        &clock,
        board.id,
        &alice(),
        column_id,
        "Buy milk",
        "2%",
    )
    .await
    .unwrap();

    let card = &updated.columns[0].cards[0];
    assert_eq!(card.title.as_str(), "Buy milk");
    assert_eq!(card.body, "2%");
    assert_eq!(card.created_at.unix_seconds(), 1_000);
}

#[tokio::test]
async fn add_card_is_forbidden_for_a_non_owner() {
    let repo = InMemoryBoardRepository::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(1_000);
    let board = anamnesis_app::create_board(&repo, &ids, &alice(), "My Board")
        .await
        .unwrap();
    let board = anamnesis_app::add_column(&repo, &ids, board.id, &alice(), "Todo", None)
        .await
        .unwrap();
    let column_id = board.columns[0].id;

    let result = anamnesis_app::add_card(
        &repo,
        &ids,
        &clock,
        board.id,
        &mallory(),
        column_id,
        "Buy milk",
        "",
    )
    .await;

    assert!(matches!(result, Err(AppError::Forbidden)));
}

// --- move_card ---

#[tokio::test]
async fn move_card_relocates_a_card_addressed_by_id_alone() {
    let repo = InMemoryBoardRepository::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let board = anamnesis_app::create_board(&repo, &ids, &alice(), "My Board")
        .await
        .unwrap();
    let board = anamnesis_app::add_column(&repo, &ids, board.id, &alice(), "Todo", None)
        .await
        .unwrap();
    let board = anamnesis_app::add_column(&repo, &ids, board.id, &alice(), "Doing", None)
        .await
        .unwrap();
    let todo = board.columns[0].id;
    let doing = board.columns[1].id;
    let board = anamnesis_app::add_card(&repo, &ids, &clock, board.id, &alice(), todo, "Card", "")
        .await
        .unwrap();
    let card_id = board.columns[0].cards[0].id;

    let updated = anamnesis_app::move_card(&repo, board.id, &alice(), card_id, doing, 0)
        .await
        .unwrap();

    assert!(updated.columns[0].cards.is_empty());
    assert_eq!(updated.columns[1].cards[0].id, card_id);
}

#[tokio::test]
async fn move_card_is_forbidden_for_a_non_owner() {
    let repo = InMemoryBoardRepository::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let board = anamnesis_app::create_board(&repo, &ids, &alice(), "My Board")
        .await
        .unwrap();
    let board = anamnesis_app::add_column(&repo, &ids, board.id, &alice(), "Todo", None)
        .await
        .unwrap();
    let todo = board.columns[0].id;
    let board = anamnesis_app::add_card(&repo, &ids, &clock, board.id, &alice(), todo, "Card", "")
        .await
        .unwrap();
    let card_id = board.columns[0].cards[0].id;

    let result = anamnesis_app::move_card(&repo, board.id, &mallory(), card_id, todo, 0).await;

    assert!(matches!(result, Err(AppError::Forbidden)));
}

// --- edit_card ---

#[tokio::test]
async fn edit_card_replaces_title_and_body() {
    let repo = InMemoryBoardRepository::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let board = anamnesis_app::create_board(&repo, &ids, &alice(), "My Board")
        .await
        .unwrap();
    let board = anamnesis_app::add_column(&repo, &ids, board.id, &alice(), "Todo", None)
        .await
        .unwrap();
    let todo = board.columns[0].id;
    let board =
        anamnesis_app::add_card(&repo, &ids, &clock, board.id, &alice(), todo, "Card", "old")
            .await
            .unwrap();
    let card_id = board.columns[0].cards[0].id;

    let updated = anamnesis_app::edit_card(&repo, board.id, &alice(), card_id, "Card v2", "new")
        .await
        .unwrap();

    let card = &updated.columns[0].cards[0];
    assert_eq!(card.title.as_str(), "Card v2");
    assert_eq!(card.body, "new");
}

#[tokio::test]
async fn edit_card_is_forbidden_for_a_non_owner() {
    let repo = InMemoryBoardRepository::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let board = anamnesis_app::create_board(&repo, &ids, &alice(), "My Board")
        .await
        .unwrap();
    let board = anamnesis_app::add_column(&repo, &ids, board.id, &alice(), "Todo", None)
        .await
        .unwrap();
    let todo = board.columns[0].id;
    let board = anamnesis_app::add_card(&repo, &ids, &clock, board.id, &alice(), todo, "Card", "")
        .await
        .unwrap();
    let card_id = board.columns[0].cards[0].id;

    let result = anamnesis_app::edit_card(&repo, board.id, &mallory(), card_id, "x", "y").await;

    assert!(matches!(result, Err(AppError::Forbidden)));
}

// --- delete_card ---

#[tokio::test]
async fn delete_card_removes_it_from_the_board() {
    let repo = InMemoryBoardRepository::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let board = anamnesis_app::create_board(&repo, &ids, &alice(), "My Board")
        .await
        .unwrap();
    let board = anamnesis_app::add_column(&repo, &ids, board.id, &alice(), "Todo", None)
        .await
        .unwrap();
    let todo = board.columns[0].id;
    let board = anamnesis_app::add_card(&repo, &ids, &clock, board.id, &alice(), todo, "Card", "")
        .await
        .unwrap();
    let card_id = board.columns[0].cards[0].id;

    let updated = anamnesis_app::delete_card(&repo, board.id, &alice(), card_id)
        .await
        .unwrap();

    assert!(updated.columns[0].cards.is_empty());
}

#[tokio::test]
async fn delete_card_is_forbidden_for_a_non_owner() {
    let repo = InMemoryBoardRepository::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let board = anamnesis_app::create_board(&repo, &ids, &alice(), "My Board")
        .await
        .unwrap();
    let board = anamnesis_app::add_column(&repo, &ids, board.id, &alice(), "Todo", None)
        .await
        .unwrap();
    let todo = board.columns[0].id;
    let board = anamnesis_app::add_card(&repo, &ids, &clock, board.id, &alice(), todo, "Card", "")
        .await
        .unwrap();
    let card_id = board.columns[0].cards[0].id;

    let result = anamnesis_app::delete_card(&repo, board.id, &mallory(), card_id).await;

    assert!(matches!(result, Err(AppError::Forbidden)));
}

// --- delete_board ---

#[tokio::test]
async fn delete_board_removes_it_from_the_repository() {
    let repo = InMemoryBoardRepository::new();
    let ids = SequentialIdGen::new();
    let board = anamnesis_app::create_board(&repo, &ids, &alice(), "My Board")
        .await
        .unwrap();

    anamnesis_app::delete_board(&repo, board.id, &alice())
        .await
        .unwrap();

    assert!(repo.load(board.id).await.unwrap().is_none());
}

#[tokio::test]
async fn delete_board_is_forbidden_for_a_non_owner() {
    let repo = InMemoryBoardRepository::new();
    let ids = SequentialIdGen::new();
    let board = anamnesis_app::create_board(&repo, &ids, &alice(), "My Board")
        .await
        .unwrap();

    let result = anamnesis_app::delete_board(&repo, board.id, &mallory()).await;

    assert!(matches!(result, Err(AppError::Forbidden)));
    // The board must still be there: a forbidden delete has no effect.
    assert!(repo.load(board.id).await.unwrap().is_some());
}

#[tokio::test]
async fn delete_board_is_not_found_when_it_does_not_exist() {
    let repo = InMemoryBoardRepository::new();
    let missing = anamnesis_core::BoardId::new(uuid::Uuid::from_u128(999));

    let result = anamnesis_app::delete_board(&repo, missing, &alice()).await;

    assert!(matches!(result, Err(AppError::NotFound)));
}
