//! Steps for `board_management.feature`: creating boards, organising them
//! into columns, adding/renaming/deleting cards, deleting boards.

use cucumber::gherkin::Step;
use cucumber::{given, then, when};

use anamnesis_app::AppError;
use anamnesis_core::legacy::DomainError;

use super::AppWorld;

// --- Given: a signed-in user. ---

#[given(expr = "{word} is signed in")]
async fn signed_in(world: &mut AppWorld, name: String) {
    world.user(&name);
}

// --- Given/When: a board comes to exist, plain or with a column. ---

#[given(expr = "{word} has a board named {string}")]
#[when(expr = "{word} creates a board named {string}")]
async fn has_or_creates_a_board(world: &mut AppWorld, owner: String, board: String) {
    world.ensure_board(&owner, &board).await;
}

#[given(expr = "{word} has a board named {string} with a column named {string}")]
async fn has_a_board_with_a_column(
    world: &mut AppWorld,
    owner: String,
    board: String,
    column: String,
) {
    let board_id = world.ensure_board(&owner, &board).await;
    let owner_id = world.board_owner(&board);
    let updated =
        anamnesis_app::add_column(&world.repo, &world.ids, board_id, &owner_id, &column, None)
            .await
            .expect("scenario setup: add_column must succeed");
    let column_id = updated
        .columns
        .iter()
        .find(|c| c.title.as_str() == column)
        .expect("just-added column must be present")
        .id;
    world.remember_column(&board, &column, column_id);
    world.last_board = Some(updated);
}

#[given(
    expr = "{word} has a board named {string} with a column named {string} with a work-in-progress limit of {int}"
)]
async fn has_a_board_with_a_limited_column(
    world: &mut AppWorld,
    owner: String,
    board: String,
    column: String,
    limit: u16,
) {
    let board_id = world.ensure_board(&owner, &board).await;
    let owner_id = world.board_owner(&board);
    let updated = anamnesis_app::add_column(
        &world.repo,
        &world.ids,
        board_id,
        &owner_id,
        &column,
        Some(limit),
    )
    .await
    .expect("scenario setup: add_column must succeed");
    let column_id = updated
        .columns
        .iter()
        .find(|c| c.title.as_str() == column)
        .expect("just-added column must be present")
        .id;
    world.remember_column(&board, &column, column_id);
    world.last_board = Some(updated);
}

// --- Given: seeding a column with cards, in order, via a data table. ---

#[given(expr = "{string} on {string} has the following cards in order:")]
async fn seed_cards_in_order(world: &mut AppWorld, column: String, board: String, step: &Step) {
    let table = step.table.as_ref().expect("expected a data table");
    let owner = world.board_owner(&board);
    let column_id = world.column_id(&board, &column);
    // First row is the header ("title"); the rest are the card titles, in order.
    for row in table.rows.iter().skip(1) {
        let title = &row[0];
        let updated = anamnesis_app::add_card(
            &world.repo,
            &world.ids,
            &world.clock,
            world.board_id(&board),
            &owner,
            column_id,
            title,
            "",
        )
        .await
        .expect("scenario setup: add_card must succeed");
        let card_id = updated
            .columns
            .iter()
            .flat_map(|c| c.cards.iter())
            .find(|c| c.title.as_str() == title)
            .expect("just-added card must be present")
            .id;
        world.remember_card(&board, title, card_id);
        world.last_board = Some(updated);
    }
}

// --- When: the mutating actions board_management.feature exercises. ---

#[when(expr = "{word} adds a column named {string} to {string}")]
async fn adds_a_column(world: &mut AppWorld, user: String, column: String, board: String) {
    let actor = world.user(&user);
    let board_id = world.board_id(&board);
    match anamnesis_app::add_column(&world.repo, &world.ids, board_id, &actor, &column, None).await
    {
        Ok(updated) => {
            let column_id = updated
                .columns
                .iter()
                .find(|c| c.title.as_str() == column)
                .expect("just-added column must be present")
                .id;
            world.remember_column(&board, &column, column_id);
            world.last_board = Some(updated);
            world.last_error = None;
        }
        Err(err) => world.last_error = Some(err),
    }
}

#[given(expr = "{word} adds a card {string} to {string} on {string}")]
#[when(expr = "{word} adds a card {string} to {string} on {string}")]
async fn adds_a_card(
    world: &mut AppWorld,
    user: String,
    card: String,
    column: String,
    board: String,
) {
    let actor = world.user(&user);
    let board_id = world.board_id(&board);
    let column_id = world.column_id(&board, &column);
    match anamnesis_app::add_card(
        &world.repo,
        &world.ids,
        &world.clock,
        board_id,
        &actor,
        column_id,
        &card,
        "",
    )
    .await
    {
        Ok(updated) => {
            let card_id = updated
                .columns
                .iter()
                .flat_map(|c| c.cards.iter())
                .find(|c| c.title.as_str() == card)
                .expect("just-added card must be present")
                .id;
            world.remember_card(&board, &card, card_id);
            world.last_board = Some(updated);
            world.last_error = None;
        }
        Err(err) => world.last_error = Some(err),
    }
}

