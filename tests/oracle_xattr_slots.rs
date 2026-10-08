//! Where the reference puts an xattr (#77). Two of `aged`'s xattrs are not
//! at SipHash-2-4 keyed `(hash_seed, 0)` over the namespace byte and the
//! name, with the inode's hash_seed as the lister prints it. The write
//! study repeats the steps that placed them, settled and dumped after each,
//! then sets one xattr of every name length from 1 to 24
//! (scripts/guest-write-study.sh).

mod common;

use std::collections::BTreeMap;

use common::read_text;
use fs_bcachefs::siphash::siphash24;

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
    for l in read_text(&format!("write-study/{step}.inodes.txt")).lines() {
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
    read_text(&format!("write-study/{step}.xattrs.txt"))
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
    let mut msg = vec![ns];
    msg.extend_from_slice(name.as_bytes());
    siphash24(seed, 0, &msg) >> 1
}

/// OBSERVED (#77): an xattr whose name has 1 to 7 bytes sits at
/// SipHash-2-4 keyed `(hash_seed, 0)` over the namespace byte and the name,
/// shifted right by one, with the seed its inode has in every step; one
/// whose name has 8 bytes or more never does (8 to 24 tried), on files and
/// directories alike. Where those go is open question 15.
#[test]
fn short_xattr_names_sit_at_the_siphash_slot_and_long_ones_do_not() {
    let mut wrong = Vec::new();
    let (mut short, mut long) = (0, 0);
    for step in STEPS {
        let now = seeds(step);
        for (ino, off, ns, name) in xattrs(step) {
            let fits = slot(now[&ino], ns, &name) == off;
            if name.len() < 8 {
                short += 1;
            } else {
                long += 1;
            }
            if fits != (name.len() < 8) {
                wrong.push(format!(
                    "{step}: inode {ino} {name} ({} bytes) at {off}: {}",
                    name.len(),
                    if fits {
                        "at the slot"
                    } else {
                        "not at the slot"
                    }
                ));
            }
        }
    }
    assert!(
        short >= 10 && long >= 10,
        "{short} short names, {long} long"
    );
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
