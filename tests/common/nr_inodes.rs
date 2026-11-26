//! The `nr_inodes` accounting key of a reference-made fixture changed on a
//! private copy, for the reference checker and this crate's to judge (#564).

use super::common::{copy_sparse, fixture, ScratchDir, ScratchImage};
use fs_bcachefs::bkey::{key_type, Bkey, Bpos};
use fs_bcachefs::btree;
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::write::Writer;
use fs_core::FileDevice;

const ACCOUNTING: u8 = 20;
const KEY_ACCOUNTING: u8 = 34;

#[derive(Clone, Copy, Debug)]
pub enum Change {
    /// The counter holds one inode more than the filesystem does.
    OneMore,
    /// The key at POS_MIN is removed.
    Removed,
}

pub const CHANGES: [Change; 2] = [Change::OneMore, Change::Removed];

/// A private copy of the reference-made fixture.
pub fn copy(set: &str) -> ScratchImage {
    let image = ScratchDir::new("nr-inodes").into_image("planted.img");
    copy_sparse(fixture(&format!("{set}.img")), &image);
    image
}

/// The copy with the change committed through the writer.
pub fn plant(set: &str, change: Change) -> ScratchImage {
    let source = fixture(&format!("{set}.img"));
    let image = copy(set);
    let device = FileDevice::open(&source).unwrap();
    let superblock = Superblock::read(&device).unwrap();
    let at = Bpos {
        inode: 0,
        offset: 0,
        snapshot: 0,
    };
    let counter = btree::walk(&device, &superblock, ACCOUNTING)
        .unwrap()
        .into_iter()
        .find(|k| k.key_type == KEY_ACCOUNTING && k.pos == at)
        .unwrap_or_else(|| panic!("{set}: no nr_inodes key at POS_MIN"));
    let changed = match change {
        Change::OneMore => {
            let mut key = counter;
            let count = u64::from_le_bytes(key.value[..8].try_into().unwrap());
            key.value[..8].copy_from_slice(&(count + 1).to_le_bytes());
            key
        }
        Change::Removed => Bkey {
            key_type: key_type::DELETED,
            value: Vec::new(),
            ..counter
        },
    };
    let mut writer = Writer::open(FileDevice::open_rw(&image).unwrap()).unwrap();
    writer.insert(ACCOUNTING, vec![changed]).unwrap();
    drop(writer);
    image
}
