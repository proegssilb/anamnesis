//! [`Placement`]: the horizon metaphor made concrete (`docs/DOMAIN.md` §2).
//!
//! > "What's below the horizon isn't gone — it's just not up yet."
//!
//! Every task is in exactly one of two placements. `archived_at` is handled
//! orthogonally, as a field on the entity itself (e.g. `Task::archived_at`)
//! rather than a third variant here: a task can be archived from either
//! placement, and archival is a separate axis ("visible in search or not"),
//! not a third position on the board.

use serde::{Deserialize, Serialize};

use crate::ids::ColumnId;

/// Where a task currently sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Placement {
    /// Below the horizon: the backlog. Out of sight, zero load on any WIP
    /// limit. Where most tasks live most of the time.
    Below,
    /// Above the horizon: on the global task board, in `column` at
    /// `position`. The column *is* the task's status.
    OnBoard { column: ColumnId, position: u32 },
}

impl Placement {
    /// True for [`Placement::Below`].
    pub fn is_below(&self) -> bool {
        matches!(self, Placement::Below)
    }

    /// True for [`Placement::OnBoard`].
    pub fn is_on_board(&self) -> bool {
        matches!(self, Placement::OnBoard { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn below_reports_is_below_and_not_on_board() {
        let p = Placement::Below;
        assert!(p.is_below());
        assert!(!p.is_on_board());
    }

    #[test]
    fn on_board_reports_is_on_board_and_not_below() {
        let p = Placement::OnBoard {
            column: ColumnId::new(Uuid::from_u128(1)),
            position: 0,
        };
        assert!(p.is_on_board());
        assert!(!p.is_below());
    }

    #[test]
    fn placements_compare_by_value() {
        let col = ColumnId::new(Uuid::from_u128(1));
        assert_eq!(Placement::Below, Placement::Below);
        assert_eq!(
            Placement::OnBoard {
                column: col,
                position: 3
            },
            Placement::OnBoard {
                column: col,
                position: 3
            }
        );
        assert_ne!(
            Placement::OnBoard {
                column: col,
                position: 3
            },
            Placement::OnBoard {
                column: col,
                position: 4
            }
        );
    }
}
