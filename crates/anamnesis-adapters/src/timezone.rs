//! [`TzTimezoneResolver`]: the [`TimezoneResolver`] port backed by a real
//! IANA time zone database, via [`time-tz`](https://docs.rs/time-tz) (its
//! default `db` feature, backed by the `tz` crate's own vendored copy of
//! the IANA database).
//!
//! **Why `time-tz`, not a hand-curated table (or `jiff`)**: the defect this
//! replaces was a hand-curated table of ~22 zone names with hand-written
//! DST rules (`docs/DOMAIN.md`-adjacent history — see the git log for the
//! removed `TableTimezoneResolver`). Real DST rules change by government
//! decree, often with only weeks of notice (Brazil abolished DST in 2019,
//! Mexico and Iran dropped most of it in 2022, Jordan and Syria moved
//! permanently, Chile shifts its dates), the whole Southern Hemisphere was
//! missing, and a rule reapplied to a historical timestamp silently used
//! *today's* rule instead of whichever was in force then. None of that is
//! fixable by curating a bigger table — it needs a real tzdb. `time-tz` was
//! chosen over `jiff` (also evaluated) because it fits the `time` crate
//! already used throughout this workspace (`Date`, `Time`,
//! `OffsetDateTime`) with no second date-time library or conversion layer
//! at this seam; `jiff` is an excellent, independently capable library but
//! would mean two competing date-time representations in the dependency
//! graph for no behavioural gain here.
//!
//! **How the tzdb data ships — no system tzdb required**: `time-tz`'s `db`
//! feature (the default, enabled below) pulls in real IANA tzdata *source
//! files* vendored directly inside the `time-tz` crate's own package (its
//! `tz/` directory — `africa`, `asia`, `europe`, `northamerica`, ... —
//! copied from the upstream `tz` database, not downloaded at build time).
//! `time-tz`'s `build.rs` parses those files at compile time and bakes the
//! result into the binary as a static `phf` map (`timezones::get_by_name`).
//! There is **no runtime read of `/usr/share/zoneinfo`** anywhere in this
//! path — a container with no system tzdb installed at all works
//! identically to one that has the latest copy, because neither is ever
//! consulted. The one tradeoff is that "freshness" is now pinned to
//! whichever `time-tz` version is vendored (currently tracking the tzdata
//! `2024a` release — well past every rule change named above), not to
//! whatever the host OS happens to have; bumping `time-tz` is how this gets
//! refreshed.

use anamnesis_app::RepoError;
use anamnesis_app::TimezoneResolver;
use anamnesis_core::Timestamp;
use time::{Date, OffsetDateTime, PrimitiveDateTime, Time};
use time_tz::{OffsetDateTimeExt, OffsetResult, PrimitiveDateTimeExt, timezones};

/// A [`TimezoneResolver`] backed by `time-tz`'s embedded IANA database. See
/// the module doc comment for exactly how the data ships and what it can
/// and cannot promise.
#[derive(Debug, Clone, Copy, Default)]
pub struct TzTimezoneResolver;

impl TzTimezoneResolver {
    pub fn new() -> Self {
        Self
    }
}

fn unknown_zone(iana_name: &str) -> RepoError {
    RepoError::new(format!("unknown or unsupported timezone: {iana_name}"))
}

impl TimezoneResolver for TzTimezoneResolver {
    fn local_date(&self, iana_name: &str, instant: Timestamp) -> Result<Date, RepoError> {
        let tz = timezones::get_by_name(iana_name).ok_or_else(|| unknown_zone(iana_name))?;
        let utc = OffsetDateTime::from_unix_timestamp(instant.unix_seconds())
            .expect("Timestamp was validated at construction");
        Ok(utc.to_timezone(tz).date())
    }

    fn local_time(&self, iana_name: &str, instant: Timestamp) -> Result<Time, RepoError> {
        let tz = timezones::get_by_name(iana_name).ok_or_else(|| unknown_zone(iana_name))?;
        let utc = OffsetDateTime::from_unix_timestamp(instant.unix_seconds())
            .expect("Timestamp was validated at construction");
        Ok(utc.to_timezone(tz).time())
    }

