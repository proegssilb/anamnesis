//! Small, shared rendering helpers: locating a column's `is_done` flag (for
//! bounce accounting on drop) and formatting a [`FieldData`] compactly for
//! the board's `show_on_card` fields and the task detail page.

use anamnesis_app::BoardColumn;
use anamnesis_core::{ColumnId, FieldData};

/// Whether `column_id` is an `is_done` column, per the board's current
/// column list — `None` if no such column exists (should not happen for a
/// task actually placed on the board, but a repository read is never a
/// static guarantee).
pub fn column_is_done(columns: &[BoardColumn], column_id: ColumnId) -> Option<bool> {
    columns
        .iter()
        .find(|c| c.column.id == column_id)
        .map(|c| c.column.is_done)
}

/// A short, human-readable rendering of a field value — used for the
/// board's compact `show_on_card` fields and the task detail page. Not
/// locale-aware; good enough for this phase's no-JS forms, not a currency or
/// date-formatting subsystem.
pub fn format_field_data(data: &FieldData) -> String {
    match data {
        FieldData::Number(n) => format_scaled(n.units, n.scale),
        FieldData::Currency(c) => {
            format!(
                "{} {}",
                format_scaled(c.minor_units, 2),
                c.currency.as_str()
            )
        }
        FieldData::Date(d) => format!("{d}"),
        FieldData::Time(t) => format!("{t}"),
        FieldData::DateTime(ts) => ts.unix_seconds().to_string(),
        FieldData::Line(s) | FieldData::Block(s) => s.clone(),
    }
}

fn format_scaled(units: i64, scale: u8) -> String {
    if scale == 0 {
        return units.to_string();
    }
    let factor = 10i64.pow(scale as u32);
    let whole = units / factor;
    let frac = (units % factor).abs();
    format!("{whole}.{frac:0width$}", width = scale as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anamnesis_core::{CurrencyAmount, CurrencyCode, NumberValue};

    #[test]
    fn number_with_scale_renders_a_decimal() {
        let data = FieldData::Number(NumberValue {
            units: 1234,
            scale: 2,
        });
        assert_eq!(format_field_data(&data), "12.34");
    }

    #[test]
    fn number_with_zero_scale_renders_an_integer() {
        let data = FieldData::Number(NumberValue {
            units: 42,
            scale: 0,
        });
        assert_eq!(format_field_data(&data), "42");
    }

    #[test]
    fn currency_renders_amount_and_code() {
        let data = FieldData::Currency(CurrencyAmount {
            minor_units: 500,
            currency: CurrencyCode::new("USD").unwrap(),
        });
        assert_eq!(format_field_data(&data), "5.00 USD");
    }

    #[test]
    fn line_and_block_render_verbatim() {
        assert_eq!(
            format_field_data(&FieldData::Line("hello".to_string())),
            "hello"
        );
        assert_eq!(
            format_field_data(&FieldData::Block("multi\nline".to_string())),
            "multi\nline"
        );
    }
}
