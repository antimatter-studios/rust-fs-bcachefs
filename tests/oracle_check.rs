//! The checker (fsck.bcachefs) on every fixture, and on copies of them
//! damaged in ways the reference checker also catches (tests/check_oracle.rs
//! holds the reference to that, in the guest).

mod common;

use std::path::PathBuf;

use common::{fixture, SETS};
use fs_bcachefs::bkey::{self, BkeyFormat};
use fs_bcachefs::btree::{btree_id, NodePtr};
use fs_bcachefs::check::check;
use fs_bcachefs::superblock::Superblock;
use fs_core::FileDevice;

#[test]
fn every_fixture_is_clean() {
    for set in SETS.iter().chain(&["aged-unclean"]) {
        let dev = FileDevice::open(fixture(&format!("{set}.img"))).unwrap();
        let r = check(&dev).unwrap_or_else(|e| panic!("{set}: {e}"));
        assert!(r.clean(), "{set}: {:?}", r.problems);
        assert!(
            r.inodes > 300 && r.dirents > 300 && r.extents > 300,
            "{set}: {r:?}"
        );
    }
}

/// A damaged copy of a fixture in the system's temporary directory.
pub fn damaged(set: &str, what: &str, damage: impl FnOnce(&mut Vec<u8>, &Superblock)) -> PathBuf {
    let mut b = std::fs::read(fixture(&format!("{set}.img"))).unwrap();
    let sb = Superblock::read(&FileDevice::open(fixture(&format!("{set}.img"))).unwrap()).unwrap();
    damage(&mut b, &sb);
    let dir = std::env::temp_dir().join(format!("rust-fs-bcachefs-check-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(format!("{set}-{what}.img"));
    std::fs::write(&p, &b).unwrap();
    p
}

/// The byte offset of btree `id`'s root node.
pub fn root_node_at(sb: &Superblock, id: u8) -> u64 {
    let r = sb
        .btree_roots()
        .unwrap()
        .into_iter()
        .find(|r| r.btree_id == id)
        .unwrap();
    let k = bkey::decode(
        &r.key,
        &BkeyFormat {
            key_u64s: 5,
            nr_fields: 6,
            bits: [0; 6],
            field_offset: [0; 6],
        },
    )
    .unwrap();
    NodePtr::from_key(&k).unwrap().ptrs[0].offset * 512
}

fn kinds(p: &PathBuf) -> Vec<&'static str> {
    let r = check(&FileDevice::open(p).unwrap()).unwrap();
    r.problems.iter().map(|p| p.kind).collect()
}

#[test]
fn a_damaged_btree_node_is_found() {
    let p = damaged("default", "dirents-node", |b, sb| {
        let at = root_node_at(sb, btree_id::DIRENTS) as usize;
        // Inside the node header, which the first bset's checksum covers;
        // the reference names it too (tests/check_oracle.rs).
        b[at + 40] ^= 0xff;
    });
    assert!(kinds(&p).contains(&"btree_node"), "{:?}", kinds(&p));
}

#[test]
fn damaged_file_data_is_found() {
    let p = damaged("default", "data", |b, sb| {
        let dev = FileDevice::open(fixture("default.img")).unwrap();
        let fs = fs_bcachefs::Filesystem::open(dev).unwrap();
        let ino = fs.lookup("/big/random.bin").unwrap();
        let _ = sb;
        let e = fs_bcachefs::btree::walk(
            &FileDevice::open(fixture("default.img")).unwrap(),
            fs.superblock(),
            btree_id::EXTENTS,
        )
        .unwrap()
        .into_iter()
        .find(|k| k.pos.inode == ino)
        .unwrap();
        let e = fs_bcachefs::extent::DataExtent::from_key(&e).unwrap();
        b[(e.ptr.offset * 512 + 100) as usize] ^= 0xff;
    });
    assert!(kinds(&p).contains(&"extent_data"), "{:?}", kinds(&p));
}

#[test]
fn a_damaged_superblock_is_found() {
    let p = damaged("default", "superblock", |b, _| b[4096 + 0x48] ^= 0xff);
    assert!(
        kinds(&p).contains(&"superblock_checksum"),
        "{:?}",
        kinds(&p)
    );
}
