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
