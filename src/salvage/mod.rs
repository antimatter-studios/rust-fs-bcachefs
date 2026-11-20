//! The salvage verbs (docs/three-layers.md): find candidates, walk
//! extents, rebuild the tree, report damage, read by extent and
//! materialize from a plan, on the primitive layer and never on a strict
//! open.
//!
//! Results are evidence, not trusted files. An inode found only in an older
//! version of a btree node, or in a bset whose checksum fails, is a
//! candidate with that state, never dropped and never presented as a live
//! file. Nothing here repairs the source: every method reads it as
//! `&dyn BlockRead`, and [`Salvage::materialize`] writes only to the
//! caller's [`std::io::Write`].
//!
//! The types are this crate's own until the shared salvage traits are
//! released in rust-fs-core; they are shaped to map onto them.

mod damage;
mod extents;
mod materialize;
mod tree;

pub use damage::{Damage, DamageReport, DamageSource};
pub use extents::{ExtentPiece, ExtentWalk, PieceKind, WalkEnd};
pub use materialize::{MaterializeReport, MaterializedRange, Plan, PlanStep, RangeSource};
pub use tree::{ParentClaim, Tree, TreeNode};

use crate::bkey::{key_type, Bkey, Bpos};
use crate::btree::btree_id;
use crate::error::{Error, Result};
use crate::inode::Inode;
use crate::primitive::{self, LiveNodes, NodeContext, NodeEvidence, Verdict};
use crate::superblock::Superblock;
use fs_core::BlockRead;
use std::collections::BTreeMap;
use std::ops::ControlFlow;

/// The snapshots btree, and its key type, as [`crate::Filesystem`] reads
/// them.
const SNAPSHOTS_BTREE: u8 = 9;
const KEY_SNAPSHOT: u8 = 22;

/// A salvage verb or part of one this adapter cannot do for a volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Feature {
    FindCandidates,
    WalkExtents,
    RebuildTree,
    ReportDamage,
    ReadExtent,
    Materialize,
    /// Inodes the live tree no longer holds, found in older versions of
    /// btree nodes.
    DeletedCandidates,
    /// Reading the data of an encrypted extent.
    EncryptedExtents,
}

/// What a candidate's evidence says about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CandidateState {
    /// The live tree's current key for this inode at this snapshot.
    Live,
    /// A key the live tree no longer holds: in an older version of a node
    /// the live tree does not reach, or in an earlier bset of a live node
    /// that a later bset replaced, while the live tree still holds this
    /// inode at this snapshot: an older version of it.
    Superseded,
    /// A key the live tree no longer holds, for an inode the live tree does
    /// not hold at this snapshot either: deleted since, as far as the live
    /// tree says.
    Deleted,
}

/// One inode found in a btree node, with where it was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub ino: u64,
    pub snapshot: u32,
    pub state: CandidateState,
    /// Whether the bset holding the key passed its checksum.
    pub checksum_ok: bool,
    /// Byte offset of the node it was found in, its sequence number, and
    /// the bset within it.
    pub node_offset: u64,
    pub node_seq: u64,
    pub bset: usize,
    /// The bset's journal sequence number: which of two keys is newer.
    pub journal_seq: u64,
    /// Device byte offset of the key itself, and its length: the
    /// candidate's identity.
    pub key_offset: u64,
    pub key_bytes: u64,
    pub mode: u32,
    pub size: u64,
    pub nlink: u32,
    pub mtime: u64,
}

impl Candidate {
    pub fn is_dir(&self) -> bool {
        self.mode & 0o170000 == 0o040000
    }
    pub fn is_file(&self) -> bool {
        self.mode & 0o170000 == 0o100000
    }
}

/// What a candidate scan covered.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanReport {
    pub start: u64,
    pub end: u64,
    pub candidates: u64,
    /// Ranges `(offset, length)` the source failed to read.
    pub unreadable: Vec<(u64, u64)>,
    /// Where to resume when the sink stopped the scan.
    pub resume_at: Option<u64>,
}

