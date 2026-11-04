//! What each bucket holds, against what its alloc key says it holds (#380).
//!
//! A bucket's alloc key counts its dirty and cached sectors. The reference
//! checker works both out from what occupies the bucket and corrects a key
//! that disagrees. The rules here were found by computing each candidate over
//! every clean image the reference made, and each is held to the reference's
//! printed output in the guest (docs/clean-room.md, "A bucket's sector
//! counters"):
//!
//! - an extent counts its live sectors in its pointer's bucket, and a
//!   compressed one the share of its stored size they make up, rounded up;
//! - a btree node counts the node size in its pointer's bucket;
//! - a journal bucket counts all of itself;
//! - a superblock copy counts its layout's maximum size, and the sectors
//!   before the primary copy count too;
//! - nothing is ever cached: no image has shown a cached pointer.
//!
//! What has not been observed leaves its bucket unjudged rather than guessed: a
//! pointer with a flag other than unwritten. Every loop is over keys, never over
//! buckets, so a damaged layout or journal field, however large, costs nothing.

use std::collections::{BTreeMap, BTreeSet};

use super::Report;
use crate::bkey::{key_type, Bkey};
use crate::btree::{self, btree_id, NodePtr};
use crate::extent::{alloc_v4_gen, compression, parse_entries, Crc, ExtentEntry, Ptr};
use crate::journal::{self, Replay};
use crate::superblock::{Layout, Superblock};
use crate::util::le32;
use fs_core::BlockRead;

const ALLOC_DIRTY_SECTORS: usize = 16;
const ALLOC_CACHED_SECTORS: usize = 20;
const ALLOC_DATA_TYPE: usize = 14;
/// The sector the primary superblock is at: what comes before it counts as
/// superblock in every fixture.
const SB_PRIMARY_SECTOR: u64 = 8;
/// The stripes btree, and its key type (S1's order). A stripe's buckets hold
/// blocks no extent or node names, and no single-device image has shown one.
const STRIPES: u8 = 6;
const KEY_STRIPE: u8 = 14;

/// The sectors each bucket of one device holds.
pub(super) struct BucketSectors {
    bucket_size: u64,
    dev: u64,
    first_bucket: u64,
    /// What extents and btree nodes hold, by bucket.
    held: BTreeMap<u64, u64>,
    /// The sectors superblock copies hold, as sorted, disjoint spans.
    superblock: Vec<(u64, u64)>,
    /// The journal's buckets, as `(first bucket, count)` ranges.
    journal: Vec<(u64, u64)>,
    /// Buckets holding something whose count has not been observed.
    unjudged: BTreeSet<u64>,
    /// A value did not parse, so no bucket's count is known.
    unreadable: bool,
}

impl BucketSectors {
    pub(super) fn new(bucket_size: u64, dev: u64, first_bucket: u64) -> Self {
        BucketSectors {
            bucket_size,
            dev,
            first_bucket,
            held: BTreeMap::new(),
            superblock: Vec::new(),
            journal: Vec::new(),
            unjudged: BTreeSet::new(),
            unreadable: false,
        }
    }

    /// The superblock copies a layout names, and what comes before them.
    pub(super) fn superblock(&mut self, layout: &Layout) {
        let size = 1u64
            .checked_shl(u32::from(layout.sb_max_size_bits))
            .unwrap_or(u64::MAX);
        let mut spans: Vec<(u64, u64)> = layout
            .sb_offsets
            .iter()
            .map(|&at| (at, at.saturating_add(size)))
            .collect();
        spans.push((0, SB_PRIMARY_SECTOR));
        spans.sort_unstable();
        self.superblock.clear();
        for (start, end) in spans {
            match self.superblock.last_mut() {
                Some(last) if start <= last.1 => last.1 = last.1.max(end),
                _ => self.superblock.push((start, end)),
            }
        }
    }

    /// The journal's buckets, as `(first bucket, count)` ranges.
    pub(super) fn journal(&mut self, ranges: &[(u64, u64)]) {
        self.journal.extend_from_slice(ranges);
    }

    /// One btree node's pointers.
    pub(super) fn node(&mut self, ptrs: &[Ptr], sectors: u64) {
        for ptr in ptrs {
            self.pointer(ptr, sectors);
        }
    }

    /// One extent's value, `size` being its live sectors.
    pub(super) fn extent(&mut self, size: u32, value: &[u8]) {
        let Ok(entries) = parse_entries(value) else {
            self.unreadable = true;
            return;
        };
        let mut crc = None;
        for entry in entries {
            match entry {
                ExtentEntry::Crc(c) => crc = Some(c),
                ExtentEntry::Ptr(ptr) => self.pointer(&ptr, stored_sectors(size, crc.as_ref())),
            }
        }
    }

    fn pointer(&mut self, ptr: &Ptr, sectors: u64) {
        if u64::from(ptr.dev) != self.dev {
            return;
        }
        let bucket = ptr.offset / self.bucket_size;
        if ptr.flags == 0 || ptr.is_unwritten() {
            *self.held.entry(bucket).or_default() += sectors;
        } else {
            self.unjudged.insert(bucket);
        }
    }

