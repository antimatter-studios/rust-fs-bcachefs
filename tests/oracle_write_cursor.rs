//! The writer refuses an inode cursor that cannot be advanced without
//! wrapping, writes nothing, and leaves the requested final name unpublished.
#![cfg(feature = "write")]

mod common;

use fs_bcachefs::bkey::Bkey;
use fs_bcachefs::btree;
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::write::Writer;
use fs_bcachefs::Filesystem;
use fs_core::FileDevice;

const INODE_ALLOC_CURSOR: u8 = 35;
const LOGGED_OPS: u8 = 17;

fn keys(img: &std::path::Path, id: u8) -> Vec<Bkey> {
    let dev = FileDevice::open(img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    btree::walk(&dev, &sb, id).unwrap()
}

#[test]
fn a_create_at_the_maximum_inode_cursor_is_refused_without_publishing() {
    let img = common::ScratchDir::new("writer-inode-cursor").into_image("max.img");
    common::copy_sparse(common::fixture("default.img"), &img);

    let mut cursor = keys(&img, LOGGED_OPS)
        .into_iter()
        .find(|k| k.key_type == INODE_ALLOC_CURSOR)
        .expect("the reference fixture has an inode allocation cursor");
    assert!(cursor.value.len() >= 16);
    cursor.value[8..16].copy_from_slice(&u64::MAX.to_le_bytes());

    let mut writer = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    writer.insert(LOGGED_OPS, vec![cursor]).unwrap();
    drop(writer);

    let root = Filesystem::open(FileDevice::open(&img).unwrap())
        .unwrap()
        .lookup("/")
        .unwrap();
    let before = common::sha256_file(&img);
    let result = Writer::open(FileDevice::open_rw(&img).unwrap())
        .unwrap()
        .create_file(root, b"max-cursor", &[], 0o644);
    assert!(
        result.is_err(),
        "the exhausted inode cursor was accepted: {result:?}"
    );
    // The whole image, not only the keys looked at below (#704, WRT-06).
    assert!(
        common::sha256_file(&img) == before,
        "the refused create wrote to the image"
    );

    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    assert!(fs.lookup("/max-cursor").is_err());
    let cursor = keys(&img, LOGGED_OPS)
        .into_iter()
        .find(|k| k.key_type == INODE_ALLOC_CURSOR)
        .expect("the cursor remains after refusal");
    assert_eq!(
        u64::from_le_bytes(cursor.value[8..16].try_into().unwrap()),
        u64::MAX
    );
}
