//! The journal as this crate reads it, compared with the reference's
//! listing of the same journal (`list_journal`), and the uncleanly
//! unmounted image read through a replay of it, compared with what the
//! reference sees after its own replay.

mod common;

use common::{fixture, read_text};
use fs_bcachefs::journal;
use fs_bcachefs::superblock::Superblock;
use fs_core::FileDevice;

const SET: &str = "aged-unclean";

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
        out.len() > 100,
        "the reference listed only {} entries",
        out.len()
    );
    out
}

#[test]
fn every_journal_entry_header_matches_the_reference() {
    let dev = FileDevice::open(fixture(&format!("{SET}.img"))).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let ours = journal::read_entries(&dev, &sb).unwrap();
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
        // Beyond what the reference lists, a ring holds two kinds of entry
        // it does not: older ones its buckets still carry (measured in CI:
        // entry 106 beside a listing that starts at 110), and an unflushed
        // tail after the newest flush.
        assert!(
            theirs.iter().any(|h| h.0 == j.seq) || j.seq < oldest || (j.seq > newest && !j.flush),
            "entry {}: found by this reader, not listed by the reference, inside its range",
            j.seq
        );
    }
}

/// The keys of every `btree_keys` sub-entry in the entries a replay
/// applies, as `btree=<id> <pos> len <size>`.
#[test]
fn the_replayed_keys_are_the_references() {
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
    // How many keys the window holds is the guest's timing, not ours:
    // measured 11 in CI runs 37545693891 and #39's, about 1600 locally. Any
    // window proves the comparison below; an empty one would prove nothing.
    assert!(!theirs.is_empty(), "the reference lists no keys to replay");

    let dev = FileDevice::open(fixture(&format!("{SET}.img"))).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let mut ours = Vec::new();
    for j in journal::read_entries(&dev, &sb).unwrap() {
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
