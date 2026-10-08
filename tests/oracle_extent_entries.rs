//! Extent entries beyond ptr, crc32, crc64 and reconcile (#52), on the
//! images the reference made to hold them (scripts/guest-build-fixtures.sh,
//! "EXTENT ENTRIES"):
//!
//! - `crc128`: encoded extents up to 1M, so extents outgrow the 512
//!   sectors a crc64 entry holds. Every file reads back to the manifest's
//!   SHA-256.
//! - `poison`: one data sector corrupted, then read and moved through the
//!   reference mount, which marks the extent poisoned (S1 5.5.5). The
//!   poisoned file is refused as the reference refuses it, with an I/O
//!   error, and the file beside it reads.
//!
//! Each test first checks that its fixture holds the entry kind at all, so
//! a formatter that stops making one fails here rather than passing on
//! images that never exercise the decoder.

mod common;

use common::{fixture, manifest, read_text};
use fs_bcachefs::{Error, Filesystem};
use fs_core::FileDevice;
use sha2::{Digest, Sha256};

/// The lister's lines for the first extent key of inode `ino` and the
/// entry lines indented under it, for a failure message.
fn lister_view(listing: &str, ino: u64) -> String {
    let key = format!(" {ino}:");
    listing
        .lines()
        .skip_while(|l| !(l.starts_with("u64s") && l.contains(&key)))
        .enumerate()
        .take_while(|(i, l)| *i == 0 || l.starts_with(' '))
        .map(|(_, l)| l)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn every_file_with_crc128_entries_reads_back_to_the_manifest() {
    let listing = read_text("crc128.extents.txt");
    let entries = listing
        .lines()
        .filter(|l| l.trim_start().starts_with("crc128:"))
        .count();
    assert!(
        entries > 0,
        "the crc128 fixture holds no crc128 entry (crc128.extents.txt): the formatter no longer \
         makes one with these options"
    );
    let fs = Filesystem::open(FileDevice::open(fixture("crc128.img")).unwrap()).unwrap();
    let mut files = 0;
    for e in manifest("crc128") {
        if e.kind != "file" {
            continue;
        }
        let ino = fs.lookup(&e.path).unwrap();
        let data = fs.read(ino).unwrap_or_else(|err| {
            panic!(
                "{}: {err}\nthe lister's view of inode {ino} (crc128.extents.txt):\n{}",
                e.path,
                lister_view(&listing, ino)
            )
        });
        assert_eq!(
            Some(format!("{:x}", Sha256::digest(&data))),
            e.sha256,
            "{}",
            e.path
        );
        files += 1;
    }
    assert!(files > 300, "only {files} files");
    println!("crc128: {files} files read, {entries} crc128 entries in the listing");
}

#[test]
fn a_poisoned_extent_is_refused_with_an_io_error_and_the_file_beside_it_reads() {
    let listing = read_text("poison.extents.txt");
    assert!(
        listing.to_lowercase().contains("poison"),
        "the reference marked no extent poisoned (poison.extents.txt; poison.txt has each step)"
    );
    let fs = Filesystem::open(FileDevice::open(fixture("poison.img")).unwrap()).unwrap();
    let victim = fs.lookup("/victim").unwrap();
    match fs.read(victim) {
        Err(Error::Io(m)) if m.contains("poisoned") => {}
        other => panic!(
            "/victim: {:?}\nthe lister's view of inode {victim} (poison.extents.txt):\n{}",
            other.map(|d| d.len()),
            lister_view(&listing, victim)
        ),
    }
    let intact = fs.read(fs.lookup("/intact").unwrap()).unwrap();
    assert_eq!(intact, b"an intact file\n".repeat(100));
}