    fn to_utc(&self, iana_name: &str, date: Date, time: Time) -> Result<Timestamp, RepoError> {
        let tz = timezones::get_by_name(iana_name).ok_or_else(|| unknown_zone(iana_name))?;
        let naive = PrimitiveDateTime::new(date, time);
        let resolved = match naive.assume_timezone(tz) {
            // Unambiguous: exactly one instant matches this local moment.
            OffsetResult::Some(odt) => odt,
            // Ambiguous: the repeated wall-clock hour of a fall-back
            // transition — both readings are equally "correct"; this
            // resolver deterministically takes the earlier instant (the
            // pre-transition offset), per this port's documented contract.
            OffsetResult::Ambiguous(earlier, _later) => earlier,
            // The requested local moment does not exist at all (the
            // skipped wall-clock hour of a spring-forward transition).
            OffsetResult::None => {
                return Err(RepoError::new(format!(
                    "local time {naive} does not exist in {iana_name} (falls in a DST gap)"
                )));
            }
        };
        Timestamp::from_unix_seconds(resolved.unix_timestamp())
            .map_err(|e| RepoError::from_source("computed instant out of range", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Month;

    fn ts(year: i32, month: Month, day: u8, hour: u8, minute: u8) -> Timestamp {
        let odt = PrimitiveDateTime::new(
            Date::from_calendar_date(year, month, day).unwrap(),
            Time::from_hms(hour, minute, 0).unwrap(),
        )
        .assume_utc();
        Timestamp::from_unix_seconds(odt.unix_timestamp()).unwrap()
    }

    fn date(year: i32, month: Month, day: u8) -> Date {
        Date::from_calendar_date(year, month, day).unwrap()
    }

    // --- unknown / malformed zone names ---

    #[test]
    fn an_unknown_zone_name_is_rejected_cleanly() {
        let resolver = TzTimezoneResolver::new();
        assert!(
            resolver
                .local_date("Mars/Colony_One", ts(2026, Month::June, 1, 0, 0))
                .is_err()
        );
        assert!(
            resolver
                .to_utc("", date(2026, Month::June, 1), Time::MIDNIGHT)
                .is_err()
        );
        assert!(
            resolver
                .to_utc(
                    "not a real zone name!!",
                    date(2026, Month::June, 1),
                    Time::MIDNIGHT
                )
                .is_err()
        );
    }

    // --- fixed-offset zone, no DST at all ---

    #[test]
    fn tokyo_has_a_fixed_offset_year_round() {
        // Asia/Tokyo: UTC+9, no DST, ever.
        let resolver = TzTimezoneResolver::new();
        let winter = resolver
            .to_utc("Asia/Tokyo", date(2026, Month::January, 15), Time::MIDNIGHT)
            .unwrap();
        let summer = resolver
            .to_utc("Asia/Tokyo", date(2026, Month::July, 15), Time::MIDNIGHT)
            .unwrap();
        // 00:00 JST == 15:00 UTC the previous day, both seasons alike.
        assert_eq!(winter, ts(2026, Month::January, 14, 15, 0));
        assert_eq!(summer, ts(2026, Month::July, 14, 15, 0));
    }

    #[test]
    fn phoenix_has_a_fixed_offset_year_round() {
        // America/Phoenix: UTC-7, no DST, unlike the rest of Arizona's
        // neighbours.
        let resolver = TzTimezoneResolver::new();
        let winter = resolver
            .to_utc(
                "America/Phoenix",
                date(2026, Month::January, 15),
                Time::MIDNIGHT,
            )
            .unwrap();
        let summer = resolver
            .to_utc(
                "America/Phoenix",
                date(2026, Month::July, 15),
                Time::MIDNIGHT,
            )
            .unwrap();
        assert_eq!(winter, ts(2026, Month::January, 15, 7, 0));
        assert_eq!(summer, ts(2026, Month::July, 15, 7, 0));
    }

    // --- half-hour / 45-minute offsets ---

    #[test]
    fn kolkata_is_a_fixed_half_hour_offset() {
        // Asia/Kolkata: UTC+5:30, no DST.
        let resolver = TzTimezoneResolver::new();
        let instant = resolver
            .to_utc("Asia/Kolkata", date(2026, Month::June, 1), Time::MIDNIGHT)
            .unwrap();
        assert_eq!(instant, ts(2026, Month::May, 31, 18, 30));

        let round_trip = resolver.local_date("Asia/Kolkata", instant).unwrap();
        assert_eq!(round_trip, date(2026, Month::June, 1));
    }

    #[test]
    fn kathmandu_is_a_fixed_45_minute_offset() {
        // Asia/Kathmandu: UTC+5:45 -- breaks any whole-hour or half-hour
        // assumption.
        let resolver = TzTimezoneResolver::new();
        let instant = resolver
            .to_utc("Asia/Kathmandu", date(2026, Month::June, 1), Time::MIDNIGHT)
            .unwrap();
        assert_eq!(instant, ts(2026, Month::May, 31, 18, 15));
    }

    // --- Southern Hemisphere: DST starts in the local spring (October) ---

    #[test]
    fn sydney_dst_inverts_to_the_southern_hemisphere_calendar() {
        // Australia/Sydney: standard AEST = UTC+10, daylight AEDT = UTC+11.
        // Unlike every Northern-Hemisphere zone, DST *starts* in October
        // (the local spring) and *ends* in April (the local autumn) -- the
        // exact shape the old hand-rolled rule table could not express.
        let resolver = TzTimezoneResolver::new();

        // 2026-10-04 is Sydney's DST start (2nd Sunday of October); before
        // it, in the local winter, Sydney is on standard time (+10).
        let before_start = resolver
            .to_utc(
                "Australia/Sydney",
                date(2026, Month::September, 1),
                Time::MIDNIGHT,
            )
            .unwrap();
        assert_eq!(before_start, ts(2026, Month::August, 31, 14, 0));

        // Well inside the daylight window (local summer, December) Sydney
        // is on daylight time (+11) -- one hour different from standard,
        // and the transition direction is inverted relative to the US/EU.
        let inside_dst = resolver
            .to_utc(
                "Australia/Sydney",
                date(2026, Month::December, 1),
                Time::MIDNIGHT,
            )
            .unwrap();
        assert_eq!(inside_dst, ts(2026, Month::November, 30, 13, 0));
    }

    // --- US zone: both transition directions ---

    #[test]
    fn new_york_spring_forward_and_fall_back_both_honoured() {
        let resolver = TzTimezoneResolver::new();

        // Just before the 2026 US spring-forward (2nd Sunday of March,
        // 2026-03-08): still standard time, EST = UTC-5.
        let before_spring = resolver
            .to_utc(
                "America/New_York",
                date(2026, Month::March, 1),
                Time::MIDNIGHT,
            )
            .unwrap();
        assert_eq!(before_spring, ts(2026, Month::March, 1, 5, 0));

        // Just after: daylight time, EDT = UTC-4.
        let after_spring = resolver
            .to_utc(
                "America/New_York",
                date(2026, Month::March, 15),
                Time::MIDNIGHT,
            )
            .unwrap();
        assert_eq!(after_spring, ts(2026, Month::March, 15, 4, 0));

        // Just before the 2026 US fall-back (1st Sunday of November,
        // 2026-11-01): still daylight time, EDT = UTC-4.
        let before_fall = resolver
            .to_utc(
                "America/New_York",
                date(2026, Month::October, 25),
                Time::MIDNIGHT,
            )
            .unwrap();
        assert_eq!(before_fall, ts(2026, Month::October, 25, 4, 0));

        // Just after: back to standard time, EST = UTC-5.
        let after_fall = resolver
            .to_utc(
                "America/New_York",
                date(2026, Month::November, 8),
                Time::MIDNIGHT,
            )
            .unwrap();
        assert_eq!(after_fall, ts(2026, Month::November, 8, 5, 0));
    }

    // --- EU zone: both transition directions ---

    #[test]
    fn berlin_spring_forward_and_fall_back_both_honoured() {
        let resolver = TzTimezoneResolver::new();

        // EU rule: last Sunday of March / last Sunday of October. 2026's
        // spring transition is 2026-03-29.
        let before_spring = resolver
            .to_utc("Europe/Berlin", date(2026, Month::March, 1), Time::MIDNIGHT)
            .unwrap();
        assert_eq!(before_spring, ts(2026, Month::February, 28, 23, 0)); // CET, UTC+1

        let after_spring = resolver
            .to_utc(
                "Europe/Berlin",
                date(2026, Month::April, 15),
                Time::MIDNIGHT,
            )
            .unwrap();
        assert_eq!(after_spring, ts(2026, Month::April, 14, 22, 0)); // CEST, UTC+2

        // 2026's autumn transition is 2026-10-25.
        let before_fall = resolver
            .to_utc(
                "Europe/Berlin",
                date(2026, Month::October, 1),
                Time::MIDNIGHT,
            )
            .unwrap();
        assert_eq!(before_fall, ts(2026, Month::September, 30, 22, 0)); // CEST, UTC+2

        let after_fall = resolver
            .to_utc(
                "Europe/Berlin",
                date(2026, Month::November, 8),
                Time::MIDNIGHT,
            )
            .unwrap();
        assert_eq!(after_fall, ts(2026, Month::November, 7, 23, 0)); // CET, UTC+1
    }

    // --- historical rule change: the test the old table could never pass ---

    #[test]
    fn sao_paulo_historical_date_uses_the_rule_in_force_then_not_todays() {
        // Brazil observed DST for decades, then abolished it outright in
        // 2019. A hand-rolled "today's rule applied everywhere" table
        // (the old `TableTimezoneResolver`) has no way to get this right
        // for a date before the abolition -- it would apply 2026's
        // (DST-less) rule retroactively. A real tzdb applies whichever
        // rule was actually in force on the historical date itself.
        let resolver = TzTimezoneResolver::new();

        // 2018-12-01: mid-summer in the Southern Hemisphere, and DST was
        // still in force in Sao Paulo that year -- standard offset -3,
        // daylight saving -2.
        let historical_summer = resolver
            .to_utc(
                "America/Sao_Paulo",
                date(2018, Month::December, 1),
                Time::MIDNIGHT,
            )
            .unwrap();
        assert_eq!(
            historical_summer,
            ts(2018, Month::December, 1, 2, 0),
            "2018 predates the 2019 abolition: DST (-2) must still apply"
        );

        // 2026-12-01: the same calendar date, years after DST was
        // abolished (2019) -- standard offset -3 applies year-round now,
        // one hour different from the 2018 answer above.
        let post_abolition_summer = resolver
            .to_utc(
                "America/Sao_Paulo",
                date(2026, Month::December, 1),
                Time::MIDNIGHT,
            )
            .unwrap();
        assert_eq!(
            post_abolition_summer,
            ts(2026, Month::December, 1, 3, 0),
            "2026 postdates the abolition: no DST, even in local summer"
        );
    }

    // --- round trip: local_date and to_utc agree with each other ---

    #[test]
    fn local_date_round_trips_through_to_utc() {
        let resolver = TzTimezoneResolver::new();
        let original = date(2026, Month::July, 4);
        let instant = resolver
            .to_utc("Australia/Sydney", original, Time::MIDNIGHT)
            .unwrap();
        let recovered = resolver.local_date("Australia/Sydney", instant).unwrap();
        assert_eq!(recovered, original);
    }

    // --- local_time: the instant -> local-wall-clock-time-of-day seam that
    // DateTime field prefill needs (`crate::timezone`'s module doc comment). ---

    #[test]
    fn local_time_round_trips_through_to_utc() {
        let resolver = TzTimezoneResolver::new();
        let original_date = date(2026, Month::July, 4);
        let original_time = Time::from_hms(14, 30, 0).unwrap();
        let instant = resolver
            .to_utc("America/New_York", original_date, original_time)
            .unwrap();
        assert_eq!(
            resolver.local_date("America/New_York", instant).unwrap(),
            original_date
        );
        assert_eq!(
            resolver.local_time("America/New_York", instant).unwrap(),
            original_time
        );
    }

    #[test]
    fn local_time_an_unknown_zone_is_rejected() {
        let resolver = TzTimezoneResolver::new();
        assert!(
            resolver
                .local_time("Mars/Colony_One", ts(2026, Month::June, 1, 0, 0))
                .is_err()
        );
    }

    #[test]
    fn local_time_reflects_the_zones_offset_not_utcs() {
        // 00:00 UTC on 2026-06-01 is 09:00 the same day in Tokyo (UTC+9, no
        // DST) -- a real offset shift, not just a pass-through of the UTC
        // clock time.
        let resolver = TzTimezoneResolver::new();
        let instant = ts(2026, Month::June, 1, 0, 0);
        let local = resolver.local_time("Asia/Tokyo", instant).unwrap();
        assert_eq!(local, Time::from_hms(9, 0, 0).unwrap());
    }
}
