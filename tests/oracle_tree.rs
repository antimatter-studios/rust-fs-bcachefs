//! The btrees as this crate walks them, compared key by key with the
//! reference lister's listing of the same image: every key's type,
//! position and size, in order.

mod common;

use common::{fixture, read_text, SETS};
use fs_bcachefs::btree::{self, btree_id};
use fs_bcachefs::superblock::Superblock;
use fs_core::FileDevice;

/// Key type names, as the reference lister prints them, for the types the
/// three btrees hold in these fixtures.
fn type_name(t: u8) -> String {
    match t {
        6 => "extent".into(),
        7 => "reservation".into(),
        10 => "dirent".into(),
        17 => "inline_data".into(),
        29 => "inode_v3".into(),
        1 => "whiteout".into(),
        4 => "hash_whiteout".into(),
        n => format!("type{n}"),
    }
}

/// `(type, pos, len)` of every key line in the lister's output.
fn lister_keys(text: &str) -> Vec<String> {
    text.lines()
        .filter(|l| l.starts_with("u64s "))
        .map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            // u64s N type T POS len L ver V ...
            format!("{} {} len {}", w[3], w[4], w[6])
        })
        .collect()
}

#[test]
fn every_btree_key_matches_the_reference_lister() {
    for set in SETS {
        let dev = FileDevice::open(fixture(&format!("{set}.img"))).unwrap();
        let sb = Superblock::read(&dev).unwrap();
        for (id, name) in [
            (btree_id::INODES, "inodes"),
            (btree_id::DIRENTS, "dirents"),
            (btree_id::EXTENTS, "extents"),
        ] {
            let ours: Vec<String> = btree::walk(&dev, &sb, id)
                .unwrap_or_else(|e| panic!("{set} {name}: {e}"))
                .iter()
                .map(|k| format!("{} {} len {}", type_name(k.key_type), k.pos, k.size))
                .collect();
            let theirs = lister_keys(&read_text(&format!("{set}.{name}.txt")));
            assert!(
                !theirs.is_empty(),
                "{set} {name}: the lister printed no keys"
            );
            assert_eq!(ours.len(), theirs.len(), "{set} {name}: key count");
            for (i, (a, b)) in ours.iter().zip(&theirs).enumerate() {
                assert_eq!(a, b, "{set} {name}: key {i}");
            }
        }
    }
}