/// The salvage adapter for one bcachefs volume: its geometry and which
/// nodes the live tree reaches. It holds no device; every verb takes the
/// source again.
#[derive(Debug, Clone)]
pub struct Salvage {
    sb: Superblock,
    ctx: NodeContext,
    live: LiveNodes,
    /// Each snapshot's parent, from the live snapshots btree, as far as it
    /// reads.
    parents: BTreeMap<u32, u32>,
    /// Every node on the device, once [`Salvage::index`] or the first walk
    /// has scanned it.
    index: std::sync::OnceLock<NodeIndex>,
    /// Every `(inode, snapshot)` the live inodes btree holds.
    live_inodes: std::collections::BTreeSet<(u64, u32)>,
}

/// Every node on the device, and the ranges the scan that found them
/// could not read.
type NodeIndex = (Vec<NodeSummary>, Vec<(u64, u64)>);

/// Where a node is and what it covers, kept by [`Salvage::index`] so that
/// a walk reads only the nodes that can hold its keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeSummary {
    pub offset: u64,
    pub seq: u64,
    pub btree_id: u8,
    pub level: u8,
    pub min_key: Bpos,
    pub max_key: Bpos,
    /// Whether the live tree reaches it.
    pub live: bool,
    pub valid: bool,
}

impl Salvage {
    /// Set up salvage of the volume on `dev`. Refused, naming why, when no
    /// superblock copy validates (nothing says what the volume's nodes look
    /// like), when the volume is encrypted (its nodes and journal can only be
    /// judged with its key), or when it spans more than one device.
    pub fn open(dev: &dyn BlockRead) -> Result<Self> {
        let survey = primitive::survey_superblocks(dev);
        let sb = survey.superblock.ok_or_else(|| {
            Error::Corrupt(
                "no superblock copy validates: the volume's geometry and node magic are unknown"
                    .into(),
            )
        })?;
        Self::with_superblock(dev, sb)
    }

    /// [`Salvage::open`] with a superblock the caller chose, such as an
    /// older copy from [`primitive::survey_superblocks`].
    pub fn with_superblock(dev: &dyn BlockRead, sb: Superblock) -> Result<Self> {
        if let Some(crypt) = sb.crypt()? {
            return Err(Error::Unsupported(if crypt.is_sealed() {
                "the volume is encrypted, its master key sealed by a passphrase: salvage reads \
                 no encrypted volume"
                    .into()
            } else {
                "the volume is encrypted (its master key stored unsealed): salvage reads no \
                 encrypted volume"
                    .into()
            }));
        }
        if sb.nr_devices != 1 {
            return Err(Error::Unsupported(format!(
                "the volume spans {} devices: salvage reads single-device volumes only",
                sb.nr_devices
            )));
        }
        let ctx = NodeContext::from_superblock(&sb);
        let live = primitive::live_nodes(dev, &sb);
        let mut salvage = Salvage {
            sb,
            ctx,
            live,
            parents: BTreeMap::new(),
            index: std::sync::OnceLock::new(),
            live_inodes: Default::default(),
        };
        salvage.parents = salvage.load_parents(dev);
        salvage.live_inodes = salvage.load_live_inodes(dev);
        Ok(salvage)
    }

    /// Each snapshot's parent: the high half of a snapshot key's first word,
    /// as [`crate::Filesystem`] reads it, from the live snapshots leaves
    /// that read.
    fn load_parents(&self, dev: &dyn BlockRead) -> BTreeMap<u32, u32> {
        let mut out = BTreeMap::new();
        for (&offset, n) in &self.live.nodes {
            if n.btree_id != SNAPSHOTS_BTREE || n.level != 0 {
                continue;
            }
            let Ok(Some(node)) = primitive::inspect_node(dev, &self.ctx, offset) else {
                continue;
            };
            for k in node.merged_keys(false) {
                if k.key_type == KEY_SNAPSHOT && k.value.len() >= 8 {
                    let parent = (crate::util::le64(&k.value, 0) >> 32) as u32;
                    out.insert(k.pos.offset as u32, parent);
                }
            }
        }
        out
    }

    fn load_live_inodes(&self, dev: &dyn BlockRead) -> std::collections::BTreeSet<(u64, u32)> {
        let mut out = std::collections::BTreeSet::new();
        for (&offset, n) in &self.live.nodes {
            if n.btree_id != btree_id::INODES || n.level != 0 {
                continue;
            }
            let Ok(Some(node)) = primitive::inspect_node(dev, &self.ctx, offset) else {
                continue;
            };
            for k in node.merged_keys(false) {
                if matches!(k.key_type, key_type::INODE_V3 | key_type::INODE_V2) {
                    out.insert((k.pos.offset, k.pos.snapshot));
                }
            }
        }
        out
    }

