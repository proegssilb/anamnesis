//! [`TableTimezoneResolver`]: the [`TimezoneResolver`] port backed by a
//! small, hand-curated table of IANA zone names — not a full tzdb.
//!
//! `anamnesis-core` deliberately carries no timezone database
//! (`anamnesis_core::recurrence`'s module doc comment); this adapter is the
//! seam Phase C left for "a real tzdata source" to fill in. **This is that
//! seam filled in narrowly, not a general IANA lookup**: it recognises a
//! fixed list of common zone names, encoding two real, tested civil DST
//! rules —
//!
//! - the current US rule (second Sunday of March / first Sunday of
//!   November, 2 AM local, +1h saving) — exactly the rule
//!   `anamnesis_core::recurrence`'s own tests already exercise for
//!   `America/New_York`, reused here rather than re-derived, and
//! - the current EU rule (last Sunday of March / last Sunday of October,
//!   +1h saving, at the local wall-clock time that rule's 01:00 UTC
//!   instant works out to for a given standard offset)
//!
//! — and applying each to the small set of zones in [`resolve`]'s match
//! arms. A name outside that list resolves to `None`, not a best-effort
//! guess. Growing the table is a matter of adding a match arm with the
//! correct standard offset and rule, not a structural change.
//!
//! **Zones that resolve** (see the Phase E report for the authoritative
//! list): `UTC`; fixed-offset `America/Phoenix`, `Pacific/Honolulu`,
//! `Asia/Tokyo`, `Asia/Shanghai`, `Asia/Kolkata`, `Asia/Dubai`; US-rule
//! `America/New_York`, `America/Chicago`, `America/Denver`,
//! `America/Los_Angeles`, `America/Anchorage`; EU-rule `Europe/London`,
//! `Europe/Berlin`, `Europe/Paris`, `Europe/Madrid`, `Europe/Rome`,
//! `Europe/Amsterdam`, `Europe/Athens`, `Europe/Helsinki`,
//! `Europe/Bucharest`. Nothing else — in particular no Southern Hemisphere
//! zones (Australia, South America) and no historical rule changes.

use anamnesis_app::TimezoneResolver;
use anamnesis_core::Timezone;
use time::{Month, UtcOffset, Weekday};

fn offset(hours: i8, minutes: i8) -> UtcOffset {
    UtcOffset::from_hms(hours, minutes, 0).expect("fixed offset table entries are valid")
}

/// The current US DST rule: 2nd Sunday of March / 1st Sunday of November,
/// both at 02:00 local, +1h saving — identical to the rule
/// `anamnesis_core::recurrence`'s own tests use for `America/New_York`.
fn us_dst_rule() -> anamnesis_core::DstRule {
    use anamnesis_core::{DstRule, DstTransition, WeekOfMonth};
    DstRule {
        saving: offset(1, 0),
        starts: DstTransition {
            month: Month::March,
            week: WeekOfMonth::Second,
            weekday: Weekday::Sunday,
            local_time: time::Time::from_hms(2, 0, 0).expect("valid time"),
        },
        ends: DstTransition {
            month: Month::November,
            week: WeekOfMonth::First,
            weekday: Weekday::Sunday,
            local_time: time::Time::from_hms(2, 0, 0).expect("valid time"),
        },
    }
}

/// The current EU DST rule for a zone whose standard offset is
/// `standard_offset`: DST starts and ends at 01:00 UTC on the last Sunday of
/// March/October respectively, which — worked out to that zone's local wall
/// clock — is `standard_offset + 1h` (start, still on standard time) and
/// `standard_offset + 2h` (end, while still on daylight time). All European
/// zones this table covers carry a whole-hour standard offset, so this
/// integer-hour arithmetic is exact for them.
fn eu_dst_rule(standard_offset: UtcOffset) -> anamnesis_core::DstRule {
    use anamnesis_core::{DstRule, DstTransition, WeekOfMonth};
    let base = standard_offset.whole_hours();
    let starts_hour = (i32::from(base) + 1).rem_euclid(24) as u8;
    let ends_hour = (i32::from(base) + 2).rem_euclid(24) as u8;
    DstRule {
        saving: offset(1, 0),
        starts: DstTransition {
            month: Month::March,
            week: WeekOfMonth::Last,
            weekday: Weekday::Sunday,
            local_time: time::Time::from_hms(starts_hour, 0, 0).expect("valid hour"),
        },
        ends: DstTransition {
            month: Month::October,
            week: WeekOfMonth::Last,
            weekday: Weekday::Sunday,
            local_time: time::Time::from_hms(ends_hour, 0, 0).expect("valid hour"),
        },
    }
}

fn eu_zone(standard_offset: UtcOffset) -> Timezone {
    Timezone::with_dst(standard_offset, eu_dst_rule(standard_offset))
}

fn us_zone(standard_offset: UtcOffset) -> Timezone {
    Timezone::with_dst(standard_offset, us_dst_rule())
}

/// A [`TimezoneResolver`] backed by the hand-curated table documented on
/// this module.
#[derive(Debug, Clone, Copy, Default)]
pub struct TableTimezoneResolver;

impl TableTimezoneResolver {
    pub fn new() -> Self {
        Self
    }
}

