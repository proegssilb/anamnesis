//! Steps for `authorization.feature`: a second user can neither read nor
//! mutate another user's board.

use cucumber::when;

use super::AppWorld;

/// Dispatches one of a fixed set of plain-language actions against a named
/// board, always seeded with a column "To Do" and a card "Alice's card" by
/// the feature's `Background`. Captured as free text (not `{string}`) so the
/// `Examples` table in the scenario outline can list each action unquoted.
#[when(regex = r#"^(\S+) tries to (.+) on "([^"]+)"$"#)]
async fn tries_to_act_on_board(world: &mut AppWorld, user: String, action: String, board: String) {
    let actor = world.user(&user);
    let board_id = world.board_id(&board);

    let result = match action.as_str() {
        "view the board" => anamnesis_app::view_board(&world.repo, board_id, &actor)
            .await
            .map(|_| ()),
        "add a column" => anamnesis_app::add_column(
            &world.repo,
            &world.ids,
            board_id,
            &actor,
            "New Column",
            None,
        )
        .await
        .map(|_| ()),
        "add a card" => {
            let column_id = world.column_id(&board, "To Do");
            anamnesis_app::add_card(
                &world.repo,
                &world.ids,
                &world.clock,
                board_id,
                &actor,
                column_id,
                "New Card",
                "",
            )
            .await
            .map(|_| ())
        }
        "move a card" => {
            let card_id = world.card_id(&board, "Alice's card");
            let column_id = world.column_id(&board, "To Do");
            anamnesis_app::move_card(&world.repo, board_id, &actor, card_id, column_id, 0)
                .await
                .map(|_| ())
        }
        "edit a card" => {
            let card_id = world.card_id(&board, "Alice's card");
            anamnesis_app::edit_card(&world.repo, board_id, &actor, card_id, "Edited", "")
                .await
                .map(|_| ())
        }
        "delete a card" => {
            let card_id = world.card_id(&board, "Alice's card");
            anamnesis_app::delete_card(&world.repo, board_id, &actor, card_id)
                .await
                .map(|_| ())
        }
        "delete the board" => anamnesis_app::delete_board(&world.repo, board_id, &actor).await,
        other => panic!("authorization.feature: unknown action {other:?}"),
    };

    world.last_error = result.err();
}