    /// `snapshot` and its ancestors, nearest first.
    pub fn snapshot_chain(&self, snapshot: u32) -> Vec<u32> {
        let mut out = vec![snapshot];
        let mut at = snapshot;
        while let Some(&p) = self.parents.get(&at) {
            if p == 0 || out.len() >= 64 || out.contains(&p) {
                break;
            }
            out.push(p);
            at = p;
        }
        out
    }

    /// Scan the whole device once and keep where every node is, so that the
    /// verbs after it read only the nodes that can hold what they look for.
    /// Without it, the first walk scans the device and keeps the index.
    pub fn index(&mut self, dev: &dyn BlockRead) -> ScanReport {
        let (index, report) = self.scan_index(dev);
        self.index = std::sync::OnceLock::from((index, report.unreadable.clone()));
        report
    }

    fn scan_index(&self, dev: &dyn BlockRead) -> (Vec<NodeSummary>, ScanReport) {
        let mut out = Vec::new();
        let scan = primitive::scan_nodes(dev, &self.ctx, 0, u64::MAX, |node| {
            out.push(NodeSummary {
                offset: node.offset,
                seq: node.seq,
                btree_id: node.btree_id,
                level: node.level,
                min_key: node.min_key,
                max_key: node.max_key,
                live: self.is_live(&node),
                valid: node.is_valid(),
            });
            ControlFlow::Continue(())
        });
        let report = ScanReport {
            start: scan.start,
            end: scan.end,
            candidates: 0,
            unreadable: scan.unreadable,
            resume_at: scan.resume_at,
        };
        (out, report)
    }

    fn is_live(&self, node: &NodeEvidence) -> bool {
        self.live
            .nodes
            .get(&node.offset)
            .is_some_and(|n| n.seq == node.seq)
    }

    pub fn superblock(&self) -> &Superblock {
        &self.sb
    }

    pub fn node_context(&self) -> &NodeContext {
        &self.ctx
    }

    pub fn live_nodes(&self) -> &LiveNodes {
        &self.live
    }

    /// The features this adapter lacks for this volume. Encrypted volumes
    /// are refused at [`Salvage::open`], so none is missing today; the list
    /// is here so a caller asks instead of assuming.
    pub fn missing_features(&self) -> Vec<Feature> {
        Vec::new()
    }

    /// Every inode key whose bytes start between `start` and `end` (device
    /// bytes), in every inodes-btree leaf node, live or not, passed to
    /// `sink`, which may stop the scan. One inode number at several
    /// snapshots is several candidates; one key in several node versions is
    /// a candidate per version. A stopped scan resumes at its report's
    /// `resume_at`, just past the last candidate it passed on.
    pub fn find_candidates(
        &self,
        dev: &dyn BlockRead,
        start: u64,
        end: u64,
        mut sink: impl FnMut(Candidate) -> ControlFlow<()>,
    ) -> ScanReport {
        let mut count = 0u64;
        let mut resume_at = None;
        // A node that starts before `start` can hold keys after it.
        let from = start.saturating_sub(self.ctx.node_bytes as u64);
        let scan = primitive::scan_nodes(dev, &self.ctx, from, end, |node| {
            if node.btree_id != btree_id::INODES || node.level != 0 {
                return ControlFlow::Continue(());
            }
            for c in self.candidates_in(&node) {
                if c.key_offset < start || c.key_offset >= end {
                    continue;
                }
                count += 1;
                let at = c.key_offset + 1;
                if sink(c).is_break() {
                    resume_at = Some(at);
                    return ControlFlow::Break(());
                }
            }
            ControlFlow::Continue(())
        });
        ScanReport {
            start,
            end: scan.end,
            candidates: count,
            unreadable: scan
                .unreadable
                .into_iter()
                .filter(|&(o, l)| o + l > start)
                .map(|(o, l)| (o.max(start), o + l - o.max(start)))
                .collect(),
            resume_at,
        }
    }

    /// Every candidate on the device, collected.
    pub fn all_candidates(&self, dev: &dyn BlockRead) -> (Vec<Candidate>, ScanReport) {
        let mut out = Vec::new();
        let report = self.find_candidates(dev, 0, u64::MAX, |c| {
            out.push(c);
            ControlFlow::Continue(())
        });
        (out, report)
    }

