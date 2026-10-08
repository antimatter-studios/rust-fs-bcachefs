//! Every field of every inode (#81): the varints after `dev` hold per-inode
//! options, and bits 32..35 of the flags word are copied by the writer
//! unread. The write study's `options` image has options set through the
//! reference mount (scripts/guest-write-study.sh); every inode of it and of
//! every other dumped image is decoded here and compared, field by field,
//! with the reference lister's `bi_<name>=value` lines.

mod common;

use std::collections::BTreeMap;

use common::{fixture, read_text};
use fs_bcachefs::btree::{self, btree_id};
use fs_bcachefs::inode::InodeV3Raw;
use fs_bcachefs::superblock::Superblock;
use fs_core::FileDevice;

/// The varint fields after the four times (two varints each), in the
/// order the lister prints them.
const NAMES: &[&str] = &[
    "uid",
    "gid",
    "nlink",
    "generation",
    "dev",
    "data_checksum",
    "compression",
    "project",
    "background_compression",
    "data_replicas",
    "promote_target",
    "foreground_target",
    "background_target",
    "erasure_code",
    "fields_set",
    "dir",
    "dir_offset",
    "subvol",
    "parent_subvol",
    "nocow",
    "depth",
    "inodes_32bit",
    "casefold",
    "unused_ec_max_data_blocks",
];

/// Every `<image>.inodes.txt` beside a clean `<image>.img`.
fn dumps() -> Vec<String> {
    let mut out = Vec::new();
    for sub in ["", "write-study/"] {
        for e in std::fs::read_dir(fixture(sub)).unwrap() {
            let name = e.unwrap().file_name().to_string_lossy().into_owned();
            let Some(stem) = name.strip_suffix(".inodes.txt") else {
                continue;
            };
            let img = common::fixtures_dir().join(format!("{sub}{stem}.img"));
            if img.exists()
                && Superblock::read(&FileDevice::open(&img).unwrap())
                    .is_ok_and(|sb| sb.btree_roots().is_ok())
            {
                out.push(format!("{sub}{stem}"));
            }
        }
    }
    out.sort();
    out
}

/// `inode -> (field -> value)` as the lister printed it.
fn lister(image: &str) -> BTreeMap<u64, BTreeMap<String, u64>> {
    let mut out: BTreeMap<u64, BTreeMap<String, u64>> = BTreeMap::new();
    let mut ino = None;
    for l in read_text(&format!("{image}.inodes.txt")).lines() {
        if l.starts_with("u64s ") {
            ino = (l.split_whitespace().nth(3) == Some("inode_v3"))
                .then(|| l.split_whitespace().nth(4))
                .flatten()
                .and_then(|p| p.split(':').nth(1))
                .and_then(|n| n.parse().ok());
        } else if let (Some(i), Some((k, v))) = (ino, l.trim().split_once('=')) {
            if let (Some(name), Ok(v)) = (k.strip_prefix("bi_"), v.parse()) {
                out.entry(i).or_default().insert(name.to_string(), v);
            } else if k == "flags" {
                let hex = v.trim_matches(|c| c == '(' || c == ')');
                let f = u64::from_str_radix(hex, 16).unwrap();
                out.entry(i).or_default().insert("flags".into(), f);
            }
        }
    }
    out
}

/// `inode -> decoded value` from this crate.
fn ours(image: &str) -> BTreeMap<u64, InodeV3Raw> {
    let dev = FileDevice::open(fixture(&format!("{image}.img"))).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    btree::walk(&dev, &sb, btree_id::INODES)
        .unwrap()
        .into_iter()
        .filter(|k| k.key_type == fs_bcachefs::bkey::key_type::INODE_V3)
        .map(|k| (k.pos.offset, InodeV3Raw::parse(&k.value).unwrap()))
        .collect()
}

#[test]
fn the_options_were_set_through_the_reference_mount() {
    let text = read_text("write-study/options.txt");
    assert!(
        !text.contains("error") && text.lines().count() >= 5,
        "options.txt:\n{text}"
    );
}

#[test]
fn every_varint_field_decodes_as_the_lister_prints_it() {
    let mut nonzero_options = 0;
    for image in dumps() {
        let theirs = lister(&image);
        for (ino, raw) in ours(&image) {
            let fields = &theirs[&ino];
            for (i, name) in NAMES.iter().enumerate() {
                let v = raw.varints.get(8 + i).copied().unwrap_or(0);
                assert_eq!(
                    Some(&v),
                    fields.get(*name),
                    "{image} inode {ino}: bi_{name}"
                );
                if (5..15).contains(&i) && v != 0 {
                    nonzero_options += 1;
                }
            }
        }
    }
    assert!(
        nonzero_options >= 3,
        "only {nonzero_options} option fields set"
    );
}

/// HYPOTHESIS, checked here: bits 32..35 of the flags word are 3 in every
/// inode (open question 12), options or not. The lister prints only the
/// low 32 bits, which must match this crate's.
#[test]
fn flag_bits_32_to_35_are_3_in_every_inode() {
    let mut other = Vec::new();
    for image in dumps() {
        let theirs = lister(&image);
        for (ino, raw) in ours(&image) {
            assert_eq!(
                raw.flags & 0xffff_ffff,
                theirs[&ino]["flags"],
                "{image} inode {ino}: the low 32 flag bits"
            );
            if (raw.flags >> 32) & 0xf != 3 {
                other.push(format!("{image} inode {ino}: flags {:#x}", raw.flags));
            }
        }
    }
    assert!(other.is_empty(), "{}", other.join("\n"));
}
