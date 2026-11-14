//! The typed-phrase layer over [`Schedule`]: `parse` turns what a user types
//! ("every tue at 6pm until 8am", "3 months after done") into a schedule and
//! `describe` renders the canonical phrase back.

use std::fmt;

use time::{Date, Time, Weekday};

use super::{Anchor, MonthDay, Nth, PopIn, Rule, Schedule, WeekdaySet, first_start};

/// Why a phrase was not understood: the offending word (when there is one)
/// and a suggestion of the nearest valid form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub word: Option<String>,
    pub hint: String,
}

impl ParseError {
    fn at(word: &str, hint: &str) -> Self {
        Self {
            word: Some(word.to_owned()),
            hint: hint.to_owned(),
        }
    }

    /// An error with no single offending word (usually: the phrase ended early).
    fn bare(hint: &str) -> Self {
        Self {
            word: None,
            hint: hint.to_owned(),
        }
    }

    /// Blame the next word if there is one, otherwise the end of the phrase.
    fn next_word(cursor: &Cursor, hint: &str) -> Self {
        match cursor.peek() {
            Some(word) => Self::at(word, hint),
            None => Self::bare(hint),
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.word {
            Some(word) => write!(f, "didn't understand '{word}' — {}", self.hint),
            None => f.write_str(&self.hint),
        }
    }
}

impl std::error::Error for ParseError {}

type Parsed<T> = Result<T, ParseError>;

/// Lowercased words of the phrase; commas separate like whitespace does.
struct Cursor {
    words: Vec<String>,
    pos: usize,
}

impl Cursor {
    fn new(input: &str) -> Self {
        let words = input
            .to_lowercase()
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|word| !word.is_empty())
            .map(str::to_owned)
            .collect();
        Self { words, pos: 0 }
    }

    fn peek(&self) -> Option<&str> {
        self.words.get(self.pos).map(String::as_str)
    }

    fn advance(&mut self) -> Option<String> {
        let word = self.words.get(self.pos).cloned();
        self.pos += usize::from(word.is_some());
        word
    }

    /// Consume `word` if it is next.
    fn eat(&mut self, word: &str) -> bool {
        let found = self.peek() == Some(word);
        self.pos += usize::from(found);
        found
    }

    fn peek_is_weekday(&self) -> bool {
        self.peek().and_then(weekdays_named).is_some()
    }

    fn peek_is_number(&self) -> bool {
        self.peek()
            .is_some_and(|word| word.starts_with(|c: char| c.is_ascii_digit()))
    }
}

#[derive(Clone, Copy)]
enum Unit {
    Day,
    Week,
    Month,
}

/// A parsed cadence, before the anchor is decided.
struct Cadence {
    rule: Rule,
    after_done: bool,
}

/// Parse a schedule phrase. `today` supplies the defaults a phrase leaves
/// out: the weekday or day-of-month for "every week" / "every month", the
/// calendar start date, and the `set_on` date of after-completion schedules.
pub fn parse(input: &str, today: Date) -> Result<Schedule, ParseError> {
    let mut cursor = Cursor::new(input);
    let cadence = cadence(&mut cursor, today)?;
    let pop_in = pop_in(&mut cursor)?;
    if let Some(word) = cursor.peek() {
        return Err(trailing_word(word));
    }
    let anchor = if cadence.after_done {
        Anchor::AfterCompletion { set_on: today }
    } else {
        Anchor::Calendar {
            start: first_start(&cadence.rule, today),
        }
    };
    let schedule = Schedule {
        rule: cadence.rule,
        anchor,
        pop_in,
    };
    schedule
        .validate()
        .map_err(|e| ParseError::bare(&e.to_string()))?;
    Ok(schedule)
}

fn trailing_word(word: &str) -> ParseError {
    if word == "until" {
        ParseError::at(
            word,
            "'until' needs an 'at' time first, like 'at 6pm until 8am'",
        )
    } else {
        ParseError::at(
            word,
            "this isn't part of a schedule; try 'at 6pm' for a time",
        )
    }
}

// cadence := "every" ... | N unit "after done"
fn cadence(cursor: &mut Cursor, today: Date) -> Parsed<Cadence> {
    if cursor.eat("every") {
        return every_cadence(cursor, today);
    }
    if cursor.peek_is_number() {
        let (n, unit) = count_and_unit(cursor)?;
        if !after_done(cursor)? {
            return Err(ParseError::next_word(
                cursor,
                "try 'after done' (e.g. '3 months after done') or start with 'every'",
            ));
        }
        return Ok(Cadence {
            rule: build_rule(n, unit, WeekdaySet::new(), default_day(today))?,
            after_done: true,
        });
    }
    Err(ParseError::next_word(
        cursor,
        "try 'every tue at 6pm', 'every 2 weeks on sat' or '3 months after done'",
    ))
}

