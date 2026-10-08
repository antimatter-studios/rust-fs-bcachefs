//! Where the reference stops storing a file inline (#79): the write study's
//! `inline-*` images hold one file of every size from 1 to 2100 bytes,
//! written through the reference mount (scripts/guest-write-study.sh). The
//! mount's own `stat` gives each file's inode and size, and the reference
//! lister's extents dump gives each inode's key types, so the limit is read
//! without this crate's reader. The writer's [`INLINE_MAX`] must be it.

#![cfg(feature = "write")]

mod common;

use std::collections::{BTreeMap, BTreeSet};

use common::{fixture, read_text};
use fs_bcachefs::write::INLINE_MAX;
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

/// The largest size the reference stored inline, checked to be a clean
/// threshold: every file up to it inline only, every file past it without
/// inline data.
fn observed_limit(image: &str) -> usize {
    let types = key_types(image);
    let mut by_size: Vec<(usize, bool)> = sizes(image)
        .iter()
        .filter(|(n, _)| n.starts_with('s'))
        .map(|(n, &(ino, size))| {
            let t = types
                .get(&ino)
                .unwrap_or_else(|| panic!("{image}: {n} (inode {ino}) has no extents key"));
            let inline = t.contains("inline_data");
            assert!(
                !inline || t.len() == 1,
                "{image}: {n} mixes inline data with {t:?}"
            );
            (size, inline)
        })
        .collect();
    by_size.sort();
    assert_eq!(by_size.len(), 2100, "{image}: one file per size, 1 to 2100");
    let limit = by_size
        .iter()
        .take_while(|(_, inline)| *inline)
        .last()
        .map_or(0, |&(s, _)| s);
    let past: Vec<usize> = by_size
        .iter()
        .filter(|&&(s, inline)| s > limit && inline)
        .map(|&(s, _)| s)
        .collect();
    assert!(
        past.is_empty(),
        "{image}: inline up to {limit} bytes, then inline again at {past:?}"
    );
    limit
}

#[test]
fn the_writer_stores_inline_exactly_what_the_reference_does() {
    for image in IMAGES {
        let limit = observed_limit(image);
        assert_eq!(
            INLINE_MAX, limit,
            "{image}: the reference stored files of up to {limit} bytes inline"
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
