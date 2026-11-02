//! An encrypted filesystem's journal entries as this crate reads them
//! (#338, part of #277), compared with the reference journal lister's
//! listing of the same journal.
//!
//! `encrypted-unclean.img` was formatted `--encrypted --no_passphrase`,
//! written to through the reference mount and killed before a clean
//! shutdown (#337), so its journal holds entries a replay needs, every one
//! authenticated and encrypted under the master key its crypt field keeps
//! unsealed. `oracle_journal.rs` checks the same things of the plaintext
//! journal of `aged-unclean.img`.

mod common;

use common::{read_text, ScratchDir};
use fs_bcachefs::journal;
use fs_bcachefs::superblock::Superblock;
use fs_core::FileDevice;
use std::path::Path;

const SET: &str = "encrypted-unclean";
const IMG: &str = "encrypted-unclean.img";

/// `(seq, bytes, last_seq, flush, sector)` of every entry the reference
/// listed.
fn reference_headers() -> Vec<(u64, usize, u64, bool, u64)> {
    let text = read_text(&format!("{SET}.journal-headers.txt"));
    let mut out = Vec::new();
    let mut cur: Option<(u64, usize, u64, bool)> = None;
    for line in text.lines() {
        let t = line.trim();
        if let Some(n) = t.strip_prefix("journal entry") {
            cur = Some((n.trim().parse().unwrap(), 0, 0, false));
        } else if let (Some(c), Some(v)) = (cur.as_mut(), t.strip_prefix("bytes")) {
            c.1 = v.trim().parse().unwrap();
        } else if let (Some(c), Some(v)) = (cur.as_mut(), t.strip_prefix("last seq")) {
            c.2 = v.trim().parse().unwrap();
        } else if let (Some(c), Some(v)) = (cur.as_mut(), t.strip_prefix("flush")) {
            c.3 = v.trim() == "1";
        } else if let (Some(c), Some(v)) = (cur, t.strip_prefix("written at")) {
            let sector = v.rsplit("(sector ").next().unwrap().trim_end_matches(')');
            out.push((c.0, c.1, c.2, c.3, sector.parse().unwrap()));
            cur = None;
        }
    }
    assert!(
        !out.is_empty(),
        "the reference listed no journal entries of {IMG}:\n{}",
        read_text(&format!("{SET}.txt"))
    );
    out
}

