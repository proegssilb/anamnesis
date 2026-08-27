//! [`Area`]: a parallel domain of life — the masks a person wears
//! (`docs/DOMAIN.md` §3). Tiny, displayed as a grid; no invariants beyond a
//! valid [`Title`].

use serde::{Deserialize, Serialize};

use crate::error::DomainError;
use crate::ids::{AreaId, Timestamp};
use crate::title::Title;

/// A parallel domain of life (e.g. "Home", "Work", "Health").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Area {
    pub id: AreaId,
    pub title: Title,
    pub description: String,
    pub position: u32,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// Creates a new area. `id` and `now` are supplied by the caller — this
/// crate generates no ids and reads no clock.
pub fn create_area(
    id: AreaId,
    title: impl AsRef<str>,
    description: impl Into<String>,
    position: u32,
    now: Timestamp,
) -> Result<Area, DomainError> {
    let title = Title::new(title)?;
    Ok(Area {
        id,
        title,
        description: description.into(),
        position,
        created_at: now,
        updated_at: now,
    })
}

/// Replaces an area's title and description, stamping `updated_at`.
pub fn edit_area(
    area: &Area,
    title: impl AsRef<str>,
    description: impl Into<String>,
    now: Timestamp,
) -> Result<Area, DomainError> {
    let title = Title::new(title)?;
    Ok(Area {
        title,
        description: description.into(),
        updated_at: now,
        ..area.clone()
    })
}

/// Moves an area to a new position in the grid. Infallible: any `u32` is a
/// valid position, ordering among areas is a display concern resolved by the
/// caller.
pub fn reposition_area(area: &Area, position: u32) -> Area {
    Area {
        position,
        ..area.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn id(n: u128) -> AreaId {
        AreaId::new(Uuid::from_u128(n))
    }

    fn ts(secs: i64) -> Timestamp {
        Timestamp::from_unix_seconds(secs).unwrap()
    }

    #[test]
    fn create_area_builds_an_area_with_the_given_fields() {
        let area = create_area(id(1), "Home", "household stuff", 0, ts(100)).unwrap();

        assert_eq!(area.id, id(1));
        assert_eq!(area.title.as_str(), "Home");
        assert_eq!(area.description, "household stuff");
        assert_eq!(area.position, 0);
        assert_eq!(area.created_at, ts(100));
        assert_eq!(area.updated_at, ts(100));
    }

    #[test]
    fn create_area_rejects_an_invalid_title() {
        let result = create_area(id(1), "   ", "", 0, ts(0));
        assert!(matches!(result, Err(DomainError::InvalidTitle(_))));
    }

    #[test]
    fn edit_area_replaces_title_and_description_and_stamps_updated_at() {
        let area = create_area(id(1), "Home", "old", 0, ts(0)).unwrap();

        let edited = edit_area(&area, "Home 2", "new", ts(50)).unwrap();

        assert_eq!(edited.title.as_str(), "Home 2");
        assert_eq!(edited.description, "new");
        assert_eq!(edited.updated_at, ts(50));
        assert_eq!(edited.created_at, ts(0), "created_at must not change");
        assert_eq!(edited.id, area.id);
    }

    #[test]
    fn reposition_area_only_changes_position() {
        let area = create_area(id(1), "Home", "", 0, ts(0)).unwrap();

        let moved = reposition_area(&area, 5);

        assert_eq!(moved.position, 5);
        assert_eq!(moved.title, area.title);
        assert_eq!(moved.updated_at, area.updated_at);
    }
}
