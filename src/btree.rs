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
        Self::parse_upto(b, magic, block_bytes, expect_seq, u64::MAX)
    }

    /// [`Node::parse`], ignoring the keys of every bset whose journal
    /// sequence number is above `max_journal_seq`: after an unclean
    /// shutdown those bsets belong to journal entries that were never
    /// committed (S1 9.7.5, "sequence blacklisting"). Their checksums are
    /// still verified.
    pub fn parse_upto(
        b: &[u8],
        magic: u64,
        block_bytes: usize,
        expect_seq: Option<u64>,
        max_journal_seq: u64,
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
            let committed = le64(b, bset_at + 8) <= max_journal_seq;
            let mut p = keys_at;
            while committed && p < end {
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

/// Every leaf key of one btree, in order, on a cleanly unmounted
/// filesystem. Reads the whole tree: fine for the images this spike reads,
/// and the first thing to replace with a cursor.
///
/// An image that was not shut down cleanly is refused: its superblock's
/// roots are stale. [`walk_replayed`] reads it through its journal.
pub fn walk(dev: &dyn BlockRead, sb: &Superblock, id: u8) -> Result<Vec<Bkey>> {
    walk_replayed(dev, sb, id, None)
}

/// Every leaf key of one btree, in order, with a journal replay applied
/// over it when one is given: the root is the one the replay's newest
/// flush entry recorded, bsets from uncommitted journal entries are
/// ignored, and the replayed keys replace the keys at their positions (a
/// deleted key or a whiteout removes it). Nothing is written.
pub fn walk_replayed(
    dev: &dyn BlockRead,
    sb: &Superblock,
    id: u8,
    replay: Option<&crate::journal::Replay>,
) -> Result<Vec<Bkey>> {
    let (root_level, root_key) = root_of(sb, id, replay)?;
    if u32::from(root_level) >= MAX_DEPTH {
        return Err(Error::Corrupt(format!(
            "btree {id} root at level {root_level}"
        )));
    }
    let walk = Walk {
        dev,
        sb,
        id,
        replay,
        max_seq: replay.map(|r| r.seq).unwrap_or(u64::MAX),
    };
    let mut out = Vec::new();
    walk.descend(&NodePtr::from_key(&root_key)?, root_level, &mut out)?;
    Ok(out)
}

/// The position just after `p` in key order.
pub fn successor(p: Bpos) -> Bpos {
    if p.snapshot < u32::MAX {
        Bpos {
            snapshot: p.snapshot + 1,
            ..p
        }
    } else if p.offset < u64::MAX {
        Bpos {
            inode: p.inode,
            offset: p.offset + 1,
            snapshot: 0,
        }
    } else {
        Bpos {
            inode: p.inode.saturating_add(1),
            offset: 0,
            snapshot: 0,
        }
    }
}

const SPOS_MAX: Bpos = Bpos {
    inode: u64::MAX,
    offset: u64::MAX,
    snapshot: u32::MAX,
};

/// A cursor over one btree: [`Cursor::seek`] to a position, then
/// [`Cursor::next`] key by key. It reads only the nodes on its path -- the
/// root and one node per level down to the leaf holding the position --
/// and moves to the next leaf only when the current one is used up,
/// applying a journal replay to each node it reads as [`walk_replayed`]
/// does.
pub struct Cursor<'a> {
    walk: Walk<'a>,
    cache: Option<&'a NodeCache>,
    root_level: u8,
    root: NodePtr,
    /// The leaf being read, from where the cursor stands.
    leaf: std::vec::IntoIter<Bkey>,
    /// The largest position the leaf covers; SPOS_MAX for the last.
    leaf_max: Bpos,
    started: bool,
}

impl<'a> Cursor<'a> {
    /// A cursor over btree `id`; on an uncleanly unmounted filesystem pass
    /// its replay, as for [`walk_replayed`].
    pub fn new(
        dev: &'a dyn BlockRead,
        sb: &'a Superblock,
        id: u8,
        replay: Option<&'a crate::journal::Replay>,
    ) -> Result<Self> {
        let (root_level, root_key) = root_of(sb, id, replay)?;
        Ok(Cursor {
            walk: Walk {
                dev,
                sb,
                id,
                replay,
                max_seq: replay.map(|r| r.seq).unwrap_or(u64::MAX),
            },
            root_level,
            root: NodePtr::from_key(&root_key)?,
            cache: None,
            leaf: Vec::new().into_iter(),
            leaf_max: SPOS_MAX,
            started: false,
        })
    }

    /// Read nodes through `cache`, so the root and interior nodes every
    /// lookup passes through are read once.
    pub fn with_cache(mut self, cache: &'a NodeCache) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Stand before the first key at or after `pos`.
    pub fn seek(&mut self, pos: Bpos) -> Result<()> {
        let mut ptr = self.root.clone();
        let mut level = self.root_level;
        let mut max = SPOS_MAX;
        loop {
            let node = match self.cache {
                Some(c) => c.get(self.walk.dev, self.walk.sb, &ptr, self.walk.max_seq)?,
                None => read_node_upto(self.walk.dev, self.walk.sb, &ptr, self.walk.max_seq)?,
            };
            let keys = self
                .walk
                .apply(node.keys, level, node.min_key, node.max_key);
            if level == 0 {
                let rest: Vec<Bkey> = keys.into_iter().filter(|k| k.pos >= pos).collect();
                self.leaf = rest.into_iter();
                self.leaf_max = max;
                self.started = true;
                return Ok(());
            }
            let child = keys
                .into_iter()
                .filter(|k| k.key_type == key_type::BTREE_PTR_V2)
                .find(|k| k.pos >= pos)
                .ok_or_else(|| {
                    Error::Corrupt(format!("no child of an interior node covers {pos}"))
                })?;
            max = child.pos;
            ptr = NodePtr::from_key(&child)?;
            level -= 1;
        }
    }

    /// The next key, or `None` past the last.
    pub fn next_key(&mut self) -> Result<Option<Bkey>> {
        if !self.started {
            self.seek(Bpos::default())?;
        }
        loop {
            if let Some(k) = self.leaf.next() {
                return Ok(Some(k));
            }
            if self.leaf_max == SPOS_MAX {
                return Ok(None);
            }
            let from = successor(self.leaf_max);
            self.seek(from)?;
        }
    }
}

/// Parsed nodes, by where they are and how much of them is written: the
/// root and the interior nodes are what every lookup reads, and they do not
/// change under a reader. Bounded: it starts over past a few hundred nodes.
#[derive(Default)]
pub struct NodeCache {
    nodes: std::sync::Mutex<std::collections::HashMap<(u64, u64, u16, u64), Node>>,
}

/// How many nodes a cache holds before it starts over.
const NODE_CACHE_CAP: usize = 512;

impl NodeCache {
    fn get(
        &self,
        dev: &dyn BlockRead,
        sb: &Superblock,
        ptr: &NodePtr,
        max_seq: u64,
    ) -> Result<Node> {
        let key = (ptr.ptrs[0].offset, ptr.seq, ptr.sectors_written, max_seq);
        if let Some(n) = self.nodes.lock().ok().and_then(|m| m.get(&key).cloned()) {
            return Ok(n);
        }
        let node = read_node_upto(dev, sb, ptr, max_seq)?;
        if let Ok(mut m) = self.nodes.lock() {
            if m.len() >= NODE_CACHE_CAP {
                m.clear();
            }
            m.insert(key, node.clone());
        }
        Ok(node)
    }
}

/// A btree's root: from the replay's newest flush entry, or the clean
/// field.
fn root_of(sb: &Superblock, id: u8, replay: Option<&crate::journal::Replay>) -> Result<(u8, Bkey)> {
    match replay {
        Some(r) => r
            .roots
            .iter()
            .find(|(b, _, _)| *b == id)
            .map(|(_, level, k)| (*level, k.clone()))
            .ok_or_else(|| Error::NotFound(format!("btree {id} has no root in the journal"))),
        None => {
            if !sb.is_clean() {
                return Err(Error::Unsupported(
                    "the filesystem was not cleanly unmounted; its journal must be replayed first"
                        .into(),
                ));
            }
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
            Ok((root.level, key))
        }
    }
}

/// One walk of one btree, with the replay (if any) it applies.
struct Walk<'a> {
    dev: &'a dyn BlockRead,
    sb: &'a Superblock,
    id: u8,
    replay: Option<&'a crate::journal::Replay>,
    max_seq: u64,
}

impl Walk<'_> {
    /// Read the node at `level` (0 = leaf) and everything below it.
    fn descend(&self, ptr: &NodePtr, level: u8, out: &mut Vec<Bkey>) -> Result<()> {
        let node = read_node_upto(self.dev, self.sb, ptr, self.max_seq)?;
        let keys = self.apply(node.keys, level, node.min_key, node.max_key);
        if level == 0 {
            out.extend(keys);
            return Ok(());
        }
        for k in keys {
            if k.key_type != key_type::BTREE_PTR_V2 {
                return Err(Error::Corrupt(format!(
                    "key type {} in an interior node",
                    k.key_type
                )));
            }
            self.descend(&NodePtr::from_key(&k)?, level - 1, out)?;
        }
        Ok(())
    }

    /// A node's keys with the replayed keys of its btree and level that
    /// fall within it applied: each replaces the key at its position, and a
    /// deleted key or a whiteout removes it.
    fn apply(&self, keys: Vec<Bkey>, level: u8, min: Bpos, max: Bpos) -> Vec<Bkey> {
        let Some(overlay) = self
            .replay
            .and_then(|r| r.keys.get(&(self.id, level)))
            .filter(|o| o.iter().any(|k| k.pos >= min && k.pos <= max))
        else {
            return keys;
        };
        let mut merged: std::collections::BTreeMap<Bpos, Bkey> =
            keys.into_iter().map(|k| (k.pos, k)).collect();
        for k in overlay.iter().filter(|k| k.pos >= min && k.pos <= max) {
            if self.id == ACCOUNTING && k.key_type == ACCOUNTING_KEY {
                // The journal carries accounting as deltas, applied once:
                // only one newer than the key it adds to (by version, journal
                // sequence first) counts (S8: signed counters in the
                // journal's accounting keys, totals in the btree's).
                match merged.get_mut(&k.pos) {
                    Some(old)
                        if (k.version_lo, k.version_hi) <= (old.version_lo, old.version_hi) => {}
                    Some(old) => {
                        let n = old.value.len().min(k.value.len()) / 8;
                        for i in 0..n {
                            let v = le64(&old.value, i * 8).wrapping_add(le64(&k.value, i * 8));
                            old.value[i * 8..i * 8 + 8].copy_from_slice(&v.to_le_bytes());
                        }
                        old.version_lo = k.version_lo;
                        old.version_hi = k.version_hi;
                    }
                    None => {
                        merged.insert(k.pos, k.clone());
                    }
                }
                continue;
            }
            if k.key_type == key_type::DELETED || k.key_type == key_type::WHITEOUT {
                merged.remove(&k.pos);
            } else {
                merged.insert(k.pos, k.clone());
            }
        }
        merged.into_values().collect()
    }
}

/// The accounting btree and its key type (S1's orders; S8).
const ACCOUNTING: u8 = 20;
const ACCOUNTING_KEY: u8 = 34;

pub fn read_node(dev: &dyn BlockRead, sb: &Superblock, ptr: &NodePtr) -> Result<Node> {
    read_node_upto(dev, sb, ptr, u64::MAX)
}

fn read_node_upto(
    dev: &dyn BlockRead,
    sb: &Superblock,
    ptr: &NodePtr,
    max_seq: u64,
) -> Result<Node> {
    let p = &ptr.ptrs[0];
    let len = ptr.sectors_written as usize * 512;
    if len == 0 || len > (sb.btree_node_size() as usize * 512).max(512) {
        return Err(Error::Corrupt(format!(
            "btree pointer claims {len} bytes written"
        )));
    }
    let mut b = vec![0u8; len];
    dev.read_at(p.offset * 512, &mut b)?;
    Node::parse_upto(
        &b,
        node_magic(&sb.uuid),
        sb.block_size as usize * 512,
        Some(ptr.seq),
        max_seq,
    )
}
