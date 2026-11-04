//! A filesystem the reference formatter wrote and nothing has started (#505).
//!
//! Below the newest format version the formatter leaves the first start to do
//! the rest: such an image holds three superblock copies and nothing else. The
//! reference checker starts it in memory and accepts it. All there is to judge
//! is the superblock, and so these are its rules, each measured with the
//! reference checker on copies of what the formatter wrote
//! (docs/clean-room.md, "Images formatted and never started"; the table in
//! `tests/common/never_started.rs`).
//!
//! No journal or btree is read: the reference reads neither here, and what the
//! device holds outside the copies is whatever it held before it was formatted.

use super::superblock_rules::{self, bits, member_sectors, Bounds};
use super::Report;
use crate::superblock::Superblock;
use fs_core::BlockRead;

/// `metadata_replicas` is 1: above that the reference cannot start one device,
/// even at a durability of 2.
const BOUNDS: Bounds = Bounds {
    minimum_floor: 9,
    feature_bits_end: 21,
    metadata_replicas_max: 1,
    shard_inode_numbers_bits_max: 8,
};
/// At or above this share of the device the checker reports the reserve. The
/// reference accepts up to 96.6% of a 256 MiB device and reports from 97.3%, and
/// between them its verdict varies from run to run, as at 97% of a 512 MiB one.
const GC_RESERVE_PERCENT_OF_DEVICE: u128 = 97;

/// Judge `sb`, read from `sector` of `dev`, which [`Superblock::never_started`]
/// says is one.
pub(super) fn judge(sb: &Superblock, sector: u64, dev: &dyn BlockRead, r: &mut Report) {
    let Some(m) = superblock_rules::judge(sb, sector, dev, &BOUNDS, r) else {
        return;
    };
    // What a first start needs, and a filesystem that has started has no more
    // use for. The member's flags: its state in the low bits of byte 40, the
    // data it may hold from byte 42, its durability plus one in byte 43.
    let mut wrong = |what: String| r.add("invalid_sb_members", what);
    if m[40] & 0x0f != 0 {
        wrong(format!("state {}", m[40] & 0x0f));
    }
    if m[42] & 0b110 != 0b110 {
        wrong("the device may not hold both journal and btree".into());
    }
    if (m[43] >> 4) & 0b11 == 1 {
        wrong("durability 0".into());
    }
    // In sectors, and what remains has to hold the filesystem's first metadata.
    let sectors = member_sectors(m);
    let reserve = u128::from(bits(sb.flags[2], 4, 55));
    if reserve * 100 >= sectors * GC_RESERVE_PERCENT_OF_DEVICE {
        r.add(
            "invalid_sb_option",
            format!("gc_reserve_bytes of {reserve} sectors on a device of {sectors}"),
        );
    }
}
