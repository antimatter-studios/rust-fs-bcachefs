//! A write that fails leaves the file as it was (#168). On a full
//! filesystem of 4096-byte blocks, a small write to an existing file grew
//! it, then failed when committing the change needed a bucket for a node it
//! could not get: `write_at` returned an error and the file had changed
//! anyway. A caller told its write failed has to be able to trust that
//! nothing was written.
#![cfg(feature = "write")]

mod common;

use common::fixture;
use fs_bcachefs::inode::ROOT_INO;
use fs_bcachefs::write::Writer;
use fs_bcachefs::Filesystem;
use fs_core::FileDevice;

fn scratch(name: &str, test: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "rust-fs-bcachefs-failed-write-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let img = dir.join(format!("{test}.img"));
    std::fs::copy(fixture(name), &img).unwrap();
    img
}

fn read(img: &std::path::Path, path: &str) -> Vec<u8> {
    let fs = Filesystem::open(FileDevice::open(img).unwrap()).unwrap();
    fs.read(fs.lookup(path).unwrap()).unwrap()
}

#[test]
fn a_write_refused_for_space_leaves_the_file_unchanged() {
    let img = scratch("block4k.img", "full-then-write");
    let path = "/many/file-0213.txt";
    let before = read(&img, path);

    // Fill the filesystem until a create is refused for space.
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    let mut made = 0;
    while w
        .create_file(ROOT_INO, format!("fill-{made}").as_bytes(), &[7u8; 9000], 0o644)
        .is_ok()
    {
        made += 1;
        assert!(made < 100_000, "the filesystem never filled");
    }
    drop(w);
    assert!(made > 0, "nothing could be created on a copy of block4k");

    // A small write past the end of the file, which needs room it cannot get.
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    let ino = fs.lookup(path).unwrap();
    drop(fs);
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    let result = w.write_at(ino, before.len() as u64 - 1, &[0xAB; 11]);
    drop(w);

    let after = read(&img, path);
    match result {
        Err(_) => assert_eq!(
            after,
            before,
            "write_at returned an error but the file changed ({} bytes, was {})",
            after.len(),
            before.len()
        ),
        // Had the write found room it must have written exactly that.
        Ok(()) => {
            let mut want = before.clone();
            want.truncate(before.len() - 1);
            want.extend_from_slice(&[0xAB; 11]);
            assert_eq!(after, want, "the write succeeded but did not write what it was given");
        }
    }
}
