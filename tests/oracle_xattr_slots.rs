//! Where the reference puts an xattr (#77). Two of `aged`'s xattrs were
//! not at SipHash-2-4 of the namespace byte and the name; the write study
//! repeated the steps that placed them, settled and dumped after each, then
//! set one xattr of every name length from 1 to 24
//! (scripts/guest-write-study.sh). Names of 1 to 7 bytes, 15 and 23 fit;
//! the others fit once the message's final partial word is taken as the
//! reference takes it (`xattr::name_slot`).

mod common;

use std::collections::BTreeMap;

use common::read_text;
use fs_bcachefs::xattr::name_slot;

const STEPS: &[&str] = &[
    "xattr-first",
    "xattr-dir",
    "xattr-second",
    "xattr-together",
    "xattr-lengths",
];

/// `inode -> hash_seed` in a step's inodes dump.
fn seeds(step: &str) -> BTreeMap<u64, u64> {
    let mut out = BTreeMap::new();
    let mut ino = None;
    for l in read_text(&format!("{step}.inodes.txt")).lines() {
        if l.starts_with("u64s ") {
            ino = l
                .split_whitespace()
                .nth(4)
                .and_then(|p| p.split(':').nth(1))
                .and_then(|n| n.parse().ok());
        } else if let (Some(i), Some(v)) = (ino, l.trim().strip_prefix("hash_seed=")) {
            out.insert(i, u64::from_str_radix(v, 16).unwrap());
        }
    }
    out
}

/// `(inode, offset, namespace byte, name)` of every xattr in a step.
fn xattrs(step: &str) -> Vec<(u64, u64, u8, String)> {
    read_text(&format!("{step}.xattrs.txt"))
        .lines()
        .filter(|l| l.starts_with("u64s ") && l.split_whitespace().nth(3) == Some("xattr"))
        .map(|l| {
            let mut p = l.split_whitespace().nth(4).unwrap().split(':');
            let ino = p.next().unwrap().parse().unwrap();
            let off = p.next().unwrap().parse().unwrap();
            let full = l.split_once(" : ").unwrap().1;
            let (ns, rest) = full.split_once('.').unwrap();
            let name = rest.split(':').next().unwrap().to_string();
            let ns = match ns {
                "user" => 0,
                "trusted" => 3,
                other => panic!("{step}: namespace {other}"),
            };
            (ino, off, ns, name)
        })
        .collect()
}

fn slot(seed: u64, ns: u8, name: &str) -> u64 {
    name_slot(seed, ns, name.as_bytes())
}

/// Every xattr of every step, and every one of `aged`'s 45, sits at
/// [`slot`]: names of 1 to 24 bytes, on files and directories.
#[test]
fn every_xattr_sits_at_its_slot() {
    let mut wrong = Vec::new();
    let mut n = 0;
    let steps = STEPS.iter().map(|s| format!("write-study/{s}"));
    for dump in steps.chain(["aged".to_string()]) {
        let now = seeds(&dump);
        for (ino, off, ns, name) in xattrs(&dump) {
            n += 1;
            if slot(now[&ino], ns, &name) != off {
                wrong.push(format!(
                    "{dump}: inode {ino} {name} ({} bytes) at {off}",
                    name.len()
                ));
            }
        }
    }
    assert!(n >= 60, "only {n} xattrs");
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// On a crc32c image (#106), every xattr sits at its crc32c slot, or, when
/// names collide, in the run after it: the four colliding names share one
/// slot and take consecutive offsets in the order they were set.
#[test]
fn crc32c_xattrs_sit_at_their_slot_or_in_its_run() {
    let mut wrong = Vec::new();
    let mut n = 0;
    for step in ["xcollide", "xcollide-remove", "xcollide-reset"] {
        let dump = format!("write-study/{step}");
        let now = seeds(&dump);
        for (ino, off, ns, name) in xattrs(&dump) {
            n += 1;
            let h = fs_bcachefs::xattr::slot(
                fs_bcachefs::inode::HASH_TYPE_CRC32C,
                now[&ino],
                ns,
                name.as_bytes(),
            )
            .unwrap();
            if off < h || off - h >= 8 {
                wrong.push(format!(
                    "{dump}: inode {ino} {name} ({} bytes) at {off}; slot {h}",
                    name.len()
                ));
            }
        }
    }
    assert!(n >= 80, "only {n} xattrs");
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
