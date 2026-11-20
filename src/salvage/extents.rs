//! Walk extents: an inode's extent keys as pieces of its data.

use super::{Candidate, CandidateState, FoundKey, Salvage};
use crate::bkey::key_type;
use crate::btree::btree_id;
use crate::extent::{self, DataExtent};
use fs_core::BlockRead;

/// What one piece of a file is made of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PieceKind {
    /// Stored data: read, checksum-checked and decompressed from the device
    /// as the extent describes ([`Salvage::read_piece`]).
    Data {
        extent: DataExtent,
        /// Bytes into the extent's live data where this piece starts.
        data_skip: u64,
        /// Byte offset on the device of the stored data.
        device_offset: u64,
        /// Bytes stored there: compressed, when the extent is.
        stored_bytes: u64,
        compression: u8,
        csum_type: u8,
        /// Allocated and never written: reads as zeros.
        unwritten: bool,
        /// The data failed its checksum and no good copy was left: reading
        /// it is an error.
        poisoned: bool,
    },
    /// The bytes are in the key itself.
    Inline(Vec<u8>),
    /// Space reserved and never written, or a whiteout hiding an older
    /// snapshot's data: zeros.
    Zeros,
    /// The filesystem recorded this data as lost (an `error` extent).
    Lost,
    /// Shared data in the reflink btree, from this sector index. A walk
    /// follows it into the shared extents; one left in a walk is a range
    /// the reflink btree has nothing for.
    Reflink { index: u64 },
    /// An extent key whose value does not decode.
    Undecodable(String),
}

/// One piece of a file: `length` bytes at byte `file_offset`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtentPiece {
    pub file_offset: u64,
    pub length: u64,
    pub kind: PieceKind,
    /// The key it came from: its snapshot, whether the live tree holds it,
    /// whether its bset's checksum held, and its node.
    pub snapshot: u32,
    pub live: bool,
    pub checksum_ok: bool,
    pub node_offset: u64,
    pub journal_seq: u64,
}

/// Why a walk is not the whole file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WalkEnd {
    /// Every byte up to the inode's size is a piece or a hole.
    Complete,
    /// Some of it is not: see the walk's `conflicts`, `unreadable`, and the
    /// pieces that cannot be read.
    Incomplete,
}

/// An inode's data as the extent keys found for it describe it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtentWalk {
    pub ino: u64,
    pub snapshot: u32,
    /// The inode's size in bytes, as the candidate says.
    pub size: u64,
    /// Pieces in file order, never overlapping.
    pub pieces: Vec<ExtentPiece>,
    /// Ranges `(file offset, length)` below `size` that no key covers. In
    /// bcachefs a range with no extent is a hole and reads as zeros; a lost
    /// node can look the same, which `unreadable` and the damage report
    /// tell apart.
    pub holes: Vec<(u64, u64)>,
    /// Ranges two keys claimed with different contents; the newer one is in
    /// `pieces`, by journal sequence.
    pub conflicts: Vec<(u64, u64)>,
    /// Device ranges that failed to read while the keys were gathered.
    pub unreadable: Vec<(u64, u64)>,
    pub end: WalkEnd,
}

impl ExtentWalk {
    /// Bytes the pieces cover.
    pub fn mapped(&self) -> u64 {
        self.pieces.iter().map(|p| p.length).sum()
    }
}

