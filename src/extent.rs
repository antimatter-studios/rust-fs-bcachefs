//! Extent values: a list of typed entries (device pointers and checksum
//! entries), and what they say about where a file's data is.
//!
//! Provenance (docs/clean-room.md): an entry's type is the position of the
//! lowest set bit of its first word; a pointer has a device, a 44-bit
//! sector offset and a generation; crc32/crc64/crc128 entries are 8/16/24
//! bytes and carry compressed size, uncompressed size, offset, nonce,
//! checksum type, compression type and the checksum; a crc entry applies
//! to the pointers after it (S1, 9.1.3). Bit positions were found by
//! hexdump against the reference lister's printed values (S3, S4):
//!
//! ```text
//! ptr    bit 0 = 1; bits 1..3 flags (unread); bits 4..47 sector offset;
//!        bits 48..55 device; bits 56..63 generation   (device and
//!        generation: only ever 0 in the fixtures -- an open question)
//! crc32  bits 0..1 = 0b10; bits 2..8 compressed_size-1; 9..15
//!        uncompressed_size-1; 16..22 offset; 24..27 csum type;
//!        28..31 compression type; 32..63 checksum
//! crc64  bits 0..2 = 0b100; 3..11 compressed_size-1; 12..20
//!        uncompressed_size-1; 21..29 offset; 30..39 nonce; 40..43 csum
//!        type; 44..47 compression type; 48..63 checksum high 16 bits;
//!        second word: checksum low 64 bits
//! ```

use crate::error::{Error, Result};
use crate::util::le64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ptr {
    pub dev: u8,
    pub offset: u64,
    pub gen: u8,
    pub flags: u8,
}

/// The generation a bucket's `alloc_v4` key records: byte 4 of its second
/// word (docs/clean-room.md, "Allocating space": word 1 holds the flags in
/// byte 0, the generation in byte 4, the oldest generation in byte 5).
pub fn alloc_v4_gen(value: &[u8]) -> Option<u8> {
    value.get(12).copied()
}

impl Ptr {
    /// Whether this pointer may be read on device `dev_idx`, whose bucket
    /// `bucket_gen` is the generation the allocator currently records for
    /// the pointer's bucket (`None` when the bucket has no alloc key).
    ///
    /// S1 9.1.3.1: a pointer carries "a generation number that must match
    /// the bucket's current generation to be valid (stale pointers are
    /// detected and dropped during reads)", and flags that "distinguish
    /// cached pointers ... from dirty pointers, and mark unwritten
    /// reservations". A pointer at another device is not on this one; a
    /// stale pointer names a reused bucket; a flagged pointer has a meaning
    /// (cached, unwritten) this reader has never observed and refuses.
    pub fn check(&self, dev_idx: u8, bucket_gen: Option<u8>) -> crate::Result<()> {
        if self.dev != dev_idx {
            return Err(crate::Error::Corrupt(format!(
                "pointer to sector {} is for device {}, and this is device {dev_idx} of a \
                 single-device filesystem",
                self.offset, self.dev
            )));
        }
        if self.flags != 0 {
            return Err(crate::Error::Unsupported(format!(
                "pointer to sector {} carries flags {:#05b} (cached or unwritten): not read",
                self.offset, self.flags
            )));
        }
        match bucket_gen {
            Some(g) if g != self.gen => Err(crate::Error::Corrupt(format!(
                "stale pointer to sector {}: generation {}, the bucket's is {g}",
                self.offset, self.gen
            ))),
            Some(_) => Ok(()),
            None => Err(crate::Error::Corrupt(format!(
                "pointer to sector {} names a bucket with no alloc key",
                self.offset
            ))),
        }
    }
}

/// A checksum/compression entry, unpacked. Sizes are in sectors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Crc {
    pub compressed_size: u32,
    pub uncompressed_size: u32,
    pub offset: u32,
    pub nonce: u32,
    pub csum_type: u8,
    pub compression_type: u8,
    pub csum_hi: u64,
    pub csum_lo: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtentEntry {
    Ptr(Ptr),
    Crc(Crc),
}

/// Compression types as stored in a crc entry. INFERRED from the lz4,
/// gzip and zstd fixtures against the lister's `compress` column: 0 none,
/// 2 gzip, 3 lz4, 4 zstd, 5 incompressible (stored raw) -- not the
/// numbering of the filesystem option. Type 1 never appeared.
pub mod compression {
    pub const NONE: u8 = 0;
    pub const GZIP: u8 = 2;
    pub const LZ4: u8 = 3;
    pub const ZSTD: u8 = 4;
    pub const INCOMPRESSIBLE: u8 = 5;
}

fn bits(w: u64, lo: u32, n: u32) -> u64 {
    (w >> lo) & ((1u64 << n) - 1)
}

