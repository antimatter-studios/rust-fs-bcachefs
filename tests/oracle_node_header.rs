//! A btree node is checked against the pointer that leads to it (#162).
//! The reader believed what a node's header said about itself; the
//! reference checks it against the parent and refuses a node that
//! disagrees ("incorrect btree id", "incorrect min_key", "incorrect max
//! key", "invalid bkey format", "keys out of order"). Each case damages one
//! thing in the `dirents` root of a copy of `default`, recomputes the
//! node's checksum, and expects listing the root directory to be refused.
#![allow(dead_code, unused_imports)]

mod common;

include!("../fuzz/shared/helpers.rs");

use common::fixture;
use fs_bcachefs::btree::{btree_id, flags_id_and_level, flags_with_id_and_level};
use fs_bcachefs::inode::ROOT_INO;
use fs_bcachefs::Filesystem;
use fs_core::{BlockRead, FileDevice};
use std::io::{Seek, SeekFrom, Write};

/// A copy of `default` with `damage` applied to its dirents root node,
/// whose checksum is then recomputed.
fn damaged(test: &str, damage: impl Fn(&mut [u8])) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "rust-fs-bcachefs-node-header-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let img = dir.join(format!("{test}.img"));
    std::fs::copy(fixture("default.img"), &img).unwrap();

    let dev = FileDevice::open(&img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let root = sb
        .btree_roots()
        .unwrap()
        .into_iter()
        .find(|r| r.btree_id == btree_id::DIRENTS)
        .expect("default records a dirents root");
    let ptr = NodePtr::from_key(&bkey::decode(&root.key, &UNPACKED).unwrap()).unwrap();
    let at = ptr.ptrs[0].offset * 512;
    let mut b = vec![0u8; usize::from(ptr.sectors_written) * 512];
    dev.read_at(at, &mut b).unwrap();
    drop(dev);

    damage(&mut b);
    restamp_node(&mut b, usize::from(sb.block_size) * 512);
    let mut f = std::fs::OpenOptions::new().write(true).open(&img).unwrap();
    f.seek(SeekFrom::Start(at)).unwrap();
    f.write_all(&b).unwrap();
    img
}

fn refused(img: &std::path::Path, what: &str) {
    let fs = Filesystem::open(FileDevice::open(img).unwrap()).unwrap();
    let got = fs.readdir(ROOT_INO);
    assert!(
        got.is_err(),
        "a dirents root with {what} was read: {} entries",
        got.map(|v| v.len()).unwrap_or(0)
    );
}

fn le64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

#[test]
fn a_node_naming_another_btree_is_refused() {
    let img = damaged("btree-id", |b| {
        let flags = le64_at(b, 24);
        let (_, level) = flags_id_and_level(flags);
        let other = flags_with_id_and_level(flags, btree_id::XATTRS, level);
        b[24..32].copy_from_slice(&other.to_le_bytes());
    });
    refused(&img, "the xattrs btree's id in its header");
}

#[test]
fn a_node_whose_min_key_is_not_its_pointers_is_refused() {
    // min_key at 32: snapshot u32, offset u64 at +4, inode u64 at +12.
    let img = damaged("min-key", |b| {
        b[36..44].copy_from_slice(&1u64.to_le_bytes())
    });
    refused(&img, "a min_key its pointer does not have");
}

#[test]
fn a_node_whose_max_key_is_not_its_pointers_position_is_refused() {
    // max_key at 52: its inode field at +12.
    let img = damaged("max-key", |b| {
        b[64..72].copy_from_slice(&1u64.to_le_bytes())
    });
    refused(&img, "a max_key short of its pointer's position");
}

#[test]
fn a_key_format_whose_field_overflows_is_refused() {
    // The format at 80: key_u64s, nr_fields, bits[6] at +2, field_offset[6]
    // u64 at +8. A 64-bit field (inode 0, offset 1) with bits set, given an
    // offset at the top of its range, unpacks past it.
    let img = damaged("format", |b| {
        let field = [0usize, 1]
            .into_iter()
            .find(|&i| b[80 + 2 + i] > 0)
            .expect("the root's format packs the inode or offset field");
        let at = 80 + 8 + field * 8;
        b[at..at + 8].copy_from_slice(&u64::MAX.to_le_bytes());
    });
    refused(&img, "a key format whose field offset overflows");
}

#[test]
fn keys_out_of_order_in_a_bset_are_refused() {
    // The bsets are walked as `Node::parse` walks them: the first one's
    // header at 136, its keys 24 bytes on (its length in u64s at +22), each
    // later one on a block boundary, 16 bytes in. The first two keys of the
    // first bset holding two are swapped, whatever their lengths: the
    // fixtures are rebuilt on every run, and how the dirents root's keys
    // fall into bsets differs from one build to the next.
    let block = {
        let dev = FileDevice::open(fixture("default.img")).unwrap();
        (usize::from(Superblock::read(&dev).unwrap().block_size) * 512).max(512)
    };
    let img = damaged("key-order", move |b| {
        let seq = le64_at(b, 136);
        let mut at = 136usize;
        while at + 24 <= b.len() && le64_at(b, at) == seq {
            let keys = at + 24;
            let end = keys + usize::from(u16::from_le_bytes([b[at + 22], b[at + 23]])) * 8;
            if keys < end && b[keys] > 0 {
                let q = keys + usize::from(b[keys]) * 8;
                if q < end && b[q] > 0 {
                    let r = q + usize::from(b[q]) * 8;
                    let (first, second) = (b[keys..q].to_vec(), b[q..r].to_vec());
                    b[keys..keys + second.len()].copy_from_slice(&second);
                    b[keys + second.len()..r].copy_from_slice(&first);
                    return;
                }
            }
            at = end.div_ceil(block) * block + 16;
        }
        panic!("no bset of the dirents root holds two keys");
    });
    refused(&img, "two keys out of order in a bset");
}
