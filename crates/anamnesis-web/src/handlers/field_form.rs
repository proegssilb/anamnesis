//! Parses raw form input into a [`FieldData`] for one [`FieldKind`]
//! (`docs/DOMAIN.md` §3's custom field vocabulary) — the missing piece that
//! made every custom field unusable before this phase: a project's field
//! *definitions* and any existing *values* rendered, but nothing could ever
//! set one.
//!
//! **Never a float.** Every numeric kind here is parsed digit-by-digit
//! straight from the decimal string a person typed into an integer
//! (`NumberValue`'s scaled units, `CurrencyAmount`'s minor units) — no
//! `f64`/`str::parse::<f64>` anywhere in this path, so there is no rounding
//! step at which precision could be lost.

use time::macros::format_description;

use anamnesis_app::TimezoneResolver;
use anamnesis_core::{CurrencyAmount, CurrencyCode, FieldData, FieldKind, NumberValue};

use crate::error::WebError;

fn bad(message: impl Into<String>) -> WebError {
    WebError::BadRequest(message.into())
}

/// Splits a plain decimal string (`"12.34"`, `"-5"`, `"0.5"`, `"3"`) into
/// the integer `(units, scale)` pair `NumberValue`/`CurrencyAmount` store —
/// `units * 10^-scale`. The integer and fractional digits are concatenated
/// and parsed as one integer; no float is ever constructed.
fn parse_scaled_decimal(raw: &str, max_scale: u8) -> Result<(i64, u8), WebError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(bad("a number is required"));
    }
    let (negative, raw) = match raw.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, raw),
    };
    let (int_part, frac_part) = match raw.split_once('.') {
        Some((a, b)) => (a, b),
        None => (raw, ""),
    };
    if int_part.is_empty() && frac_part.is_empty() {
        return Err(bad("not a valid number"));
    }
    if !int_part.chars().all(|c| c.is_ascii_digit())
        || !frac_part.chars().all(|c| c.is_ascii_digit())
    {
        return Err(bad("not a valid number"));
    }
    let scale = frac_part.len();
    if scale > max_scale as usize {
        return Err(bad(format!(
            "at most {max_scale} decimal place(s) are allowed"
        )));
    }
    let int_part = if int_part.is_empty() { "0" } else { int_part };
    let digits = format!("{int_part}{frac_part}");
    let magnitude: i64 = digits
        .parse()
        .map_err(|_| bad("that number is out of range"))?;
    Ok((if negative { -magnitude } else { magnitude }, scale as u8))
}

/// Parses a `Number` field's raw form value: any number of decimal places
/// (up to 9), scale taken from however many the user actually typed —
/// `docs/DOMAIN.md` §3's "scaled integer", never normalised.
pub fn parse_number(raw: &str) -> Result<FieldData, WebError> {
    let (units, scale) = parse_scaled_decimal(raw, 9)?;
    Ok(FieldData::Number(NumberValue { units, scale }))
}

/// Parses a `Currency` field's raw amount plus its ISO 4217 code. Always
/// normalises to exactly 2 decimal places of minor units — matching
/// `crate::handlers::format::format_field_data`'s own fixed-2-decimal
/// display — so `"5"` and `"5.0"` both become 500 minor units; a typed
/// value with *more* than 2 decimal places is rejected rather than silently
/// rounded.
pub fn parse_currency(raw_amount: &str, raw_code: &str) -> Result<FieldData, WebError> {
    let (mut minor_units, scale) = parse_scaled_decimal(raw_amount, 2)?;
    if scale < 2 {
        minor_units *= 10i64.pow((2 - scale) as u32);
    }
    let currency = CurrencyCode::new(raw_code.trim().to_uppercase())
        .map_err(|_| bad("currency must be a 3-letter ISO 4217 code, e.g. USD"))?;
    Ok(FieldData::Currency(CurrencyAmount {
        minor_units,
        currency,
    }))
}

