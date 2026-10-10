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
    assert_eq!(r.get("reflink").map(String::as_str), Some("ok"), "{text}");
    assert_eq!(r.get("snapshot").map(String::as_str), Some("ok"), "{text}");
    assert_eq!(r.get("unmount").map(String::as_str), Some("ok"), "{text}");
    assert_eq!(r.get("fsck").map(String::as_str), Some("clean"), "{text}");
}

/// Every file, directory and symlink the kernel wrote reads back here at the
/// inode number, mode, size and contents its mount reported: the reflinked
/// pair (#7), big/random.bin and its clone, both read through a `reflink_p`
/// into the reflink btree; and the subvolume as it is now, and its snapshot
/// as it was (#12).
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
                // A hash that is not 64 hex digits is a garbled record,
                // not a misread file (#134).
                assert!(
                    e.sha256
                        .as_ref()
                        .is_some_and(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit())),
                    "kernel {}: the record's hash {:?} is garbled",
                    e.path,
                    e.sha256
                );
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
    let paths: Vec<String> = manifest("kernel").into_iter().map(|e| e.path).collect();
    for p in ["/sv/new.txt", "/snap/gone.txt", "/snap/inner/file.txt"] {
        assert!(
            paths.iter().any(|q| q == p),
            "the kernel's manifest has no {p}"
        );
    }
    assert!(
        manifest("kernel")
            .iter()
            .any(|e| e.path == "/big/random-clone.bin"),
        "the kernel's manifest has no reflinked clone"
    );
}

/// Every `reflink_p` the kernel wrote (#7) is laid out as the reference
/// lister prints it: the index into the reflink btree in the low 56 bits
/// of the first word, `may_update_opts` at bit 57, and both pads 0 in the
/// second word (no image has shown a pad that is not, so where each sits
/// in it is not known).
#[test]
fn every_reflink_pointer_holds_the_index_the_lister_printed() {
    use fs_bcachefs::btree::{self, btree_id};
    let listed: Vec<(u64, u64, bool)> = read_text("kernel.extents.txt")
        .lines()
        .filter(|l| l.contains("type reflink_p "))
        .map(|l| {
            let pos = l.split_whitespace().nth(4).unwrap();
            let offset: u64 = pos.split(':').nth(1).unwrap().parse().unwrap();
            let idx: u64 = l
                .split(" idx ")
                .nth(1)
                .and_then(|r| r.split_whitespace().next())
                .unwrap()
                .parse()
                .unwrap();
            assert!(l.contains("front_pad 0 back_pad 0"), "{l}");
            (offset, idx, l.contains("may_update_opts"))
        })
        .collect();
    let dev = FileDevice::open(fixture("kernel.img")).unwrap();
    let sb = fs_bcachefs::superblock::Superblock::read(&dev).unwrap();
    let ours: Vec<(u64, u64, bool)> = btree::walk(&dev, &sb, btree_id::EXTENTS)
        .unwrap()
        .into_iter()
        .filter(|k| k.key_type == fs_bcachefs::bkey::key_type::REFLINK_P)
        .map(|k| {
            let w0 = u64::from_le_bytes(k.value[..8].try_into().unwrap());
            let w1 = u64::from_le_bytes(k.value[8..16].try_into().unwrap());
            assert_eq!(w1, 0, "{}: the pads' word", k.pos);
            (
                k.pos.offset,
                fs_bcachefs::extent::reflink_p_idx(&k.value).unwrap(),
                w0 >> 57 & 1 == 1,
            )
        })
        .collect();
    assert!(listed.len() >= 40, "only {} reflink_p listed", listed.len());
    assert_eq!(ours, listed);
}
