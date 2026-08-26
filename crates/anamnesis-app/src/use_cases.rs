//! The use cases: orchestration only. Each one loads an aggregate through a
//! port, calls exactly one pure `anamnesis-core` transition, saves the
//! result through a port, and maps errors. Every use case that touches an
//! existing board checks `can_view` first and returns `AppError::Forbidden`
//! on failure — including the read paths, so a board can never be read by
//! anyone but its owner.

use anamnesis_core::{self as core, Board, BoardId, CardId, ColumnId, UserId};

use crate::error::AppError;
use crate::ports::{BoardRepository, BoardSummary, Clock, IdGen};

/// Loads `id` and checks that `user` may view it, mapping a missing board to
/// [`AppError::NotFound`] and a present-but-foreign board to
/// [`AppError::Forbidden`]. Every use case below that addresses an existing
/// board starts here.
async fn load_visible(
    repo: &dyn BoardRepository,
    id: BoardId,
    user: &UserId,
) -> Result<Board, AppError> {
    let board = repo.load(id).await?.ok_or(AppError::NotFound)?;
    if !core::can_view(&board, user) {
        return Err(AppError::Forbidden);
    }
    Ok(board)
}

/// Creates a new, empty board owned by `owner`.
pub async fn create_board(
    repo: &dyn BoardRepository,
    id_gen: &dyn IdGen,
    owner: &UserId,
    title: &str,
) -> Result<Board, AppError> {
    let board = core::create_board(BoardId::new(id_gen.next()), owner.clone(), title)?;
    repo.save(&board).await?;
    Ok(board)
}

/// Lists the boards owned by `current_user`. There is no parameter naming a
/// different user to list on their behalf, so a caller can never see another
/// user's boards by construction — the same read-leak guard the other use
/// cases enforce with `can_view`, applied at the query's shape rather than
/// after the fact.
pub async fn list_boards(
    repo: &dyn BoardRepository,
    current_user: &UserId,
) -> Result<Vec<BoardSummary>, AppError> {
    Ok(repo.list_for_owner(current_user).await?)
}

/// Returns a board for viewing. `Forbidden` for anyone but its owner.
pub async fn view_board(
    repo: &dyn BoardRepository,
    board_id: BoardId,
    current_user: &UserId,
) -> Result<Board, AppError> {
    load_visible(repo, board_id, current_user).await
}

/// Appends a new column to a board.
pub async fn add_column(
    repo: &dyn BoardRepository,
    id_gen: &dyn IdGen,
    board_id: BoardId,
    current_user: &UserId,
    title: &str,
    wip_limit: Option<u16>,
) -> Result<Board, AppError> {
    let board = load_visible(repo, board_id, current_user).await?;
    let board = core::add_column(&board, ColumnId::new(id_gen.next()), title, wip_limit)?;
    repo.save(&board).await?;
    Ok(board)
}

/// Appends a new card to a column on a board.
#[allow(clippy::too_many_arguments)]
pub async fn add_card(
    repo: &dyn BoardRepository,
    id_gen: &dyn IdGen,
    clock: &dyn Clock,
    board_id: BoardId,
    current_user: &UserId,
    column_id: ColumnId,
    title: &str,
    body: &str,
) -> Result<Board, AppError> {
    let board = load_visible(repo, board_id, current_user).await?;
    let board = core::add_card(
        &board,
        column_id,
        CardId::new(id_gen.next()),
        title,
        body,
        clock.now(),
    )?;
    repo.save(&board).await?;
    Ok(board)
}

/// Moves a card to a position within a (possibly different) column. Cards
/// are unique board-wide, so the card alone (plus the board it lives on)
/// addresses it.
pub async fn move_card(
    repo: &dyn BoardRepository,
    board_id: BoardId,
    current_user: &UserId,
    card_id: CardId,
    to_column: ColumnId,
    to_index: usize,
) -> Result<Board, AppError> {
    let board = load_visible(repo, board_id, current_user).await?;
    let board = core::move_card(&board, card_id, to_column, to_index)?;
    repo.save(&board).await?;
    Ok(board)
}

/// Replaces a card's title and body.
pub async fn edit_card(
    repo: &dyn BoardRepository,
    board_id: BoardId,
    current_user: &UserId,
    card_id: CardId,
    title: &str,
    body: &str,
) -> Result<Board, AppError> {
    let board = load_visible(repo, board_id, current_user).await?;
    let board = core::edit_card(&board, card_id, title, body)?;
    repo.save(&board).await?;
    Ok(board)
}

/// Removes a card from wherever it sits on the board.
pub async fn delete_card(
    repo: &dyn BoardRepository,
    board_id: BoardId,
    current_user: &UserId,
    card_id: CardId,
) -> Result<Board, AppError> {
    let board = load_visible(repo, board_id, current_user).await?;
    let board = core::remove_card(&board, card_id)?;
    repo.save(&board).await?;
    Ok(board)
}

/// Deletes an entire board.
pub async fn delete_board(
    repo: &dyn BoardRepository,
    board_id: BoardId,
    current_user: &UserId,
) -> Result<(), AppError> {
    load_visible(repo, board_id, current_user).await?;
    repo.delete(board_id).await?;
    Ok(())
}
