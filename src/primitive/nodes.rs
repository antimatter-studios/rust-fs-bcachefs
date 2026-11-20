//! Btree nodes read at any offset, found by scanning, and reached from the
//! live roots, each with every bset's verdict.

use super::Verdict;
use crate::bkey::{self, key_type, Bkey, BkeyFormat, Bpos};
use crate::btree::{self, flags_id_and_level, node_magic, NodePtr, BSET_HEADER, HEADER_BSET};
use crate::error::{Error, Result};
use crate::superblock::Superblock;
use crate::util::{le16, le32, le64};
use fs_core::BlockRead;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::ControlFlow;

/// Deeper than any real btree: a pointer cycle or a corrupt level stops here.
const MAX_DEPTH: u8 = 16;

/// Bytes a scan reads from the source at once.
const SCAN_CHUNK: u64 = 1 << 20;

/// What a node's bytes are judged against: the filesystem's node magic, its
/// block size (every bset after the first starts on a block boundary) and
/// its node size (the most a node can hold).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeContext {
    pub magic: u64,
    pub block_bytes: usize,
    pub node_bytes: usize,
}

impl NodeContext {
    pub fn from_superblock(sb: &Superblock) -> Self {
        NodeContext {
            magic: node_magic(&sb.uuid),
            block_bytes: (sb.block_size as usize * 512).max(512),
            node_bytes: (sb.btree_node_size() as usize * 512).max(512),
        }
    }
}

/// One bset of a node: where it is, what it claims, and whether its
/// checksum holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BsetEvidence {
    /// Byte offset of the record (the node header, or a `btree_node_entry`)
    /// within the node.
    pub record_offset: usize,
    pub journal_seq: u64,
    pub csum_type: u8,
    /// The checksum's verdict.
    pub verdict: Verdict,
    /// Keys decoded from this bset.
    pub keys: usize,
    /// Why decoding its keys stopped early, if it did.
    pub key_error: Option<String>,
}

/// A key and the bset it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeKey {
    pub bset: usize,
    /// Byte offset of the packed key within the node, and its length.
    pub at: usize,
    pub bytes: usize,
    pub key: Bkey,
}

/// A btree node as its bytes describe it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeEvidence {
    /// Byte offset of the node on the device.
    pub offset: u64,
    pub seq: u64,
    pub btree_id: u8,
    pub level: u8,
    pub min_key: Bpos,
    pub max_key: Bpos,
    /// Whether the node's records carry MACs: an encrypted filesystem's.
    /// Their keys are not decoded here.
    pub encrypted: bool,
    pub bsets: Vec<BsetEvidence>,
    /// Every key that decoded, in bset order, deleted keys included.
    pub keys: Vec<NodeKey>,
    /// Bytes from the node's start to the end of its last bset of its own
    /// sequence, rounded up to a block.
    pub written_bytes: usize,
    /// Damage that is not one bset's checksum: a key format that does not
    /// parse, a bset that runs past the node.
    pub issues: Vec<String>,
}

impl NodeEvidence {
    /// Whether every bset's checksum holds, every key decoded and nothing
    /// else is wrong.
    pub fn is_valid(&self) -> bool {
        self.issues.is_empty()
            && !self.bsets.is_empty()
            && self
                .bsets
                .iter()
                .all(|b| b.verdict.is_valid() && b.key_error.is_none())
    }

    /// The node's keys as the reader sees them: later bsets win at a
    /// position, and deleted keys are dropped. With `valid_only`, keys from a
    /// bset whose checksum fails are left out.
    pub fn merged_keys(&self, valid_only: bool) -> Vec<Bkey> {
        let mut all: Vec<&NodeKey> = self
            .keys
            .iter()
            .filter(|k| !valid_only || self.bsets[k.bset].verdict.is_valid())
            .collect();
        all.sort_by(|a, b| a.key.pos.cmp(&b.key.pos).then(a.bset.cmp(&b.bset)));
        let mut merged: Vec<Bkey> = Vec::with_capacity(all.len());
        for k in all {
            match merged.last_mut() {
                Some(last) if last.pos == k.key.pos => *last = k.key.clone(),
                _ => merged.push(k.key.clone()),
            }
        }
        merged.retain(|k| k.key_type != key_type::DELETED);
        merged
    }
}

