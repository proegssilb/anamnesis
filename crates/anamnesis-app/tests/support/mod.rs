//! Test doubles shared by the unit tests and the cucumber steps. Not
//! production code — nothing here is compiled into the `anamnesis-app`
//! library.
//!
//! This module is compiled separately into each integration-test binary
//! that includes it, and no single binary exercises every convenience method
//! on every double (e.g. the cucumber steps never need `IdentityProvider`)
//! — hence the blanket allow rather than per-binary dead code.
#![allow(dead_code)]

use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use bytes::Bytes;

use anamnesis_app::{
    AuthenticatedIdentity, ByteStream, Clock, IdGen, IdentityError, IdentityProvider,
    LoginCallback, LoginRedirect, RepoError, TimezoneResolver,
};
use anamnesis_core::{Timestamp, UserId};
use uuid::Uuid;

/// Wraps `bytes` as a single-chunk [`ByteStream`], for tests that just need
/// *some* upload body rather than one that exercises real multi-chunk
/// behaviour (`anamnesis-adapters/tests/blob_store_contract.rs` covers that).
pub fn byte_stream(bytes: Vec<u8>) -> ByteStream<'static> {
    Box::pin(futures_util::stream::once(
        async move { Ok(Bytes::from(bytes)) },
    ))
}

/// A `Clock` that always reports the same instant.
#[derive(Debug, Clone, Copy)]
pub struct FixedClock(Timestamp);

impl FixedClock {
    pub fn at(seconds: i64) -> Self {
        Self(Timestamp::from_unix_seconds(seconds).unwrap())
    }
}

impl Default for FixedClock {
    fn default() -> Self {
        Self::at(0)
    }
}

impl Clock for FixedClock {
    fn now(&self) -> Timestamp {
        self.0
    }
}

/// A `TimezoneResolver` over `time-tz`'s embedded IANA database, so
/// scenarios about local times (DST gaps included) run against real zone
/// rules. Mirrors `anamnesis_adapters::TzTimezoneResolver`, which this crate
/// cannot depend on (adapters depends on it); ambiguous times resolve to
/// the earlier instant and gap times error, per the port's contract.
#[derive(Debug, Clone, Copy, Default)]
pub struct TzResolver;

/// The zone tests that do not care about local time pass to [`TzResolver`].
pub const UTC_ZONE: &str = "UTC";

fn zone_named(name: &str) -> Result<&'static time_tz::Tz, RepoError> {
    time_tz::timezones::get_by_name(name)
        .ok_or_else(|| RepoError::new(format!("unknown timezone: {name}")))
}

fn utc_instant(instant: Timestamp) -> time::OffsetDateTime {
    time::OffsetDateTime::from_unix_timestamp(instant.unix_seconds()).unwrap()
}

impl TimezoneResolver for TzResolver {
    fn local_date(&self, zone: &str, instant: Timestamp) -> Result<time::Date, RepoError> {
        use time_tz::OffsetDateTimeExt;
        Ok(utc_instant(instant).to_timezone(zone_named(zone)?).date())
    }

    fn local_time(&self, zone: &str, instant: Timestamp) -> Result<time::Time, RepoError> {
        use time_tz::OffsetDateTimeExt;
        Ok(utc_instant(instant).to_timezone(zone_named(zone)?).time())
    }

    fn to_utc(
        &self,
        zone: &str,
        date: time::Date,
        time: time::Time,
    ) -> Result<Timestamp, RepoError> {
        use time_tz::{OffsetResult, PrimitiveDateTimeExt};
        let local = time::PrimitiveDateTime::new(date, time);
        let resolved = match local.assume_timezone(zone_named(zone)?) {
            OffsetResult::Some(odt) | OffsetResult::Ambiguous(odt, _) => odt,
            OffsetResult::None => {
                return Err(RepoError::new(format!(
                    "{local} falls in a DST gap in {zone}"
                )));
            }
        };
        Ok(Timestamp::from_unix_seconds(resolved.unix_timestamp()).unwrap())
    }
}

/// An `IdGen` that hands out deterministic, strictly increasing ids.
#[derive(Debug, Default)]
pub struct SequentialIdGen {
    next: AtomicU64,
}

impl SequentialIdGen {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn starting_at(start: u64) -> Self {
        Self {
            next: AtomicU64::new(start),
        }
    }
}

impl IdGen for SequentialIdGen {
    fn next(&self) -> Uuid {
        let n = self.next.fetch_add(1, Ordering::SeqCst);
        Uuid::from_u128(n as u128)
    }
}

/// An `IdentityProvider` double: `begin_login` always returns the same
/// canned redirect; `complete_login` succeeds only when the callback's state
/// and PKCE verifier match what `begin_login` handed out, returning a fixed
/// user id (and its string form as the display name) on success.
#[derive(Debug)]
pub struct StubIdentityProvider {
    user: UserId,
}

impl StubIdentityProvider {
    pub fn always_authenticating_as(user: UserId) -> Self {
        Self { user }
    }

    pub fn canned_redirect() -> LoginRedirect {
        LoginRedirect {
            authorize_url: "https://idp.example/authorize?client_id=test".to_string(),
            csrf_state: "stub-state".to_string(),
            pkce_verifier: "stub-verifier".to_string(),
            nonce: "stub-nonce".to_string(),
        }
    }
}

#[async_trait]
impl IdentityProvider for StubIdentityProvider {
    async fn begin_login(&self) -> Result<LoginRedirect, IdentityError> {
        Ok(Self::canned_redirect())
    }

    async fn complete_login(
        &self,
        callback: LoginCallback,
    ) -> Result<AuthenticatedIdentity, IdentityError> {
        let redirect = Self::canned_redirect();
        if callback.state != callback.expected_state
            || callback.expected_state != redirect.csrf_state
            || callback.pkce_verifier != redirect.pkce_verifier
            || callback.expected_nonce != redirect.nonce
        {
            return Err(IdentityError::new("callback did not match issued state"));
        }
        Ok(AuthenticatedIdentity {
            display_name: self.user.to_string(),
            user_id: self.user.clone(),
            groups: Vec::new(),
        })
    }
}
