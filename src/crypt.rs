//! The superblock's crypt field (type 2), which an encrypted filesystem
//! carries (S1 9.1.4, #159).
//!
//! S1 (9.1.4.1) says what the field is for, in prose: it keeps a random
//! 256-bit master key, sealed by a key derived from the passphrase through
//! scrypt with parameters the field also stores, and a magic value stored
//! with the key verifies the passphrase. The byte layout is S4, observed
//! on the two encrypted fixtures (docs/clean-room.md, "Encryption"):
//!
//! ```text
//! bytes  0..8    zero on both
//! bytes  8..16   zero without a passphrase (`encrypted`);
//!                0e 00 03 00 04 00 00 00 with one (`encrypted-passphrase`)
//! bytes 16..24   "bch**key", in the clear without a passphrase
//! bytes 24..56   the master key, in the clear without a passphrase
//! ```
//!
//! With a passphrase bytes 16..56 do not read the magic: the magic and the
//! key are sealed together. Nothing here unseals or uses the key yet; the
//! reader refuses both kinds, naming which.

use crate::error::{Error, Result};

/// The key magic: the bytes "bch**key". OBSERVED at bytes 16..24 of the
/// `encrypted` fixture's crypt field, which was formatted without a
/// passphrase, so the magic S1 says is stored with the master key is
/// stored there in the clear.
pub const KEY_MAGIC: [u8; 8] = *b"bch**key";

/// The length of a crypt field's body: 56 bytes on both encrypted
/// fixtures, with and without a passphrase. OBSERVED.
pub const FIELD_BYTES: usize = 56;

/// An encrypted filesystem's crypt field.
#[derive(Clone, PartialEq, Eq)]
pub struct CryptField {
    /// Bytes 0..16: zero without a passphrase, non-zero in bytes 8..16
    /// with one. Whether those are S1's scrypt parameters, and how they
    /// are encoded, is #273's to establish.
    pub head: [u8; 16],
    /// Bytes 16..56: the key magic and the master key after it, in the
    /// clear or sealed by the passphrase's key.
    pub key: [u8; 40],
}

/// Prints whether the key is sealed, never the key: without a passphrase
/// those bytes are the master key itself.
impl std::fmt::Debug for CryptField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CryptField")
            .field("head", &self.head)
            .field("sealed", &self.is_sealed())
            .finish_non_exhaustive()
    }
}

impl CryptField {
    /// The field from its body. A body shorter than the key is corrupt;
    /// a longer one has not been observed, and is refused rather than
    /// read as though its extra bytes meant nothing.
    pub fn parse(body: &[u8]) -> Result<Self> {
        if body.len() < FIELD_BYTES {
            return Err(Error::Corrupt(format!(
                "a crypt field of {} bytes, too short for its key",
                body.len()
            )));
        }
        if body.len() > FIELD_BYTES {
            return Err(Error::Unsupported(format!(
                "a crypt field of {} bytes: only {FIELD_BYTES} have been observed",
                body.len()
            )));
        }
        let mut head = [0u8; 16];
        head.copy_from_slice(&body[..16]);
        let mut key = [0u8; 40];
        key.copy_from_slice(&body[16..FIELD_BYTES]);
        Ok(CryptField { head, key })
    }

    /// Whether a passphrase seals the master key: its magic does not read
    /// in the clear.
    pub fn is_sealed(&self) -> bool {
        self.key[..8] != KEY_MAGIC
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unsealed() -> Vec<u8> {
        let mut body = vec![0u8; FIELD_BYTES];
        body[16..24].copy_from_slice(&KEY_MAGIC);
        body[24..].copy_from_slice(&[0x5a; 32]);
        body
    }

    #[test]
    fn a_key_whose_magic_reads_in_the_clear_is_unsealed() {
        let c = CryptField::parse(&unsealed()).unwrap();
        assert!(!c.is_sealed());
        assert_eq!(c.head, [0; 16]);
        assert_eq!(c.key[8..], [0x5a; 32]);
    }

    #[test]
    fn a_key_whose_magic_does_not_read_is_sealed() {
        for at in 16..24 {
            let mut body = unsealed();
            body[at] ^= 1;
            assert!(CryptField::parse(&body).unwrap().is_sealed(), "byte {at}");
        }
    }

    #[test]
    fn a_field_of_another_length_is_refused() {
        let body = unsealed();
        assert!(matches!(
            CryptField::parse(&body[..FIELD_BYTES - 8]),
            Err(Error::Corrupt(_))
        ));
        let mut longer = body.clone();
        longer.extend_from_slice(&[0; 8]);
        assert!(matches!(
            CryptField::parse(&longer),
            Err(Error::Unsupported(_))
        ));
    }

    #[test]
    fn debug_output_never_shows_the_key() {
        let shown = format!("{:?}", CryptField::parse(&unsealed()).unwrap());
        assert!(shown.contains("sealed: false"), "{shown}");
        assert!(
            !shown.contains("90"),
            "the key's bytes (0x5a = 90): {shown}"
        );
    }
}
