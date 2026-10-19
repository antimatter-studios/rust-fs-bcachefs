//! The encrypted `--wide_macs` fixture (#334, part of #276).
//!
//! `encrypted.img` and `encrypted-compressed.img` carry only the default
//! 80-bit data MAC. S1 9.1.4.3 says the `wide_macs` option keeps all 128
//! bits instead. `encrypted-wide-macs.img` was formatted `--encrypted
//! --no_passphrase --wide_macs` from the fixture tree, had a random file
//! written through the reference mount, and was settled. These tests check
//! every data extent there carries the 128-bit MAC, so the read built on it
//! (#335) cannot pass on an image that never exercises it.

mod common;

use common::{fixture, fixtures_dir, manifest, read_text};
use fs_bcachefs::bkey;
use fs_bcachefs::btree;
use fs_bcachefs::extent::DataExtent;
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::Filesystem;
use fs_core::FileDevice;
use sha2::{Digest, Sha256};

/// The 128-bit Poly1305 MAC S1 9.1.4.3 names for `wide_macs`, by the
/// lister's name and by its checksum type.
const WIDE_MAC: &str = "chacha20_poly1305_128";
const CSUM_WIDE_MAC: u8 = 4;

/// Every data extent of the image: its first pointer with the crc entry
/// before it.
fn data_extents() -> Vec<DataExtent> {
    let dev = FileDevice::open(fixture("encrypted-wide-macs.img")).unwrap();
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
fn every_data_extent_of_the_wide_macs_image_carries_a_128_bit_mac() {
    let record = read_text("encrypted-wide-macs.txt");
    let ours = data_extents();
    assert!(
        !ours.is_empty(),
        "encrypted-wide-macs.img has no data extents:\n{record}"
    );
    for e in &ours {
        let csum = e.crc.map(|c| c.csum_type);
        assert_eq!(
            csum,
            Some(CSUM_WIDE_MAC),
            "extent at {}: checksum type (encrypted-wide-macs.txt has each step)",
            e.file_start
        );
    }
    let text = read_text("encrypted-wide-macs.extents.txt");
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
            l.contains(&format!("csum {WIDE_MAC} ")),
            "the lister's crc entry does not name {WIDE_MAC}: {l}"
        );
    }
}

/// The reference checker passed the image and the reference mount read
/// it: the manifest the later read compares against is the reference's.
#[test]
fn the_reference_checked_and_read_the_wide_macs_image() {
    let record = read_text("encrypted-wide-macs.txt");
    let fsck = read_text("encrypted-wide-macs.fsck.txt");
    assert!(
        !fsck.contains("fsck: exit"),
        "the reference checker did not pass encrypted-wide-macs.img:\n{fsck}\n{record}"
    );
    assert!(
        fixtures_dir().join("encrypted-wide-macs.json").exists(),
        "the reference implementation did not read encrypted-wide-macs.img:\n{record}"
    );
}

/// Every file the reference mount recorded reads back from the image byte
/// for byte (its SHA-256), each verified against its full 128-bit MAC
/// first (#335): the tag's 16 bytes are the crc entry's low then high
/// checksum word, little-endian.
#[test]
fn every_file_of_the_wide_macs_image_reads_back_as_the_reference_manifest() {
    let record = read_text("encrypted-wide-macs.txt");
    let img = fixture("encrypted-wide-macs.img");
    let fs = Filesystem::open(FileDevice::open(&img).unwrap())
        .unwrap_or_else(|e| panic!("{}: {e}\n{record}", img.display()));
    let mut files = 0;
    for e in manifest("encrypted-wide-macs") {
        let ino = fs
            .lookup(&e.path)
            .unwrap_or_else(|err| panic!("{}: {err}", e.path));
        let i = fs.inode(ino).unwrap();
        match e.kind.as_str() {
            "dir" => assert!(i.is_dir(), "{}: not a directory", e.path),
            "symlink" => {
                let target = fs
                    .read(ino)
                    .unwrap_or_else(|err| panic!("{}: {err}", e.path));
                assert_eq!(
                    target,
                    e.target.as_deref().unwrap().as_bytes(),
                    "{}: symlink target",
                    e.path
                );
            }
            "file" => {
                let data = fs
                    .read(ino)
                    .unwrap_or_else(|err| panic!("{}: {err}", e.path));
                assert_eq!(Some(data.len() as u64), e.size, "{}: size", e.path);
                assert_eq!(
                    Some(format!("{:x}", Sha256::digest(&data))),
                    e.sha256,
                    "{}: contents",
                    e.path
                );
                files += 1;
            }
            k => panic!("{}: kind {k}", e.path),
        }
    }
    assert!(files > 0, "the manifest names no files:\n{record}");
}
