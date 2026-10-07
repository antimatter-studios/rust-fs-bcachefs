// Shared by both halves of the fuzzing setup, included textually rather
// than depended on.
//
// `tests/fuzz_decoders.rs` (the gate, stable toolchain, every pull
// request) and `fuzz/src/lib.rs` (the explorer, cargo-fuzz on nightly)
// both `include!` this file. A crate dependency would have been tidier,
// but the fuzz crate depends on `libfuzzer-sys`, which builds libFuzzer's
// C++ runtime, and making the gate depend on the fuzz crate would drag
// that into every pull request build.
//
// What matters is that the two halves drive a decoder identically: the
// same checksum re-stamp, the same walk over a node's keys. If they did
// not, a reproducer from one would not reproduce in the other.
//
// THE CHECKSUM IS RE-STAMPED, AND THAT IS THE POINT. Every structure here
// verifies its checksum before it looks at anything else, so a mutated
// block is refused on the first line and the decoding behind it -- the
// part with the arithmetic in it -- is never reached. A fuzzer left like
// that spends its budget proving that crc32c works. A crafted image has a
// *valid* checksum: whoever wrote it computed one, because they wanted the
// block to be read. Re-stamping is not a cheat that weakens the test; it
// is what makes the test resemble the threat. Each structure is also fed
// as mutated, un-stamped, which is what a torn write looks like.
//
// This used to be a `fuzzing` cargo feature on the library that made
// `csum::verify` accept everything. A feature that turns verification off
// is a line of production code waiting to be enabled by mistake; the
// re-stamp does the same job from outside the crate.

use fs_bcachefs::bkey::{self, Bkey, BkeyFormat, Bpos};
use fs_bcachefs::btree::{Node, NodePtr};
use fs_bcachefs::csum;
use fs_bcachefs::extent::{self, DataExtent};
use fs_bcachefs::inode::{Dirent, Inode, InodeV3Raw};
use fs_bcachefs::journal;
use fs_bcachefs::superblock::{Layout, Superblock, SB_HEADER_BYTES};
use fs_bcachefs::xattr::Xattr;

/// The format of an unpacked key, as a journal entry or a seed holds it.
pub const UNPACKED: BkeyFormat = BkeyFormat {
    key_u64s: 5,
    nr_fields: 6,
    bits: [0; 6],
    field_offset: [0; 6],
};

fn le16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().expect("4 bytes"))
}

fn le64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().expect("8 bytes"))
}

/// Stamp the `csum_type` checksum of `b[from..to]` into the 16-byte field
/// at `at`, the way the reference lays one out: the value little-endian in
/// the low bytes, the rest zero. A type this crate cannot compute leaves
/// the field alone, and the decoder then refuses it, which is right.
fn stamp(b: &mut [u8], at: usize, csum_type: u8, from: usize, to: usize) {
    if let Ok(c) = csum::compute(csum_type, &b[from..to]) {
        b[at..at + 16].fill(0);
        b[at..at + 8].copy_from_slice(&c.to_le_bytes());
    }
}

/// Re-stamp a superblock: the type is flags[0] bits 2..5 and the sum
/// covers byte 16 to the end of the fields (docs/clean-room.md).
pub fn restamp_superblock(b: &mut [u8]) {
    if b.len() < SB_HEADER_BYTES {
        return;
    }
    let csum_type = ((le64(b, 0x90) >> 2) & 0xf) as u8;
    let end = SB_HEADER_BYTES.saturating_add((le32(b, 0x7c) as usize).saturating_mul(8));
    if end > b.len() {
        return;
    }
    stamp(b, 0, csum_type, 16, end);
}

/// Re-stamp every bset record of a btree node: the header's sum covers
/// byte 16 to the end of the first bset's keys, and each later
/// `btree_node_entry`, starting on a block boundary, covers its own bset
/// the same way. The walk is the one `Node::parse` makes.
pub fn restamp_node(b: &mut [u8], block: usize) {
    const HEADER_BSET: usize = 136;
    const BSET_HEADER: usize = 24;
    if b.len() < HEADER_BSET + BSET_HEADER {
        return;
    }
    let block = block.max(512);
    let seq = le64(b, HEADER_BSET);
    let (mut start, mut bset_at) = (0usize, HEADER_BSET);
    while bset_at + BSET_HEADER <= b.len() && le64(b, bset_at) == seq {
        let csum_type = (le32(b, bset_at + 16) & 0xf) as u8;
        let end = bset_at + BSET_HEADER + le16(b, bset_at + 22) as usize * 8;
        if end > b.len() {
            return;
        }
        stamp(b, start, csum_type, start + 16, end);
        start = end.div_ceil(block) * block;
        bset_at = start + 16;
    }
}

