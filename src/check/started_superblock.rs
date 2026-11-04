//! The superblock of a filesystem that has started (#533).
//!
//! The reference checker refuses a damaged one before it reads anything else.
//! Most of its rules are those of an image nothing has started, measured again
//! on a started image (docs/clean-room.md, "Superblock damage on a started
//! filesystem"; the table in `tests/common/started_superblock.rs`). A first
//! start's own rules are not among them: the state, the data a member may hold,
//! its durability and the device reserve say nothing once it has started.

use super::superblock_rules::{self, incompat_in_use, Bounds};
use super::Report;
use crate::superblock::Superblock;
use fs_core::BlockRead;

const BOUNDS: Bounds = Bounds {
    minimum_floor: 14,
    feature_bits_end: 24,
    metadata_replicas_max: 4,
    shard_inode_numbers_bits_max: 15,
};
/// The reference says a filesystem without it is no longer supported.
const FEATURE_NEW_EXTENT_OVERWRITE: u64 = 1 << 9;
/// The reference refuses it with the superblock beside a member marked as holding
/// freespace, and reports it in a pass over the filesystem without that mark.
const FEATURE_BIT_21: u64 = 1 << 21;

/// Judge `sb`, read from `sector` of `dev`.
pub(super) fn judge(sb: &Superblock, sector: u64, dev: &dyn BlockRead, r: &mut Report) {
    superblock_rules::judge(sb, sector, dev, &BOUNDS, r);
    if sb.features[0] & FEATURE_NEW_EXTENT_OVERWRITE == 0 {
        r.add("invalid_sb_features", "new_extent_overwrite is not set");
    }
    if sb.features[0] & FEATURE_BIT_21 != 0 {
        r.add("invalid_sb_features", "feature bit 21 is set");
    }
    // The reference asks this of a superblock it is about to write back, so it
    // reports it only after starting. Its verdict held on every run measured.
    let in_use = incompat_in_use(sb);
    if in_use > u64::from(sb.version.0) {
        r.add(
            "invalid_sb_version",
            format!(
                "incompatible version {in_use:#x} in use, of version {:#x}",
                sb.version.0
            ),
        );
    }
}
