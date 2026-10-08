//! A btree node's header flags (#80): where the btree id and the level
//! go, for every id the reference writes, 16 and above included. Every
//! node of every btree of every clean fixture and write-study image is
//! read, and its flags checked against the layout this crate infers; a
//! mismatch prints every distinct `(btree id, level, flags)` seen.

mod common;

use std::collections::BTreeSet;

use common::fixture;
use fs_bcachefs::bkey::{self, BkeyFormat};
use fs_bcachefs::btree::{self, NodePtr};
use fs_bcachefs::superblock::Superblock;
use fs_core::{BlockRead, FileDevice};

/// Every `.img` in the fixtures and the write study whose superblock
/// records its roots (a cleanly shut down image), except the refused sets:
/// the encrypted one and the two members of the two-device filesystem,
/// which this reader declines by design.
fn clean_images() -> Vec<String> {
    let mut out = Vec::new();
    for sub in ["", "write-study/"] {
        for e in std::fs::read_dir(fixture(sub)).unwrap() {
            let name = e.unwrap().file_name().to_string_lossy().into_owned();
            if let Some(stem) = name
                .strip_suffix(".img")
                .filter(|s| *s != "encrypted" && !s.starts_with("multi-"))
            {
                let path = format!("{sub}{stem}");
                let dev = FileDevice::open(fixture(&format!("{path}.img"))).unwrap();
                if Superblock::read(&dev).is_ok_and(|sb| sb.btree_roots().is_ok()) {
                    out.push(path);
                }
            }
        }
    }
    out.sort();
    out
}

/// `(btree id, level, flags)` of every node of every btree in `image`.
fn node_flags(image: &str) -> BTreeSet<(u8, u8, u64)> {
    let dev = FileDevice::open(fixture(&format!("{image}.img"))).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let unpacked = BkeyFormat {
        key_u64s: 5,
        nr_fields: 6,
        bits: [0; 6],
        field_offset: [0; 6],
    };
    let mut out = BTreeSet::new();
    for root in sb.btree_roots().unwrap() {
        let mut todo = vec![(
            root.level,
            NodePtr::from_key(&bkey::decode(&root.key, &unpacked).unwrap()).unwrap(),
        )];
        while let Some((level, ptr)) = todo.pop() {
            let node = match btree::read_node(&dev as &dyn BlockRead, &sb, &ptr) {
                Ok(n) => n,
                Err(e) => panic!("{image} btree {}: {e}", root.btree_id),
            };
            out.insert((root.btree_id, level, node.flags));
            if level > 0 {
                for k in node
                    .keys
                    .iter()
                    .filter(|k| k.key_type == bkey::key_type::BTREE_PTR_V2)
                {
                    todo.push((level - 1, NodePtr::from_key(k).unwrap()));
                }
            }
        }
    }
    out
}

/// HYPOTHESIS, checked here: the id's low four bits are bits 0..4, the
/// level bits 4..8, and the id's higher bits start at bit 9.
fn fits(id: u8, level: u8, flags: u64) -> bool {
    flags & 0xf == u64::from(id & 0xf)
        && (flags >> 4) & 0xf == u64::from(level)
        && (flags >> 9) & 0xf == u64::from(id >> 4)
}

#[test]
fn every_node_carries_its_btree_id_and_level_in_its_flags() {
    let mut seen = BTreeSet::new();
    for image in clean_images() {
        seen.extend(node_flags(&image));
    }
    let high: BTreeSet<u8> = seen.iter().map(|s| s.0).filter(|&id| id >= 16).collect();
    let table: Vec<String> = seen
        .iter()
        .map(|(id, level, f)| format!("id {id:2} level {level} flags {f:#018x}"))
        .collect();
    assert!(
        high.len() >= 2,
        "only ids {high:?} of 16 and above were seen:\n{}",
        table.join("\n")
    );
    let misfits: Vec<&(u8, u8, u64)> = seen.iter().filter(|(i, l, f)| !fits(*i, *l, *f)).collect();
    assert!(
        misfits.is_empty(),
        "{} of {} do not fit; every (id, level, flags) seen:\n{}",
        misfits.len(),
        seen.len(),
        table.join("\n")
    );
}
