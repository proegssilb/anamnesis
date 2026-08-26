//! [`SystemClock`]: the shell's one legitimate read of wall-clock time.
//!
//! The core never reads a clock; this is where that read happens, kept to a
//! single trivial adapter so the boundary is easy to audit.

use std::time::{SystemTime, UNIX_EPOCH};

use anamnesis_app::Clock;
use anamnesis_core::Timestamp;

/// A [`Clock`] backed by [`SystemTime::now`].
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is before the Unix epoch")
            .as_secs();
        let seconds = i64::try_from(seconds)
            .expect("system clock far enough past the Unix epoch to overflow i64");
        Timestamp::from_unix_seconds(seconds).expect("current time is representable as a Timestamp")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_reports_a_timestamp_close_to_the_real_wall_clock() {
        let before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let observed = SystemClock.now().unix_seconds();

        let after = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        assert!(
            (before..=after).contains(&observed),
            "expected {observed} to fall within [{before}, {after}]"
        );
    }
}