    /// The sectors of `bucket` the superblock's copies hold.
    fn superblock_sectors(&self, bucket: u64) -> u64 {
        let Some(start) = bucket.checked_mul(self.bucket_size) else {
            return 0;
        };
        let end = start.saturating_add(self.bucket_size);
        self.superblock
            .iter()
            .map(|&(a, b)| b.min(end).saturating_sub(a.max(start)))
            .sum()
    }

    fn holds(&self, bucket: u64) -> u64 {
        let journal = self
            .journal
            .iter()
            .any(|&(first, count)| bucket >= first && bucket - first < count);
        self.held.get(&bucket).copied().unwrap_or(0)
            + self.superblock_sectors(bucket)
            + if journal { self.bucket_size } else { 0 }
    }

    /// Each alloc key against what its bucket was found to hold.
    pub(super) fn judge(&self, alloc: &[Bkey], r: &mut Report) {
        if self.unreadable {
            return;
        }
        for k in alloc {
            let bucket = k.pos.offset;
            if k.key_type != key_type::ALLOC_V4
                || k.pos.inode != self.dev
                || bucket < self.first_bucket
                || k.value.len() < ALLOC_CACHED_SECTORS + 4
                || self.unjudged.contains(&bucket)
            {
                continue;
            }
            let mut wrong = |kind: &'static str, field: &str, got: u32, want: u64| {
                if u64::from(got) != want {
                    r.add(
                        kind,
                        format!(
                            "bucket {}:{bucket} gen {} data type {} has wrong {field}: got {got}, \
                             should be {want}",
                            self.dev,
                            alloc_v4_gen(&k.value).unwrap_or(0),
                            data_type_name(k.value[ALLOC_DATA_TYPE]),
                        ),
                    );
                }
            };
            wrong(
                "alloc_key_dirty_sectors_wrong",
                "dirty_sectors",
                le32(&k.value, ALLOC_DIRTY_SECTORS),
                self.holds(bucket),
            );
            wrong(
                "alloc_key_cached_sectors_wrong",
                "cached_sectors",
                le32(&k.value, ALLOC_CACHED_SECTORS),
                0,
            );
        }
    }
}

/// What an extent of `size` live sectors holds on disk: those sectors when its
/// data is stored raw, and otherwise their share of its stored size, rounded up.
fn stored_sectors(size: u32, crc: Option<&Crc>) -> u64 {
    match crc {
        Some(c)
            if !matches!(
                c.compression_type,
                compression::NONE | compression::INCOMPRESSIBLE
            ) =>
        {
            (u64::from(size) * u64::from(c.compressed_size))
                .div_ceil(u64::from(c.uncompressed_size))
        }
        _ => u64::from(size),
    }
}

/// The name the reference prints for a data type (docs/clean-room.md,
/// "Allocating space").
fn data_type_name(data_type: u8) -> String {
    match data_type {
        0 => "free".to_string(),
        1 => "sb".to_string(),
        2 => "journal".to_string(),
        3 => "btree".to_string(),
        4 => "user".to_string(),
        9 => "need_discard".to_string(),
        other => other.to_string(),
    }
}

/// Everything a clean single-device filesystem holds. `None` when part of it
/// could not be read: what stopped it is reported where it was read, and no
/// count can be trusted without it.
pub(super) fn of_filesystem(
    dev: &dyn BlockRead,
    sb: &Superblock,
    roots: &[u8],
    trees: &BTreeMap<u8, Vec<Bkey>>,
    replay: Option<&Replay>,
    bucket_size: u64,
    first_bucket: u64,
) -> Option<BucketSectors> {
    if !roots.iter().all(|id| trees.contains_key(id)) {
        return None;
    }
    let stripes = trees.get(&STRIPES).into_iter().flatten();
    if stripes.into_iter().any(|k| k.key_type == KEY_STRIPE) {
        return None;
    }
    let mut held = BucketSectors::new(bucket_size, u64::from(sb.dev_idx), first_bucket);
    held.superblock(&sb.layout);
    held.journal(&journal::bucket_ranges(sb).ok()?);
    let node_sectors = u64::from(sb.btree_node_size());
    for &id in roots {
        let nodes = btree::node_pointers(dev, sb, id, replay).ok()??;
        for (_, key) in nodes {
            held.node(&NodePtr::from_key(&key).ok()?.ptrs, node_sectors);
        }
    }
    let extents = trees.get(&btree_id::EXTENTS).into_iter().flatten();
    for k in extents.filter(|k| k.key_type == key_type::EXTENT) {
        held.extent(k.size, &k.value);
    }
    let indirect = trees.get(&btree_id::REFLINK).into_iter().flatten();
    for k in indirect.filter(|k| k.key_type == key_type::REFLINK_V) {
        held.extent(k.size, k.value.get(8..)?);
    }
    Some(held)
}

#[cfg(test)]
#[path = "bucket_sectors_tests.rs"]
mod tests;
