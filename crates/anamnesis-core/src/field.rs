//! [`FieldDefinition`] / [`FieldValue`]: a project's custom task vocabulary
//! (`docs/DOMAIN.md` §3).
//!
//! **`Currency` stores integer minor units plus an ISO 4217 code — never a
//! float. `Number` stores a scaled integer (value + scale) — for the same
//! reason: floats silently lose precision on exactly the operations (sums,
//! comparisons) this data exists to support.**

use serde::{Deserialize, Serialize};

use crate::error::DomainError;
use crate::ids::{FieldId, ProjectId, TaskId};
use crate::title::Title;

/// What kind of value a [`FieldDefinition`] holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FieldKind {
    Number,
    Currency,
    Date,
    Time,
    DateTime,
    Line,
    Block,
}

/// A project's custom field: a named, typed, ordered slot for task data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldDefinition {
    pub id: FieldId,
    pub project_id: ProjectId,
    pub name: Title,
    pub kind: FieldKind,
    pub position: u32,
    /// Drives compact card rendering — some fields deliberately do not
    /// appear on the board.
    pub show_on_card: bool,
}

/// A scaled integer: `units * 10^-scale`. Never a float, so summing or
/// comparing amounts never accumulates rounding error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NumberValue {
    pub units: i64,
    pub scale: u8,
}

/// An ISO 4217 currency code: exactly three uppercase ASCII letters (e.g.
/// `"USD"`, `"JPY"`). Format-validated only — this crate does not maintain
/// or check against the real ISO 4217 list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct CurrencyCode([u8; 3]);

impl CurrencyCode {
    /// Validates `raw` as three uppercase ASCII letters.
    pub fn new(raw: impl AsRef<str>) -> Result<Self, DomainError> {
        let raw = raw.as_ref();
        let bytes = raw.as_bytes();
        if bytes.len() != 3 || !bytes.iter().all(|b| b.is_ascii_uppercase()) {
            return Err(DomainError::InvalidCurrencyCode);
        }
        Ok(Self([bytes[0], bytes[1], bytes[2]]))
    }

    /// The three-letter code as a `&str`.
    pub fn as_str(&self) -> &str {
        // Safe: constructed only from three ASCII bytes.
        std::str::from_utf8(&self.0).expect("CurrencyCode is always valid ASCII")
    }
}

impl std::fmt::Display for CurrencyCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TryFrom<String> for CurrencyCode {
    type Error = DomainError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        CurrencyCode::new(value)
    }
}

impl From<CurrencyCode> for String {
    fn from(code: CurrencyCode) -> Self {
        code.as_str().to_string()
    }
}

/// An amount of money: integer minor units (e.g. cents) plus its currency.
/// Never a float, for the same reason as [`NumberValue`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrencyAmount {
    pub minor_units: i64,
    pub currency: CurrencyCode,
}

/// A stored value for one task's [`FieldDefinition`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldValue {
    pub field_id: FieldId,
    pub task_id: TaskId,
    pub data: FieldData,
}

/// The actual typed payload of a [`FieldValue`] — one variant per
/// [`FieldKind`], checked to match by [`set_field_value`]. `Line` vs `Block`
/// is purely an editing-surface hint (single line vs multi-line text); both
/// are plain, unbounded `String`s here — a length limit, if wanted, belongs
/// in a Phase D use case near the UI, not baked into this core type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FieldData {
    Number(NumberValue),
    Currency(CurrencyAmount),
    Date(time::Date),
    Time(time::Time),
    DateTime(crate::ids::Timestamp),
    Line(String),
    Block(String),
}

impl FieldData {
    /// The [`FieldKind`] this payload corresponds to.
    pub fn kind(&self) -> FieldKind {
        match self {
            FieldData::Number(_) => FieldKind::Number,
            FieldData::Currency(_) => FieldKind::Currency,
            FieldData::Date(_) => FieldKind::Date,
            FieldData::Time(_) => FieldKind::Time,
            FieldData::DateTime(_) => FieldKind::DateTime,
            FieldData::Line(_) => FieldKind::Line,
            FieldData::Block(_) => FieldKind::Block,
        }
    }
}

/// Creates a new field definition.
pub fn create_field_definition(
    id: FieldId,
    project_id: ProjectId,
    name: impl AsRef<str>,
    kind: FieldKind,
    position: u32,
    show_on_card: bool,
) -> Result<FieldDefinition, DomainError> {
    Ok(FieldDefinition {
        id,
        project_id,
        name: Title::new(name)?,
        kind,
        position,
        show_on_card,
    })
}

/// Renames a field definition.
pub fn rename_field_definition(
    definition: &FieldDefinition,
    name: impl AsRef<str>,
) -> Result<FieldDefinition, DomainError> {
    Ok(FieldDefinition {
        name: Title::new(name)?,
        ..definition.clone()
    })
}

/// Moves a field definition to a new position among its project's fields.
pub fn set_field_position(definition: &FieldDefinition, position: u32) -> FieldDefinition {
    FieldDefinition {
        position,
        ..definition.clone()
    }
}

/// Toggles whether a field appears on the compact card view.
pub fn set_show_on_card(definition: &FieldDefinition, show_on_card: bool) -> FieldDefinition {
    FieldDefinition {
        show_on_card,
        ..definition.clone()
    }
}

