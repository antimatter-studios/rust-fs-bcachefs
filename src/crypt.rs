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
//! key are sealed together; unsealing them is #273's.
//!
//! An unsealed key decrypts and authenticates the btree nodes (#271). S1
//! (9.1.4) says metadata is encrypted with ChaCha20 under the master key,
//! with a 128-bit nonce and separate domain separators, and authenticated
//! with a 128-bit Poly1305 MAC stored where a checksum would be. The
//! ciphers are RFC 8439's. Everything else is S4: candidate computations
//! over `encrypted.img`, each kept only where it reproduced the stored MAC
//! of every record of every node, or decrypted a header to the bounds its
//! parent's pointer names (docs/clean-room.md, "Encryption: btree nodes"):
//!
//! ```text
//! nonce:     four little-endian u32 words, ChaCha20's state words 12..15:
//!            word 0 its 32-bit block counter, words 1..3 RFC 8439's nonce
//! node:      word 0 the record's byte offset in the node, words 1..2 the
//!            node's seq, word 3 0x2000_0000 ^ the bset's journal_seq
//! Poly1305:  keyed with the first 32 keystream bytes of the same nonce
//!            with 0x8000_0000 XORed into word 3
//! ```

use crate::error::{Error, Result};
use chacha20::cipher::{KeyIvInit, StreamCipher, StreamCipherSeek};
use poly1305::universal_hash::KeyInit;

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

    /// The master key, when it is stored unsealed; `None` when a
    /// passphrase seals it. The 32 bytes after the magic decrypt every
    /// node of `encrypted.img` (S4, #271).
    pub fn master_key(&self) -> Option<MasterKey> {
        if self.is_sealed() {
            return None;
        }
        let mut k = [0u8; 32];
        k.copy_from_slice(&self.key[8..]);
        Some(MasterKey(k))
    }
}

/// A btree bset's checksum type when its record is encrypted: ChaCha20
/// with a 128-bit Poly1305 MAC. OBSERVED: every bset of `encrypted.img`
/// carries type 4, and S1 (9.1.4.3) says metadata always uses 128-bit MACs.
pub const CSUM_CHACHA20_POLY1305_128: u8 = 4;

/// Word 3 of a btree node record's nonce, before the bset's journal
/// sequence number is XORed in: the domain separator of btree nodes. S1
/// says each kind of encrypted thing has its own; this value is OBSERVED,
/// the only one of the sixteen top-nibble values whose keystream decrypts
/// the nodes of `encrypted.img`.
pub const NONCE_BTREE_NODE: u32 = 0x2000_0000;

/// XORed into word 3 of a nonce for the keystream whose first 32 bytes key
/// Poly1305: S1's `BCH_NONCE_POLY` domain. OBSERVED: with
/// [`NONCE_BTREE_NODE`], the value that reproduces every stored MAC of
/// `encrypted.img`.
pub const NONCE_POLY: u32 = 0x8000_0000;

/// A nonce: four words loaded, little-endian, into ChaCha20's state words
/// 12..15. Word 0 is RFC 8439's 32-bit block counter (2.3), words 1..3 its
/// 96-bit nonce; S1 calls all 128 bits the nonce.
pub type Nonce = [u32; 4];

/// The filesystem's master key. Its `Debug` never prints it.
#[derive(Clone)]
pub struct MasterKey([u8; 32]);

impl std::fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MasterKey(..)")
    }
}

impl MasterKey {
    pub fn new(bytes: [u8; 32]) -> Self {
        MasterKey(bytes)
    }

