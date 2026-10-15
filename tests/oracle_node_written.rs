//! The reference checker refuses a torn extent-tree node whose parent
//! requires more sectors than the surviving bsets cover (#222).
//! Reference 1.39.7, run 38579070302, default seed 42: expected 61 sectors,
//! found 37. Rebuilt fixtures have different layouts, so locate the same
//! boundary between bsets rather than hard-coding the old byte offset.

mod common;

use fs_bcachefs::bkey::{self, BkeyFormat};
use fs_bcachefs::btree::{btree_id, read_node, NodePtr};
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::Error;
use fs_core::{BlockRead, FileDevice};
use std::io::{Seek, SeekFrom, Write};

const UNPACKED: BkeyFormat = BkeyFormat {
    key_u64s: 5,
    nr_fields: 6,
    bits: [0; 6],
    field_offset: [0; 6],
};

#[test]
fn torn_extent_nodes_are_refused_without_rejecting_complete_multiple_bsets() {
    let mut multiple_bsets = 0;
    for &set in common::SETS {
        let path = common::fixture(&format!("{set}.img"));
        let dev = FileDevice::open(&path).unwrap();
        let sb = Superblock::read(&dev).unwrap();
        let root = sb
            .btree_roots()
            .unwrap()
            .into_iter()
            .find(|r| r.btree_id == btree_id::EXTENTS)
            .expect("the fixture has an extents root");
        let ptr = NodePtr::from_key(&bkey::decode(&root.key, &UNPACKED).unwrap()).unwrap();
        let complete = read_node(&dev, &sb, &ptr).expect("the reference-made node is valid");
        let mut bytes = vec![0; usize::from(ptr.sectors_written) * 512];
        dev.read_at(ptr.ptrs[0].offset * 512, &mut bytes).unwrap();
        let first_end = 160 + usize::from(u16::from_le_bytes([bytes[158], bytes[159]])) * 8;
        let block = (usize::from(sb.block_size) * 512).max(512);
        let next = first_end.div_ceil(block) * block;
        if next == bytes.len() {
            continue; // This node is already covered by its only bset.
        }
        assert!(
            next + 40 <= bytes.len(),
            "a later bset has a complete header"
        );
        assert_eq!(&bytes[136..144], &bytes[next + 16..next + 24]);
        multiple_bsets += 1;

        let dir = std::env::temp_dir().join(format!(
            "rust-fs-bcachefs-node-written-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let damaged = dir.join(format!("{set}.img"));
        std::fs::copy(&path, &damaged).unwrap();
        // The first bset and its checksum stay intact; the next bset no
        // longer belongs to this node. Its parent's written count stays.
        bytes[next + 16] ^= 1;
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .open(&damaged)
            .unwrap();
        out.seek(SeekFrom::Start(ptr.ptrs[0].offset * 512)).unwrap();
        out.write_all(&bytes).unwrap();
        drop(out);
        let damaged_dev = FileDevice::open(&damaged).unwrap();
        let got = read_node(&damaged_dev, &sb, &ptr);
        assert!(
            matches!(got, Err(Error::Corrupt(_))),
            "{set}: a prefix with {} keys and only {} of {} required sectors was accepted",
            complete.keys.len(),
            next / 512,
            ptr.sectors_written
        );
        let report = fs_bcachefs::check::check(&damaged_dev).unwrap();
        assert!(!report.clean(), "{set}: the checker accepted the torn node");
    }
    assert!(
        multiple_bsets > 0,
        "chore fixtures must provide multi-bset extent nodes"
    );
}
