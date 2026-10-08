//! Where the reference stores a file's data (#79): the write study's
//! `inline-*` images hold one file of every size from 1 to 2100 bytes,
//! written through the reference mount (scripts/guest-write-study.sh), on
//! 512- and 4096-byte blocks. The mount's own `stat` gives each file's
//! inode and size, and the reference lister's extents dump gives each
//! inode's key types, so what the reference did is read without this
//! crate's reader. The writer's [`data_layout`] must choose the same for
//! every size.

#![cfg(feature = "write")]

mod common;

use std::collections::{BTreeMap, BTreeSet};

use common::{fixture, parse_size, read_text};
use fs_bcachefs::write::{data_layout, inline_max};
use fs_bcachefs::Filesystem;
use fs_core::FileDevice;

const IMAGES: &[&str] = &["inline-default", "inline-bs4k"];

/// `name -> (inode, size)`, from the mount's `stat -c '%i %s %n'`.
fn sizes(image: &str) -> BTreeMap<String, (u64, usize)> {
    read_text(&format!("write-study/{image}.sizes.txt"))
        .lines()
        .map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            (
                w[2].to_string(),
                (w[0].parse().unwrap(), w[1].parse().unwrap()),
            )
        })
        .collect()
}

/// `inode -> the key types the lister shows for it` in the extents btree.
fn key_types(image: &str) -> BTreeMap<u64, BTreeSet<String>> {
    let mut out: BTreeMap<u64, BTreeSet<String>> = BTreeMap::new();
    for l in read_text(&format!("write-study/{image}.extents.txt"))
        .lines()
        .filter(|l| l.starts_with("u64s "))
    {
        // u64s N type T INODE:OFFSET:SNAPSHOT len L ...
        let w: Vec<&str> = l.split_whitespace().collect();
        let ino = w[4].split(':').next().unwrap().parse().unwrap();
        out.entry(ino).or_default().insert(w[3].to_string());
    }
    out
}

/// The image's block size in bytes, as the superblock printer shows it.
fn block_bytes(image: &str) -> usize {
    let text = read_text(&format!("write-study/{image}.super.txt"));
    let v = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("block_size:"))
        .unwrap_or_else(|| panic!("{image}: no block_size line"));
    parse_size(v.trim()) as usize
}

#[test]
fn the_writer_lays_out_every_size_as_the_reference_did() {
    for image in IMAGES {
        let block = block_bytes(image);
        let types = key_types(image);
        let mut files = 0;
        let mut largest_inline = 0;
        for (name, &(ino, size)) in sizes(image).iter().filter(|(n, _)| n.starts_with('s')) {
            let t = types
                .get(&ino)
                .unwrap_or_else(|| panic!("{image}: {name} (inode {ino}) has no extents key"));
            let seen = (t.contains("extent"), t.contains("inline_data"));
            assert_eq!(
                t.len(),
                usize::from(seen.0) + usize::from(seen.1),
                "{image} {name}: {t:?}"
            );
            let (extents, inline) = data_layout(size, block);
            assert_eq!(
                (extents > 0, inline > 0),
                seen,
                "{image} {name}: the writer would put {extents} bytes in extents and {inline} inline; the reference keyed {t:?}"
            );
            if !seen.0 {
                largest_inline = largest_inline.max(size);
            }
            files += 1;
        }
        assert_eq!(files, 2100, "{image}: one file per size, 1 to 2100");
        assert_eq!(
            largest_inline,
            inline_max(block),
            "{image}: {block}-byte blocks"
        );
    }
}

#[test]
fn a_file_grown_past_the_limit_holds_no_inline_data() {
    for image in IMAGES {
        let (ino, size) = sizes(image)["grow"];
        assert_eq!(size, 3000, "{image}");
        let t = &key_types(image)[&ino];
        assert!(!t.contains("inline_data"), "{image}: grow keeps {t:?}");
    }
}

#[test]
fn every_file_reads_back_at_its_size() {
    for image in IMAGES {
        let fs = Filesystem::open(
            FileDevice::open(fixture(&format!("write-study/{image}.img"))).unwrap(),
        )
        .unwrap_or_else(|e| panic!("{image}: {e}"));
        for (name, &(_, size)) in &sizes(image) {
            let data = fs
                .read(fs.lookup(&format!("/d/{name}")).unwrap())
                .unwrap_or_else(|e| panic!("{image} {name}: {e}"));
            let fill = if name == "grow" { b'g' } else { b'x' };
            assert_eq!(data, vec![fill; size], "{image} {name}");
        }
    }
}
