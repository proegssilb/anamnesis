//! Small, shared rendering helpers: locating a column's `is_done` flag (for
//! bounce accounting on drop) and formatting a [`FieldData`] compactly for
//! the board's `show_on_card` fields and the task detail page.

use anamnesis_app::{BoardColumn, TimezoneResolver};
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
/// `DateTime` needs `timezone`/`iana_name` to turn its stored UTC instant
/// back into the exact local wall-clock string an
/// `<input type="datetime-local">` needs (`"YYYY-MM-DDTHH:MM"`) —
/// [`anamnesis_app::TimezoneResolver::local_date`] plus
/// [`anamnesis_app::TimezoneResolver::local_time`] (the latter added
/// specifically to close this gap; see its own doc comment) are the two
/// halves of that conversion. If the configured zone is somehow unresolvable
/// (should not happen for a zone the rest of the app already validated at
/// startup — see `main.rs`), this falls back to an empty prefill rather than
/// failing the whole page render: the stored value is still shown as text via
/// [`format_field_data`] above the form either way.
pub fn field_input_value(
    data: &FieldData,
    timezone: &dyn TimezoneResolver,
    iana_name: &str,
) -> (String, Option<String>) {
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
        FieldData::DateTime(ts) => {
            let value = match (
                timezone.local_date(iana_name, *ts),
                timezone.local_time(iana_name, *ts),
            ) {
                (Ok(d), Ok(t)) => format!(
                    "{:04}-{:02}-{:02}T{:02}:{:02}",
                    d.year(),
                    d.month() as u8,
                    d.day(),
                    t.hour(),
                    t.minute()
                ),
                _ => String::new(),
            };
            (value, None)
        }
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
    fn datetime_field_prefills_the_local_wall_clock_value() {
        // Regression coverage for the DateTime prefill gap this phase closes
        // (`crate::handlers::format::field_input_value`'s doc comment):
        // 14:30 America/New_York on 2026-07-04 is 18:30 UTC (EDT, UTC-4).
        let resolver = anamnesis_adapters::TzTimezoneResolver::new();
        let ts = anamnesis_core::Timestamp::from_unix_seconds(
            time::macros::datetime!(2026-07-04 18:30:00 UTC).unix_timestamp(),
        )
        .unwrap();
        let (value, currency) =
            field_input_value(&FieldData::DateTime(ts), &resolver, "America/New_York");
        assert_eq!(value, "2026-07-04T14:30");
        assert_eq!(currency, None);
    }

    #[test]
    fn datetime_field_prefill_falls_back_to_empty_for_an_unknown_zone() {
        let resolver = anamnesis_adapters::TzTimezoneResolver::new();
        let ts = anamnesis_core::Timestamp::from_unix_seconds(0).unwrap();
        let (value, _) = field_input_value(&FieldData::DateTime(ts), &resolver, "Not/AZone");
        assert_eq!(value, "");
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
