//! An extent's data checksum against the `data_checksum` option that applies
//! to it (#614).
//!
//! Data is checksummed with the type the option in force names: the inode's
//! own (set on it or inherited, docs/clean-room.md open question 7), or else the
//! filesystem's. An extent written under other options records the work that
//! would bring it in line in a reconcile entry, which puts it in the
//! reconcile_work btree. The reference checker reports an extent that is under
//! neither as "extent with incorrect/missing reconcile opts", counted as
//! `extent_io_opts_not_set`, with the work it needs, `need_rb=data_checksum`:
//! on `default.img` (crc32c) for a crc32 entry whose type was cleared to none,
//! "data_checksum 0 != 1", and on `crc64.img` for a crc32c checksum that
//! verifies in place of a crc64 one, "data_checksum 0 != 2"
//! (tests/check_oracle_extent_checksum.rs).
//!
//! Judged only where every part was observed: an extent with a crc entry whose
//! checksum type is none, crc32c, crc64 or xxhash, no reconcile or poisoned
//! entry, no unwritten pointer, and an inode that is not nocow. An extent with
//! no crc entry is not judged here.

use super::Report;
use crate::bkey::Bkey;
use crate::extent::{self, ExtentEntry};

/// The `data_checksum` option values (none 0, crc32c 1, crc64 2, xxhash 3), by
/// name.
const OPTION_NAMES: [&str; 4] = ["none", "crc32c", "crc64", "xxhash"];

/// The `data_checksum` option a crc entry's checksum type is written under:
/// none (0), crc32c (5), crc64 (6) and xxhash (7). `None` for any other type,
/// which is not judged.
fn option_of(csum_type: u8) -> Option<u64> {
    match csum_type {
        0 => Some(0),
        5 => Some(1),
        6 => Some(2),
        7 => Some(3),
        _ => None,
    }
}

/// The `data_checksum` option in force for an inode: its own field, stored as
/// the value plus one, or the filesystem's when that field is 0.
pub(super) fn option_in_force(inode_field: u64, filesystem: u8) -> u64 {
    inode_field
        .checked_sub(1)
        .unwrap_or_else(|| u64::from(filesystem))
}

fn name(option: u64) -> String {
    OPTION_NAMES
        .get(option as usize)
        .map_or_else(|| format!("unknown {option}"), |n| n.to_string())
}

/// Extent `k`, whose inode's `data_checksum` option in force is `want`.
pub(super) fn judge(k: &Bkey, want: u64, r: &mut Report) {
    if !matches!(extent::reconciling(&k.value), Ok(false))
        || !matches!(extent::poisoned(&k.value), Ok(false))
    {
        return;
    }
    let Ok(entries) = extent::parse_entries(&k.value) else {
        return;
    };
    if entries
        .iter()
        .any(|e| matches!(e, ExtentEntry::Ptr(p) if p.is_unwritten()))
    {
        return;
    }
    let have = entries.iter().find_map(|e| match e {
        ExtentEntry::Crc(c) => option_of(c.csum_type).filter(|&have| have != want),
        ExtentEntry::Ptr(_) => None,
    });
    if let Some(have) = have {
        r.add(
            "extent_io_opts_not_set",
            format!(
                "extent {}: its data checksum is {} where its options ask for {}, and it \
                 records no reconcile work",
                k.pos,
                name(have),
                name(want)
            ),
        );
    }
}