impl Salvage {
    /// The candidate's extents, from every version of every node that holds
    /// any. A live candidate takes the live tree's keys; a superseded one
    /// takes the newest key at each position from any version, since the
    /// live tree may no longer have them. Keys of the candidate's snapshot
    /// and its ancestors count, the nearer snapshot winning, as a read of
    /// the file would.
    pub fn walk_extents(&self, dev: &dyn BlockRead, c: &Candidate) -> ExtentWalk {
        let chain = self.snapshot_chain(c.snapshot);
        let (found, unreadable) = self.keys_of(dev, btree_id::EXTENTS, c.ino);
        let mut keys: Vec<(usize, FoundKey)> = found
            .into_iter()
            .filter_map(|k| {
                let rank = chain.iter().position(|&s| s == k.key.pos.snapshot)?;
                Some((rank, k))
            })
            .filter(|(_, k)| c.state != CandidateState::Live || k.live)
            .collect();
        // Nearest snapshot first, then newest journal sequence, then a key
        // whose checksum held, then the live one.
        keys.sort_by(|(ra, a), (rb, b)| {
            ra.cmp(rb)
                .then(b.journal_seq.cmp(&a.journal_seq))
                .then(b.checksum_ok.cmp(&a.checksum_ok))
                .then(b.live.cmp(&a.live))
        });
        let size = c.size;
        let mut pieces: Vec<ExtentPiece> = Vec::new();
        let mut conflicts = Vec::new();
        for (_, k) in keys {
            if !is_extent_key(k.key.key_type) {
                continue;
            }
            let start = k.key.start_offset().saturating_mul(512);
            let end = k.key.pos.offset.saturating_mul(512);
            let piece = piece_of(&k, start, end);
            // Earlier keys rank higher: take only what they left uncovered.
            for (s, len) in uncovered(&pieces, start, end) {
                pieces.push(trim(&piece, s, s + len));
            }
            for p in &pieces {
                let (s, e) = (
                    p.file_offset.max(start),
                    (p.file_offset + p.length).min(end),
                );
                if s < e && p.kind != piece.kind && p.node_offset != k.node_offset {
                    conflicts.push((s, e - s));
                }
            }
        }
        let mut reflink_unreadable = Vec::new();
        if pieces
            .iter()
            .any(|p| matches!(p.kind, PieceKind::Reflink { .. }))
        {
            let (shared, unreadable) = self.keys_of(dev, btree_id::REFLINK, 0);
            reflink_unreadable = unreadable;
            pieces = pieces
                .into_iter()
                .flat_map(|p| match p.kind {
                    PieceKind::Reflink { index } => follow_reflink(&p, index, &shared),
                    _ => vec![p],
                })
                .collect();
        }
        let mut unreadable = unreadable;
        unreadable.extend(reflink_unreadable);
        pieces.sort_by_key(|p| p.file_offset);
        conflicts.sort();
        conflicts.dedup();
        let holes = uncovered(&pieces, 0, size);
        let unreadable_piece = pieces.iter().any(|p| {
            matches!(
                p.kind,
                PieceKind::Lost | PieceKind::Reflink { .. } | PieceKind::Undecodable(_)
            ) || matches!(p.kind, PieceKind::Data { poisoned: true, .. })
        });
        let end = if conflicts.is_empty() && unreadable.is_empty() && !unreadable_piece {
            WalkEnd::Complete
        } else {
            WalkEnd::Incomplete
        };
        ExtentWalk {
            ino: c.ino,
            snapshot: c.snapshot,
            size,
            pieces,
            holes,
            conflicts,
            unreadable,
            end,
        }
    }
}

fn is_extent_key(t: u8) -> bool {
    matches!(
        t,
        key_type::EXTENT
            | key_type::INLINE_DATA
            | key_type::RESERVATION
            | key_type::ERROR
            | key_type::REFLINK_P
            | key_type::WHITEOUT
            | key_type::EXTENT_WHITEOUT
    )
}

fn piece_of(k: &FoundKey, start: u64, end: u64) -> ExtentPiece {
    let kind = match k.key.key_type {
        key_type::EXTENT => match DataExtent::from_key(&k.key) {
            Ok(e) => PieceKind::Data {
                data_skip: 0,
                device_offset: e.ptr.offset.saturating_mul(512),
                stored_bytes: e
                    .crc
                    .map(|c| u64::from(c.compressed_size))
                    .unwrap_or(e.len)
                    .saturating_mul(512),
                compression: e.crc.map(|c| c.compression_type).unwrap_or(0),
                csum_type: e.crc.map(|c| c.csum_type).unwrap_or(0),
                unwritten: e.ptr.is_unwritten(),
                poisoned: extent::poisoned(&k.key.value).unwrap_or(false),
                extent: e,
            },
            Err(err) => PieceKind::Undecodable(err.to_string()),
        },
        key_type::INLINE_DATA => {
            let n = k.key.value.len().min(k.key.size as usize * 512);
            PieceKind::Inline(k.key.value[..n].to_vec())
        }
        key_type::RESERVATION | key_type::WHITEOUT | key_type::EXTENT_WHITEOUT => PieceKind::Zeros,
        key_type::ERROR => PieceKind::Lost,
        key_type::REFLINK_P => match extent::reflink_p_idx(&k.key.value) {
            Ok(index) => PieceKind::Reflink { index },
            Err(err) => PieceKind::Undecodable(err.to_string()),
        },
        t => PieceKind::Undecodable(format!("key type {t}")),
    };
    ExtentPiece {
        file_offset: start,
        length: end.saturating_sub(start),
        kind,
        snapshot: k.key.pos.snapshot,
        live: k.live,
        checksum_ok: k.checksum_ok,
        node_offset: k.node_offset,
        journal_seq: k.journal_seq,
    }
}

