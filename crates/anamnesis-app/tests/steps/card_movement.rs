//! Steps for `card_movement.feature`: reordering within a column, moving
//! across columns, work-in-progress limits, index clamping.

use cucumber::gherkin::Step;
use cucumber::{then, when};

use super::AppWorld;

#[when(expr = "{word} moves the card {string} on {string} to position {int} in {string}")]
#[when(expr = "{word} tries to move the card {string} on {string} to position {int} in {string}")]
async fn moves_a_card(
    world: &mut AppWorld,
    user: String,
    card: String,
    board: String,
    to_index: usize,
    to_column: String,
) {
    let actor = world.user(&user);
    let board_id = world.board_id(&board);
    let card_id = world.card_id(&board, &card);
    let to_column_id = world.column_id(&board, &to_column);
    match anamnesis_app::move_card(
        &world.repo,
        board_id,
        &actor,
        card_id,
        to_column_id,
        to_index,
    )
    .await
    {
        Ok(updated) => {
            world.last_board = Some(updated);
            world.last_error = None;
        }
        Err(err) => world.last_error = Some(err),
    }
}

#[then(expr = "{string} on {string} has the following cards in order:")]
async fn has_cards_in_order(world: &mut AppWorld, column: String, board: String, step: &Step) {
    let table = step.table.as_ref().expect("expected a data table");
    let expected: Vec<&str> = table.rows.iter().skip(1).map(|r| r[0].as_str()).collect();
    let reloaded = world.reload(&board).await;
    let found = reloaded
        .columns
        .iter()
        .find(|c| c.title.as_str() == column)
        .unwrap_or_else(|| panic!("no column named {column:?}"));
    let actual: Vec<&str> = found.cards.iter().map(|c| c.title.as_str()).collect();
    assert_eq!(actual, expected);
}
