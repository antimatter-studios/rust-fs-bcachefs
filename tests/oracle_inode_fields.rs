//! Every field of every inode (#81): the varints after `dev` hold per-inode
//! options, and bits 32..35 of the flags word are copied by the writer
//! unread. The write study tries to set options through the reference
//! mount (`options`) and its offline key editor (`kvdb-options`), and
//! records that both refuse (scripts/guest-write-study.sh); every inode of
//! every dumped image is decoded here and compared, field by field, with
//! the reference lister's `bi_<name>=value` lines.

mod common;

use std::collections::BTreeMap;

use common::{fixture, read_text};
use fs_bcachefs::btree::{self, btree_id};
use fs_bcachefs::inode::{InodeV3Raw, FIELD_NAMES};
use fs_bcachefs::superblock::Superblock;
use fs_core::FileDevice;

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
                // `(15300000)`, or with the names of the set flags first:
                // `has_case_insensitive(15300400)` (the casefold set).
                let hex = v.rsplit('(').next().unwrap().trim_end_matches(')');
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

/// OBSERVED: the reference mount refuses every per-inode option, set as a
/// `bcachefs.*` xattr, with "Operation not supported" (the reference
/// tool's `set-file-option` goes the same way: the probe's casefold
/// attempt). So no image holds an inode with options, and the option
/// fields are checked only as zeros. A reference that takes them fails
/// this test, and the options image becomes the first to show them.
#[test]
fn the_reference_mount_refuses_per_inode_options() {
    let text = read_text("write-study/options.txt");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 5, "options.txt:\n{text}");
    assert!(
        lines
            .iter()
            .all(|l| l.starts_with("error ") && l.contains("Operation not supported")),
        "options.txt:\n{text}"
    );
}

/// OBSERVED: the reference tool's offline key editor, the other way S1
/// names to set an inode's fields, edits an inode only up to its fixed
/// header in the pinned release: asked for `bi_compression` it answers
/// that `bch_inode_v3` has no such field. An editor that takes it fails
/// this test, and the write study's `kvdb-options` step is where options
/// are first seen.
#[test]
fn the_reference_offline_editor_cannot_set_per_inode_options_either() {
    let text = read_text("write-study/kvdb-options.txt");
    let update = text
        .split("## update inodes ")
        .nth(1)
        .unwrap_or_else(|| panic!("kvdb-options.txt has no update step:\n{text}"));
    assert!(
        update.contains("bch_inode_v3 has no field 'bi_compression'"),
        "kvdb-options.txt:\n{text}"
    );
}

#[test]
fn every_varint_field_decodes_as_the_lister_prints_it() {
    let mut nonzero = 0;
    for image in dumps() {
        let theirs = lister(&image);
        for (ino, raw) in ours(&image) {
            let fields = &theirs[&ino];
            for name in FIELD_NAMES {
                let v = raw.field(name).unwrap();
                assert_eq!(
                    Some(&v),
                    fields.get(*name),
                    "{image} inode {ino}: bi_{name}"
                );
                nonzero += usize::from(v != 0);
            }
        }
    }
    assert!(nonzero >= 1000, "only {nonzero} non-zero fields compared");
}

/// OBSERVED: bits 32..35 of the flags word are 3 in every inode of every
/// image (open question 12). The lister prints only the low 32 bits, which
/// must match this crate's.
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