// "every" weekday-list | "every" [N] unit ( "after done" | [ "on" on-spec ] )
fn every_cadence(cursor: &mut Cursor, today: Date) -> Parsed<Cadence> {
    if cursor.peek_is_weekday() {
        let on = weekday_list(cursor)?;
        return Ok(Cadence {
            rule: Rule::Weeks { n: 1, on },
            after_done: false,
        });
    }
    let (n, unit) = count_and_unit(cursor)?;
    if after_done(cursor)? {
        let rule = build_rule(n, unit, WeekdaySet::new(), default_day(today))?;
        return Ok(Cadence {
            rule,
            after_done: true,
        });
    }
    Ok(Cadence {
        rule: calendar_rule(cursor, n, unit, today)?,
        after_done: false,
    })
}

fn default_day(today: Date) -> MonthDay {
    MonthDay::Day(today.day())
}

// [ "on" on-spec ], where the on-spec depends on the unit.
fn calendar_rule(cursor: &mut Cursor, n: u16, unit: Unit, today: Date) -> Parsed<Rule> {
    let default_weekdays = WeekdaySet::new().with(today.weekday());
    let (weekdays, day) = if !cursor.eat("on") {
        (default_weekdays, default_day(today))
    } else {
        match unit {
            Unit::Day => {
                return Err(ParseError::at(
                    "on",
                    "'on' only applies to weeks or months, like 'every 2 weeks on sat'",
                ));
            }
            Unit::Week => (weekday_list(cursor)?, default_day(today)),
            Unit::Month => (default_weekdays, month_day(cursor)?),
        }
    };
    build_rule(n, unit, weekdays, day)
}

fn build_rule(n: u16, unit: Unit, weekdays: WeekdaySet, day: MonthDay) -> Parsed<Rule> {
    let small = || {
        u8::try_from(n).map_err(|_| {
            ParseError::at(
                &n.to_string(),
                "that interval is too large; try 255 or fewer",
            )
        })
    };
    Ok(match unit {
        Unit::Day => Rule::Days { n },
        Unit::Week => Rule::Weeks {
            n: small()?,
            on: weekdays,
        },
        Unit::Month => Rule::Months {
            n: small()?,
            on: day,
        },
    })
}

// [N] unit
fn count_and_unit(cursor: &mut Cursor) -> Parsed<(u16, Unit)> {
    let n = if cursor.peek_is_number() {
        count(cursor)?
    } else {
        1
    };
    Ok((n, unit(cursor)?))
}

fn count(cursor: &mut Cursor) -> Parsed<u16> {
    let word = cursor.advance().unwrap_or_default();
    match word.parse::<u16>() {
        Ok(0) => Err(ParseError::at(&word, "the interval must be at least 1")),
        Ok(n) => Ok(n),
        Err(_) => Err(ParseError::at(&word, "expected a whole number like 2")),
    }
}

fn unit(cursor: &mut Cursor) -> Parsed<Unit> {
    let Some(word) = cursor.advance() else {
        return Err(ParseError::bare(
            "expected day, week or month after 'every'",
        ));
    };
    match word.as_str() {
        "day" | "days" => Ok(Unit::Day),
        "week" | "weeks" => Ok(Unit::Week),
        "month" | "months" => Ok(Unit::Month),
        "fortnight" | "fortnights" => Err(ParseError::at(&word, "try 'every 2 weeks'")),
        _ => Err(ParseError::at(
            &word,
            "expected a weekday, or day(s), week(s) or month(s), like 'every 2 weeks'",
        )),
    }
}

// "after done"
fn after_done(cursor: &mut Cursor) -> Parsed<bool> {
    if !cursor.eat("after") {
        return Ok(false);
    }
    if cursor.eat("done") {
        Ok(true)
    } else {
        Err(ParseError::next_word(
            cursor,
            "'after' must be followed by 'done'",
        ))
    }
}

const WEEKDAY_NAMES: [(&str, Weekday); 7] = [
    ("monday", Weekday::Monday),
    ("tuesday", Weekday::Tuesday),
    ("wednesday", Weekday::Wednesday),
    ("thursday", Weekday::Thursday),
    ("friday", Weekday::Friday),
    ("saturday", Weekday::Saturday),
    ("sunday", Weekday::Sunday),
];