#[when(expr = "{word} renames the card {string} on {string} to {string}")]
async fn renames_a_card(
    world: &mut AppWorld,
    user: String,
    old_title: String,
    board: String,
    new_title: String,
) {
    let actor = world.user(&user);
    let board_id = world.board_id(&board);
    let card_id = world.card_id(&board, &old_title);
    let current = world.reload(&board).await;
    let body = current
        .columns
        .iter()
        .flat_map(|c| c.cards.iter())
        .find(|c| c.id == card_id)
        .expect("renamed card must exist")
        .body
        .clone();
    match anamnesis_app::edit_card(&world.repo, board_id, &actor, card_id, &new_title, &body).await
    {
        Ok(updated) => {
            world.remember_card(&board, &new_title, card_id);
            world.last_board = Some(updated);
            world.last_error = None;
        }
        Err(err) => world.last_error = Some(err),
    }
}

#[when(expr = "{word} deletes the card {string} from {string}")]
async fn deletes_a_card(world: &mut AppWorld, user: String, card: String, board: String) {
    let actor = world.user(&user);
    let board_id = world.board_id(&board);
    let card_id = world.card_id(&board, &card);
    match anamnesis_app::delete_card(&world.repo, board_id, &actor, card_id).await {
        Ok(updated) => {
            world.last_board = Some(updated);
            world.last_error = None;
        }
        Err(err) => world.last_error = Some(err),
    }
}

#[when(expr = "{word} deletes the board {string}")]
async fn deletes_a_board(world: &mut AppWorld, user: String, board: String) {
    let actor = world.user(&user);
    let board_id = world.board_id(&board);
    world.last_error = anamnesis_app::delete_board(&world.repo, board_id, &actor)
        .await
        .err();
}

// --- Then: assertions board_management.feature makes. ---

#[then(expr = "the board {string} exists")]
async fn board_exists(world: &mut AppWorld, board: String) {
    let _ = world.reload(&board).await;
}

#[then(expr = "{word} can see {string} in their list of boards")]
async fn can_see_board(world: &mut AppWorld, user: String, board: String) {
    let actor = world.user(&user);
    let summaries = anamnesis_app::list_boards(&world.repo, &actor)
        .await
        .unwrap();
    assert!(
        summaries.iter().any(|s| s.title.as_str() == board),
        "expected {user} to see {board:?} in their boards, got {summaries:?}"
    );
}

#[then(expr = "{word} can no longer see {string} in their list of boards")]
#[then(expr = "{word} cannot see {string} in their list of boards")]
async fn cannot_see_board(world: &mut AppWorld, user: String, board: String) {
    let actor = world.user(&user);
    let summaries = anamnesis_app::list_boards(&world.repo, &actor)
        .await
        .unwrap();
    assert!(
        !summaries.iter().any(|s| s.title.as_str() == board),
        "expected {user} not to see {board:?} in their boards, got {summaries:?}"
    );
}

#[then(expr = "{string} has the following columns in order:")]
async fn has_columns_in_order(world: &mut AppWorld, board: String, step: &Step) {
    let table = step.table.as_ref().expect("expected a data table");
    let expected: Vec<&str> = table.rows.iter().skip(1).map(|r| r[0].as_str()).collect();
    let reloaded = world.reload(&board).await;
    let actual: Vec<&str> = reloaded.columns.iter().map(|c| c.title.as_str()).collect();
    assert_eq!(actual, expected);
}

#[then(expr = "{string} on {string} contains a card {string}")]
async fn column_contains_card(world: &mut AppWorld, column: String, board: String, card: String) {
    let reloaded = world.reload(&board).await;
    let found = reloaded
        .columns
        .iter()
        .find(|c| c.title.as_str() == column)
        .unwrap_or_else(|| panic!("no column named {column:?}"));
    assert!(
        found.cards.iter().any(|c| c.title.as_str() == card),
        "expected a card named {card:?} in {column:?}, got {:?}",
        found
            .cards
            .iter()
            .map(|c| c.title.as_str())
            .collect::<Vec<_>>()
    );
}

#[then(expr = "{string} on {string} has no cards")]
async fn column_has_no_cards(world: &mut AppWorld, column: String, board: String) {
    let reloaded = world.reload(&board).await;
    let found = reloaded
        .columns
        .iter()
        .find(|c| c.title.as_str() == column)
        .unwrap_or_else(|| panic!("no column named {column:?}"));
    assert!(
        found.cards.is_empty(),
        "expected no cards, got {:?}",
        found.cards
    );
}

#[then(expr = "{word} is told the column is full")]
async fn told_column_is_full(world: &mut AppWorld, _user: String) {
    assert!(
        matches!(
            world.last_error,
            Some(AppError::Domain(DomainError::WipLimitExceeded))
        ),
        "expected a WIP-limit error, got {:?}",
        world.last_error
    );
}

#[then(expr = "{word} is forbidden")]
async fn told_forbidden(world: &mut AppWorld, _user: String) {
    assert!(
        matches!(world.last_error, Some(AppError::Forbidden)),
        "expected Forbidden, got {:?}",
        world.last_error
    );
}