/// The journal entries this crate reads from the image at `img`.
fn entries(img: &Path) -> Vec<journal::Jset> {
    let dev = FileDevice::open(img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    journal::read_entries(&dev, &sb).unwrap_or_else(|e| panic!("{}: {e}", img.display()))
}

/// The checksum type in the flags of the jset at `sector` of `img`.
fn csum_type(img: &[u8], sector: u64) -> u32 {
    let at = sector as usize * 512 + 36;
    u32::from_le_bytes(img[at..at + 4].try_into().unwrap()) & 0xf
}

/// Every entry the reference lists is encrypted: its checksum is the
/// 128-bit MAC (type 4, as every bset of an encrypted node), so the
/// comparisons below cannot pass on a journal this crate reads in the
/// clear.
#[test]
fn every_listed_entry_is_encrypted() {
    let img = std::fs::read(common::fixture(IMG)).unwrap();
    for (seq, _, _, _, sector) in reference_headers() {
        assert_eq!(
            csum_type(&img, sector),
            4,
            "entry {seq} at sector {sector} is not authenticated with a 128-bit MAC"
        );
    }
}

#[test]
fn every_encrypted_journal_entry_header_matches_the_reference() {
    let ours = entries(&common::fixture(IMG));
    let theirs = reference_headers();
    for (seq, bytes, last_seq, flush, sector) in &theirs {
        let j = ours.iter().find(|j| j.seq == *seq).unwrap_or_else(|| {
            panic!("entry {seq}: the reference lists it, this reader did not find it")
        });
        assert_eq!(
            (j.bytes, j.last_seq, j.flush, j.sector),
            (*bytes, *last_seq, *flush, *sector),
            "entry {seq}: (bytes, last seq, flush, sector)"
        );
    }
    let newest = theirs.iter().map(|h| h.0).max().unwrap();
    let oldest = theirs.iter().map(|h| h.0).min().unwrap();
    for j in &ours {
        // As in oracle_journal.rs: a ring also holds older entries the
        // reference does not list, and an unflushed tail after its newest
        // flush.
        assert!(
            theirs.iter().any(|h| h.0 == j.seq) || j.seq < oldest || (j.seq > newest && !j.flush),
            "entry {}: found by this reader, not listed by the reference, inside its range",
            j.seq
        );
    }
}

/// The keys of every `btree_keys` sub-entry in the entries a replay
/// applies, as `btree=<id> <pos> len <size>`: what the decrypted entries
/// hold, key for key, against what the reference decrypted.
#[test]
fn the_encrypted_entries_replayed_keys_are_the_references() {
    let text = read_text(&format!("{SET}.journal-dirty.txt"));
    let (from, to) = text
        .lines()
        .find_map(|l| l.split("replaying entries ").nth(1))
        .map(|r| {
            let r = r.split_whitespace().next().unwrap();
            let (a, b) = r.split_once('-').unwrap();
            (a.parse::<u64>().unwrap(), b.parse::<u64>().unwrap())
        })
        .expect("the reference names the entries it replays");
    let theirs: Vec<String> = text
        .lines()
        .filter_map(|l| l.trim().strip_prefix("btree_keys: btree="))
        .map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            // NAME level=L u64s N type T POS len L ...
            format!("{} {} len {}", w[0], w[6], w[8])
        })
        .collect();
    assert!(!theirs.is_empty(), "the reference lists no keys to replay");

    let mut ours = Vec::new();
    for j in entries(&common::fixture(IMG)) {
        if j.seq < from || j.seq > to {
            continue;
        }
        for e in j
            .entries
            .iter()
            .filter(|e| e.entry_type == journal::entry_type::BTREE_KEYS)
        {
            for k in &e.keys {
                ours.push(format!(
                    "{} {} len {}",
                    journal::btree_name(e.btree_id),
                    k.pos,
                    k.size
                ));
            }
        }
    }
    assert_eq!(ours.len(), theirs.len(), "keys in entries {from}..={to}");
    for (i, (a, b)) in ours.iter().zip(&theirs).enumerate() {
        assert_eq!(a, b, "key {i}");
    }
}

/// An entry whose MAC does not verify is not read as an entry, as one
/// whose checksum does not hold is not: a byte changed in its encrypted
/// body, or in the stored MAC itself. The same entries read from the
/// unchanged image.
#[test]
fn an_encrypted_entry_whose_mac_does_not_verify_is_not_read() {
    let theirs = reference_headers();
    // The newest two entries the replay applies, so a scan that stops at
    // the first damaged entry of a bucket still has read the others.
    let flushes: Vec<_> = theirs.iter().filter(|h| h.3).collect();
    assert!(flushes.len() >= 2, "fewer than two flush entries listed");
    let (body, mac) = (flushes[flushes.len() - 1], flushes[flushes.len() - 2]);
    let clean = entries(&common::fixture(IMG));
    for h in [body, mac] {
        assert!(clean.iter().any(|j| j.seq == h.0), "entry {} unread", h.0);
    }
    for (h, at, what) in [(body, 100usize, "its encrypted body"), (mac, 3, "its MAC")] {
        let dir = ScratchDir::new("encrypted-journal-mac");
        let path = dir.copy_fixture(IMG);
        let mut img = std::fs::read(&path).unwrap();
        img[h.4 as usize * 512 + at] ^= 0x20;
        std::fs::write(&path, &img).unwrap();
        assert!(
            !entries(&path).iter().any(|j| j.seq == h.0),
            "entry {} read with a byte of {what} changed",
            h.0
        );
    }
}
