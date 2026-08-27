//! The eight pure state transitions plus the `can_view` policy.
//!
//! Every transition here is `fn(&Board, ...) -> Result<Board, DomainError>`:
//! it reads the current aggregate, applies one rule, and returns a brand new
//! `Board` rather than mutating in place. `now` and freshly minted ids are
//! parameters, never reads — this module contains no clock and no RNG.

use crate::ids::{BoardId, CardId, ColumnId, Timestamp, UserId};
use crate::title::Title;

use super::error::DomainError;
use super::model::{Board, Card, Column};

/// Finds the `(column_index, card_index)` of the card with id `card` in
/// `board`, if it exists anywhere on the board.
fn locate_card(board: &Board, card: CardId) -> Option<(usize, usize)> {
    for (ci, column) in board.columns.iter().enumerate() {
        if let Some(vi) = column.cards.iter().position(|c| c.id == card) {
            return Some((ci, vi));
        }
    }
    None
}

fn find_column_index(board: &Board, column: ColumnId) -> Option<usize> {
    board.columns.iter().position(|c| c.id == column)
}

/// Moves `card` to position `to_index` within `to_column`.
///
/// `to_index` is interpreted against the destination column *after* the card
/// has been removed from wherever it currently sits — so for a same-column
/// move this already accounts for the shift the removal causes, and the card
/// lands exactly at `to_index` rather than one short. An index past the end
/// of the destination column clamps to the end rather than erroring.
///
/// A WIP limit on the destination column is checked against its contents
/// with the card already removed, so a same-column reorder never trips a
/// limit the card itself was already counted against; a cross-column move
/// into a column already at its limit does.
pub fn move_card(
    board: &Board,
    card: CardId,
    to_column: ColumnId,
    to_index: usize,
) -> Result<Board, DomainError> {
    let (from_ci, from_vi) = locate_card(board, card).ok_or(DomainError::CardNotFound(card))?;
    let to_ci =
        find_column_index(board, to_column).ok_or(DomainError::ColumnNotFound(to_column))?;

    let mut columns = board.columns.clone();
    let moving = columns[from_ci].cards.remove(from_vi);

    let wip_limit = columns[to_ci].wip_limit;
    let target_len = columns[to_ci].cards.len();
    if let Some(limit) = wip_limit
        && target_len >= limit as usize
    {
        return Err(DomainError::WipLimitExceeded);
    }

    let clamped = to_index.min(columns[to_ci].cards.len());
    columns[to_ci].cards.insert(clamped, moving);

    Ok(Board {
        columns,
        ..board.clone()
    })
}

/// Builds a brand new, empty board owned by `owner`.
///
/// `id` is supplied by the caller — this crate never generates one.
pub fn create_board(
    id: BoardId,
    owner: UserId,
    title: impl AsRef<str>,
) -> Result<Board, DomainError> {
    let title = Title::new(title)?;
    Ok(Board {
        id,
        owner,
        title,
        columns: Vec::new(),
    })
}

/// Appends a new, empty column to `board`.
///
/// `id` is supplied by the caller and must not already name a column on
/// this board.
pub fn add_column(
    board: &Board,
    id: ColumnId,
    title: impl AsRef<str>,
    wip_limit: Option<u16>,
) -> Result<Board, DomainError> {
    if find_column_index(board, id).is_some() {
        return Err(DomainError::DuplicateId);
    }
    let title = Title::new(title)?;

    let mut columns = board.columns.clone();
    columns.push(Column {
        id,
        title,
        wip_limit,
        cards: Vec::new(),
    });

    Ok(Board {
        columns,
        ..board.clone()
    })
}

/// Replaces the title of an existing column.
pub fn rename_column(
    board: &Board,
    column: ColumnId,
    title: impl AsRef<str>,
) -> Result<Board, DomainError> {
    let ci = find_column_index(board, column).ok_or(DomainError::ColumnNotFound(column))?;
    let title = Title::new(title)?;

    let mut columns = board.columns.clone();
    columns[ci].title = title;

    Ok(Board {
        columns,
        ..board.clone()
    })
}

/// Removes a column. Fails with `ColumnNotEmpty` unless it holds no cards.
pub fn remove_column(board: &Board, column: ColumnId) -> Result<Board, DomainError> {
    let ci = find_column_index(board, column).ok_or(DomainError::ColumnNotFound(column))?;
    if !board.columns[ci].cards.is_empty() {
        return Err(DomainError::ColumnNotEmpty);
    }

    let mut columns = board.columns.clone();
    columns.remove(ci);

    Ok(Board {
        columns,
        ..board.clone()
    })
}

