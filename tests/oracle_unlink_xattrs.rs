//! Removing a file's last link removes its extended attributes too (#161).
//! The writer deleted the inode and freed its extents but left its xattr
//! keys in the xattrs btree, where the reference checker reports each as
//! `key_in_missing_inode` (all 54 images a writer stress run left). The
//! keys are looked for directly: none may be left at the removed inode.
#![cfg(feature = "write")]

mod common;

use common::fixture;
use fs_bcachefs::bkey::Bkey;
use fs_bcachefs::btree::{self, btree_id};
use fs_bcachefs::inode::ROOT_INO;
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::write::Writer;
use fs_bcachefs::Filesystem;
use fs_core::FileDevice;

fn scratch(name: &str, test: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "rust-fs-bcachefs-unlink-xattrs-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let img = dir.join(format!("{test}.img"));
    std::fs::copy(fixture(name), &img).unwrap();
    img
}

fn xattrs_of(img: &std::path::Path, ino: u64) -> Vec<Bkey> {
    let dev = FileDevice::open(img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    match btree::walk(&dev, &sb, btree_id::XATTRS) {
        Ok(keys) => keys.into_iter().filter(|k| k.pos.inode == ino).collect(),
        Err(_) => Vec::new(),
    }
}

#[test]
fn unlinking_a_file_removes_its_extended_attributes() {
    let img = scratch("default.img", "unlink-xattrs");
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    let ino = w
        .create_file(ROOT_INO, b"with-xattrs", b"some data", 0o644)
        .unwrap();
    w.set_xattr(ino, b"user.one", b"first value").unwrap();
    w.set_xattr(ino, b"user.two", &[7u8; 200]).unwrap();
    drop(w);
    assert_eq!(xattrs_of(&img, ino).len(), 2, "both xattrs were written");

    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.unlink(ROOT_INO, b"with-xattrs").unwrap();
    drop(w);

    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    assert!(fs.lookup("/with-xattrs").is_err(), "the file is gone");
    let left: Vec<_> = xattrs_of(&img, ino)
        .into_iter()
        .filter(|k| {
            k.key_type != fs_bcachefs::bkey::key_type::DELETED
                && k.key_type != fs_bcachefs::bkey::key_type::WHITEOUT
        })
        .collect();
    assert!(
        left.is_empty(),
        "the removed file's xattrs are still in the xattrs btree: {left:?}"
    );
}
