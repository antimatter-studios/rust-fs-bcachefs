//! Per-inode options (#81), on the write study's `kvdb-options` image: the
//! reference mount refuses to set options, so the reference tool's offline
//! key editor set them (scripts/guest-write-study.sh, "PER-INODE OPTIONS,
//! OFFLINE"). Each file `<field>-<value>` got that value in that field,
//! the directory `o` a compression value, and the mount then wrote data
//! into each file and made `o/new`, `o/empty` and `o/sub`.
//!
//! What the writer must do with that: give a file or directory it creates
//! the options the reference gave its own in the same directory, and not
//! write data into a file whose compression it would not honour (it writes
//! uncompressed data only).
#![cfg(feature = "write")]

mod common;

use std::collections::BTreeMap;

use common::fixture;
use fs_bcachefs::btree::{self, btree_id};
use fs_bcachefs::inode::InodeV3Raw;
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::write::Writer;
use fs_bcachefs::{Error, Filesystem};
use fs_core::FileDevice;

/// The option fields of an inode (S1 7.2 names the per-inode options),
/// with the field recording which of them were set on the inode itself.
const OPTION_FIELDS: &[&str] = &[
    "data_checksum",
    "compression",
    "background_compression",
    "data_replicas",
    "promote_target",
    "foreground_target",
    "background_target",
    "erasure_code",
    "fields_set",
    "nocow",
    "inodes_32bit",
    "casefold",
];

fn scratch(test: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "rust-fs-bcachefs-inode-options-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let img = dir.join(format!("{test}.img"));
    std::fs::copy(fixture("write-study/kvdb-options.img"), &img).unwrap();
    img
}

/// Every inode of an image, decoded by this crate.
fn inodes(img: &std::path::Path) -> BTreeMap<u64, InodeV3Raw> {
    let dev = FileDevice::open(img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    btree::walk(&dev, &sb, btree_id::INODES)
        .unwrap()
        .into_iter()
        .filter(|k| k.key_type == fs_bcachefs::bkey::key_type::INODE_V3)
        .map(|k| (k.pos.offset, InodeV3Raw::parse(&k.value).unwrap()))
        .collect()
}

fn options(raw: &InodeV3Raw) -> BTreeMap<&'static str, u64> {
    OPTION_FIELDS
        .iter()
        .map(|&n| (n, raw.field(n).unwrap()))
        .collect()
}

/// The image is only a test of anything if the editor set the options:
/// `o` and each `compression-*` file carry a non-zero compression field.
#[test]
fn the_offline_editor_set_the_options() {
    let img = fixture("write-study/kvdb-options.img");
    let all = inodes(&img);
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    for path in ["/o", "/compression-1", "/compression-2"] {
        let ino = fs.lookup(path).unwrap();
        assert_ne!(
            all[&ino].field("compression"),
            Some(0),
            "{path}: no compression option set (write-study/kvdb-options.txt has each step)"
        );
    }
}

#[test]
fn a_file_or_directory_made_in_a_directory_with_options_takes_what_the_reference_gave_its_own() {
    let img = scratch("create");
    let (o, theirs_file, theirs_dir) = {
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        (
            fs.lookup("/o").unwrap(),
            fs.lookup("/o/empty").unwrap(),
            fs.lookup("/o/sub").unwrap(),
        )
    };
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    let ours_file = w.create_file(o, b"ours", b"", 0o644).unwrap();
    let ours_dir = w.mkdir(o, b"ours-dir", 0o755).unwrap();
    drop(w);
    let all = inodes(&img);
    assert_eq!(
        options(&all[&ours_file]),
        options(&all[&theirs_file]),
        "a file made in /o: this writer's (left) and the reference's /o/empty (right)"
    );
    assert_eq!(
        options(&all[&ours_dir]),
        options(&all[&theirs_dir]),
        "a directory made in /o: this writer's (left) and the reference's /o/sub (right)"
    );
}

#[test]
fn data_is_not_written_into_a_file_whose_compression_this_writer_does_not_honour() {
    let img = scratch("refuse");
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    let paths = [
        "/compression-1",
        "/compression-2",
        "/compression-3",
        "/compression-4",
        "/background_compression-2",
    ];
    let inos: Vec<u64> = paths.iter().map(|p| fs.lookup(p).unwrap()).collect();
    let plain = fs.lookup("/plain").unwrap();
    drop(fs);
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    for (path, ino) in paths.iter().zip(inos) {
        match w.write_file(ino, b"some data\n") {
            Err(Error::Unsupported(m)) if m.contains("compression") => {}
            other => panic!("{path}: {other:?}"),
        }
    }
    w.write_file(plain, b"some data\n").unwrap();
}
