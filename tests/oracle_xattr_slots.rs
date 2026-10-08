//! Where the reference puts an xattr (#77). Two of `aged`'s xattrs are not
//! at SipHash-2-4 keyed `(hash_seed, 0)` over the namespace byte and the
//! name, with the inode's hash_seed as the lister prints it. The write
//! study repeats the steps that placed them, settled and dumped after each
//! (scripts/guest-write-study.sh): this checks every xattr of every step
//! against the seed its inode has in that step, and names any earlier
//! step whose seed places one that does not fit.

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

#[test]
fn every_xattr_sits_at_the_slot_of_its_inodes_seed() {
    let mut misfits = Vec::new();
    let mut n = 0;
    for (i, step) in STEPS.iter().enumerate() {
        let now = seeds(step);
        for (ino, off, ns, name) in xattrs(step) {
            n += 1;
            if slot(now[&ino], ns, &name) == off {
                continue;
            }
            let earlier: Vec<String> = STEPS[..i]
                .iter()
                .filter(|s| {
                    seeds(s)
                        .get(&ino)
                        .is_some_and(|&seed| slot(seed, ns, &name) == off)
                })
                .map(|s| s.to_string())
                .collect();
            misfits.push(format!(
                "{step}: inode {ino} {name} at {off}, seed now {:x}; earlier seeds that place it: {earlier:?}; the inode's seed in each step: {:x?}",
                now[&ino],
                STEPS.iter().map(|s| seeds(s).get(&ino).copied()).collect::<Vec<_>>()
            ));
        }
    }
    assert!(n >= 10, "only {n} xattrs");
    assert!(misfits.is_empty(), "{}", misfits.join("\n"));
}