/// Re-stamp a journal entry: the type is the low 4 bits of the flags at
/// 36, the sum covers byte 16 to the end of the sub-entries.
pub fn restamp_jset(b: &mut [u8]) {
    const JSET_HEADER: usize = 56;
    if b.len() < JSET_HEADER {
        return;
    }
    let csum_type = (le32(b, 36) & 0xf) as u8;
    let end = JSET_HEADER.saturating_add((le32(b, 40) as usize).saturating_mul(8));
    if end > b.len() {
        return;
    }
    stamp(b, 0, csum_type, 16, end);
}

/// Every value decoder over one key, whatever its type says.
pub fn values(k: &Bkey) {
    let _ = Inode::from_key(k);
    let _ = Dirent::from_key(k);
    let _ = DataExtent::from_key(k);
    let _ = NodePtr::from_key(k);
    let _ = Xattr::from_key(k);
}

/// A key of `key_type` wrapping `value`, at a plausible extent position.
fn key_of(key_type: u8, value: &[u8]) -> Bkey {
    Bkey {
        key_type,
        size: 8,
        version_hi: 0,
        version_lo: 0,
        pos: Bpos {
            inode: 4096,
            offset: 16,
            snapshot: u32::MAX,
        },
        value: value.to_vec(),
    }
}

// ------------------------------------------------------------ the targets

/// The superblock: as handed over, then re-stamped, then everything a
/// mount reads out of it; and the layout embedded in it.
pub fn superblock(data: &[u8]) {
    let _ = Superblock::parse(data);
    let mut stamped = data.to_vec();
    restamp_superblock(&mut stamped);
    if let Ok(sb) = Superblock::parse(&stamped) {
        let _ = sb.members();
        let _ = sb.btree_roots();
        let _ = sb.field_names();
        let _ = sb.feature_names();
        let _ = sb.compat_names();
        let _ = sb.is_encrypted();
        let _ = sb.label_str();
    }
    if data.len() >= SB_HEADER_BYTES {
        let _ = Layout::parse(&data[0xf0..SB_HEADER_BYTES]);
    }
}

/// A btree node, accepting whatever magic it carries so the fuzzer gets
/// past that check, at both block sizes the fixtures use, torn and
/// re-stamped; every key's value through every decoder. Returns how many
/// keys decoded, so a seed that yields none can be noticed.
pub fn btree_node(data: &[u8]) -> usize {
    if data.len() < 24 {
        return 0;
    }
    let magic = le64(data, 16);
    let mut keys = 0;
    for block in [512usize, 4096] {
        let _ = Node::parse(data, magic, block, None);
        let mut stamped = data.to_vec();
        restamp_node(&mut stamped, block);
        if let Ok(node) = Node::parse(&stamped, magic, block, None) {
            for k in &node.keys {
                values(k);
            }
            keys += node.keys.len();
        }
    }
    keys
}

/// A journal entry, torn and re-stamped; every key it holds through every
/// decoder. Returns how many keys decoded.
pub fn jset(data: &[u8]) -> usize {
    if data.len() < 24 {
        return 0;
    }
    let magic = le64(data, 16);
    let _ = journal::parse_jset(data, magic);
    let mut stamped = data.to_vec();
    restamp_jset(&mut stamped);
    let mut keys = 0;
    if let Ok(Some(j)) = journal::parse_jset(&stamped, magic) {
        for e in &j.entries {
            for k in &e.keys {
                values(k);
                keys += 1;
            }
        }
    }
    keys
}

/// One unpacked key from raw bytes, every key type over the same value.
pub fn key_values(data: &[u8]) {
    if let Ok(mut k) = bkey::decode(data, &UNPACKED) {
        for t in [6u8, 10, 11, 17, 18, 23, 29] {
            k.key_type = t;
            values(&k);
        }
    }
}

/// An `inode_v3` value: the full decoder and the encoder behind the write
/// path, which must agree -- decode, encode, decode again gives the same
/// inode, or one of them is wrong about the varints.
pub fn inode_v3(value: &[u8]) {
    if let Ok(raw) = InodeV3Raw::parse(value) {
        let again =
            InodeV3Raw::parse(&raw.encode()).expect("an inode this crate encoded must parse again");
        assert_eq!(
            again, raw,
            "an inode changed in a decode-encode-decode round trip"
        );
    }
    let _ = Inode::from_key(&key_of(29, value));
}

/// An extent value: the entry list, the data extent a read builds from it,
/// and the same bytes read as a btree pointer, which shares the parser.
pub fn extent_entries(value: &[u8]) {
    let _ = extent::parse_entries(value);
    let _ = DataExtent::from_key(&key_of(6, value));
    let _ = NodePtr::parse(value);
}

/// A compressed extent's stored sectors through the hand-written LZ4
/// block decoder at several claimed output lengths, and through the zstd
/// and deflate wrappers.
pub fn lz4_block(data: &[u8]) {
    for out_len in [0usize, 1, 39, 512, 4096, 65536] {
        let _ = fs_bcachefs::compress::lz4_block(data, out_len);
    }
    for t in [
        extent::compression::LZ4,
        extent::compression::ZSTD,
        extent::compression::GZIP,
    ] {
        let _ = fs_bcachefs::compress::decompress(t, data, 4096);
    }
}
