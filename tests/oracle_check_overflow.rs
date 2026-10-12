//! The checker judges keys whose sizes are near the top of their range
//! (#150). Its comparison of an extent's start with its inode's size works
//! in bytes, and an extent that starts 2^55 sectors in has a start of 2^64
//! bytes: with overflow checks the product panics, without them it wraps to
//! 0 and an extent far past the end of its file reads as inside it. The
//! writer's raw `insert` makes the extent on a fixture copy.
#![cfg(feature = "write")]

mod common;

use common::fixture;
use fs_bcachefs::bkey::{key_type, Bkey};
use fs_bcachefs::btree::{self, btree_id};
use fs_bcachefs::check;
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::write::Writer;
use fs_core::FileDevice;

fn scratch(name: &str, test: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "rust-fs-bcachefs-check-overflow-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let img = dir.join(format!("{test}.img"));
    std::fs::copy(fixture(name), &img).unwrap();
    img
}

fn keys(img: &std::path::Path, id: u8) -> Vec<Bkey> {
    let dev = FileDevice::open(img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    btree::walk(&dev, &sb, id).unwrap()
}

#[test]
fn an_extent_starting_2_to_the_55_sectors_in_is_past_the_end_of_its_inode() {
    let img = scratch("default.img", "extent-far-past-end");
    // A real extent of a real file, moved so it starts 2^55 sectors in.
    let mut far = keys(&img, btree_id::EXTENTS)
        .into_iter()
        .find(|k| k.key_type == key_type::EXTENT)
        .expect("the fixture has a data extent");
    far.pos.offset = (1u64 << 55) + u64::from(far.size);
    assert_eq!(far.start_offset(), 1u64 << 55);
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.insert(btree_id::EXTENTS, vec![far]).unwrap();
    drop(w);

    let report = check::check(&FileDevice::open(&img).unwrap()).expect("the checker runs");
    assert!(
        report
            .problems
            .iter()
            .any(|p| p.kind == "extent_past_end_of_inode"),
        "an extent 2^55 sectors past the start of a small file was not reported: {:?}",
        report.problems
    );
}
