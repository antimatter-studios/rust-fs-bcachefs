//! Btree nodes: the header, the bsets inside them, and walking a btree
//! from its root to every leaf key.
//!
//! Provenance (docs/clean-room.md): the node structure (a header with
//! checksum, magic, sequence number, flags carrying the btree id and level,
//! a min/max key interval, a key format and the first bset; then
//! `btree_node_entry` records each wrapping one more bset) is documented
//! (S1, 9.8.5). The byte layout below was found by hexdump and checked
//! against the reference lister's `nodes-ondisk` mode (S3, S4):
//!
//! ```text
//! header:  csum[16] magic u64 @16 flags u64 @24 min_key @32 max_key @52
//!          8 unread bytes @72, bkey_format @80 (56 bytes), bset @136
//! bset:    seq u64, journal_seq u64, flags u32, version u16, u64s u16,
//!          then u64s*8 bytes of keys
//! entry:   csum[16], bset @16; each entry starts on a block boundary
//! ```
//!
//! A bset's checksum covers from just after the checksum to the end of its
//! keys, with the checksum type in the low 4 bits of the bset's flags.

use crate::bkey::{self, key_type, Bkey, BkeyFormat, Bpos};
use crate::error::{Error, Result};
use crate::extent::{self, ExtentEntry};
use crate::superblock::Superblock;
use crate::util::{le16, le32, le64};
use fs_core::BlockRead;

/// A btree node's magic is this constant XOR the first eight bytes of the
/// filesystem's internal UUID read as a little-endian u64. INFERRED: the
/// magic differs per filesystem, and this XOR is the same constant for
/// every fixture.
pub const BSET_MAGIC_BASE: u64 = 0x9013_5c78_b99e_07f5;

/// The node magic for a filesystem with this internal UUID.
pub fn node_magic(uuid: &[u8; 16]) -> u64 {
    BSET_MAGIC_BASE ^ le64(uuid, 0)
}
const HEADER_BSET: usize = 136;
const BSET_HEADER: usize = 24;
/// Deeper than any real btree; a cycle or a corrupt level stops here.
const MAX_DEPTH: u32 = 16;

/// Btree ids, in the order the Principles of Operation lists them (S1,
/// 11.3); extents 0, inodes 1 and dirents 2 confirmed by the reference
/// lister reading the roots this reader found.
pub mod btree_id {
    pub const EXTENTS: u8 = 0;
    pub const INODES: u8 = 1;
    pub const DIRENTS: u8 = 2;
    pub const XATTRS: u8 = 3;
}

/// A pointer to a btree node: a `btree_ptr_v2` value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodePtr {
    pub seq: u64,
    pub sectors_written: u16,
    pub min_key: Bpos,
    pub ptrs: Vec<extent::Ptr>,
}

impl NodePtr {
    /// Decode a `btree_ptr_v2` value: mem_ptr u64 (unread), seq u64,
    /// sectors_written u16, flags u16, min_key, then extent entries.
    pub fn parse(v: &[u8]) -> Result<Self> {
        if v.len() < 40 {
            return Err(Error::Corrupt(
                "btree_ptr_v2 value shorter than 40 bytes".into(),
            ));
        }
        let ptrs = extent::parse_entries(&v[40..])?
            .into_iter()
            .filter_map(|e| match e {
                ExtentEntry::Ptr(p) => Some(p),
                _ => None,
            })
            .collect::<Vec<_>>();
        if ptrs.is_empty() {
            return Err(Error::Corrupt(
                "btree pointer with no device pointer".into(),
            ));
        }
        Ok(NodePtr {
            seq: le64(v, 8),
            sectors_written: le16(v, 16),
            min_key: Bpos::parse(&v[20..40])?,
            ptrs,
        })
    }

    pub fn from_key(k: &Bkey) -> Result<Self> {
        if k.key_type != key_type::BTREE_PTR_V2 {
            return Err(Error::Unsupported(format!(
                "btree pointer key type {}",
                k.key_type
            )));
        }
        Self::parse(&k.value)
    }
}

/// One parsed node: its header facts and its keys, merged across bsets.
#[derive(Debug, Clone)]
pub struct Node {
    pub seq: u64,
    pub flags: u64,
    pub min_key: Bpos,
    pub max_key: Bpos,
    pub format: BkeyFormat,
    pub keys: Vec<Bkey>,
}

