//! Encryption: ChaCha20 under the filesystem's master key, authenticated
//! with Poly1305 (S1 2.1.2, 9.1.4).
//!
//! Provenance (docs/clean-room.md, "Encryption"): S1 documents the scheme
//! in prose -- a random master key kept in the superblock's crypt field,
//! sealed by a key derived from the passphrase; every encrypted thing has a
//! 128-bit nonce, with separate domain separators for data and for the
//! Poly1305 key; the MAC is stored where a checksum would be, 128 bits for
//! metadata. The byte layouts are S4: candidate computations over the
//! `encrypted` fixture, whose master key the formatter stored unsealed
//! (`--no_passphrase`), each kept only where it decrypts known plaintext
//! (a node's min and max keys, a bset of well-formed keys) or reproduces
//! the stored MAC:
//!
//! ```text
//! crypt field body:  flags u64, kdf u64, key magic u64 ("bch**key"),
//!                    master key[32]   (magic and key sealed together)
//! nonce:  four little-endian u32 words; word 0 is ChaCha20's block
//!         counter and words 1..3 its 96-bit nonce (RFC 8439's layout)
//! btree:  word 0 the record's byte offset in the node, words 1..2 the
//!         node's seq, word 3 the bset's journal_seq (low half) ^ 0x2000_0000
//! MAC:    Poly1305 keyed with the first 32 keystream bytes of the nonce
//!         with word 3 ^ 0x8000_0000, over the record from byte 16
//! ```

use crate::error::{Error, Result};
use chacha20::cipher::{KeyIvInit, StreamCipher, StreamCipherSeek};
use poly1305::universal_hash::KeyInit;

/// The crypt field's key magic, stored in front of the master key: the
/// bytes "bch**key". OBSERVED in the `encrypted` fixture's crypt field,
/// stored in the clear beside a master key that decrypts its nodes; S1
/// (9.1.4.1) says a magic value stored with the sealed master key verifies
/// the passphrase.
pub const KEY_MAGIC: [u8; 8] = *b"bch**key";

/// Word 3 of a btree node's nonce, before the bset's journal sequence is
/// XORed in. OBSERVED (S4).
pub const NONCE_BTREE: u32 = 0x2000_0000;

/// XORed into word 3 of a nonce for the keystream whose first 32 bytes
/// key Poly1305 (S1's `BCH_NONCE_POLY`). OBSERVED (S4).
pub const NONCE_POLY: u32 = 0x8000_0000;

/// Checksum type 4: ChaCha20 with a full 128-bit Poly1305 MAC, the type of
/// every bset of the `encrypted` fixture (S1 9.1.4.3: metadata always has
/// 128-bit MACs).
pub const CSUM_CHACHA20_POLY1305_128: u8 = 4;

/// Whether a checksum type is a MAC, which needs the master key and a
/// nonce rather than the bytes alone.
pub fn is_mac(csum_type: u8) -> bool {
    matches!(csum_type, 3 | 4)
}

/// The filesystem's master key.
#[derive(Clone)]
pub struct Key([u8; 32]);

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Key(..)")
    }
}

impl Key {
    pub fn new(bytes: [u8; 32]) -> Self {
        Key(bytes)
    }

