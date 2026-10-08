//! Where a directory entry goes when its name's hash slot is taken (#78).
//! SipHash slots are 63 bits wide, so the write study collides names under
//! `--str_hash=crc32c` instead: four names of one length whose XOR
//! differences have a CRC of 0, created in order through the reference
//! mount, then the second removed, then created again
//! (scripts/guest-write-study.sh). What the reference did is read from its
//! lister's dirents dump; this crate's reader must find every entry.

mod common;

use std::collections::BTreeMap;

use common::{fixture, read_text};
use fs_bcachefs::Filesystem;
use fs_core::FileDevice;

/// In creation order. The second is the one removed and created again.
const NAMES: [&str; 4] = [
    "CAAAAAAAAAAAAAAA",
    "CBBF@MBKAAAAAAAA",
    "COA@GJOCFAAAAAAA",
    "CLBGFFLIFAAAAAAA",
];

/// CRC-32C, reflected, from `init`, without a final XOR.
fn crc32c(init: u32, data: &[u8]) -> u32 {
    let mut c = init;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = (c >> 1) ^ if c & 1 != 0 { 0x82F6_3B78 } else { 0 };
        }
    }
    c
}

/// The four names collide under CRC-32C from any starting value and with
/// any final XOR: equal-length messages differ in CRC by the CRC (from 0)
/// of their XOR, and each pair's XOR has a CRC of 0.
#[test]
fn the_four_names_collide_under_crc32c_whatever_the_seed() {
    for init in [0, 0xffff_ffff, 0x1234_5678, 0xdead_beef] {
        let c: Vec<u32> = NAMES.iter().map(|n| crc32c(init, n.as_bytes())).collect();
        assert!(c.iter().all(|&x| x == c[0]), "init {init:#x}: {c:x?}");
    }
}

/// One key in directory `/d` as the lister shows it.
#[derive(Debug, Clone, PartialEq)]
struct Slot {
    offset: u64,
    key_type: String,
    name: Option<String>,
}

/// Every key of directory `/d` in `image`'s dirents dump, in key order.
fn slots(image: &str) -> Vec<Slot> {
    let text = read_text(&format!("write-study/{image}.dirents.txt"));
    let keys: Vec<Vec<&str>> = text
        .lines()
        .filter(|l| l.starts_with("u64s "))
        .map(|l| l.split_whitespace().collect())
        .collect();
    // u64s N type T DIR:OFFSET:SNAPSHOT len L ver V : NAME -> INO type K
    let d = keys
        .iter()
        .find(|w| w.get(10) == Some(&"d") && w.get(11) == Some(&"->"))
        .unwrap_or_else(|| panic!("{image}: no dirent for /d"))[12];
    keys.iter()
        .filter(|w| w[4].split(':').next() == Some(d))
        .map(|w| Slot {
            offset: w[4].split(':').nth(1).unwrap().parse().unwrap(),
            key_type: w[3].to_string(),
            name: (w[3] == "dirent").then(|| w[10].to_string()),
        })
        .collect()
}

fn offsets(image: &str) -> BTreeMap<String, u64> {
    slots(image)
        .into_iter()
        .filter_map(|s| s.name.map(|n| (n, s.offset)))
        .collect()
}

fn reader_finds_every_name(image: &str, names: &[&str]) {
    let fs =
        Filesystem::open(FileDevice::open(fixture(&format!("write-study/{image}.img"))).unwrap())
            .unwrap_or_else(|e| panic!("{image}: {e}"));
    for n in names {
        let ino = fs
            .lookup(&format!("/d/{n}"))
            .unwrap_or_else(|e| panic!("{image}: /d/{n}: {e}"));
        assert_eq!(
            fs.read(ino).unwrap(),
            format!("{n}\n").as_bytes(),
            "{image} {n}"
        );
    }
}

/// HYPOTHESIS, checked here: a taken slot sends the entry to the next
/// offset up, so the four land on four consecutive offsets in creation
/// order.
#[test]
fn colliding_names_take_consecutive_offsets_in_creation_order() {
    let at = offsets("collide");
    let got: Vec<u64> = NAMES.iter().map(|n| at[*n]).collect();
    let want: Vec<u64> = (0..4).map(|i| got[0] + i).collect();
    assert_eq!(
        got,
        want,
        "collide: every key of /d: {:#?}",
        slots("collide")
    );
    reader_finds_every_name("collide", &NAMES);
    reader_finds_every_name("collide", &["plain"]);
}

/// HYPOTHESIS, checked here: removing an entry from the middle of a run
/// leaves a `hash_whiteout` in its slot, so the entries after it stay where
/// they are and are still found.
#[test]
fn a_removed_colliding_name_leaves_a_whiteout_in_its_slot() {
    let before = offsets("collide");
    let after = slots("collide-unlink");
    let gap = before[NAMES[1]];
    let at_gap: Vec<&Slot> = after.iter().filter(|s| s.offset == gap).collect();
    assert_eq!(
        at_gap
            .iter()
            .map(|s| s.key_type.as_str())
            .collect::<Vec<_>>(),
        ["hash_whiteout"],
        "collide-unlink: every key of /d: {after:#?}"
    );
    for n in [NAMES[0], NAMES[2], NAMES[3]] {
        assert_eq!(offsets("collide-unlink")[n], before[n], "{n} moved");
    }
    reader_finds_every_name("collide-unlink", &[NAMES[0], NAMES[2], NAMES[3]]);
}

/// HYPOTHESIS, checked here: a name created again where its run has a
/// whiteout takes the whiteout's slot.
#[test]
fn a_name_created_again_takes_the_whiteout_slot() {
    let before = offsets("collide");
    let again = offsets("collide-recreate");
    for n in NAMES {
        assert_eq!(
            again[n],
            before[n],
            "{n}: every key of /d: {:#?}",
            slots("collide-recreate")
        );
    }
    reader_finds_every_name("collide-recreate", &NAMES);
}