/// Appends a new card to the end of `column`.
///
/// `id` is supplied by the caller and must not already name a card anywhere
/// on this board. `now` is supplied by the caller; this crate reads no
/// clock.
pub fn add_card(
    board: &Board,
    column: ColumnId,
    id: CardId,
    title: impl AsRef<str>,
    body: impl Into<String>,
    now: Timestamp,
) -> Result<Board, DomainError> {
    let ci = find_column_index(board, column).ok_or(DomainError::ColumnNotFound(column))?;

    if let Some(limit) = board.columns[ci].wip_limit
        && board.columns[ci].cards.len() >= limit as usize
    {
        return Err(DomainError::WipLimitExceeded);
    }
    if locate_card(board, id).is_some() {
        return Err(DomainError::DuplicateId);
    }
    let title = Title::new(title)?;

    let mut columns = board.columns.clone();
    columns[ci].cards.push(Card {
        id,
        title,
        body: body.into(),
        created_at: now,
    });

    Ok(Board {
        columns,
        ..board.clone()
    })
}

/// Replaces a card's title and body. Its id and `created_at` are unchanged.
pub fn edit_card(
    board: &Board,
    card: CardId,
    title: impl AsRef<str>,
    body: impl Into<String>,
) -> Result<Board, DomainError> {
    let (ci, vi) = locate_card(board, card).ok_or(DomainError::CardNotFound(card))?;
    let title = Title::new(title)?;

    let mut columns = board.columns.clone();
    columns[ci].cards[vi].title = title;
    columns[ci].cards[vi].body = body.into();

    Ok(Board {
        columns,
        ..board.clone()
    })
}

/// Removes a card from wherever it currently sits.
pub fn remove_card(board: &Board, card: CardId) -> Result<Board, DomainError> {
    let (ci, vi) = locate_card(board, card).ok_or(DomainError::CardNotFound(card))?;

    let mut columns = board.columns.clone();
    columns[ci].cards.remove(vi);

    Ok(Board {
        columns,
        ..board.clone()
    })
}

