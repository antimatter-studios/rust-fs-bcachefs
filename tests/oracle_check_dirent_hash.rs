//! A dirent away from its name's hash slot (#221): the reference checker
//! reports "hash table key at wrong offset" for a dirent that a lookup from
//! its name's slot cannot reach (differential run 38752704728, job
//! 116482951023, kernel seed 45), and this crate called the image clean.
//!
//! A lookup walks from the name's slot along a run of taken slots, so a
//! dirent displaced past a collision or a whiteout is valid; only one with
//! an empty slot between is damage. Each test works on its own copy of
//! `default`, removed when the test ends, pass or panic.
#![cfg(feature = "write")]

mod common;

use common::fixture;
use fs_bcachefs::bkey::{key_type, Bkey, Bpos};
use fs_bcachefs::btree::{self, btree_id};
use fs_bcachefs::check;
use fs_bcachefs::inode::Dirent;
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::write::Writer;
use fs_core::FileDevice;

/// A damaged copy of a fixture, removed when dropped.
fn scratch(test: &str) -> common::ScratchImage {
    let img = common::ScratchDir::new("check-dirent-hash").into_image(format!("{test}.img"));
    std::fs::copy(fixture("default.img"), &img).unwrap();
    img
}

/// The dirent key naming `name`.
fn dirent_key(img: &std::path::Path, name: &[u8]) -> Bkey {
    let dev = FileDevice::open(img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    btree::walk(&dev, &sb, btree_id::DIRENTS)
        .unwrap()
        .into_iter()
        .find(|k| Dirent::from_key(k).is_ok_and(|d| d.name == name))
        .expect("the dirent's key")
}

fn wrong_offsets(img: &std::path::Path) -> Vec<String> {
    let report = check::check(&FileDevice::open(img).unwrap()).expect("the checker runs");
    report
        .problems
        .iter()
        .filter(|p| p.kind == "hash_table_key_wrong_offset")
        .map(|p| p.detail.clone())
        .collect()
}

/// `d` moved `by` slots on, with `at_slot` left in its place.
fn displace(img: &std::path::Path, d: &Bkey, by: u64, at_slot: Bkey) {
    let moved = Bkey {
        pos: Bpos {
            offset: d.pos.offset + by,
            ..d.pos
        },
        ..d.clone()
    };
    let mut w = Writer::open(FileDevice::open_rw(img).unwrap()).unwrap();
    w.insert(btree_id::DIRENTS, vec![at_slot, moved]).unwrap();
    drop(w);
}

#[test]
fn a_dirent_away_from_its_slot_is_hash_table_key_wrong_offset() {
    let img = scratch("dirent-wrong-offset");
    let d = dirent_key(&img, b"hello.txt");
    assert!(wrong_offsets(&img).is_empty(), "the fixture is clean");
    // Five slots on, with nothing between: a lookup from the name's slot
    // stops at the first empty one and never reaches it.
    let gone = Bkey {
        key_type: key_type::DELETED,
        value: Vec::new(),
        ..d.clone()
    };
    displace(&img, &d, 5, gone);
    assert!(
        !wrong_offsets(&img).is_empty(),
        "a dirent five slots past its hash, with empty slots between, was not \
         reported as hash_table_key_wrong_offset"
    );
}

#[test]
fn a_dirent_past_a_whiteout_in_its_slot_is_reachable() {
    let img = scratch("dirent-past-whiteout");
    let d = dirent_key(&img, b"hello.txt");
    // One slot on, with a whiteout in its own slot: a lookup walks over the
    // whiteout and finds it, as after a colliding name was removed.
    let whiteout = Bkey {
        key_type: key_type::HASH_WHITEOUT,
        value: Vec::new(),
        ..d.clone()
    };
    displace(&img, &d, 1, whiteout);
    let found = wrong_offsets(&img);
    assert!(
        found.is_empty(),
        "a dirent reachable over a whiteout was reported: {found:?}"
    );
}
