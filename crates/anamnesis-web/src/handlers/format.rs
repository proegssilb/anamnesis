//! Small, shared rendering helpers: locating a column's `is_done` flag (for
//! bounce accounting on drop) and formatting a [`FieldData`] compactly for
//! the board's `show_on_card` fields and the task detail page.

use anamnesis_app::BoardColumn;
use anamnesis_core::{ColumnId, FieldData, FieldKind};

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

/// The lowercase name this crate's templates and forms use for a
/// [`FieldKind`] — `task.html`'s per-kind edit-form branch, and
/// `crate::handlers::field_form::parse_field_data`'s dispatch, both key off
/// this same spelling.
pub fn format_field_kind(kind: FieldKind) -> &'static str {
    match kind {
        FieldKind::Number => "number",
        FieldKind::Currency => "currency",
        FieldKind::Date => "date",
        FieldKind::Time => "time",
        FieldKind::DateTime => "datetime",
        FieldKind::Line => "line",
        FieldKind::Block => "block",
    }
}

/// A form-input-ready rendering of a field value: `(value, currency_code)`.
/// `value` is what an `<input>`'s/`<textarea>`'s starting content should be
/// to show the currently stored value when editing it; `currency_code` is
/// additionally populated for `Currency` (its own separate input).
///
/// `DateTime` intentionally prefills empty: [`anamnesis_app::TimezoneResolver`]
/// (this crate's only UTC-instant/local-wall-clock conversion seam) exposes
/// `local_date` (a calendar date only) and `to_utc`, but no
/// instant-to-local-time-of-day conversion — so there is no correct way to
/// turn a stored UTC instant back into the exact local wall-clock string an
/// `<input type="datetime-local">` needs. The field can still be *set*
/// (`crate::handlers::field_form::parse_datetime` goes the other direction,
/// which the port fully supports) — it just does not round-trip back into
/// its own edit form's prefilled value; the stored value is still shown as
/// text via [`format_field_data`] above the form.
pub fn field_input_value(data: &FieldData) -> (String, Option<String>) {
    match data {
        FieldData::Number(n) => (format_scaled(n.units, n.scale), None),
        FieldData::Currency(c) => (
            format_scaled(c.minor_units, 2),
            Some(c.currency.as_str().to_string()),
        ),
        FieldData::Date(d) => (
            format!("{:04}-{:02}-{:02}", d.year(), d.month() as u8, d.day()),
            None,
        ),
        FieldData::Time(t) => (format!("{:02}:{:02}", t.hour(), t.minute()), None),
        FieldData::DateTime(_) => (String::new(), None),
        FieldData::Line(s) | FieldData::Block(s) => (s.clone(), None),
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
