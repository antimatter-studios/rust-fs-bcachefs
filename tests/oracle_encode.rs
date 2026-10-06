//! The encoders a writer needs, checked against every key the reference
//! tools wrote in every fixture: decoding then encoding gives back the
//! stored bytes, and every dirent sits at the hash of its name.

mod common;

use common::{fixture, SETS};
use fs_bcachefs::bkey::key_type;
use fs_bcachefs::btree::{self, btree_id};
use fs_bcachefs::inode::{dirent_hash, Dirent, InodeV3Raw};
use fs_bcachefs::superblock::Superblock;
use fs_core::FileDevice;

const STUDY: &[&str] = &[
    "write-study/base",
    "write-study/create-small",
    "write-study/mkdir",
];

fn sets() -> impl Iterator<Item = String> {
    SETS.iter()
        .map(|s| s.to_string())
        .chain(STUDY.iter().map(|s| s.to_string()))
}

#[test]
fn every_inode_and_dirent_re_encodes_to_its_stored_bytes() {
    for set in sets() {
        let dev = FileDevice::open(fixture(&format!("{set}.img"))).unwrap();
        let sb = Superblock::read(&dev).unwrap();
        let mut n = 0;
        for k in btree::walk(&dev, &sb, btree_id::INODES).unwrap() {
            if k.key_type == key_type::INODE_V3 {
                let raw =
                    InodeV3Raw::parse(&k.value).unwrap_or_else(|e| panic!("{set} {}: {e}", k.pos));
                assert_eq!(raw.encode(), k.value, "{set} inode {}", k.pos);
                n += 1;
            }
        }
        for k in btree::walk(&dev, &sb, btree_id::DIRENTS).unwrap() {
            if k.key_type == key_type::DIRENT {
                let d = Dirent::from_key(&k).unwrap();
                assert_eq!(d.encode_value(), k.value, "{set} dirent {}", k.pos);
                n += 1;
            }
        }
        assert!(n > 4, "{set}: only {n} keys");
    }
}

#[test]
fn every_dirent_sits_at_the_hash_of_its_name() {
    for set in sets() {
        let dev = FileDevice::open(fixture(&format!("{set}.img"))).unwrap();
        let sb = Superblock::read(&dev).unwrap();
        let seeds: std::collections::HashMap<u64, u64> = btree::walk(&dev, &sb, btree_id::INODES)
            .unwrap()
            .iter()
            .filter(|k| k.key_type == key_type::INODE_V3)
            .map(|k| (k.pos.offset, InodeV3Raw::parse(&k.value).unwrap().hash_seed))
            .collect();
        let mut probed = 0;
        for k in btree::walk(&dev, &sb, btree_id::DIRENTS).unwrap() {
            if k.key_type != key_type::DIRENT {
                continue;
            }
            let d = Dirent::from_key(&k).unwrap();
            let h = dirent_hash(seeds[&k.pos.inode], &d.name);
            // A collision moves an entry to a later free slot; none is
            // expected in these directories, and one would show here.
            if h != k.pos.offset {
                probed += 1;
            }
        }
        assert_eq!(probed, 0, "{set}: dirents not at their name's hash");
    }
}
