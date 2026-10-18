//! The encrypted, compressed fixture (#329, part of #275).
//!
//! `encrypted.img`'s data extents are all uncompressed and start at their
//! data's start, so an extent nonce's compression type, uncompressed size
//! and per-crc nonce offset (S1 9.1.4.1) are never observed there.
//! `encrypted-compressed.img` was formatted `--encrypted --no_passphrase
//! --compression=lz4`, written through the reference mount, settled, and
//! then partly overwritten through the mount, which splits an extent and
//! leaves its later part with a crc offset above 0 (as `crc128-overwrite`
//! did). These tests check the image holds both shapes, so the reads built
//! on it (#330, #331) cannot pass on an image that never exercises them.

mod common;

use common::{fixture, fixtures_dir, read_text};
use fs_bcachefs::bkey;
use fs_bcachefs::btree;
use fs_bcachefs::crypt;
use fs_bcachefs::extent::{compression, DataExtent};
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::Error;
use fs_core::{BlockRead, FileDevice};
use std::collections::HashMap;

/// The 80-bit Poly1305 MAC every encrypted data extent carries (S4,
/// docs/clean-room.md), by the lister's name and by its checksum type.
const DATA_MAC: &str = "chacha20_poly1305_80";
const CSUM_DATA_MAC: u8 = 3;

/// Every data extent of the image: its first pointer with the crc entry
/// before it.
fn data_extents() -> Vec<DataExtent> {
    let dev = FileDevice::open(fixture("encrypted-compressed.img")).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    btree::walk(&dev, &sb, btree::btree_id::EXTENTS)
        .unwrap()
        .into_iter()
        .filter(|k| k.key_type == bkey::key_type::EXTENT)
        .map(|k| DataExtent::from_key(&k).unwrap_or_else(|e| panic!("{}: {e}", k.pos)))
        .collect()
}

/// The lister's crc entries, one line each.
fn lister_crc_lines(text: &str) -> Vec<&str> {
    text.lines()
        .map(str::trim)
        .filter(|l| l.starts_with("crc32:") || l.starts_with("crc64:") || l.starts_with("crc128:"))
        .collect()
}

#[test]
fn the_image_holds_a_compressed_and_a_partly_overwritten_encrypted_extent() {
    let record = read_text("encrypted-compressed.txt");
    let ours = data_extents();
    assert!(
        !ours.is_empty(),
        "encrypted-compressed.img has no data extents:\n{record}"
    );
    for e in &ours {
        let csum = e.crc.map(|c| c.csum_type);
        assert_eq!(
            csum,
            Some(CSUM_DATA_MAC),
            "extent at {}: checksum type",
            e.file_start
        );
    }
    let text = read_text("encrypted-compressed.extents.txt");
    let lines = lister_crc_lines(&text);
    assert_eq!(
        lines.len(),
        ours.len(),
        "crc entries: the lister printed {}, this crate read {}",
        lines.len(),
        ours.len()
    );
    for l in &lines {
        assert!(
            l.contains(&format!("csum {DATA_MAC} ")),
            "the lister's crc entry does not name {DATA_MAC}: {l}"
        );
    }
    let compressed = ours.iter().filter_map(|e| e.crc).any(|c| {
        c.compression_type != compression::NONE && c.compressed_size < c.uncompressed_size
    });
    assert!(
        compressed,
        "no encrypted extent is stored compressed (encrypted-compressed.extents.txt; \
         encrypted-compressed.txt has each step)"
    );
    let offset = ours.iter().filter_map(|e| e.crc).any(|c| c.offset > 0);
    let listed_offset = lines
        .iter()
        .any(|l| l.contains(" offset ") && !l.contains(" offset 0 "));
    assert!(
        offset && listed_offset,
        "no encrypted crc entry with an offset above 0 (this crate: {offset}, the lister: \
         {listed_offset}; encrypted-compressed.txt has each step)"
    );
}

/// The reference checker passed the image and the reference mount read
/// it: the manifest the later reads compare against is the reference's.
#[test]
fn the_reference_checked_and_read_the_image() {
    let record = read_text("encrypted-compressed.txt");
    let fsck = read_text("encrypted-compressed.fsck.txt");
    assert!(
        !fsck.contains("fsck: exit"),
        "the reference checker did not pass encrypted-compressed.img:\n{fsck}\n{record}"
    );
    assert!(
        fixtures_dir().join("encrypted-compressed.json").exists(),
        "the reference implementation did not read encrypted-compressed.img:\n{record}"
    );
}

/// The version of every extent key, as the lister printed it (`ver N`),
/// by the key's position.
fn lister_versions(text: &str) -> HashMap<String, u64> {
    let mut out = HashMap::new();
    for l in text.lines() {
        let w: Vec<&str> = l.split_whitespace().collect();
        let pos = w.iter().position(|x| *x == "extent").map(|i| w[i + 1]);
        let ver = w.iter().position(|x| *x == "ver").map(|i| w[i + 1]);
        if let (Some(pos), Some(ver)) = (pos, ver) {
            out.insert(pos.to_string(), ver.parse().unwrap());
        }
    }
    out
}

/// An incompressible extent (#356, part of #330) is encrypted with the
/// nonce an uncompressed one is: its MAC, which the reference
/// implementation wrote, verifies under it, and a wrong nonce would match
/// an 80-bit MAC with probability 2^-80. An LZ4 extent's nonce has not
/// been found, so it is still refused, naming its compression type.
#[test]
fn incompressible_encrypted_extents_verify_with_the_uncompressed_nonce() {
    let dev = FileDevice::open(fixture("encrypted-compressed.img")).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let key = sb
        .crypt()
        .unwrap()
        .and_then(|c| c.master_key())
        .expect("encrypted-compressed.img stores its master key unsealed");
    let versions = lister_versions(&read_text("encrypted-compressed.extents.txt"));
    let (mut incompressible, mut lz4) = (0, 0);
    for k in btree::walk(&dev, &sb, btree::btree_id::EXTENTS).unwrap() {
        if k.key_type != bkey::key_type::EXTENT {
            continue;
        }
        let e = DataExtent::from_key(&k).unwrap();
        let c = e.crc.expect("every extent of the image has a crc entry");
        let pos = k.pos.to_string();
        let ver = *versions
            .get(&pos)
            .unwrap_or_else(|| panic!("{pos}: the lister printed no version"));
        let mut data = vec![0u8; c.compressed_size as usize * 512];
        dev.read_at(e.ptr.offset * 512, &mut data).unwrap();
        let got = crypt::decrypt_extent(&key, &c, 0, ver, &mut data);
        match c.compression_type {
            compression::INCOMPRESSIBLE => {
                got.unwrap_or_else(|err| panic!("{pos} (version {ver}): {err}"));
                incompressible += 1;
            }
            compression::LZ4 => match got {
                Err(Error::Unsupported(m)) if m.contains("compression type") => lz4 += 1,
                other => panic!("{pos}: an LZ4 extent was not refused: {other:?}"),
            },
            t => panic!("{pos}: compression type {t}, which the image was not built with"),
        }
    }
    assert!(
        incompressible > 0 && lz4 > 0,
        "the image must hold both: {incompressible} incompressible, {lz4} LZ4 extents"
    );
}
