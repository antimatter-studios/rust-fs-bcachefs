//! What each bucket holds, against what its alloc key says it holds (#380).
//!
//! Not yet judged: every call below records nothing and reports nothing.

use super::Report;
use crate::bkey::Bkey;
use crate::extent::Ptr;
use crate::superblock::Layout;

/// The sectors each bucket of one device holds.
pub(super) struct BucketSectors;

#[allow(dead_code)]
impl BucketSectors {
    pub(super) fn new(_bucket_size: u64, _dev: u64, _first_bucket: u64) -> Self {
        BucketSectors
    }

    /// The superblock copies a layout names, and what comes before them.
    pub(super) fn superblock(&mut self, _layout: &Layout) {}

    /// The journal's buckets, as `(first bucket, count)` ranges.
    pub(super) fn journal(&mut self, _ranges: &[(u64, u64)]) {}

    /// One btree node's pointers.
    pub(super) fn node(&mut self, _ptrs: &[Ptr], _sectors: u64) {}

    /// One extent's value, `size` being its live sectors.
    pub(super) fn extent(&mut self, _size: u32, _value: &[u8]) {}

    /// Each alloc key against what its bucket was found to hold.
    pub(super) fn judge(&self, _alloc: &[Bkey], _r: &mut Report) {}
}

#[cfg(test)]
#[path = "bucket_sectors_tests.rs"]
mod tests;