const WEEKDAYS: [Weekday; 5] = [
    Weekday::Monday,
    Weekday::Tuesday,
    Weekday::Wednesday,
    Weekday::Thursday,
    Weekday::Friday,
];

const WEEKEND: [Weekday; 2] = [Weekday::Saturday, Weekday::Sunday];

/// The weekdays a word names: "weekday", "weekend", or one day by full name
/// or a 3+ letter prefix ("tue", "tues", "thurs"), with an optional plural "s".
fn weekdays_named(word: &str) -> Option<Vec<Weekday>> {
    let stem = word.strip_suffix("s").unwrap_or(word);
    match stem {
        "weekday" => return Some(WEEKDAYS.to_vec()),
        "weekend" => return Some(WEEKEND.to_vec()),
        _ => {}
    }
    WEEKDAY_NAMES
        .iter()
        .find(|(name, _)| {
            [word, stem]
                .iter()
                .any(|w| w.len() >= 3 && name.starts_with(w))
        })
        .map(|(_, day)| vec![*day])
}

// weekday-list
fn weekday_list(cursor: &mut Cursor) -> Parsed<WeekdaySet> {
    let mut set = WeekdaySet::new();
    while let Some(days) = cursor.peek().and_then(weekdays_named) {
        set = days.iter().fold(set, |set, day| set.with(*day));
        cursor.pos += 1;
    }
    if set.is_empty() {
        return Err(ParseError::next_word(
            cursor,
            "expected a weekday like 'sat' (or 'weekday' / 'weekend')",
        ));
    }
    Ok(set)
}

fn single_weekday(cursor: &mut Cursor) -> Parsed<Weekday> {
    let Some(word) = cursor.advance() else {
        return Err(ParseError::bare("expected a weekday like 'fri'"));
    };
    match weekdays_named(&word).as_deref() {
        Some([day]) => Ok(*day),
        _ => Err(ParseError::at(
            &word,
            "expected a single weekday like 'fri'",
        )),
    }
}

// "the" ordinal-day | "the last day" | "the" nth weekday
fn month_day(cursor: &mut Cursor) -> Parsed<MonthDay> {
    if !cursor.eat("the") {
        return Err(ParseError::next_word(
            cursor,
            "try 'on the 15th', 'on the last day' or 'on the last fri'",
        ));
    }
    let Some(word) = cursor.advance() else {
        return Err(ParseError::bare(
            "expected a day after 'the', like 'the 15th'",
        ));
    };
    if word == "last" {
        return if cursor.eat("day") {
            Ok(MonthDay::LastDay)
        } else {
            let weekday = single_weekday(cursor)?;
            Ok(MonthDay::Nth {
                nth: Nth::Last,
                weekday,
            })
        };
    }
    let value = ordinal(&word)?;
    if cursor.peek_is_weekday() {
        let nth = nth_from(value, &word)?;
        let weekday = single_weekday(cursor)?;
        return Ok(MonthDay::Nth { nth, weekday });
    }
    if (1..=31).contains(&value) {
        Ok(MonthDay::Day(value))
    } else {
        Err(ParseError::at(
            &word,
            "the day of the month must be 1st to 31st",
        ))
    }
}

fn nth_from(value: u8, word: &str) -> Parsed<Nth> {
    match value {
        1 => Ok(Nth::First),
        2 => Ok(Nth::Second),
        3 => Ok(Nth::Third),
        4 => Ok(Nth::Fourth),
        _ => Err(ParseError::at(
            word,
            "with a weekday use 1st to 4th or 'last', like 'the 2nd sat'",
        )),
    }
}

/// "15th", "2nd", or the words "first" to "fourth".
fn ordinal(word: &str) -> Parsed<u8> {
    let words = ["first", "second", "third", "fourth"];
    if let Some(index) = words.iter().position(|w| *w == word) {
        return Ok(index as u8 + 1);
    }
    let digits = word.trim_end_matches(char::is_alphabetic);
    let suffix = &word[digits.len()..];
    if !["st", "nd", "rd", "th"].contains(&suffix) {
        return Err(ParseError::at(word, "expected an ordinal like '15th'"));
    }
    digits
        .parse::<u8>()
        .map_err(|_| ParseError::at(word, "the day of the month must be 1st to 31st"))
}

// [ "at" time [ "until" time ] ]
fn pop_in(cursor: &mut Cursor) -> Parsed<Option<PopIn>> {
    if !cursor.eat("at") {
        return Ok(None);
    }
    let at = time_of_day(cursor)?;
    let until = if cursor.eat("until") {
        Some(time_of_day(cursor)?)
    } else {
        None
    };
    Ok(Some(PopIn { at, until }))
}

