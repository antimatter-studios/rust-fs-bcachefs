//! Reading a few bytes must not allocate the whole of every key that covers
//! them, at the size the key states (#151). A reservation or a whiteout was
//! read as a buffer of zeros as large as the key, and an extent with no
//! checksum entry as a buffer as large as the extent, both before anything
//! bounded them: a 32-bit size in sectors is up to 2 TiB, the allocation
//! fails, and Rust aborts. An abort cannot be caught, so each case runs in a
//! child process. The writer makes the keys on fixture copies.
#![cfg(feature = "write")]

mod common;

use common::fixture;
use fs_bcachefs::bkey::{key_type, Bkey, Bpos};
use fs_bcachefs::btree::{self, btree_id};
use fs_bcachefs::extent::DataExtent;
use fs_bcachefs::inode::{InodeV3Raw, ROOT_INO};
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::write::Writer;
use fs_bcachefs::Filesystem;
use fs_core::FileDevice;

fn scratch(name: &str, test: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "rust-fs-bcachefs-crafted-extent-{}",
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

const CHILD: &str = "RUST_FS_BCACHEFS_CRAFTED_EXTENT_CHILD";

/// Run test `name` again in a child process with CHILD set, and return what
/// it printed; the child does the read, so an abort ends only the child.
fn in_child(name: &str) -> (std::process::ExitStatus, String) {
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([name, "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, name)
        .output()
        .unwrap();
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status, text)
}

fn is_child(name: &str) -> bool {
    std::env::var(CHILD).is_ok_and(|v| v == name)
}

#[test]
fn a_reservation_of_2_to_the_32_sectors_reads_as_zeros_without_aborting() {
    let name = "a_reservation_of_2_to_the_32_sectors_reads_as_zeros_without_aborting";
    if is_child(name) {
        let img = scratch("default.img", "reservation-u32-max");
        let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
        let ino = w.create_file(ROOT_INO, b"sparse", b"", 0o644).unwrap();
        drop(w);
        // The empty file claims 1 GiB, all of it under one reservation of
        // u32::MAX sectors.
        let mut inode = keys(&img, btree_id::INODES)
            .into_iter()
            .find(|k| k.pos.offset == ino && k.key_type == key_type::INODE_V3)
            .expect("the new file's inode");
        let mut raw = InodeV3Raw::parse(&inode.value).unwrap();
        raw.size = 1 << 30;
        inode.value = raw.encode();
        let reservation = Bkey {
            key_type: key_type::RESERVATION,
            size: u32::MAX,
            version_hi: 0,
            version_lo: 0,
            pos: Bpos {
                inode: ino,
                offset: u64::from(u32::MAX),
                snapshot: u32::MAX,
            },
            value: vec![0; 8],
        };
        let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
        w.insert(btree_id::INODES, vec![inode]).unwrap();
        w.insert(btree_id::EXTENTS, vec![reservation]).unwrap();
        drop(w);
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        match fs.read_range(ino, 0, 4096) {
            Ok(v) if v.len() == 4096 && v.iter().all(|&b| b == 0) => println!("CHILD-ZEROS"),
            other => println!("CHILD-WRONG {:?}", other.map(|v| v.len())),
        }
        return;
    }
    let (status, text) = in_child(name);
    assert!(
        status.success() && text.contains("CHILD-ZEROS"),
        "4096 bytes under a reservation of u32::MAX sectors did not read as zeros \
         (status {status:?}; an abort is SIGABRT):\n{text}"
    );
}

#[test]
fn an_uncompressed_extent_of_2_to_the_31_sectors_is_refused_without_aborting() {
    let name = "an_uncompressed_extent_of_2_to_the_31_sectors_is_refused_without_aborting";
    if is_child(name) {
        // nocsum's extents carry no checksum entry, so a read takes the
        // extent's own size as the number of sectors to read.
        let img = scratch("nocsum.img", "extent-2-31");
        // One with no checksum entry at all: an extent that has one is read
        // by its entry's sizes, which are bounded; one without is read by
        // the key's own size, which is what is stretched here. And its
        // file's last: keys sort by their end, so a stretched extent with
        // others after it is never reached by a read of its own start.
        let all = keys(&img, btree_id::EXTENTS);
        let k = all
            .iter()
            .find(|k| {
                k.key_type == key_type::EXTENT
                    && DataExtent::from_key(k).is_ok_and(|e| e.crc.is_none())
                    && !all.iter().any(|o| {
                        o.pos.inode == k.pos.inode
                            && o.key_type == key_type::EXTENT
                            && o.pos.offset > k.pos.offset
                    })
            })
            .cloned()
            .expect("nocsum has a file whose last extent has no checksum entry");
        let ino = k.pos.inode;
        let at = k.start_offset() * 512;
        // The same extent, from the same start, claiming 2^31 sectors: the
        // original goes, so the two are not refused as an overlap first.
        let mut gone = k.clone();
        gone.key_type = key_type::DELETED;
        gone.value = Vec::new();
        let mut long = k.clone();
        long.size = 1 << 31;
        long.pos.offset = k.start_offset() + (1 << 31);
        let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
        w.insert(btree_id::EXTENTS, vec![gone, long]).unwrap();
        drop(w);
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        // At the extent's own start, so the read reaches it.
        match fs.read_range(ino, at, 4096) {
            Err(e) => println!("CHILD-REFUSED {e}"),
            Ok(v) => println!("CHILD-READ {} bytes", v.len()),
        }
        return;
    }
    let (status, text) = in_child(name);
    assert!(
        status.success() && text.contains("CHILD-REFUSED"),
        "an extent claiming 2^31 sectors of a small device was not refused \
         (status {status:?}; an abort is SIGABRT):\n{text}"
    );
}