/// Parses a `Date` field's raw `<input type="date">` value (`"YYYY-MM-DD"`).
pub fn parse_date(raw: &str) -> Result<time::Date, WebError> {
    let format = format_description!("[year]-[month]-[day]");
    time::Date::parse(raw.trim(), &format).map_err(|_| bad("not a valid date"))
}

/// Parses a `Time` field's raw `<input type="time">` value (`"HH:MM"` or
/// `"HH:MM:SS"` — browsers send the shorter form unless seconds are shown).
pub fn parse_time(raw: &str) -> Result<time::Time, WebError> {
    let raw = raw.trim();
    if raw.matches(':').count() >= 2 {
        time::Time::parse(raw, &format_description!("[hour]:[minute]:[second]"))
    } else {
        time::Time::parse(raw, &format_description!("[hour]:[minute]"))
    }
    .map_err(|_| bad("not a valid time"))
}

/// Parses a `DateTime` field's raw `<input type="datetime-local">` value
/// (`"YYYY-MM-DDTHH:MM"`), resolving that local wall-clock moment to a UTC
/// [`anamnesis_core::Timestamp`] in `iana_name` via `timezone` — the same
/// seam `crate::handlers::board::fetch_suggestion` already uses (in the
/// opposite direction) to turn `now` into a local calendar date.
pub fn parse_datetime(
    raw: &str,
    timezone: &dyn TimezoneResolver,
    iana_name: &str,
) -> Result<FieldData, WebError> {
    let raw = raw.trim();
    let (date_part, time_part) = raw
        .split_once('T')
        .ok_or_else(|| bad("not a valid date and time"))?;
    let date = parse_date(date_part)?;
    let time = parse_time(time_part)?;
    let ts = timezone.to_utc(iana_name, date, time)?;
    Ok(FieldData::DateTime(ts))
}

/// Parses a `Line`/`Block` field's raw value verbatim (`docs/DOMAIN.md` §3:
/// both are unbounded `String`s in core; `Line` vs `Block` is purely an
/// editing-surface hint).
pub fn parse_text(kind: FieldKind, raw: &str) -> FieldData {
    match kind {
        FieldKind::Block => FieldData::Block(raw.to_string()),
        _ => FieldData::Line(raw.to_string()),
    }
}

/// Parses a field *definition*'s `kind` `<select>` value (`"number"`,
/// `"currency"`, `"date"`, `"time"`, `"datetime"`, `"line"`, `"block"` — the
/// exact spelling `crate::handlers::format::format_field_kind` produces, so a
/// round trip through the add-field form matches what the fields list
/// already renders) into a [`FieldKind`] — the reverse of
/// `format_field_kind`, needed once a project admin can actually create a
/// field definition through the UI rather than by hand-writing SQL.
pub fn parse_field_kind(raw: &str) -> Result<FieldKind, WebError> {
    match raw {
        "number" => Ok(FieldKind::Number),
        "currency" => Ok(FieldKind::Currency),
        "date" => Ok(FieldKind::Date),
        "time" => Ok(FieldKind::Time),
        "datetime" => Ok(FieldKind::DateTime),
        "line" => Ok(FieldKind::Line),
        "block" => Ok(FieldKind::Block),
        other => Err(bad(format!("{other:?} is not a known field kind"))),
    }
}

/// Parses `value` (and, for `Currency`, `currency`) into a [`FieldData`]
/// matching `kind` — the one dispatch point every field-kind parser above
/// funnels through.
pub fn parse_field_data(
    kind: FieldKind,
    value: &str,
    currency: &str,
    timezone: &dyn TimezoneResolver,
    iana_name: &str,
) -> Result<FieldData, WebError> {
    match kind {
        FieldKind::Number => parse_number(value),
        FieldKind::Currency => parse_currency(value, currency),
        FieldKind::Date => Ok(FieldData::Date(parse_date(value)?)),
        FieldKind::Time => Ok(FieldData::Time(parse_time(value)?)),
        FieldKind::DateTime => parse_datetime(value, timezone, iana_name),
        FieldKind::Line | FieldKind::Block => Ok(parse_text(kind, value)),
    }
}
