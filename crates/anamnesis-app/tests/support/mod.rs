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

use anamnesis_app::{Clock, IdGen, IdentityError, IdentityProvider, LoginCallback, LoginRedirect};
use anamnesis_core::{Timestamp, UserId};
use uuid::Uuid;

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
/// user id on success.
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

    async fn complete_login(&self, callback: LoginCallback) -> Result<UserId, IdentityError> {
        let redirect = Self::canned_redirect();
        if callback.state != callback.expected_state
            || callback.expected_state != redirect.csrf_state
            || callback.pkce_verifier != redirect.pkce_verifier
            || callback.expected_nonce != redirect.nonce
        {
            return Err(IdentityError::new("callback did not match issued state"));
        }
        Ok(self.user.clone())
    }
}
