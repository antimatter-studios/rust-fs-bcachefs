//! The reference kernel module as an oracle (#110). The FUSE mount cannot
//! make reflinks, snapshots, casefolded directories or per-inode options,
//! and stalls on a removal followed by a sync (#94). The harness guest
//! boots a kernel that carries the reference module (scripts/vm-setup.sh),
//! and the fixture build mounts an image through it, writes a tree, unmounts
//! cleanly and records what happened (scripts/guest-build-fixtures.sh).
//! Here that image is read back and compared with what the kernel's own
//! mount reported, entry by entry.

mod common;

use common::{fixture, manifest, read_text};
use fs_bcachefs::Filesystem;
use fs_core::FileDevice;
use sha2::{Digest, Sha256};

/// `key: value` lines of kernel.txt.
fn record() -> std::collections::BTreeMap<String, String> {
    read_text("kernel.txt")
        .lines()
        .filter_map(|l| l.split_once(": "))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

/// The guest ran a kernel with the reference module loaded, mounted an
/// image through it, unmounted it cleanly, and the reference checker passed
/// the result.
#[test]
fn the_reference_kernel_module_mounted_and_unmounted_an_image() {
    let r = record();
    let text = read_text("kernel.txt");
    assert!(
        r.get("kernel").is_some_and(|k| !k.is_empty()),
        "kernel.txt names no kernel:\n{text}"
    );
    assert_eq!(
        r.get("module").map(String::as_str),
        Some("loaded"),
        "{text}"
    );
    assert_eq!(r.get("mount").map(String::as_str), Some("ok"), "{text}");
    assert_eq!(r.get("unmount").map(String::as_str), Some("ok"), "{text}");
    assert_eq!(r.get("fsck").map(String::as_str), Some("clean"), "{text}");
}

/// Every file, directory and symlink the kernel wrote reads back here at the
/// inode number, mode, size and contents its mount reported.
#[test]
fn what_the_kernel_wrote_reads_back_byte_for_byte() {
    let fs = Filesystem::open(FileDevice::open(fixture("kernel.img")).unwrap())
        .unwrap_or_else(|e| panic!("kernel.img: {e}"));
    let mut files = 0;
    for e in manifest("kernel") {
        let ino = fs
            .lookup(&e.path)
            .unwrap_or_else(|err| panic!("kernel {}: {err}", e.path));
        assert_eq!(Some(ino), e.ino, "kernel {}: inode number", e.path);
        let inode = fs.inode(ino).unwrap();
        assert_eq!(inode.mode & 0o7777, e.mode, "kernel {}: mode", e.path);
        match e.kind.as_str() {
            "file" => {
                let data = fs.read(ino).unwrap();
                assert_eq!(Some(data.len() as u64), e.size, "kernel {}: size", e.path);
                assert_eq!(
                    Some(format!("{:x}", Sha256::digest(&data))),
                    e.sha256,
                    "kernel {}: contents",
                    e.path
                );
                files += 1;
            }
            "symlink" => {
                let target = String::from_utf8(fs.read(ino).unwrap()).unwrap();
                assert_eq!(Some(target), e.target, "kernel {}: target", e.path);
            }
            "dir" => assert!(inode.is_dir(), "kernel {}: not a directory", e.path),
            other => panic!("kernel {}: entry type {other}", e.path),
        }
    }
    assert!(files >= 10, "kernel: only {files} files compared");
}