    fn candidates_in(&self, node: &NodeEvidence) -> Vec<Candidate> {
        let live = self.is_live(node);
        let winners = node.merged_keys(false);
        node.keys
            .iter()
            .filter(|k| matches!(k.key.key_type, key_type::INODE_V3 | key_type::INODE_V2))
            .filter_map(|k| {
                let inode = Inode::from_key(&k.key).ok()?;
                let winner = winners.iter().any(|w| w == &k.key);
                let bset = &node.bsets[k.bset];
                Some(Candidate {
                    ino: k.key.pos.offset,
                    snapshot: k.key.pos.snapshot,
                    state: if live && winner {
                        CandidateState::Live
                    } else if self
                        .live_inodes
                        .contains(&(k.key.pos.offset, k.key.pos.snapshot))
                    {
                        CandidateState::Superseded
                    } else {
                        CandidateState::Deleted
                    },
                    checksum_ok: bset.verdict == Verdict::Valid,
                    node_offset: node.offset,
                    node_seq: node.seq,
                    bset: k.bset,
                    journal_seq: bset.journal_seq,
                    key_offset: node.offset + k.at as u64,
                    key_bytes: k.bytes as u64,
                    mode: inode.mode,
                    size: inode.size,
                    nlink: inode.link_count(),
                    mtime: inode.mtime,
                })
            })
            .collect()
    }

    /// Every leaf key of btree `id` whose position's inode is `ino`, from
    /// every version of every node that can hold one, with its provenance,
    /// and the ranges the source failed to read on the way.
    pub(crate) fn keys_of(
        &self,
        dev: &dyn BlockRead,
        id: u8,
        ino: u64,
    ) -> (Vec<FoundKey>, Vec<(u64, u64)>) {
        // The first walk scans the device once; the index is kept for the
        // walks after it, which then read only the nodes that can hold keys.
        // What the scan could not read could have held some, so every walk
        // names it.
        let (index, scanned_unreadable) = self.index.get_or_init(|| {
            let (index, report) = self.scan_index(dev);
            (index, report.unreadable)
        });
        let mut unreadable = scanned_unreadable.clone();
        let mut out = Vec::new();
        for n in index.iter().filter(|n| {
            n.btree_id == id && n.level == 0 && n.min_key.inode <= ino && ino <= n.max_key.inode
        }) {
            let node = match primitive::inspect_node(dev, &self.ctx, n.offset) {
                Ok(Some(node)) => node,
                Ok(None) => continue,
                Err(_) => {
                    unreadable.push((n.offset, self.ctx.node_bytes as u64));
                    continue;
                }
            };
            let live = self.is_live(&node);
            let winners = node.merged_keys(false);
            for k in node.keys.iter().filter(|k| k.key.pos.inode == ino) {
                let bset = &node.bsets[k.bset];
                out.push(FoundKey {
                    live: live && winners.iter().any(|w| w == &k.key),
                    checksum_ok: bset.verdict == Verdict::Valid,
                    node_offset: node.offset,
                    journal_seq: bset.journal_seq,
                    key: k.key.clone(),
                });
            }
        }
        (out, unreadable)
    }

    /// Every leaf key of btree `id`, from every node version.
    pub(crate) fn all_keys(&self, dev: &dyn BlockRead, id: u8) -> (Vec<FoundKey>, Vec<(u64, u64)>) {
        let mut out = Vec::new();
        let scan = primitive::scan_nodes(dev, &self.ctx, 0, u64::MAX, |node| {
            if node.btree_id != id || node.level != 0 {
                return ControlFlow::Continue(());
            }
            let live = self.is_live(&node);
            let winners = node.merged_keys(false);
            for k in &node.keys {
                let bset = &node.bsets[k.bset];
                out.push(FoundKey {
                    live: live && winners.iter().any(|w| w == &k.key),
                    checksum_ok: bset.verdict == Verdict::Valid,
                    node_offset: node.offset,
                    journal_seq: bset.journal_seq,
                    key: k.key.clone(),
                });
            }
            ControlFlow::Continue(())
        });
        (out, scan.unreadable)
    }
}

/// A key found in a node, with its provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FoundKey {
    pub live: bool,
    pub checksum_ok: bool,
    pub node_offset: u64,
    pub journal_seq: u64,
    pub key: Bkey,
}