/// Sets a task's value for a field, checking that `data`'s kind matches the
/// `definition` it is being stored against.
pub fn set_field_value(
    definition: &FieldDefinition,
    task_id: TaskId,
    data: FieldData,
) -> Result<FieldValue, DomainError> {
    if data.kind() != definition.kind {
        return Err(DomainError::FieldKindMismatch(definition.kind));
    }
    Ok(FieldValue {
        field_id: definition.id,
        task_id,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn fid(n: u128) -> FieldId {
        FieldId::new(Uuid::from_u128(n))
    }

    fn pid(n: u128) -> ProjectId {
        ProjectId::new(Uuid::from_u128(n))
    }

    fn tid(n: u128) -> TaskId {
        TaskId::new(Uuid::from_u128(n))
    }

    fn number_def() -> FieldDefinition {
        create_field_definition(fid(1), pid(1), "Weight", FieldKind::Number, 0, true).unwrap()
    }

    fn currency_def() -> FieldDefinition {
        create_field_definition(fid(2), pid(1), "Cost", FieldKind::Currency, 1, false).unwrap()
    }

    #[test]
    fn create_field_definition_rejects_an_invalid_name() {
        let result = create_field_definition(fid(1), pid(1), "", FieldKind::Line, 0, true);
        assert!(matches!(result, Err(DomainError::InvalidTitle(_))));
    }

    #[test]
    fn rename_field_definition_replaces_the_name() {
        let d = number_def();
        let renamed = rename_field_definition(&d, "New name").unwrap();
        assert_eq!(renamed.name.as_str(), "New name");
        assert_eq!(renamed.kind, d.kind);
    }

    #[test]
    fn set_field_position_and_show_on_card_update_independently() {
        let d = number_def();
        let repositioned = set_field_position(&d, 9);
        assert_eq!(repositioned.position, 9);
        let toggled = set_show_on_card(&d, false);
        assert!(!toggled.show_on_card);
    }

    // --- currency: integer minor units + ISO 4217 code, never a float. ---

    #[test]
    fn currency_code_accepts_three_uppercase_letters() {
        let code = CurrencyCode::new("USD").unwrap();
        assert_eq!(code.as_str(), "USD");
    }

    #[rstest::rstest]
    #[case("us")] // too short
    #[case("USDD")] // too long
    #[case("usd")] // lowercase
    #[case("U$D")] // non-letter
    #[case("")]
    fn currency_code_rejects_malformed_input(#[case] raw: &str) {
        assert_eq!(
            CurrencyCode::new(raw),
            Err(DomainError::InvalidCurrencyCode)
        );
    }

    #[test]
    fn currency_amount_round_trips_minor_units_exactly() {
        // $19.99 as 1999 minor units. If this were a float, repeated
        // addition would eventually drift; an integer never does.
        let mut total = CurrencyAmount {
            minor_units: 0,
            currency: CurrencyCode::new("USD").unwrap(),
        };
        for _ in 0..1000 {
            total.minor_units += 1999;
        }
        assert_eq!(total.minor_units, 1_999_000);
    }

    #[test]
    fn set_field_value_accepts_currency_for_a_currency_definition() {
        let d = currency_def();
        let value = set_field_value(
            &d,
            tid(1),
            FieldData::Currency(CurrencyAmount {
                minor_units: 1999,
                currency: CurrencyCode::new("USD").unwrap(),
            }),
        )
        .unwrap();
        assert_eq!(value.field_id, d.id);
        assert_eq!(value.task_id, tid(1));
    }

    #[test]
    fn set_field_value_rejects_a_mismatched_kind() {
        let d = currency_def();
        let result = set_field_value(
            &d,
            tid(1),
            FieldData::Number(NumberValue { units: 5, scale: 0 }),
        );
        assert_eq!(
            result,
            Err(DomainError::FieldKindMismatch(FieldKind::Currency))
        );
    }

    // --- number: scaled integer, never a float. ---

    #[test]
    fn scaled_number_represents_a_decimal_exactly() {
        // 123.45 as units=12345, scale=2. No float involved anywhere.
        let n = NumberValue {
            units: 12345,
            scale: 2,
        };
        assert_eq!(n.units, 12345);
        assert_eq!(n.scale, 2);
    }

    #[test]
    fn scaled_number_equality_requires_matching_scale() {
        // 12.30 (units=1230, scale=2) and 12.3 (units=123, scale=1) are the
        // same decimal value but are NOT `==` as raw (units, scale) pairs —
        // this type stores exactly what was entered, it does not normalise.
        // That is a deliberate simplicity tradeoff: normalising scale is a
        // use-case/display concern, not a core invariant.
        let a = NumberValue {
            units: 1230,
            scale: 2,
        };
        let b = NumberValue {
            units: 123,
            scale: 1,
        };
        assert_ne!(a, b);
    }

    #[test]
    fn set_field_value_accepts_number_for_a_number_definition() {
        let d = number_def();
        let value = set_field_value(
            &d,
            tid(1),
            FieldData::Number(NumberValue {
                units: 42,
                scale: 0,
            }),
        )
        .unwrap();
        assert_eq!(value.field_id, d.id);
    }

    #[test]
    fn field_data_kind_matches_every_variant() {
        assert_eq!(
            FieldData::Number(NumberValue { units: 0, scale: 0 }).kind(),
            FieldKind::Number
        );
        assert_eq!(
            FieldData::Currency(CurrencyAmount {
                minor_units: 0,
                currency: CurrencyCode::new("USD").unwrap()
            })
            .kind(),
            FieldKind::Currency
        );
        assert_eq!(
            FieldData::Date(time::Date::from_calendar_date(2026, time::Month::January, 1).unwrap())
                .kind(),
            FieldKind::Date
        );
        assert_eq!(
            FieldData::Time(time::Time::from_hms(0, 0, 0).unwrap()).kind(),
            FieldKind::Time
        );
        assert_eq!(
            FieldData::DateTime(crate::ids::Timestamp::from_unix_seconds(0).unwrap()).kind(),
            FieldKind::DateTime
        );
        assert_eq!(FieldData::Line("x".into()).kind(), FieldKind::Line);
        assert_eq!(FieldData::Block("x".into()).kind(), FieldKind::Block);
    }
}
