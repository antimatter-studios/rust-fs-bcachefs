//! Keys: positions, the per-node packing format, packed and unpacked keys.
//!
//! Provenance (docs/clean-room.md): the `bpos` and `bkey` members, the six
//! packed fields and the 3-byte packed header are documented (S1, 11.4 and
//! 9.8.5). The byte order of the members on disk and the bit order of
//! packed fields were found by hexdump and checked against the reference
//! lister's printed positions (S3, S4):
//!
//! * a `bpos` is stored snapshot (u32), offset (u64), inode (u64): the
//!   documented member order reversed;
//! * an unpacked key is u64s, format, type, pad, then 12 bytes of version,
//!   size (u32), and the bpos;
//! * a packed key is read as one little-endian integer of `key_u64s * 64`
//!   bits; the fields, in the documented order (inode, offset, snapshot,
//!   size, version_hi, version_lo), are taken from the most significant
//!   bit downward, each `bits` wide, and each adds its field offset.

use crate::error::{Error, Result};
use crate::util::{le32, le64};

/// A key position. Ordered as one integer: inode, then offset, then snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Bpos {
    pub inode: u64,
    pub offset: u64,
    pub snapshot: u32,
}

impl Bpos {
    pub const BYTES: usize = 20;
    pub const MAX: Bpos = Bpos {
        inode: u64::MAX,
        offset: u64::MAX,
        snapshot: u32::MAX,
    };

    /// Read a bpos stored snapshot, offset, inode.
    pub fn parse(b: &[u8]) -> Result<Self> {
        if b.len() < Self::BYTES {
            return Err(Error::Corrupt("bpos shorter than 20 bytes".into()));
        }
        Ok(Bpos {
            snapshot: le32(b, 0),
            offset: le64(b, 4),
            inode: le64(b, 12),
        })
    }
}

impl std::fmt::Display for Bpos {
    /// The reference lister's spelling: `inode:offset:snapshot`, with
    /// `U64_MAX` and `U32_MAX` for all-ones fields, `POS_MIN` for the
    /// all-zero position and `SPOS_MAX` for the all-ones one (S3).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.inode == 0 && self.offset == 0 && self.snapshot == 0 {
            return write!(f, "POS_MIN");
        }
        if self.inode == u64::MAX && self.offset == u64::MAX && self.snapshot == u32::MAX {
            return write!(f, "SPOS_MAX");
        }
        let wide = |v: u64| {
            if v == u64::MAX {
                "U64_MAX".to_string()
            } else {
                v.to_string()
            }
        };
        let snap = if self.snapshot == u32::MAX {
            "U32_MAX".to_string()
        } else {
            self.snapshot.to_string()
        };
        write!(f, "{}:{}:{}", wide(self.inode), wide(self.offset), snap)
    }
}

/// The per-node packing format (`bkey_format`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BkeyFormat {
    pub key_u64s: u8,
    pub nr_fields: u8,
    pub bits: [u8; 6],
    pub field_offset: [u64; 6],
}

impl BkeyFormat {
    pub const BYTES: usize = 8 + 6 * 8;

    pub fn parse(b: &[u8]) -> Result<Self> {
        if b.len() < Self::BYTES {
            return Err(Error::Corrupt("bkey_format shorter than 56 bytes".into()));
        }
        let mut bits = [0u8; 6];
        bits.copy_from_slice(&b[2..8]);
        let mut field_offset = [0u64; 6];
        for (i, o) in field_offset.iter_mut().enumerate() {
            *o = le64(b, 8 + i * 8);
        }
        let f = BkeyFormat {
            key_u64s: b[0],
            nr_fields: b[1],
            bits,
            field_offset,
        };
        let total: u32 = f.bits.iter().map(|&x| x as u32).sum();
        if f.nr_fields as usize > 6 || f.bits.iter().any(|&x| x > 64) {
            return Err(Error::Corrupt(format!("bkey_format {f:?} is out of range")));
        }
        if f.key_u64s == 0 || total + 24 > f.key_u64s as u32 * 64 {
            return Err(Error::Corrupt(format!(
                "bkey_format fields ({total} bits) do not fit {} u64s",
                f.key_u64s
            )));
        }
        Ok(f)
    }
}

