//! The superblock's journal sequence blacklist, as the reference checker
//! left it on the aged image after replaying the unclean one: the range it
//! recorded, that no bset of a formatter-made image is affected, and that
//! the aged image still reads whole with the blacklist applied (its two
//! live bsets sit exactly at the range's end).

mod common;

use common::{fixture, read_text, SETS};
use fs_bcachefs::btree::{self, btree_id};
use fs_bcachefs::superblock::Superblock;
use fs_core::FileDevice;

/// The last journal entry the reference checker replayed on the aged image,
/// from its own report ("replaying entries 302-305"). The journal's length
/// differs between fixture builds, so the range is read, never assumed.
fn last_replayed() -> u64 {
    let text = read_text("aged.replay.txt");
    let range = text
        .lines()
        .find_map(|l| l.split("replaying entries ").nth(1))
        .unwrap_or_else(|| panic!("aged.replay.txt names no replayed range:\n{text}"));
    range.trim().rsplit('-').next().unwrap().parse().unwrap()
}

#[test]
fn the_aged_image_records_the_replayed_range_and_the_formatted_ones_record_none() {
    for set in SETS {
        let dev = FileDevice::open(fixture(&format!("{set}.img"))).unwrap();
        let sb = Superblock::read(&dev).unwrap();
        let bl = sb.journal_seq_blacklist().unwrap();
        if *set == "aged" {
            // The checker blacklisted from the sequence after the last entry
            // it replayed to 64 past the last entry it read.
            assert_eq!(bl.len(), 1, "{set}: {bl:?}");
            let (start, end) = bl[0];
            assert_eq!(start, last_replayed() + 1, "{set}");
            assert!(end > start && end - start > 64, "{set}: {bl:?}");
            // And with the range applied, every btree still walks.
            for id in [
                btree_id::EXTENTS,
                btree_id::INODES,
                btree_id::DIRENTS,
                btree_id::XATTRS,
            ] {
                let keys = btree::walk(&dev, &sb, id).unwrap();
                assert!(!keys.is_empty(), "{set}: btree {id} walked to nothing");
            }
        } else {
            assert!(bl.is_empty(), "{set}: {bl:?}");
        }
    }
}