/// Read the node bytes at `b`, found at device offset `offset`. `None` when
/// they do not start with this filesystem's node magic. Never fails on
/// damage: a bad checksum, a key that does not decode or a bset that runs
/// past the node is recorded in the evidence, and the keys that did decode
/// are kept.
pub fn parse_node(b: &[u8], offset: u64, ctx: &NodeContext) -> Option<NodeEvidence> {
    if b.len() < HEADER_BSET + BSET_HEADER || le64(b, 16) != ctx.magic {
        return None;
    }
    let block = ctx.block_bytes.max(512);
    let seq = le64(b, HEADER_BSET);
    let (btree_id, level) = flags_id_and_level(le64(b, 24));
    let encrypted =
        (le32(b, HEADER_BSET + 16) & 0xf) as u8 == crate::crypt::CSUM_CHACHA20_POLY1305_128;
    let mut node = NodeEvidence {
        offset,
        seq,
        btree_id,
        level,
        min_key: Bpos::parse(&b[32..52]).unwrap_or_default(),
        max_key: Bpos::parse(&b[52..72]).unwrap_or_default(),
        encrypted,
        bsets: Vec::new(),
        keys: Vec::new(),
        written_bytes: 0,
        issues: Vec::new(),
    };
    let format = if encrypted {
        None
    } else {
        match BkeyFormat::parse(&b[80..80 + BkeyFormat::BYTES]) {
            Ok(f) => Some(f),
            Err(e) => {
                node.issues.push(format!("key format: {e}"));
                None
            }
        }
    };
    let mut start = 0usize;
    let mut bset_at = HEADER_BSET;
    while bset_at + BSET_HEADER <= b.len() && le64(b, bset_at) == seq {
        let bflags = le32(b, bset_at + 16);
        let keys_at = bset_at + BSET_HEADER;
        let end = keys_at + le16(b, bset_at + 22) as usize * 8;
        let csum_type = (bflags & 0xf) as u8;
        let journal_seq = le64(b, bset_at + 8);
        if end > b.len() {
            node.issues.push(format!(
                "the bset at byte {start} runs past the node ({end} bytes, {} available)",
                b.len()
            ));
            break;
        }
        let verdict = if encrypted {
            Verdict::Unsupported("an encrypted bset: its MAC needs the master key".into())
        } else if !crate::csum::is_known(csum_type) {
            Verdict::Unsupported(format!("checksum type {csum_type}"))
        } else {
            let stored = le64(b, start) & btree::csum_mask(csum_type);
            match crate::csum::verify(csum_type, &b[start + 16..end], stored) {
                Ok(()) => Verdict::Valid,
                Err(computed) => Verdict::BadChecksum { stored, computed },
            }
        };
        let index = node.bsets.len();
        let mut decoded = 0usize;
        let mut key_error = None;
        if let Some(format) = &format {
            let mut p = keys_at;
            while p < end {
                let n = b[p] as usize;
                if n == 0 {
                    key_error = Some(format!("a zero-length key at byte {p}"));
                    break;
                }
                let kend = p + n * 8;
                if kend > end {
                    key_error = Some(format!("the key at byte {p} runs past its bset"));
                    break;
                }
                match bkey::decode(&b[p..kend], format) {
                    Ok(key) => node.keys.push(NodeKey {
                        bset: index,
                        at: p,
                        bytes: n * 8,
                        key,
                    }),
                    Err(e) => {
                        key_error = Some(format!("the key at byte {p}: {e}"));
                        break;
                    }
                }
                decoded += 1;
                p = kend;
            }
        }
        node.bsets.push(BsetEvidence {
            record_offset: start,
            journal_seq,
            csum_type,
            verdict,
            keys: decoded,
            key_error,
        });
        start = end.div_ceil(block) * block;
        bset_at = start + 16;
    }
    node.written_bytes = start.min(b.len());
    if node.bsets.is_empty() {
        node.issues
            .push("the node holds no bset of its own sequence".into());
    }
    Some(node)
}