fn time_of_day(cursor: &mut Cursor) -> Parsed<Time> {
    let Some(word) = cursor.advance() else {
        return Err(ParseError::bare("expected a time like 6pm or 18:00"));
    };
    parse_time(&word).ok_or_else(|| {
        ParseError::at(
            &word,
            "try a time like 6pm, 6:30pm, 18:00, noon or midnight",
        )
    })
}

/// `6pm`, `6:30pm`, `18:00`, `noon`, `midnight`. A bare hour ("6") is
/// ambiguous and rejected.
fn parse_time(word: &str) -> Option<Time> {
    match word {
        "noon" => return Time::from_hms(12, 0, 0).ok(),
        "midnight" => return Time::from_hms(0, 0, 0).ok(),
        _ => {}
    }
    let (body, pm) = match (word.strip_suffix("am"), word.strip_suffix("pm")) {
        (Some(body), _) => (body, Some(false)),
        (_, Some(body)) => (body, Some(true)),
        _ => (word, None),
    };
    let (hour, minute, has_colon) = clock_parts(body)?;
    let hour = match pm {
        Some(pm) if (1..=12).contains(&hour) => hour % 12 + if pm { 12 } else { 0 },
        Some(_) => return None,
        None if has_colon => hour,
        None => return None,
    };
    Time::from_hms(hour, minute, 0).ok()
}

/// `h` or `h:mm` as (hour, minute, had a colon).
fn clock_parts(body: &str) -> Option<(u8, u8, bool)> {
    match body.split_once(':') {
        Some((hour, minute)) if minute.len() == 2 => {
            Some((hour.parse().ok()?, minute.parse().ok()?, true))
        }
        Some(_) => None,
        None => Some((body.parse().ok()?, 0, false)),
    }
}

/// The canonical phrase for a schedule; `parse` accepts it back.
pub fn describe(schedule: &Schedule) -> String {
    let mut phrase = match schedule.anchor {
        Anchor::AfterCompletion { .. } => format!("{} after done", every(&schedule.rule)),
        Anchor::Calendar { .. } => calendar_phrase(&schedule.rule),
    };
    if let Some(pop_in) = schedule.pop_in {
        phrase.push_str(" at ");
        phrase.push_str(&format_time(pop_in.at));
        if let Some(until) = pop_in.until {
            phrase.push_str(" until ");
            phrase.push_str(&format_time(until));
        }
    }
    phrase
}

/// "every day", "every 10 days", "every month", "every 3 months".
fn every(rule: &Rule) -> String {
    let (n, unit) = match *rule {
        Rule::Days { n } => (u32::from(n), "day"),
        Rule::Weeks { n, .. } => (u32::from(n), "week"),
        Rule::Months { n, .. } => (u32::from(n), "month"),
    };
    if n == 1 {
        format!("every {unit}")
    } else {
        format!("every {n} {unit}s")
    }
}

fn calendar_phrase(rule: &Rule) -> String {
    match *rule {
        Rule::Days { .. } => every(rule),
        Rule::Weeks { n: 1, on } => format!("every {}", weekday_phrase(on)),
        Rule::Weeks { on, .. } => format!("{} on {}", every(rule), weekday_phrase(on)),
        Rule::Months { on, .. } => format!("{} on {}", every(rule), month_day_phrase(on)),
    }
}

fn weekday_phrase(set: WeekdaySet) -> String {
    set.iter()
        .map(|day| day.to_string()[..3].to_lowercase())
        .collect::<Vec<_>>()
        .join(", ")
}

fn month_day_phrase(on: MonthDay) -> String {
    match on {
        MonthDay::Day(day) => format!("the {}", ordinal_text(day)),
        MonthDay::LastDay => "the last day".to_owned(),
        MonthDay::Nth { nth, weekday } => {
            let nth = match nth {
                Nth::First => "1st",
                Nth::Second => "2nd",
                Nth::Third => "3rd",
                Nth::Fourth => "4th",
                Nth::Last => "last",
            };
            format!(
                "the {nth} {}",
                weekday_phrase(WeekdaySet::new().with(weekday))
            )
        }
    }
}