impl Node {
    /// Parse the written part of a node. `block_bytes` is the filesystem
    /// block size: every `btree_node_entry` starts on a block boundary.
    /// `expect_seq` is the sequence number the parent's pointer recorded.
    pub fn parse(
        b: &[u8],
        magic: u64,
        block_bytes: usize,
        expect_seq: Option<u64>,
    ) -> Result<Self> {
        if b.len() < HEADER_BSET + BSET_HEADER {
            return Err(Error::Corrupt("btree node shorter than its header".into()));
        }
        if le64(b, 16) != magic {
            return Err(Error::BadMagic { what: "btree node" });
        }
        let format = BkeyFormat::parse(&b[80..80 + BkeyFormat::BYTES])?;
        let seq = le64(b, HEADER_BSET);
        if let Some(e) = expect_seq {
            if e != seq {
                return Err(Error::Corrupt(format!(
                    "btree node seq {seq:#x}, pointer expected {e:#x}"
                )));
            }
        }
        let mut node = Node {
            seq,
            flags: le64(b, 24),
            min_key: Bpos::parse(&b[32..52])?,
            max_key: Bpos::parse(&b[52..72])?,
            format,
            keys: Vec::new(),
        };
        // (bset index, key) so later bsets win a tie at the same position.
        let mut all: Vec<(usize, Bkey)> = Vec::new();
        let mut start = 0usize; // where this record's checksum starts
        let mut bset_at = HEADER_BSET;
        let mut index = 0usize;
        let block = block_bytes.max(512);
        loop {
            if bset_at + BSET_HEADER > b.len() {
                break;
            }
            let bseq = le64(b, bset_at);
            if bseq != seq {
                break; // stale or unwritten space: not part of this node
            }
            let bflags = le32(b, bset_at + 16);
            let u64s = le16(b, bset_at + 22) as usize;
            let keys_at = bset_at + BSET_HEADER;
            let end = keys_at + u64s * 8;
            if end > b.len() {
                return Err(Error::Corrupt(
                    "bset runs past the written part of the node".into(),
                ));
            }
            let csum_type = (bflags & 0xf) as u8;
            if !crate::csum::is_known(csum_type) {
                return Err(Error::Unsupported(format!(
                    "btree node checksum type {csum_type}"
                )));
            }
            let stored = le64(b, start) & csum_mask(csum_type);
            crate::csum::verify(csum_type, &b[start + 16..end], stored).map_err(|computed| {
                Error::BadChecksum {
                    what: "btree node",
                    stored,
                    computed,
                }
            })?;
            let mut p = keys_at;
            while p < end {
                let n = b[p] as usize;
                if n == 0 {
                    return Err(Error::Corrupt("zero-length key in a bset".into()));
                }
                let kend = p + n * 8;
                if kend > end {
                    return Err(Error::Corrupt("key runs past its bset".into()));
                }
                all.push((index, bkey::decode(&b[p..kend], &node.format)?));
                p = kend;
            }
            index += 1;
            start = end.div_ceil(block) * block;
            bset_at = start + 16;
        }
        if index == 0 {
            return Err(Error::Corrupt(
                "btree node holds no bset of its own sequence".into(),
            ));
        }
        // Stable sort by position keeps bset order within a position; the
        // last one (newest bset) wins.
        all.sort_by(|a, b| a.1.pos.cmp(&b.1.pos).then(a.0.cmp(&b.0)));
        let mut merged: Vec<Bkey> = Vec::with_capacity(all.len());
        for (_, k) in all {
            if let Some(last) = merged.last_mut() {
                if last.pos == k.pos {
                    *last = k;
                    continue;
                }
            }
            merged.push(k);
        }
        merged.retain(|k| k.key_type != key_type::DELETED);
        node.keys = merged;
        Ok(node)
    }
}

/// How many bytes of the 16-byte checksum field a type uses.
fn csum_mask(t: u8) -> u64 {
    match t {
        1 | 5 => 0xffff_ffff,
        _ => u64::MAX,
    }
}

/// Every leaf key of one btree, in order. Reads the whole tree: fine for
/// the images this spike reads, and the first thing to replace with a
/// cursor.
pub fn walk(dev: &dyn BlockRead, sb: &Superblock, id: u8) -> Result<Vec<Bkey>> {
    let roots = sb.btree_roots()?;
    let root = roots
        .iter()
        .find(|r| r.btree_id == id)
        .ok_or_else(|| Error::NotFound(format!("btree {id} has no root")))?;
    let key = bkey::decode(
        &root.key,
        &BkeyFormat {
            key_u64s: 5,
            nr_fields: 6,
            bits: [0; 6],
            field_offset: [0; 6],
        },
    )?;
    let mut out = Vec::new();
    descend(dev, sb, &NodePtr::from_key(&key)?, MAX_DEPTH, &mut out)?;
    Ok(out)
}

fn descend(
    dev: &dyn BlockRead,
    sb: &Superblock,
    ptr: &NodePtr,
    budget: u32,
    out: &mut Vec<Bkey>,
) -> Result<()> {
    if budget == 0 {
        return Err(Error::Corrupt("btree deeper than its root's level".into()));
    }
    let node = read_node(dev, sb, ptr)?;
    for k in node.keys {
        if k.key_type == key_type::BTREE_PTR_V2 {
            descend(dev, sb, &NodePtr::from_key(&k)?, budget - 1, out)?;
        } else {
            out.push(k);
        }
    }
    Ok(())
}

pub fn read_node(dev: &dyn BlockRead, sb: &Superblock, ptr: &NodePtr) -> Result<Node> {
    let p = &ptr.ptrs[0];
    let len = ptr.sectors_written as usize * 512;
    if len == 0 || len > (sb.btree_node_size() as usize * 512).max(512) {
        return Err(Error::Corrupt(format!(
            "btree pointer claims {len} bytes written"
        )));
    }
    let mut b = vec![0u8; len];
    dev.read_at(p.offset * 512, &mut b)?;
    Node::parse(
        &b,
        node_magic(&sb.uuid),
        sb.block_size as usize * 512,
        Some(ptr.seq),
    )
}