/// Read and judge the node at device byte `offset`: up to one node's size,
/// cut short at the end of the device. `Ok(None)` when no node of this
/// filesystem starts there; an error only when the bytes cannot be read.
pub fn inspect_node(
    dev: &dyn BlockRead,
    ctx: &NodeContext,
    offset: u64,
) -> Result<Option<NodeEvidence>> {
    let size = dev.size_bytes();
    if offset >= size {
        return Err(Error::Corrupt(format!(
            "node offset {offset} is past the end of the device ({size} bytes)"
        )));
    }
    let len = (ctx.node_bytes as u64).min(size - offset) as usize;
    let mut b = crate::util::zeroed(len, "a btree node")?;
    dev.read_at(offset, &mut b)?;
    Ok(parse_node(&b, offset, ctx))
}

/// What a node scan covered.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodeScan {
    pub start: u64,
    pub end: u64,
    /// How many nodes were reported.
    pub found: u64,
    /// Ranges `(offset, length)` the source failed to read; nothing in them
    /// was judged.
    pub unreadable: Vec<(u64, u64)>,
    /// Where to resume when the callback stopped the scan; `None` when it
    /// ran to `end`.
    pub resume_at: Option<u64>,
}

/// Find every btree node of this filesystem between `start` and `end`
/// (device bytes, `end` clamped to the device), live or not: every block
/// boundary whose bytes carry the node magic is read as a node and passed
/// to `found`. A changed node is written to a new place, so a scan finds
/// older versions of nodes as well as the ones the live tree points at;
/// [`live_nodes`] says which are which.
///
/// A range the source fails to read is recorded and skipped, one block at a
/// time, and the scan goes on. `found` may stop the scan by returning
/// `ControlFlow::Break`; the report then says where to resume.
pub fn scan_nodes(
    dev: &dyn BlockRead,
    ctx: &NodeContext,
    start: u64,
    end: u64,
    mut found: impl FnMut(NodeEvidence) -> ControlFlow<()>,
) -> NodeScan {
    let block = ctx.block_bytes.max(512) as u64;
    let end = end.min(dev.size_bytes());
    let mut report = NodeScan {
        start,
        end,
        ..Default::default()
    };
    let mut at = start.div_ceil(block) * block;
    while at < end {
        let chunk_end = (at + SCAN_CHUNK).min(end);
        // 24 bytes past the chunk let the last block's magic be checked.
        let read_len = (chunk_end - at + 24).min(dev.size_bytes() - at) as usize;
        let mut buf = vec![0u8; read_len];
        let hits: Vec<u64> = if dev.read_at(at, &mut buf).is_ok() {
            (0..chunk_end - at)
                .step_by(block as usize)
                .filter(|&o| {
                    let o = o as usize;
                    o + 24 <= buf.len() && le64(&buf, o + 16) == ctx.magic
                })
                .map(|o| at + o)
                .collect()
        } else {
            // One block at a time, to keep what still reads.
            let mut hits = Vec::new();
            let mut b = vec![0u8; 24];
            let mut o = at;
            while o < chunk_end {
                if o + 24 > dev.size_bytes() {
                    break;
                }
                match dev.read_at(o, &mut b) {
                    Ok(()) if le64(&b, 16) == ctx.magic => hits.push(o),
                    Ok(()) => {}
                    Err(_) => push_range(&mut report.unreadable, o, block),
                }
                o += block;
            }
            hits
        };
        let mut next = chunk_end;
        let mut skip_to = 0u64;
        for hit in hits {
            if hit < skip_to {
                continue;
            }
            let node = match inspect_node(dev, ctx, hit) {
                Ok(Some(n)) => n,
                Ok(None) => continue,
                Err(_) => {
                    push_range(&mut report.unreadable, hit, block);
                    continue;
                }
            };
            skip_to = hit + (node.written_bytes as u64).max(block);
            report.found += 1;
            if found(node).is_break() {
                report.resume_at = Some(skip_to.min(end));
                return report;
            }
            next = next.max(skip_to.div_ceil(block) * block);
        }
        at = next;
    }
    report
}

fn push_range(ranges: &mut Vec<(u64, u64)>, offset: u64, len: u64) {
    match ranges.last_mut() {
        Some((o, l)) if *o + *l == offset => *l += len,
        _ => ranges.push((offset, len)),
    }
}