/// The value type tags this reader names, by their position in the
/// Principles of Operation's list of key types (S1, 11.5); the numbers of
/// the ones read here (extent 6, dirent 10, btree_ptr_v2 18, inode_v3 29)
/// were confirmed against the reference lister.
pub mod key_type {
    pub const DELETED: u8 = 0;
    pub const WHITEOUT: u8 = 1;
    /// "Marks an extent as containing unrecoverable errors" (S1 11.5):
    /// a range whose data is permanently lost; reads of it are I/O errors.
    pub const ERROR: u8 = 2;
    pub const HASH_WHITEOUT: u8 = 4;
    pub const EXTENT: u8 = 6;
    pub const RESERVATION: u8 = 7;
    /// The v1 inode encoding (legacy, S1 11.5); not decoded here.
    pub const INODE: u8 = 8;
    pub const DIRENT: u8 = 10;
    pub const XATTR: u8 = 11;
    /// A pointer from the extents btree into the reflink btree (S1 9.1.6,
    /// 11.5): the file's data is shared, and lives there. Its value is the
    /// index into the reflink btree, then the front and back pads
    /// (docs/clean-room.md, "Reflinks").
    pub const REFLINK_P: u8 = 15;
    /// The shared data a `reflink_p` points at, in the reflink btree: a
    /// refcount, then the entries of an extent (S1 9.1.6).
    pub const REFLINK_V: u8 = 16;
    pub const INLINE_DATA: u8 = 17;
    pub const BTREE_PTR_V2: u8 = 18;
    /// Per-bucket allocation metadata, current form (S1 11.5; layout in
    /// docs/clean-room.md, "Allocating space").
    pub const ALLOC_V4: u8 = 27;
    /// The v2 inode encoding (0.18 to 0.22, S1 11.6); not decoded here.
    pub const INODE_V2: u8 = 23;
    pub const INODE_V3: u8 = 29;
    /// A whiteout "specific to the extents btree, blocking visibility of
    /// ancestor snapshot extent versions" (S1 11.5; 1.29).
    pub const EXTENT_WHITEOUT: u8 = 36;
}

/// The format byte of an unpacked key ("current" format).
pub const KEY_FORMAT_CURRENT: u8 = 1;
/// Bytes of an unpacked key.
pub const BKEY_BYTES: usize = 40;

/// A key with its value, unpacked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bkey {
    pub key_type: u8,
    pub size: u32,
    pub version_hi: u32,
    pub version_lo: u64,
    pub pos: Bpos,
    pub value: Vec<u8>,
}

impl Bkey {
    /// Start of an extent key: the position is its END (S1, 11.4).
    pub fn start_offset(&self) -> u64 {
        self.pos.offset.saturating_sub(self.size as u64)
    }
}

/// Decode the key at the start of `b` (whose length is its `u64s * 8`).
pub fn decode(b: &[u8], fmt: &BkeyFormat) -> Result<Bkey> {
    if b.len() < 8 {
        return Err(Error::Corrupt("key shorter than one u64".into()));
    }
    let u64s = b[0] as usize;
    if u64s == 0 || u64s * 8 > b.len() {
        return Err(Error::Corrupt(format!(
            "key u64s {u64s} does not fit {}",
            b.len()
        )));
    }
    let key_format = b[1] & 0x7f;
    let key_type = b[2];
    let b = &b[..u64s * 8];
    if key_format == KEY_FORMAT_CURRENT {
        if b.len() < BKEY_BYTES {
            return Err(Error::Corrupt("unpacked key shorter than 40 bytes".into()));
        }
        return Ok(Bkey {
            key_type,
            version_hi: le32(b, 4),
            version_lo: le64(b, 8),
            size: le32(b, 16),
            pos: Bpos::parse(&b[20..40])?,
            value: b[BKEY_BYTES..].to_vec(),
        });
    }
    if key_format != 0 {
        return Err(Error::Unsupported(format!("key format {key_format}")));
    }
    let ku = fmt.key_u64s as usize;
    if ku > u64s {
        return Err(Error::Corrupt(format!(
            "packed key of {u64s} u64s, format needs {ku}"
        )));
    }
    let mut f = [0u64; 6];
    let mut top = ku * 64; // bit just above the next field
    for (i, out) in f.iter_mut().enumerate() {
        let bits = fmt.bits[i] as usize;
        top -= bits;
        let raw = extract_bits(&b[..ku * 8], top, bits);
        *out = raw.wrapping_add(fmt.field_offset[i]);
    }
    Ok(Bkey {
        key_type,
        pos: Bpos {
            inode: f[0],
            offset: f[1],
            snapshot: f[2] as u32,
        },
        size: f[3] as u32,
        version_hi: f[4] as u32,
        version_lo: f[5],
        value: b[ku * 8..].to_vec(),
    })
}

