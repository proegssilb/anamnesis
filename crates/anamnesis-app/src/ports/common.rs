//! Infrastructure ports shared by every use case, old and new: the clock and
//! id generator. The core (in either domain model) never reads a clock or
//! generates an id — this is exactly where the shell's clock and randomness
//! enter the system (`docs/DOMAIN.md` §7: "`Clock` already exists").

use anamnesis_core::Timestamp;

/// Supplies "now" as a parameter to use cases that need it.
pub trait Clock: Send + Sync {
    fn now(&self) -> Timestamp;
}

/// Supplies freshly minted ids to use cases that need them.
pub trait IdGen: Send + Sync {
    fn next(&self) -> uuid::Uuid;
}

/// Encrypts/decrypts a project sync config's external-tracker token at
/// rest (issues #40/#41). CPU-only, no I/O — unlike every other port in
/// this module family, this one is synchronous, mirroring [`Clock`]/
/// [`IdGen`]. `encrypt`'s output is opaque to callers (an adapter is free
/// to prepend a nonce, an AEAD tag, or anything else it needs to decrypt
/// its own output) and is stored verbatim as
/// `crate::ProjectSyncConfig::encrypted_token`.
pub trait TokenCipher: Send + Sync {
    fn encrypt(&self, plaintext: &str) -> Result<Vec<u8>, crate::error::AppError>;
    fn decrypt(&self, ciphertext: &[u8]) -> Result<String, crate::error::AppError>;
}
