//! Identity and time newtypes.
//!
//! Every value here is either wrapped from an externally supplied `Uuid`/
//! `String`, or constructed from an already-known instant. Nothing in this
//! module reads a clock or generates randomness: `now` and freshly minted
//! ids are parameters supplied by the caller (the imperative shell), never
//! reads performed by the core.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! uuid_id {
    ($name:ident) => {
        #[doc = concat!("A `", stringify!($name), "`, a `Uuid` wrapped for type-level distinction.")]
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            #[doc = concat!("Wraps an externally supplied `Uuid` as a `", stringify!($name), "`.")]
            ///
            /// The id is a parameter, never generated here: this crate does no RNG.
            pub fn new(id: Uuid) -> Self {
                Self(id)
            }

            #[doc = concat!("Builds a `", stringify!($name), "` from a `u128`, as a `const fn`.")]
            ///
            /// Used for well-known, fixed ids (e.g. built-in relationship
            /// kinds) that must exist without any id generator having run.
            pub const fn from_u128(id: u128) -> Self {
                Self(Uuid::from_u128(id))
            }

            /// The underlying `Uuid`.
            pub fn as_uuid(&self) -> Uuid {
                self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                std::fmt::Display::fmt(&self.0, f)
            }
        }

        impl From<Uuid> for $name {
            fn from(id: Uuid) -> Self {
                Self(id)
            }
        }
    };
}

uuid_id!(BoardId);
uuid_id!(ColumnId);
uuid_id!(CardId);
uuid_id!(AreaId);
uuid_id!(ProjectId);
uuid_id!(TaskId);
uuid_id!(RelationshipId);
uuid_id!(KindId);
uuid_id!(FieldId);
uuid_id!(TangleId);

/// The identity of a user, taken verbatim from the `sub` claim of an OIDC id
/// token. Opaque to the core beyond equality.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UserId(String);

impl UserId {
    /// Wraps an externally supplied identity string as a `UserId`.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The underlying identity string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for UserId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A point in time, supplied by the caller.
///
/// The core never reads a clock: every function that needs "now" takes a
/// `Timestamp` as a parameter. Represented as whole seconds since the Unix
/// epoch, which is all the domain needs (creation ordering, not sub-second
/// precision).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(i64);

/// Why a candidate value could not become a [`Timestamp`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TimestampError {
    /// The given seconds value falls outside the range `time` can represent.
    #[error("timestamp out of range")]
    OutOfRange,
}

impl Timestamp {
    /// Builds a `Timestamp` from whole seconds since the Unix epoch.
    pub fn from_unix_seconds(seconds: i64) -> Result<Self, TimestampError> {
        // Round-trip through `time` to reject values it cannot represent,
        // without retaining any I/O-capable clock functionality.
        time::OffsetDateTime::from_unix_timestamp(seconds)
            .map_err(|_| TimestampError::OutOfRange)?;
        Ok(Self(seconds))
    }

    /// The whole seconds since the Unix epoch.
    pub fn unix_seconds(&self) -> i64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn board_column_card_ids_wrap_and_expose_a_uuid() {
        let raw = Uuid::from_u128(1);
        let board = BoardId::new(raw);
        let column = ColumnId::new(raw);
        let card = CardId::new(raw);
        assert_eq!(board.as_uuid(), raw);
        assert_eq!(column.as_uuid(), raw);
        assert_eq!(card.as_uuid(), raw);
    }

    #[test]
    fn ids_of_the_same_kind_compare_by_value() {
        let a = CardId::new(Uuid::from_u128(1));
        let b = CardId::new(Uuid::from_u128(1));
        let c = CardId::new(Uuid::from_u128(2));
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn domain_ids_wrap_and_expose_a_uuid() {
        let raw = Uuid::from_u128(1);
        assert_eq!(AreaId::new(raw).as_uuid(), raw);
        assert_eq!(ProjectId::new(raw).as_uuid(), raw);
        assert_eq!(TaskId::new(raw).as_uuid(), raw);
        assert_eq!(RelationshipId::new(raw).as_uuid(), raw);
        assert_eq!(KindId::new(raw).as_uuid(), raw);
        assert_eq!(FieldId::new(raw).as_uuid(), raw);
    }

    #[test]
    fn from_u128_builds_a_stable_well_known_id() {
        const FIXED: KindId = KindId::from_u128(1);
        assert_eq!(FIXED, KindId::from_u128(1));
        assert_ne!(FIXED, KindId::from_u128(2));
    }

    #[test]
    fn user_id_wraps_a_string() {
        let user = UserId::new("alice");
        assert_eq!(user.as_str(), "alice");
    }

    #[test]
    fn timestamp_wraps_and_exposes_unix_seconds() {
        let ts = Timestamp::from_unix_seconds(1_700_000_000).unwrap();
        assert_eq!(ts.unix_seconds(), 1_700_000_000);
    }

    #[test]
    fn timestamps_of_the_same_instant_are_equal() {
        let a = Timestamp::from_unix_seconds(100).unwrap();
        let b = Timestamp::from_unix_seconds(100).unwrap();
        let c = Timestamp::from_unix_seconds(200).unwrap();
        assert_eq!(a, b);
        assert!(a < c);
    }
}
