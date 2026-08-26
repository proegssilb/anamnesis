//! [`UuidIdGen`]: the shell's one legitimate source of fresh randomness for
//! ids. The core never generates one; this is where that read happens.

use anamnesis_app::IdGen;

/// An [`IdGen`] that mints random (v4) UUIDs.
#[derive(Debug, Default, Clone, Copy)]
pub struct UuidIdGen;

impl IdGen for UuidIdGen {
    fn next(&self) -> uuid::Uuid {
        uuid::Uuid::new_v4()
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    #[test]
    fn next_mints_a_version_4_uuid() {
        let id = UuidIdGen.next();
        assert_eq!(id.get_version(), Some(uuid::Version::Random));
    }

    #[test]
    fn successive_calls_mint_distinct_ids() {
        let a = UuidIdGen.next();
        let b = UuidIdGen.next();
        assert_ne!(a, b);
    }

    #[test]
    fn never_mints_the_nil_uuid() {
        assert_ne!(UuidIdGen.next(), Uuid::nil());
    }
}