    /// XOR `data` with the keystream of `nonce`, starting at the block its
    /// word 0 names. A keystream that would run past ChaCha20's 32-bit
    /// block counter is refused: whether the reference wraps it or carries
    /// into word 1 has not been observed.
    pub fn apply(&self, nonce: [u32; 4], data: &mut [u8]) -> Result<()> {
        let blocks = (data.len() as u64).div_ceil(64);
        if u64::from(nonce[0]) + blocks > 1 << 32 {
            return Err(Error::Unsupported(format!(
                "{} bytes at ChaCha20 block {:#x} run past its 32-bit block counter",
                data.len(),
                nonce[0]
            )));
        }
        let mut iv = [0u8; 12];
        for (i, w) in nonce[1..].iter().enumerate() {
            iv[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
        }
        let out_of_range = |_| Error::Unsupported("ChaCha20 keystream out of range".into());
        let mut c = chacha20::ChaCha20::new((&self.0).into(), (&iv).into());
        c.try_seek(u64::from(nonce[0]) * 64).map_err(out_of_range)?;
        c.try_apply_keystream(data).map_err(out_of_range)
    }

    /// The Poly1305 key for `nonce`: the first 32 keystream bytes of the
    /// nonce with its domain bit set.
    fn poly_key(&self, nonce: [u32; 4]) -> Result<[u8; 32]> {
        let mut k = [0u8; 32];
        self.apply(
            [nonce[0], nonce[1], nonce[2], nonce[3] ^ NONCE_POLY],
            &mut k,
        )?;
        Ok(k)
    }

    /// The Poly1305 MAC of `data` under `nonce`.
    pub fn mac(&self, nonce: [u32; 4], data: &[u8]) -> Result<[u8; 16]> {
        let tag = poly1305::Poly1305::new((&self.poly_key(nonce)?).into()).compute_unpadded(data);
        Ok(tag.into())
    }
}

/// The master key from a crypt field's body, when it is stored unsealed:
/// its magic reads in the clear. `Ok(None)` when it is sealed by a
/// passphrase key.
pub fn unsealed_key(body: &[u8]) -> Result<Option<Key>> {
    let Some(sealed) = body.get(16..56) else {
        return Err(Error::Corrupt(format!(
            "a crypt field of {} bytes, too short for its key",
            body.len()
        )));
    };
    if sealed[..8] != KEY_MAGIC {
        return Ok(None);
    }
    let mut k = [0u8; 32];
    k.copy_from_slice(&sealed[8..]);
    Ok(Some(Key(k)))
}

/// Word 3 of a data extent's nonce. OBSERVED (S4); S1's
/// `BCH_NONCE_EXTENT`.
pub const NONCE_EXTENT: u32 = 0x1000_0000;

/// Checksum type 3: ChaCha20 with Poly1305 truncated to 80 bits (S1
/// 9.1.4.3, `chacha20_poly1305_80`), every data extent of the `encrypted`
/// fixture: its crc64 entry holds the tag's first eight bytes in the
/// checksum's low word and the next two in its high 16 bits.
pub const CSUM_CHACHA20_POLY1305_80: u8 = 3;

/// The nonce of a data extent's stored data, from its key's version and
/// its crc entry: `[0, version_lo, 0, NONCE_EXTENT]`, OBSERVED on every
/// extent of the `encrypted` fixture, whose versions are 1 up and whose
/// extents are uncompressed with crc nonce 0. S1 (9.1.4.1) says the nonce
/// is made of the whole 96-bit version, the compression type and the
/// uncompressed size, "combined with a per-CRC nonce offset": where those
/// go is not observed, so an extent that would need them is refused rather
/// than given a guessed nonce.
pub fn extent_nonce(
    version_hi: u32,
    version_lo: u64,
    crc: &crate::extent::Crc,
) -> Result<[u32; 4]> {
    let unobserved = if version_hi != 0 || version_lo >> 32 != 0 {
        format!("version {version_hi:#x}:{version_lo:#x}, beyond 32 bits")
    } else if crc.nonce != 0 {
        format!("crc nonce offset {}", crc.nonce)
    } else if crc.compression_type != crate::extent::compression::NONE {
        format!("compression type {}", crc.compression_type)
    } else {
        return Ok([0, version_lo as u32, 0, NONCE_EXTENT]);
    };
    Err(Error::Unsupported(format!(
        "an encrypted extent with {unobserved}: where that goes in its nonce has not been \
         observed (docs/clean-room.md, \"Encryption\")"
    )))
}

impl Key {
    /// Verify the MAC of, and decrypt in place, the stored data of an
    /// extent whose key has version `version_hi:version_lo`.
    pub fn open_extent(
        &self,
        version_hi: u32,
        version_lo: u64,
        crc: &crate::extent::Crc,
        data: &mut [u8],
    ) -> Result<()> {
        if crc.csum_type != CSUM_CHACHA20_POLY1305_80 {
            return Err(Error::Unsupported(format!(
                "extent MAC type {}: only 80-bit MACs have been seen on data",
                crc.csum_type
            )));
        }
        let nonce = extent_nonce(version_hi, version_lo, crc)?;
        let tag = self.mac(nonce, data)?;
        let lo = u64::from_le_bytes(tag[..8].try_into().expect("8 bytes"));
        let hi = u64::from(u16::from_le_bytes([tag[8], tag[9]]));
        if (lo, hi) != (crc.csum_lo, crc.csum_hi) {
            return Err(Error::BadChecksum {
                what: "extent data MAC",
                stored: crc.csum_lo,
                computed: lo,
            });
        }
        self.apply(nonce, data)
    }
}

/// The nonce of a btree node record at byte `offset` of a node with
/// sequence number `seq`, whose bset carries `journal_seq`.
pub fn btree_nonce(offset: usize, seq: u64, journal_seq: u64) -> [u32; 4] {
    [
        offset as u32,
        seq as u32,
        (seq >> 32) as u32,
        NONCE_BTREE ^ journal_seq as u32,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 8439 2.4.2's test vector: key 00..1f, nonce 000000000000004a00000000,
    /// counter 1.
    #[test]
    fn the_keystream_is_rfc_8439_chacha20() {
        let mut key = [0u8; 32];
        for (i, b) in key.iter_mut().enumerate() {
            *b = i as u8;
        }
        let mut data = *b"Ladies and Gentlemen of the class of '99: If I could offer you o";
        Key(key).apply([1, 0, 0x4a00_0000, 0], &mut data).unwrap();
        assert_eq!(
            data[..16],
            [
                0x6e, 0x2e, 0x35, 0x9a, 0x25, 0x68, 0xf9, 0x80, 0x41, 0xba, 0x07, 0x28, 0xdd, 0x0d,
                0x69, 0x81
            ]
        );
    }

    /// RFC 8439 2.6.2's vector: key 80..9f, nonce 000000000001020304050607,
    /// counter 0. `poly_key` sets word 3's domain bit, so the vector's
    /// word 3 is given with that bit already flipped.
    #[test]
    fn the_poly1305_key_is_rfc_8439_s_one_time_key() {
        let mut key = [0u8; 32];
        for (i, b) in key.iter_mut().enumerate() {
            *b = 0x80 + i as u8;
        }
        let k = Key(key)
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
        let key = Key([0; 32]);
        let mut one_block = [0u8; 64];
        assert!(key.apply([u32::MAX, 0, 0, 0], &mut one_block).is_ok());
        let mut two_blocks = [0u8; 65];
        assert!(matches!(
            key.apply([u32::MAX, 0, 0, 0], &mut two_blocks),
            Err(Error::Unsupported(_))
        ));
    }

    #[test]
    fn an_unsealed_key_is_read_and_a_sealed_one_is_not() {
        let mut body = vec![0u8; 56];
        body[16..24].copy_from_slice(&KEY_MAGIC);
        body[24..].copy_from_slice(&[9; 32]);
        assert_eq!(unsealed_key(&body).unwrap().unwrap().0, [9; 32]);
        body[16] ^= 1;
        assert!(unsealed_key(&body).unwrap().is_none());
        assert!(unsealed_key(&body[..40]).is_err());
    }
}