/// The part `[s, e)` of a piece: data keeps its whole extent and moves its
/// `data_skip`, a reflink pointer moves its index.
fn trim(p: &ExtentPiece, s: u64, e: u64) -> ExtentPiece {
    let mut out = p.clone();
    out.file_offset = s;
    out.length = e - s;
    let cut = s - p.file_offset;
    match &mut out.kind {
        PieceKind::Inline(bytes) => {
            let to = ((e - p.file_offset) as usize).min(bytes.len());
            *bytes = bytes.get(cut as usize..to).unwrap_or_default().to_vec();
        }
        PieceKind::Data { data_skip, .. } => *data_skip += cut,
        PieceKind::Reflink { index } => *index += cut / 512,
        _ => {}
    }
    out
}

/// The parts of `[start, end)` no piece covers.
fn uncovered(pieces: &[ExtentPiece], start: u64, end: u64) -> Vec<(u64, u64)> {
    let mut covered: Vec<(u64, u64)> = pieces
        .iter()
        .map(|p| (p.file_offset, p.file_offset + p.length))
        .filter(|&(s, e)| s < end && e > start)
        .collect();
    covered.sort();
    let mut out = Vec::new();
    let mut at = start;
    for (s, e) in covered {
        if s > at {
            out.push((at, s.min(end)));
        }
        at = at.max(e);
        if at >= end {
            break;
        }
    }
    if at < end {
        out.push((at, end));
    }
    out.into_iter().map(|(s, e)| (s, e - s)).collect()
}

/// The shared extents a reflink pointer piece covers, as data pieces at the
/// file offsets they show up at. The live tree's key at each position is
/// taken when there is one, the newest otherwise. What the reflink btree
/// has nothing for stays a reflink piece, which cannot be read.
fn follow_reflink(p: &ExtentPiece, index: u64, shared: &[FoundKey]) -> Vec<ExtentPiece> {
    let hi = index + p.length.div_ceil(512);
    let mut keys: Vec<&FoundKey> = shared
        .iter()
        .filter(|k| {
            k.key.key_type == key_type::REFLINK_V
                && k.key.value.len() >= 8
                && k.key.start_offset() < hi
                && k.key.pos.offset > index
        })
        .collect();
    keys.sort_by(|a, b| {
        b.live
            .cmp(&a.live)
            .then(b.journal_seq.cmp(&a.journal_seq))
            .then(b.checksum_ok.cmp(&a.checksum_ok))
    });
    let mut out: Vec<ExtentPiece> = Vec::new();
    for k in keys {
        let from = k.key.start_offset().max(index);
        let to = k.key.pos.offset.min(hi);
        let as_extent = crate::bkey::Bkey {
            key_type: key_type::EXTENT,
            value: k.key.value[8..].to_vec(),
            ..k.key.clone()
        };
        let mut piece = piece_of(
            &FoundKey {
                key: as_extent,
                ..k.clone()
            },
            0,
            0,
        );
        if let PieceKind::Data { data_skip, .. } = &mut piece.kind {
            *data_skip = (from - k.key.start_offset()) * 512;
        }
        piece.snapshot = p.snapshot;
        let start = p.file_offset + (from - index) * 512;
        let end = (p.file_offset + (to - index) * 512).min(p.file_offset + p.length);
        for (s, len) in uncovered(&out, start, end) {
            let mut part = piece.clone();
            if let PieceKind::Data { data_skip, .. } = &mut part.kind {
                *data_skip += s - start;
            }
            part.file_offset = s;
            part.length = len;
            out.push(part);
        }
    }
    for (s, len) in uncovered(&out, p.file_offset, p.file_offset + p.length) {
        let mut gap = p.clone();
        gap.kind = PieceKind::Reflink {
            index: index + (s - p.file_offset) / 512,
        };
        gap.file_offset = s;
        gap.length = len;
        out.push(gap);
    }
    out
}