/// Ownership policy: a user may view (and, by the same rule, mutate) a board
/// only if they own it. This is a domain rule, not an app-layer `if`.
pub fn can_view(board: &Board, user: &UserId) -> bool {
    &board.owner == user
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use uuid::Uuid;

    fn uid(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn title(s: &str) -> Title {
        Title::new(s).unwrap()
    }

    fn ts(secs: i64) -> Timestamp {
        Timestamp::from_unix_seconds(secs).unwrap()
    }

    fn card(n: u128, name: &str) -> Card {
        Card {
            id: CardId::new(uid(n)),
            title: title(name),
            body: String::new(),
            created_at: ts(0),
        }
    }

    /// A board owned by `alice` with a single column holding `cards`.
    fn board_with_column(cards: Vec<Card>, wip_limit: Option<u16>) -> (Board, ColumnId) {
        let column_id = ColumnId::new(uid(1_000));
        let board = Board {
            id: BoardId::new(uid(1)),
            owner: UserId::new("alice"),
            title: title("Board"),
            columns: vec![Column {
                id: column_id,
                title: title("Column"),
                wip_limit,
                cards,
            }],
        };
        (board, column_id)
    }

    fn card_titles(board: &Board, column: ColumnId) -> Vec<String> {
        board.columns[find_column_index(board, column).unwrap()]
            .cards
            .iter()
            .map(|c| c.title.as_str().to_string())
            .collect()
    }

    // --- move_card: the cases PLAN.md calls out as the ones that bite. ---

    #[test]
    fn move_card_downward_within_same_column_lands_exactly_at_requested_index() {
        // A, B, C, D. Move A (index 0) down to index 2.
        // Naive remove-then-insert-at-original-index bugs land it one short
        // (index 1); the correct result is index 2, i.e. [B, C, A, D].
        let (board, col) = board_with_column(
            vec![card(1, "A"), card(2, "B"), card(3, "C"), card(4, "D")],
            None,
        );
        let a = board.columns[0].cards[0].id;

        let moved = move_card(&board, a, col, 2).unwrap();

        assert_eq!(card_titles(&moved, col), vec!["B", "C", "A", "D"]);
    }

    #[test]
    fn move_card_downward_to_last_index_appends() {
        let (board, col) = board_with_column(
            vec![card(1, "A"), card(2, "B"), card(3, "C"), card(4, "D")],
            None,
        );
        let a = board.columns[0].cards[0].id;

        let moved = move_card(&board, a, col, 3).unwrap();

        assert_eq!(card_titles(&moved, col), vec!["B", "C", "D", "A"]);
    }

    #[test]
    fn move_card_to_index_beyond_column_length_clamps_to_end() {
        let (board, col) = board_with_column(vec![card(1, "A"), card(2, "B")], None);
        let a = board.columns[0].cards[0].id;

        let moved = move_card(&board, a, col, 999).unwrap();

        assert_eq!(card_titles(&moved, col), vec!["B", "A"]);
    }

    #[test]
    fn move_card_upward_within_same_column_lands_at_requested_index() {
        let (board, col) = board_with_column(
            vec![card(1, "A"), card(2, "B"), card(3, "C"), card(4, "D")],
            None,
        );
        let d = board.columns[0].cards[3].id;

        let moved = move_card(&board, d, col, 1).unwrap();

        assert_eq!(card_titles(&moved, col), vec!["A", "D", "B", "C"]);
    }

    #[test]
    fn move_card_same_column_move_does_not_trip_a_full_wip_limit() {
        // Column is already at its WIP limit (2/2). Reordering within the
        // same column must not count the card twice against the limit.
        let (board, col) = board_with_column(vec![card(1, "A"), card(2, "B")], Some(2));
        let a = board.columns[0].cards[0].id;

        let moved = move_card(&board, a, col, 1).unwrap();

        assert_eq!(card_titles(&moved, col), vec!["B", "A"]);
    }

    #[test]
    fn move_card_cross_column_move_into_a_full_column_trips_wip_limit() {
        let full_col = ColumnId::new(uid(2_000));
        let empty_col_card = card(10, "Only");
        let source_col_id;
        let board = {
            let (b, source_id) = board_with_column(vec![card(1, "A")], None);
            source_col_id = source_id;
            let mut columns = b.columns;
            columns.push(Column {
                id: full_col,
                title: title("Full"),
                wip_limit: Some(1),
                cards: vec![empty_col_card],
            });
            Board { columns, ..b }
        };
        let a = board.columns[0].cards[0].id;
        let _ = source_col_id;

        let result = move_card(&board, a, full_col, 0);

        assert_eq!(result, Err(DomainError::WipLimitExceeded));
    }

    #[test]
    fn move_card_cross_column_move_into_a_non_full_column_succeeds() {
        let target_col = ColumnId::new(uid(2_001));
        let (board, source_col) = board_with_column(vec![card(1, "A")], None);
        let mut columns = board.columns.clone();
        columns.push(Column {
            id: target_col,
            title: title("Target"),
            wip_limit: Some(2),
            cards: vec![card(9, "Existing")],
        });
        let board = Board { columns, ..board };
        let a = board.columns[0].cards[0].id;

        let moved = move_card(&board, a, target_col, 0).unwrap();

        assert_eq!(card_titles(&moved, source_col), Vec::<String>::new());
        assert_eq!(card_titles(&moved, target_col), vec!["A", "Existing"]);
    }

    #[test]
    fn move_card_errors_when_card_not_found() {
        let (board, col) = board_with_column(vec![], None);
        let missing = CardId::new(uid(999));

        assert_eq!(
            move_card(&board, missing, col, 0),
            Err(DomainError::CardNotFound(missing))
        );
    }

    #[test]
    fn move_card_errors_when_target_column_not_found() {
        let (board, _col) = board_with_column(vec![card(1, "A")], None);
        let a = board.columns[0].cards[0].id;
        let missing = ColumnId::new(uid(999));

        assert_eq!(
            move_card(&board, a, missing, 0),
            Err(DomainError::ColumnNotFound(missing))
        );
    }

    // --- remove_column: ColumnNotEmpty is one of the PLAN.md priority cases. ---

    #[test]
    fn remove_column_errors_when_column_not_empty() {
        let (board, col) = board_with_column(vec![card(1, "A")], None);

        assert_eq!(remove_column(&board, col), Err(DomainError::ColumnNotEmpty));
    }

    #[test]
    fn remove_column_removes_an_empty_column() {
        let (board, col) = board_with_column(vec![], None);

        let result = remove_column(&board, col).unwrap();

        assert!(result.columns.is_empty());
    }

    #[test]
    fn remove_column_errors_when_column_not_found() {
        let (board, _col) = board_with_column(vec![], None);
        let missing = ColumnId::new(uid(999));

        assert_eq!(
            remove_column(&board, missing),
            Err(DomainError::ColumnNotFound(missing))
        );
    }

    // --- create_board ---

    #[test]
    fn create_board_builds_an_empty_board_owned_by_the_given_user() {
        let id = BoardId::new(uid(1));
        let owner = UserId::new("alice");

        let board = create_board(id, owner.clone(), "My Board").unwrap();

        assert_eq!(board.id, id);
        assert_eq!(board.owner, owner);
        assert_eq!(board.title.as_str(), "My Board");
        assert!(board.columns.is_empty());
    }

    #[test]
    fn create_board_rejects_an_invalid_title() {
        let result = create_board(BoardId::new(uid(1)), UserId::new("alice"), "   ");
        assert!(matches!(result, Err(DomainError::InvalidTitle(_))));
    }

    // --- add_column ---

    #[test]
    fn add_column_appends_a_new_empty_column() {
        let (board, _col) = board_with_column(vec![], None);
        let new_id = ColumnId::new(uid(2_000));

        let result = add_column(&board, new_id, "Doing", Some(3)).unwrap();

        assert_eq!(result.columns.len(), 2);
        let added = &result.columns[1];
        assert_eq!(added.id, new_id);
        assert_eq!(added.title.as_str(), "Doing");
        assert_eq!(added.wip_limit, Some(3));
        assert!(added.cards.is_empty());
    }

    #[test]
    fn add_column_rejects_a_duplicate_id() {
        let (board, col) = board_with_column(vec![], None);

        let result = add_column(&board, col, "Doing", None);

        assert_eq!(result, Err(DomainError::DuplicateId));
    }

    #[test]
    fn add_column_rejects_an_invalid_title() {
        let (board, _col) = board_with_column(vec![], None);

        let result = add_column(&board, ColumnId::new(uid(2_000)), "", None);

        assert!(matches!(result, Err(DomainError::InvalidTitle(_))));
    }

    // --- rename_column ---

    #[test]
    fn rename_column_replaces_the_title() {
        let (board, col) = board_with_column(vec![], None);

        let result = rename_column(&board, col, "Renamed").unwrap();

        assert_eq!(
            result.columns[find_column_index(&result, col).unwrap()]
                .title
                .as_str(),
            "Renamed"
        );
    }

    #[test]
    fn rename_column_errors_when_not_found() {
        let (board, _col) = board_with_column(vec![], None);
        let missing = ColumnId::new(uid(999));

        assert_eq!(
            rename_column(&board, missing, "Renamed"),
            Err(DomainError::ColumnNotFound(missing))
        );
    }

    // --- add_card ---

    #[test]
    fn add_card_appends_to_the_column() {
        let (board, col) = board_with_column(vec![], None);
        let new_id = CardId::new(uid(2_000));

        let result = add_card(&board, col, new_id, "New card", "body text", ts(42)).unwrap();

        let added = &result.columns[0].cards[0];
        assert_eq!(added.id, new_id);
        assert_eq!(added.title.as_str(), "New card");
        assert_eq!(added.body, "body text");
        assert_eq!(added.created_at, ts(42));
    }

    #[rstest]
    #[case(1, true)] // one below the limit of 2: succeeds
    #[case(2, false)] // exactly at the limit of 2: rejected
    fn add_card_respects_the_wip_limit_boundary(
        #[case] existing: usize,
        #[case] should_succeed: bool,
    ) {
        let cards = (0..existing as u128)
            .map(|n| card(n, &format!("Card {n}")))
            .collect();
        let (board, col) = board_with_column(cards, Some(2));

        let result = add_card(&board, col, CardId::new(uid(900)), "New", "", ts(0));

        assert_eq!(result.is_ok(), should_succeed);
        if !should_succeed {
            assert_eq!(result.unwrap_err(), DomainError::WipLimitExceeded);
        }
    }

    #[test]
    fn add_card_rejects_a_duplicate_id() {
        let (board, col) = board_with_column(vec![card(1, "A")], None);
        let existing = board.columns[0].cards[0].id;

        let result = add_card(&board, col, existing, "B", "", ts(0));

        assert_eq!(result, Err(DomainError::DuplicateId));
    }

    #[test]
    fn add_card_errors_when_column_not_found() {
        let (board, _col) = board_with_column(vec![], None);
        let missing = ColumnId::new(uid(999));

        assert_eq!(
            add_card(&board, missing, CardId::new(uid(2)), "B", "", ts(0)),
            Err(DomainError::ColumnNotFound(missing))
        );
    }

    // --- edit_card ---

    #[test]
    fn edit_card_replaces_title_and_body_but_keeps_id_and_created_at() {
        let (board, _col) = board_with_column(vec![card(1, "A")], None);
        let a = board.columns[0].cards[0].id;

        let result = edit_card(&board, a, "A2", "new body").unwrap();

        let edited = &result.columns[0].cards[0];
        assert_eq!(edited.id, a);
        assert_eq!(edited.title.as_str(), "A2");
        assert_eq!(edited.body, "new body");
        assert_eq!(edited.created_at, ts(0));
    }

    #[test]
    fn edit_card_errors_when_not_found() {
        let (board, _col) = board_with_column(vec![], None);
        let missing = CardId::new(uid(999));

        assert_eq!(
            edit_card(&board, missing, "x", "y"),
            Err(DomainError::CardNotFound(missing))
        );
    }

    // --- remove_card ---

    #[test]
    fn remove_card_removes_it_from_its_column() {
        let (board, col) = board_with_column(vec![card(1, "A"), card(2, "B")], None);
        let a = board.columns[0].cards[0].id;

        let result = remove_card(&board, a).unwrap();

        assert_eq!(card_titles(&result, col), vec!["B"]);
    }

    #[test]
    fn remove_card_errors_when_not_found() {
        let (board, _col) = board_with_column(vec![], None);
        let missing = CardId::new(uid(999));

        assert_eq!(
            remove_card(&board, missing),
            Err(DomainError::CardNotFound(missing))
        );
    }

    // --- can_view ---

    #[test]
    fn can_view_is_true_for_the_owner() {
        let (board, _col) = board_with_column(vec![], None);
        assert!(can_view(&board, &UserId::new("alice")));
    }

    #[test]
    fn can_view_is_false_for_a_non_owner() {
        let (board, _col) = board_with_column(vec![], None);
        assert!(!can_view(&board, &UserId::new("mallory")));
    }

    // --- cross-cutting: ids stay unique through a sequence of transitions ---

    fn assert_unique_ids(board: &Board) {
        let mut column_ids: Vec<_> = board.columns.iter().map(|c| c.id).collect();
        let before = column_ids.len();
        column_ids.sort();
        column_ids.dedup();
        assert_eq!(column_ids.len(), before, "duplicate column id in {board:?}");

        let mut card_ids: Vec<_> = board
            .columns
            .iter()
            .flat_map(|c| c.cards.iter().map(|card| card.id))
            .collect();
        let before = card_ids.len();
        card_ids.sort();
        card_ids.dedup();
        assert_eq!(card_ids.len(), before, "duplicate card id in {board:?}");
    }

    #[test]
    fn every_transition_leaves_the_boards_ids_unique() {
        let board = create_board(BoardId::new(uid(1)), UserId::new("alice"), "Board").unwrap();
        assert_unique_ids(&board);

        let todo = ColumnId::new(uid(10));
        let board = add_column(&board, todo, "Todo", None).unwrap();
        assert_unique_ids(&board);

        let doing = ColumnId::new(uid(11));
        let board = add_column(&board, doing, "Doing", Some(5)).unwrap();
        assert_unique_ids(&board);

        let c1 = CardId::new(uid(20));
        let board = add_card(&board, todo, c1, "Card 1", "", ts(0)).unwrap();
        assert_unique_ids(&board);

        let c2 = CardId::new(uid(21));
        let board = add_card(&board, todo, c2, "Card 2", "", ts(0)).unwrap();
        assert_unique_ids(&board);

        let board = move_card(&board, c1, doing, 0).unwrap();
        assert_unique_ids(&board);

        let board = edit_card(&board, c2, "Card 2 edited", "").unwrap();
        assert_unique_ids(&board);

        let board = rename_column(&board, todo, "Todo renamed").unwrap();
        assert_unique_ids(&board);

        let board = remove_card(&board, c2).unwrap();
        assert_unique_ids(&board);

        let board = remove_column(&board, todo).unwrap();
        assert_unique_ids(&board);
    }
}