fn ordinal_text(day: u8) -> String {
    let suffix = match (day % 10, day % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{day}{suffix}")
}

fn format_time(time: Time) -> String {
    let (hour, minute) = (time.hour(), time.minute());
    match (hour, minute) {
        (0, 0) => return "midnight".to_owned(),
        (12, 0) => return "noon".to_owned(),
        _ => {}
    }
    let suffix = if hour < 12 { "am" } else { "pm" };
    let hour12 = if hour % 12 == 0 { 12 } else { hour % 12 };
    if minute == 0 {
        format!("{hour12}{suffix}")
    } else {
        format!("{hour12}:{minute:02}{suffix}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use time::macros::{date, time};

    const TODAY: Date = date!(2026 - 11 - 13); // a Friday

    #[rstest]
    #[case("every tue at 6pm until 8am")]
    #[case("every 3 months after done")]
    #[case("every week after done")]
    #[case("every mon, thu")]
    #[case("every 2 weeks on sat")]
    #[case("every month on the last fri")]
    #[case("every month on the 31st")]
    #[case("every 10 days")]
    #[case("every month on the 2nd sat at 9:30am")]
    fn canonical_phrases_round_trip(#[case] p: &str) {
        assert_eq!(describe(&parse(p, TODAY).unwrap()), p);
    }

    #[rstest]
    #[case("Every Tuesday at 18:00 until 08:00", "every tue at 6pm until 8am")]
    #[case("3 months after done", "every 3 months after done")]
    #[case("every weekday", "every mon, tue, wed, thu, fri")]
    #[case("every weekend at noon", "every sat, sun at noon")]
    #[case("  every   2 weeks on Saturdays ", "every 2 weeks on sat")]
    #[case("every month on the last day", "every month on the last day")]
    #[case("every week", "every fri")]
    #[case("every 2 weeks", "every 2 weeks on fri")]
    #[case("every month", "every month on the 13th")]
    #[case("every day at midnight", "every day at midnight")]
    #[case(
        "every day at 12:15am until 12:45pm",
        "every day at 12:15am until 12:45pm"
    )]
    #[case("every month on the first fri", "every month on the 1st fri")]
    #[case("every month on the 22nd", "every month on the 22nd")]
    fn variants_normalise(#[case] input: &str, #[case] canonical: &str) {
        assert_eq!(describe(&parse(input, TODAY).unwrap()), canonical);
    }

    #[test]
    fn trash_parses_to_deadline_schedule() {
        let s = parse("every tue at 6pm until 8am", TODAY).unwrap();
        assert!(s.is_deadline());
        assert_eq!(
            s.pop_in,
            Some(PopIn {
                at: time!(18:00),
                until: Some(time!(8:00))
            })
        );
        assert_eq!(
            s.anchor,
            Anchor::Calendar {
                start: date!(2026 - 11 - 17)
            }
        );
    }

    #[test]
    fn after_done_anchors_on_today() {
        let s = parse("3 months after done", TODAY).unwrap();
        assert_eq!(s.anchor, Anchor::AfterCompletion { set_on: TODAY });
        assert!(!s.is_deadline());
    }

    #[rstest]
    #[case("", None)]
    #[case("every fortnight", Some("fortnight"))]
    #[case("every 0 days", Some("0"))]
    #[case("every month on the 32nd", Some("32nd"))]
    #[case("every tue at 25:00", Some("25:00"))]
    #[case("every 2 weeks on", None)]
    #[case("every month on the 5th fri", Some("5th"))]
    #[case("every tue until 8am", Some("until"))] // until requires at
    #[case("every 300 weeks", Some("300"))]
    #[case("every 10 days on sat", Some("on"))]
    #[case("every tue at 6", Some("6"))]
    #[case("every 2 weeks after", None)]
    #[case("3 months", None)]
    #[case("whenever", Some("whenever"))]
    fn bad_input_is_an_error_not_a_panic(#[case] input: &str, #[case] word: Option<&str>) {
        let err = parse(input, TODAY).unwrap_err();
        assert_eq!(err.word.as_deref(), word);
        assert!(!err.hint.is_empty());
    }

    #[test]
    fn fortnight_hint_suggests_every_2_weeks() {
        assert!(
            parse("every fortnight", TODAY)
                .unwrap_err()
                .hint
                .contains("every 2 weeks")
        );
    }

    #[test]
    fn error_display_names_the_word() {
        let err = parse("every fortnight", TODAY).unwrap_err();
        assert_eq!(
            err.to_string(),
            "didn't understand 'fortnight' — try 'every 2 weeks'"
        );
        assert_eq!(
            parse("", TODAY).unwrap_err().to_string(),
            parse("", TODAY).unwrap_err().hint
        );
    }
}