/// `bits` bits starting at bit `lo` of `b` read as a little-endian integer.
fn extract_bits(b: &[u8], lo: usize, bits: usize) -> u64 {
    let mut v: u64 = 0;
    for i in 0..bits {
        let bit = lo + i;
        if (b[bit / 8] >> (bit % 8)) & 1 != 0 {
            v |= 1u64 << i;
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn byte_aligned() -> BkeyFormat {
        BkeyFormat {
            key_u64s: 3,
            nr_fields: 6,
            bits: [64, 64, 32, 0, 0, 0],
            field_offset: [0; 6],
        }
    }

    #[test]
    fn a_packed_dirent_key_decodes_as_the_lister_printed_it() {
        // Bytes of the first key of a dirents leaf, copied from a hexdump;
        // the reference lister printed it as 4096:2710689797173725757:U32_MAX.
        let mut k = vec![0x06, 0x00, 0x0a, 0x00, 0xff, 0xff, 0xff, 0xff];
        k.extend_from_slice(&2710689797173725757u64.to_le_bytes());
        k.extend_from_slice(&4096u64.to_le_bytes());
        k.extend_from_slice(&[0u8; 24]);
        let key = decode(&k, &byte_aligned()).unwrap();
        assert_eq!(key.key_type, key_type::DIRENT);
        assert_eq!(key.pos.to_string(), "4096:2710689797173725757:U32_MAX");
        assert_eq!(key.value.len(), 24);
    }

    #[test]
    fn an_unpacked_extent_key_decodes() {
        let mut k = vec![0x07, 0x01, 0x06, 0x00];
        k.extend_from_slice(&[0u8; 12]);
        k.extend_from_slice(&1u32.to_le_bytes()); // size
        k.extend_from_slice(&u32::MAX.to_le_bytes()); // snapshot
        k.extend_from_slice(&1u64.to_le_bytes()); // offset
        k.extend_from_slice(&0x8000_0003u64.to_le_bytes()); // inode
        k.extend_from_slice(&[0u8; 16]);
        let key = decode(&k, &byte_aligned()).unwrap();
        assert_eq!(key.pos.to_string(), "2147483651:1:U32_MAX");
        assert_eq!(key.size, 1);
        assert_eq!(key.start_offset(), 0);
    }

    #[test]
    fn narrow_fields_take_their_offsets() {
        // inode: 8 bits + 100; offset: 16 bits; snapshot: 0 bits + MAX.
        let fmt = BkeyFormat {
            key_u64s: 1,
            nr_fields: 6,
            bits: [8, 16, 0, 0, 0, 0],
            field_offset: [100, 0, u32::MAX as u64, 0, 0, 0],
        };
        // Top byte (bits 56..63) = 5, bits 40..55 = 0x1234.
        let word: u64 = (5u64 << 56) | (0x1234u64 << 40) | 0x0a_00_01;
        let key = decode(&word.to_le_bytes(), &fmt).unwrap();
        assert_eq!(
            key.pos,
            Bpos {
                inode: 105,
                offset: 0x1234,
                snapshot: u32::MAX
            }
        );
    }

    #[test]
    fn hostile_keys_are_refused_not_panicked_on() {
        let fmt = byte_aligned();
        for k in [
            vec![],
            vec![0u8; 8],
            vec![9, 0, 0, 0, 0, 0, 0, 0],
            vec![1, 0, 0, 0, 0, 0, 0, 0],
            vec![1, 0x7f, 0, 0, 0, 0, 0, 0],
        ] {
            assert!(decode(&k, &fmt).is_err(), "{k:?}");
        }
        let bad = [3u8, 6, 65, 0, 0, 0, 0, 0];
        let mut b = bad.to_vec();
        b.extend_from_slice(&[0u8; 48]);
        assert!(BkeyFormat::parse(&b).is_err());
    }
}