    /// XOR `data` with the ChaCha20 keystream of `nonce`, from the block
    /// its word 0 names. A keystream that would run past the 32-bit block
    /// counter is refused: whether it wraps or carries into word 1 has not
    /// been observed.
    pub fn apply(&self, nonce: Nonce, data: &mut [u8]) -> Result<()> {
        let blocks = (data.len() as u64).div_ceil(64);
        if u64::from(nonce[0]) + blocks > 1 << 32 {
            return Err(Error::Unsupported(format!(
                "{} bytes from ChaCha20 block {:#x} run past its 32-bit block counter",
                data.len(),
                nonce[0]
            )));
        }
        let mut iv = [0u8; 12];
        for (i, w) in nonce[1..].iter().enumerate() {
            iv[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
        }
        let mut c = chacha20::ChaCha20::new((&self.0).into(), (&iv).into());
        let out_of_range = |_| Error::Unsupported("ChaCha20 keystream out of range".into());
        c.try_seek(u64::from(nonce[0]) * 64).map_err(out_of_range)?;
        c.try_apply_keystream(data).map_err(out_of_range)
    }

    /// The Poly1305 key for `nonce`: the first 32 keystream bytes of
    /// `nonce` with [`NONCE_POLY`] XORed into word 3.
    fn poly_key(&self, nonce: Nonce) -> Result<[u8; 32]> {
        let mut k = [0u8; 32];
        self.apply(
            [nonce[0], nonce[1], nonce[2], nonce[3] ^ NONCE_POLY],
            &mut k,
        )?;
        Ok(k)
    }

    /// The Poly1305 MAC (RFC 8439 2.5) of `data` under `nonce`.
    pub fn mac(&self, nonce: Nonce, data: &[u8]) -> Result<[u8; 16]> {
        Ok(poly1305::Poly1305::new((&self.poly_key(nonce)?).into())
            .compute_unpadded(data)
            .into())
    }
}

/// The nonce of the btree node record at byte `offset` of a node whose
/// sequence number is `seq`, and whose bset carries `journal_seq`.
/// OBSERVED on all 192 records of `encrypted.img`'s 33 nodes, whose
/// journal sequence numbers run from 0 to 0x2b: how one of 2^28 or more
/// would share word 3 with the domain separator has not been seen, so such
/// a record is refused rather than given a guessed nonce.
pub fn btree_node_nonce(offset: usize, seq: u64, journal_seq: u64) -> Result<Nonce> {
    if journal_seq >= 1 << 28 {
        return Err(Error::Unsupported(format!(
            "an encrypted bset with journal sequence number {journal_seq:#x}: how one this \
             large enters its nonce has not been observed"
        )));
    }
    let offset = u32::try_from(offset)
        .map_err(|_| Error::Corrupt(format!("a btree node record at byte {offset}")))?;
    Ok([
        offset,
        seq as u32,
        (seq >> 32) as u32,
        NONCE_BTREE_NODE ^ journal_seq as u32,
    ])
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
        let key = CryptField::parse(&unsealed())
            .unwrap()
            .master_key()
            .unwrap();
        assert_eq!(format!("{key:?}"), "MasterKey(..)");
    }

    #[test]
    fn only_an_unsealed_field_gives_its_master_key() {
        let key = CryptField::parse(&unsealed())
            .unwrap()
            .master_key()
            .unwrap();
        assert_eq!(key.0, [0x5a; 32]);
        let mut body = unsealed();
        body[16] ^= 1;
        assert!(CryptField::parse(&body).unwrap().master_key().is_none());
    }

    /// RFC 8439 2.4.2's test vector: key 00..1f, nonce
    /// 000000000000004a00000000, block counter 1.
    #[test]
    fn the_keystream_is_rfc_8439_chacha20() {
        let mut key = [0u8; 32];
        for (i, b) in key.iter_mut().enumerate() {
            *b = i as u8;
        }
        let mut data = *b"Ladies and Gentlemen of the class of '99: If I could offer you o";
        MasterKey(key)
            .apply([1, 0, 0x4a00_0000, 0], &mut data)
            .unwrap();
        assert_eq!(
            data[..16],
            [
                0x6e, 0x2e, 0x35, 0x9a, 0x25, 0x68, 0xf9, 0x80, 0x41, 0xba, 0x07, 0x28, 0xdd, 0x0d,
                0x69, 0x81
            ]
        );
    }

    /// RFC 8439 2.6.2's vector: key 80..9f, nonce 000000000001020304050607,
    /// block counter 0. `poly_key` XORs [`NONCE_POLY`] into word 3, so the
    /// vector's word 3 is given with that bit already flipped.
    #[test]
    fn the_poly1305_key_is_rfc_8439_s_one_time_key() {
        let mut key = [0u8; 32];
        for (i, b) in key.iter_mut().enumerate() {
            *b = 0x80 + i as u8;
        }
        let k = MasterKey(key)
            .poly_key([0, 0, 0x0302_0100, 0x0706_0504 ^ NONCE_POLY])
            .unwrap();
        assert_eq!(
            k,
            [
                0x8a, 0xd5, 0xa0, 0x8b, 0x90, 0x5f, 0x81, 0xcc, 0x81, 0x50, 0x40, 0x27, 0x4a, 0xb2,
                0x94, 0x71, 0xa8, 0x33, 0xb6, 0x37, 0xe3, 0xfd, 0x0d, 0xa5, 0x08, 0xdb, 0xb8, 0xe2,
                0xfd, 0xd1, 0xa6, 0x46
            ]
        );
    }

    #[test]
    fn a_keystream_past_the_block_counter_is_refused() {
        let key = MasterKey([0; 32]);
        let mut one_block = [0u8; 64];
        assert!(key.apply([u32::MAX, 0, 0, 0], &mut one_block).is_ok());
        let mut two_blocks = [0u8; 65];
        assert!(matches!(
            key.apply([u32::MAX, 0, 0, 0], &mut two_blocks),
            Err(Error::Unsupported(_))
        ));
    }

    #[test]
    fn a_node_nonce_is_its_offset_seq_and_journal_seq() {
        assert_eq!(
            btree_node_nonce(20480, 0x0e36_ac1b_434a_a5cc, 0x23).unwrap(),
            [20480, 0x434a_a5cc, 0x0e36_ac1b, 0x2000_0023]
        );
        assert!(btree_node_nonce(0, 1, (1 << 28) - 1).is_ok());
        assert!(matches!(
            btree_node_nonce(0, 1, 1 << 28),
            Err(Error::Unsupported(_))
        ));
    }
}
