//! The encrypted unclean fixture (#337, part of #277).
//!
//! `aged-unclean.img` is the only image left without a clean shutdown, and
//! its journal is plaintext. An encrypted filesystem shut down uncleanly
//! needs its journal replayed too, and its journal entries are encrypted.
//! `encrypted-unclean.img` was formatted `--encrypted --no_passphrase` from
//! the fixture tree, written to through the reference mount, and killed as
//! `aged-unclean` is, so its newest keys are in encrypted journal entries
//! only. These tests check it is that image, so the journal decryption
//! (#338) and the replay (#339) built on it cannot pass on an image that
//! never exercises them.

mod common;

use common::{fixture, fixtures_dir, read_text};
use fs_bcachefs::superblock::Superblock;
use fs_core::FileDevice;

/// The reference superblock printer's `Clean:` value.
fn printed_clean(text: &str) -> Option<&str> {
    text.lines()
        .map(str::trim)
        .find_map(|l| l.strip_prefix("Clean:"))
        .map(str::trim)
}

#[test]
fn the_encrypted_unclean_image_is_marked_unclean() {
    let record = read_text("encrypted-unclean.txt");
    let dev = FileDevice::open(fixture("encrypted-unclean.img")).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    assert!(
        !sb.is_clean(),
        "encrypted-unclean.img is marked clean by this crate; it was meant to be the unreplayed one:\n{record}"
    );
    let text = read_text("encrypted-unclean.super.txt");
    assert_eq!(
        printed_clean(&text),
        Some("0"),
        "the reference superblock printer does not call encrypted-unclean.img unclean:\n{text}\n{record}"
    );
}

/// The reference journal lister read the encrypted journal, the reference
/// checker replayed it, and the reference mount read the replayed image:
/// the manifest the replay test compares against is the reference's.
#[test]
fn the_reference_listed_and_replayed_the_encrypted_unclean_journal() {
    let record = read_text("encrypted-unclean.txt");
    for step in ["list_journal", "replay: exit", "mount: failed"] {
        assert!(
            !record.contains(step),
            "a reference step failed on encrypted-unclean.img ({step}):\n{record}"
        );
    }
    let headers = read_text("encrypted-unclean.journal-headers.txt");
    assert!(
        headers.lines().any(|l| !l.trim().is_empty()),
        "the reference journal lister printed no entries for encrypted-unclean.img:\n{record}"
    );
    let dirty = read_text("encrypted-unclean.journal-dirty.txt");
    assert!(
        dirty.lines().any(|l| !l.trim().is_empty()),
        "the reference journal lister printed no keys to replay for encrypted-unclean.img:\n{record}"
    );
    for json in ["encrypted-unclean.mounted.json", "encrypted-unclean.json"] {
        assert!(
            fixtures_dir().join(json).exists(),
            "the reference implementation did not record {json}:\n{record}"
        );
    }
}