/// Parse an extent value into its entries.
pub fn parse_entries(v: &[u8]) -> Result<Vec<ExtentEntry>> {
    let mut out = Vec::new();
    let mut p = 0;
    while p + 8 <= v.len() {
        let w = le64(v, p);
        if w == 0 {
            break; // padding at the end of the value
        }
        match w.trailing_zeros() {
            0 => {
                out.push(ExtentEntry::Ptr(Ptr {
                    flags: bits(w, 1, 3) as u8,
                    offset: bits(w, 4, 44),
                    dev: bits(w, 48, 8) as u8,
                    gen: bits(w, 56, 8) as u8,
                }));
                p += 8;
            }
            1 => {
                out.push(ExtentEntry::Crc(Crc {
                    compressed_size: bits(w, 2, 7) as u32 + 1,
                    uncompressed_size: bits(w, 9, 7) as u32 + 1,
                    offset: bits(w, 16, 7) as u32,
                    nonce: 0,
                    csum_type: bits(w, 24, 4) as u8,
                    compression_type: bits(w, 28, 4) as u8,
                    csum_hi: 0,
                    csum_lo: bits(w, 32, 32),
                }));
                p += 8;
            }
            2 => {
                if p + 16 > v.len() {
                    return Err(Error::Corrupt("crc64 entry runs past the value".into()));
                }
                out.push(ExtentEntry::Crc(Crc {
                    compressed_size: bits(w, 3, 9) as u32 + 1,
                    uncompressed_size: bits(w, 12, 9) as u32 + 1,
                    offset: bits(w, 21, 9) as u32,
                    nonce: bits(w, 30, 10) as u32,
                    csum_type: bits(w, 40, 4) as u8,
                    compression_type: bits(w, 44, 4) as u8,
                    csum_hi: bits(w, 48, 16),
                    csum_lo: le64(v, p + 8),
                }));
                p += 16;
            }
            t => return Err(Error::Unsupported(format!("extent entry type {t}"))),
        }
    }
    Ok(out)
}

/// One piece of a file's data: `len` sectors of the file starting at file
/// sector `file_start`, found `skip` sectors into the (decompressed) data
/// the pointer and crc describe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataExtent {
    pub file_start: u64,
    pub len: u64,
    pub ptr: Ptr,
    pub crc: Option<Crc>,
    pub skip: u64,
}

impl DataExtent {
    /// From an extent key (whose position is its end) and its value: the
    /// first pointer, with the crc entry that precedes it.
    pub fn from_key(k: &crate::bkey::Bkey) -> Result<Self> {
        let mut crc = None;
        for e in parse_entries(&k.value)? {
            match e {
                ExtentEntry::Crc(c) => crc = Some(c),
                ExtentEntry::Ptr(ptr) => {
                    return Ok(DataExtent {
                        file_start: k.start_offset(),
                        len: k.size as u64,
                        ptr,
                        skip: crc.map(|c| c.offset as u64).unwrap_or(0),
                        crc,
                    })
                }
            }
        }
        Err(Error::Corrupt(format!(
            "extent at {} has no pointer",
            k.pos
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_crc32_entry_decodes_as_the_lister_printed_it() {
        // "crc32: c_size 5 size 50 offset 0 csum crc32c 0:d192fb38 compress lz4"
        let w: u64 = 0xd192_fb38_3500_6212;
        let e = parse_entries(&w.to_le_bytes()).unwrap();
        let ExtentEntry::Crc(c) = e[0] else {
            panic!("{e:?}")
        };
        assert_eq!(
            (c.compressed_size, c.uncompressed_size, c.offset),
            (5, 50, 0)
        );
        assert_eq!(
            (c.csum_type, c.compression_type, c.csum_lo),
            (5, compression::LZ4, 0xd192_fb38)
        );
    }

    #[test]
    fn a_crc64_entry_and_pointer_decode() {
        // "crc64: c_size 50 size 512 offset 0 ... csum crc32c 0:8fd3928b compress lz4"
        let mut v = 0x0000_3500_001f_f18cu64.to_le_bytes().to_vec();
        v.extend_from_slice(&0x8fd3_928bu64.to_le_bytes());
        v.extend_from_slice(&0x17401u64.to_le_bytes());
        let e = parse_entries(&v).unwrap();
        let ExtentEntry::Crc(c) = e[0] else {
            panic!("{e:?}")
        };
        assert_eq!(
            (c.compressed_size, c.uncompressed_size, c.csum_lo),
            (50, 512, 0x8fd3_928b)
        );
        assert_eq!(
            e[1],
            ExtentEntry::Ptr(Ptr {
                dev: 0,
                offset: 0x1740,
                gen: 0,
                flags: 0
            })
        );
    }

    #[test]
    fn unknown_or_truncated_entries_are_refused() {
        assert!(parse_entries(&0x8u64.to_le_bytes()).is_err());
        assert!(parse_entries(&0x4u64.to_le_bytes()).is_err());
    }
}
