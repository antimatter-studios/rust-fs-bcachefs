//! A crafted inode size must not take down the process that reads the file
//! (#149). `Filesystem::read` sized its buffer from `bi_size` before reading
//! anything: past `isize::MAX` that is a capacity-overflow panic, and past
//! what memory can hold it is an allocation failure, which Rust answers by
//! aborting. An abort cannot be caught, by `catch_unwind` or by the C ABI's
//! guard, so the second case runs in a child process. The writer's raw
//! `insert` gives a fixture copy's file the size.
#![cfg(feature = "write")]

mod common;

use common::fixture;
use fs_bcachefs::bkey::{key_type, Bkey};
use fs_bcachefs::btree::{self, btree_id};
use fs_bcachefs::inode::InodeV3Raw;
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::write::Writer;
use fs_bcachefs::Filesystem;
use fs_core::FileDevice;

fn scratch(name: &str, test: &str) -> common::ScratchImage {
    let img = common::ScratchDir::new("crafted-size").into_image(format!("{test}.img"));
    std::fs::copy(fixture(name), &img).unwrap();
    img
}

fn keys(img: &std::path::Path, id: u8) -> Vec<Bkey> {
    let dev = FileDevice::open(img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    btree::walk(&dev, &sb, id).unwrap()
}

/// A copy of `default` whose `/hello.txt` claims `size` bytes, and that
/// file's inode number.
fn hello_claiming(test: &str, size: u64) -> (common::ScratchImage, u64) {
    let img = scratch("default.img", test);
    let ino = Filesystem::open(FileDevice::open(&img).unwrap())
        .unwrap()
        .lookup("/hello.txt")
        .expect("default has /hello.txt");
    let mut k = keys(&img, btree_id::INODES)
        .into_iter()
        .find(|k| k.pos.offset == ino && k.key_type == key_type::INODE_V3)
        .expect("its inode key");
    let mut raw = InodeV3Raw::parse(&k.value).unwrap();
    raw.size = size;
    k.value = raw.encode();
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.insert(btree_id::INODES, vec![k]).unwrap();
    (img, ino)
}

#[test]
fn a_file_claiming_2_to_the_63_bytes_is_refused_not_a_panic() {
    let (img, ino) = hello_claiming("size-2-63", 1 << 63);
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        fs.read(ino).is_err(),
        "a file claiming 2^63 bytes read as if it had them"
    );
}

const CHILD: &str = "RUST_FS_BCACHEFS_CRAFTED_SIZE_CHILD";

#[test]
fn a_file_claiming_16_tib_is_refused_without_aborting() {
    let name = "a_file_claiming_16_tib_is_refused_without_aborting";
    if std::env::var_os(CHILD).is_some() {
        let (img, ino) = hello_claiming("size-2-44", 1 << 44);
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        match fs.read(ino) {
            Err(e) => println!("CHILD-REFUSED {e}"),
            Ok(v) => println!("CHILD-READ {} bytes", v.len()),
        }
        return;
    }
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([name, "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.contains("CHILD-REFUSED"),
        "reading a file that claims 2^44 bytes did not come back refused \
         (status {:?}; an abort is SIGABRT):\n{stdout}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
}