/// A node the live tree reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveNode {
    pub btree_id: u8,
    pub level: u8,
    /// The sequence number the parent's pointer expects.
    pub seq: u64,
}

/// The nodes the live tree reaches, from the btree roots down, and where
/// that walk broke.
#[derive(Debug, Clone, Default)]
pub struct LiveNodes {
    /// By device byte offset.
    pub nodes: BTreeMap<u64, LiveNode>,
    /// `(offset, why)`: a pointer whose node did not read, was not a node,
    /// or was not the node the pointer expects.
    pub breaks: Vec<(u64, String)>,
    /// Where the roots came from: `"journal"` after an unclean shutdown or
    /// when the clean roots do not read, `"superblock"` otherwise.
    pub roots_from: &'static str,
}

/// Walk every btree from its root, tolerating damage: a pointer that leads
/// nowhere is recorded and the walk goes on with its siblings. The roots
/// are the journal's newest when it has to be replayed (as
/// [`crate::Filesystem::open`] decides), the superblock's otherwise; the
/// interior pointers a replay carries are walked too.
pub fn live_nodes(dev: &dyn BlockRead, sb: &Superblock) -> LiveNodes {
    let ctx = NodeContext::from_superblock(sb);
    let mut live = LiveNodes::default();
    let mut pointers: Vec<(u8, u8, Bkey)> = Vec::new();
    match btree::journal_for(dev, sb) {
        Ok(Some(replay)) => {
            live.roots_from = "journal";
            pointers.extend(replay.roots.iter().cloned());
            for (&(id, level), keys) in &replay.keys {
                if level > 0 {
                    pointers.extend(keys.iter().map(|k| (id, level - 1, k.clone())));
                }
            }
        }
        result => {
            live.roots_from = "superblock";
            if let Err(e) = result {
                live.breaks
                    .push((0, format!("the journal does not replay: {e}")));
            }
            match sb.btree_roots() {
                Ok(roots) => {
                    for r in roots {
                        match bkey::decode_unpacked(&r.key) {
                            Ok(k) => pointers.push((r.btree_id, r.level, k)),
                            Err(e) => live
                                .breaks
                                .push((0, format!("the root of btree {}: {e}", r.btree_id))),
                        }
                    }
                }
                Err(e) => live
                    .breaks
                    .push((0, format!("the superblock's roots: {e}"))),
            }
        }
    }
    let mut seen = BTreeSet::new();
    for (id, level, key) in pointers {
        walk_live(dev, &ctx, id, level, &key, 0, &mut seen, &mut live);
    }
    live
}

#[allow(clippy::too_many_arguments)]
fn walk_live(
    dev: &dyn BlockRead,
    ctx: &NodeContext,
    id: u8,
    level: u8,
    key: &Bkey,
    depth: u8,
    seen: &mut BTreeSet<u64>,
    live: &mut LiveNodes,
) {
    if key.key_type != key_type::BTREE_PTR_V2 {
        return;
    }
    let ptr = match NodePtr::from_key(key) {
        Ok(p) => p,
        Err(e) => {
            live.breaks
                .push((0, format!("a pointer in btree {id}: {e}")));
            return;
        }
    };
    let Some(offset) = ptr.ptrs[0].offset.checked_mul(512) else {
        live.breaks
            .push((0, format!("a pointer in btree {id} overflows")));
        return;
    };
    if depth > MAX_DEPTH || !seen.insert(offset) {
        return;
    }
    live.nodes.insert(
        offset,
        LiveNode {
            btree_id: id,
            level,
            seq: ptr.seq,
        },
    );
    let node = match inspect_node(dev, ctx, offset) {
        Ok(Some(n)) => n,
        Ok(None) => {
            live.breaks
                .push((offset, "no btree node at the pointer".into()));
            return;
        }
        Err(e) => {
            live.breaks.push((offset, e.to_string()));
            return;
        }
    };
    if node.seq != ptr.seq {
        live.breaks.push((
            offset,
            format!(
                "the node's seq is {:#x}, its pointer expects {:#x}",
                node.seq, ptr.seq
            ),
        ));
        return;
    }
    if level == 0 {
        return;
    }
    for child in node.merged_keys(false) {
        walk_live(dev, ctx, id, level - 1, &child, depth + 1, seen, live);
    }
}
