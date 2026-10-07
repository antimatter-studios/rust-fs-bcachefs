//! A filesystem whose names are hashed with crc32c rather than SipHash
//! (`--str_hash=crc32c`, S1 7.7): every inode says so, the reference lister
//! prints `hash_type=crc32c`, and this reader still resolves every path and
//! reads every file, by scanning the directory instead of hashing the name.
//! The inode's hash type number for crc32c is recorded here as it is seen.

mod common;

use common::{fixture, manifest, read_text};
use fs_bcachefs::check;
use fs_bcachefs::inode::{HASH_TYPE_SIPHASH, ROOT_INO};
use fs_bcachefs::Filesystem;
use fs_core::FileDevice;
use sha2::{Digest, Sha256};

#[test]
fn names_hashed_with_crc32c_are_found_by_scanning() {
    let listing = read_text("strhash.inodes.txt");
    assert!(
        listing.contains("hash_type=crc32c"),
        "the lister does not say crc32c"
    );
    assert!(
        !listing.contains("hash_type=siphash"),
        "an inode hashed with siphash"
    );

    let fs = Filesystem::open(FileDevice::open(fixture("strhash.img")).unwrap()).unwrap();
    let root = fs.inode(ROOT_INO).unwrap();
    assert_ne!(root.hash_type(), HASH_TYPE_SIPHASH, "the root says siphash");
    eprintln!("strhash: crc32c is string hash type {}", root.hash_type());

    let mut files = 0;
    for e in manifest("strhash") {
        let ino = fs
            .lookup(&e.path)
            .unwrap_or_else(|err| panic!("{}: {err}", e.path));
        let i = fs.inode(ino).unwrap();
        assert_eq!(i.hash_type(), root.hash_type(), "{}", e.path);
        match e.kind.as_str() {
            "file" => {
                let data = fs.read(ino).unwrap();
                let sum = format!("{:x}", Sha256::digest(&data));
                assert_eq!(Some(sum), e.sha256, "{}", e.path);
                files += 1;
            }
            "dir" => assert!(i.is_dir(), "{}", e.path),
            "symlink" => {
                assert!(i.is_symlink(), "{}", e.path);
                assert_eq!(
                    String::from_utf8_lossy(&fs.read(ino).unwrap()),
                    e.target.clone().unwrap(),
                    "{}",
                    e.path
                );
            }
            k => panic!("{}: kind {k}", e.path),
        }
    }
    assert!(files > 100, "only {files} files");
    let report = check::check(&FileDevice::open(fixture("strhash.img")).unwrap()).unwrap();
    assert!(report.clean(), "{:?}", report.problems);
}
