//! A key of a type its btree does not hold (#163: the reference checker's
//! "invalid bkey" in logged_ops, reconcile_work_phys and accounting). Each
//! test puts a directory entry, a key type only the dirents btree holds,
//! into one of those btrees of a copy of `default` through the writer's raw
//! `insert`, and requires the checker to name it as `invalid_bkey`. That
//! the reference rejects these keys, and for their type, is shown in the
//! guest (`tests/check_oracle_key_types.rs`, which plants every key type in
//! every btree and compares the two checkers' verdicts).
#![cfg(feature = "write")]

mod common;

use common::fixture;
use common::plant::{self, empty_root, move_root, ACCOUNTING, LOGGED_OPS, RECONCILE_WORK_PHYS};
use fs_bcachefs::bkey::{key_type, Bkey, Bpos};
use fs_bcachefs::btree::{self, btree_id};
use fs_bcachefs::check;
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::write::Writer;
use fs_core::FileDevice;

fn scratch(test: &str) -> common::ScratchImage {
    let dir = common::ScratchDir::new("check-key-types");
    let name = format!("{test}.img");
    std::fs::copy(fixture("default.img"), dir.join(&name)).unwrap();
    dir.into_image(name)
}

fn keys(img: &std::path::Path, id: u8) -> Vec<Bkey> {
    let dev = FileDevice::open(img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    btree::walk(&dev, &sb, id).unwrap()
}

fn assert_clean(img: &std::path::Path, what: &str) {
    let report = check::check(&FileDevice::open(img).unwrap()).unwrap();
    assert!(report.clean(), "{what}: {:?}", report.problems);
}

/// A directory entry of `default`, moved to `pos`.
fn dirent_at(img: &std::path::Path, pos: Bpos) -> Bkey {
    let mut k = keys(img, btree_id::DIRENTS)
        .into_iter()
        .find(|k| k.key_type == key_type::DIRENT)
        .expect("default has a directory entry");
    k.pos = pos;
    k
}

/// Plant `key` in btree `id` and require the checker to report it.
fn planted_is_reported(img: &std::path::Path, id: u8, key: Bkey) {
    let pos = key.pos;
    let mut w = Writer::open(FileDevice::open_rw(img).unwrap()).unwrap();
    w.insert(id, vec![key]).unwrap();
    drop(w);
    assert!(
        keys(img, id)
            .iter()
            .any(|k| k.pos == pos && k.key_type == key_type::DIRENT),
        "the planted key is not in the btree"
    );

    let name = plant::BTREES[usize::from(id)];
    let report = check::check(&FileDevice::open(img).unwrap()).unwrap();
    assert!(
        report.problems.iter().any(|p| p.kind == "invalid_bkey"
            && p.detail.contains(&format!("btree {name}: "))
            && p.detail.contains(&format!("key {pos} "))),
        "a dirent at {pos} in the {name} btree was not reported as invalid_bkey: {:?}",
        report.problems
    );
}

#[test]
fn a_dirent_in_the_logged_ops_btree_is_reported() {
    let img = scratch("logged-ops");
    assert_clean(&img, "the fixture is damaged");
    // Beside the inode allocation cursor, at 1:1:0.
    let pos = Bpos {
        inode: 1,
        offset: 2,
        snapshot: 0,
    };
    let key = dirent_at(&img, pos);
    planted_is_reported(&img, LOGGED_OPS, key);
}

#[test]
fn a_dirent_in_the_accounting_btree_is_reported() {
    let img = scratch("accounting");
    assert_clean(&img, "the fixture is damaged");
    // Just after the first accounting key, the inode count at POS_MIN.
    let pos = Bpos {
        inode: 0,
        offset: 1,
        snapshot: 0,
    };
    assert!(
        keys(&img, ACCOUNTING).iter().all(|k| k.pos != pos),
        "default already has an accounting key at {pos}"
    );
    let key = dirent_at(&img, pos);
    planted_is_reported(&img, ACCOUNTING, key);
}

#[test]
fn a_dirent_in_the_reconcile_work_phys_btree_is_reported() {
    let img = scratch("reconcile-work-phys");
    // `default` records no root for it: it takes the root of a btree that
    // holds no keys, which leaves the image as clean as it was.
    let donor = empty_root(&img, RECONCILE_WORK_PHYS);
    assert!(keys(&img, donor).is_empty(), "the donor btree holds keys");
    move_root(&img, donor, RECONCILE_WORK_PHYS);
    assert_clean(&img, "the moved root is damage on its own");
    let pos = Bpos {
        inode: 0,
        offset: 1,
        snapshot: 0,
    };
    let key = dirent_at(&img, pos);
    planted_is_reported(&img, RECONCILE_WORK_PHYS, key);
}