impl TimezoneResolver for TableTimezoneResolver {
    fn resolve(&self, iana_name: &str) -> Option<Timezone> {
        Some(match iana_name {
            "UTC" | "Etc/UTC" => Timezone::fixed(UtcOffset::UTC),

            // Fixed offset, no DST.
            "America/Phoenix" => Timezone::fixed(offset(-7, 0)),
            "Pacific/Honolulu" => Timezone::fixed(offset(-10, 0)),
            "Asia/Tokyo" => Timezone::fixed(offset(9, 0)),
            "Asia/Shanghai" => Timezone::fixed(offset(8, 0)),
            "Asia/Kolkata" => Timezone::fixed(offset(5, 30)),
            "Asia/Dubai" => Timezone::fixed(offset(4, 0)),

            // US DST rule.
            "America/New_York" => us_zone(offset(-5, 0)),
            "America/Chicago" => us_zone(offset(-6, 0)),
            "America/Denver" => us_zone(offset(-7, 0)),
            "America/Los_Angeles" => us_zone(offset(-8, 0)),
            "America/Anchorage" => us_zone(offset(-9, 0)),

            // EU DST rule.
            "Europe/London" => eu_zone(offset(0, 0)),
            "Europe/Berlin" | "Europe/Paris" | "Europe/Madrid" | "Europe/Rome"
            | "Europe/Amsterdam" => eu_zone(offset(1, 0)),
            "Europe/Athens" | "Europe/Helsinki" | "Europe/Bucharest" => eu_zone(offset(2, 0)),

            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_every_documented_zone() {
        let resolver = TableTimezoneResolver::new();
        for name in [
            "UTC",
            "Etc/UTC",
            "America/Phoenix",
            "Pacific/Honolulu",
            "Asia/Tokyo",
            "Asia/Shanghai",
            "Asia/Kolkata",
            "Asia/Dubai",
            "America/New_York",
            "America/Chicago",
            "America/Denver",
            "America/Los_Angeles",
            "America/Anchorage",
            "Europe/London",
            "Europe/Berlin",
            "Europe/Paris",
            "Europe/Madrid",
            "Europe/Rome",
            "Europe/Amsterdam",
            "Europe/Athens",
            "Europe/Helsinki",
            "Europe/Bucharest",
        ] {
            assert!(
                resolver.resolve(name).is_some(),
                "{name} should resolve but did not"
            );
        }
    }

    #[test]
    fn an_unknown_zone_name_resolves_to_none() {
        let resolver = TableTimezoneResolver::new();
        assert_eq!(resolver.resolve("Mars/Colony_One"), None);
        assert_eq!(resolver.resolve(""), None);
        assert_eq!(resolver.resolve("america/new_york"), None); // case-sensitive
    }

    #[test]
    fn fixed_zones_carry_no_dst_rule() {
        let resolver = TableTimezoneResolver::new();
        let phoenix = resolver.resolve("America/Phoenix").unwrap();
        assert_eq!(phoenix.standard_offset, offset(-7, 0));
        assert!(phoenix.dst.is_none());
    }

    #[test]
    fn new_york_matches_the_documented_us_rule_around_the_spring_transition() {
        // 2026-03-08 is the 2nd Sunday of March 2026 -- the US spring-forward
        // date. Just before 02:00 local the zone is still standard (-5);
        // just at/after 02:00 local it is daylight (-4).
        let resolver = TableTimezoneResolver::new();
        let ny = resolver.resolve("America/New_York").unwrap();

        let before = time::PrimitiveDateTime::new(
            time::Date::from_calendar_date(2026, Month::March, 8).unwrap(),
            time::Time::from_hms(1, 59, 0).unwrap(),
        );
        let after = time::PrimitiveDateTime::new(
            time::Date::from_calendar_date(2026, Month::March, 8).unwrap(),
            time::Time::from_hms(3, 0, 0).unwrap(),
        );
        assert_eq!(ny.to_utc(before).offset(), offset(-5, 0));
        assert_eq!(ny.to_utc(after).offset(), offset(-4, 0));
    }

    #[test]
    fn berlin_matches_the_documented_eu_rule_around_the_autumn_transition() {
        // 2026-10-25 is the last Sunday of October 2026 -- the EU
        // fall-back date. Just before 03:00 local it is still daylight
        // (+2); at/after the repeated 02:00-03:00 hour resolves to standard
        // (+1) under this module's naive comparison (see the module doc
        // comment on `anamnesis_core::recurrence::Timezone`).
        let resolver = TableTimezoneResolver::new();
        let berlin = resolver.resolve("Europe/Berlin").unwrap();

        let before = time::PrimitiveDateTime::new(
            time::Date::from_calendar_date(2026, Month::October, 25).unwrap(),
            time::Time::from_hms(2, 59, 0).unwrap(),
        );
        let after = time::PrimitiveDateTime::new(
            time::Date::from_calendar_date(2026, Month::October, 25).unwrap(),
            time::Time::from_hms(4, 0, 0).unwrap(),
        );
        assert_eq!(berlin.to_utc(before).offset(), offset(2, 0));
        assert_eq!(berlin.to_utc(after).offset(), offset(1, 0));
    }
}
