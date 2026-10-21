//! Free-space and backpointer damage (#163), planted on a copy of a fixture
//! through the writer's raw `insert`. Each case names the problem kind this
//! crate gives it and the finding the reference checker prints for it, as
//! the differential baseline records them (fuzz/differential-baseline.tsv).
//! The layouts are docs/clean-room.md's, "Allocating space". No reference
//! source.

use std::collections::BTreeMap;
use std::path::Path;

use fs_bcachefs::bkey::{Bkey, Bpos};
use fs_bcachefs::btree;
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::write::Writer;
use fs_core::FileDevice;

const ALLOC: u8 = 4;
const FREESPACE: u8 = 11;
const BACKPOINTERS: u8 = 13;
const BUCKET_GENS: u8 = 14;
const SET: u8 = 25;
const ALLOC_V4: u8 = 27;
const BACKPOINTER: u8 = 28;
const BUCKET_GENS_KEY: u8 = 30;
/// An alloc_v4 key's flags (byte 0 of word 1; bit 0 need_discard, bit 1
/// need_inc_gen), generation, oldest generation and data type.
const ALLOC_FLAGS: usize = 8;
const ALLOC_GEN: usize = 12;
const ALLOC_OLDEST_GEN: usize = 13;
const ALLOC_DATA_TYPE: usize = 14;

/// Each case: its name, the kind this crate reports, and the reference
/// checker's finding (compared without case).
pub const CASES: &[(&str, &str, &str)] = &[
    (
        "set",
        "bucket_incorrectly_set_in_freespace_btree",
        "bucket incorrectly set in freespace btree",
    ),
    (
        "hole",
        "hole_in_alloc_btree_missing_in_freespace_btree",
        "hole in alloc btree missing in freespace btree",
    ),
    (
        "unset",
        "bucket_incorrectly_unset_in_freespace_btree",
        "bucket incorrectly unset in freespace btree",
    ),
    (
        "nonexistent-device",
        "freespace_entry_for_nonexistent_bucket",
        "entry in freespace btree for nonexistant dev:bucket",
    ),
    (
        "backpointer",
        "backpointer_for_nonexistent_alloc_key",
        "backpointer for nonexistent alloc key",
    ),
];

/// Plant `case` in `img`, and return the text this crate's report of it
/// must carry in its detail.
pub fn plant(img: &Path, case: &str) -> String {
    let dev = FileDevice::open(img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let member = sb.members().unwrap()[sb.dev_idx as usize].clone();
    let first = u64::from(member.first_bucket);
    let walk = |id: u8| btree::walk(&dev, &sb, id).unwrap();
    let alloc: BTreeMap<u64, Bkey> = walk(ALLOC)
        .into_iter()
        .filter(|k| k.key_type == ALLOC_V4 && k.pos.inode == 0)
        .map(|k| (k.pos.offset, k))
        .collect();
    let runs: Vec<Bkey> = walk(FREESPACE)
        .into_iter()
        .filter(|k| k.key_type == SET && k.pos.inode == 0)
        .collect();
    let start = |k: &Bkey| k.pos.offset - u64::from(k.size);
    // A run of two or more whose first bucket has never had an alloc key.
    let fresh = || {
        runs.iter()
            .find(|k| k.size >= 2 && !alloc.contains_key(&start(k)))
            .expect("the fixture has a freespace run starting at a bucket with no alloc key")
            .clone()
    };
    let (writes, want): (Vec<(u8, Bkey)>, String) = match case {
        // A run grown by one bucket, onto a bucket in use.
        "set" => {
            let run = runs
                .iter()
                .find(|k| {
                    let s = start(k);
                    s > first
                        && alloc
                            .get(&(s - 1))
                            .is_some_and(|a| a.value[ALLOC_DATA_TYPE] != 0)
                })
                .expect("the fixture has a freespace run just above a bucket in use");
            let bucket = start(run) - 1;
            let grown = Bkey {
                size: run.size + 1,
                ..run.clone()
            };
            (vec![(FREESPACE, grown)], format!("bucket {bucket},"))
        }
        // A run shrunk by its first bucket, which has no alloc key.
        "hole" => {
            let run = fresh();
            let s = start(&run);
            let shrunk = Bkey {
                size: run.size - 1,
                ..run
            };
            (
                vec![(FREESPACE, shrunk)],
                format!("buckets {s}..{}:", s + 1),
            )
        }
        // The same, with an alloc key of data type free for the bucket.
        "unset" => {
            let run = fresh();
            let s = start(&run);
            let model = alloc
                .values()
                .find(|a| a.value.len() >= 56 && a.value[ALLOC_DATA_TYPE] != 0)
                .expect("the fixture has an alloc key in use");
            let gen = walk(BUCKET_GENS)
                .into_iter()
                .find(|k| {
                    k.key_type == BUCKET_GENS_KEY && k.pos.inode == 0 && k.pos.offset == s >> 8
                })
                .map_or(0, |k| k.value[(s & 0xff) as usize]);
            let mut v = model.value.clone();
            // Free, as the reference leaves a discarded bucket: no journal
            // sequence, no sectors, need_discard and need_inc_gen clear.
            v[0..8].fill(0);
            v[ALLOC_FLAGS] &= !3;
            v[ALLOC_GEN] = gen;
            v[ALLOC_OLDEST_GEN] = gen;
            v[ALLOC_DATA_TYPE] = 0;
            v[16..24].fill(0);
            v[48..56].fill(0);
            let free = Bkey {
                pos: Bpos {
                    inode: 0,
                    offset: s,
                    snapshot: 0,
                },
                value: v,
                ..model.clone()
            };
            let shrunk = Bkey {
                size: run.size - 1,
                ..run
            };
            (
                vec![(ALLOC, free), (FREESPACE, shrunk)],
                format!("bucket {s}: free"),
            )
        }
        // A copy of a run on a device the filesystem does not have.
        "nonexistent-device" => {
            let run = runs.first().expect("the fixture has a freespace run");
            let moved = Bkey {
                pos: Bpos {
                    inode: 1,
                    ..run.pos
                },
                ..run.clone()
            };
            let want = format!("freespace run {} ", moved.pos);
            (vec![(FREESPACE, moved)], want)
        }
        // A copy of a backpointer in a bucket with no alloc key.
        "backpointer" => {
            let bp = walk(BACKPOINTERS)
                .into_iter()
                .find(|k| k.key_type == BACKPOINTER && k.pos.inode == 0)
                .expect("the fixture has a backpointer");
            let s = start(&fresh());
            let moved = Bkey {
                pos: Bpos {
                    offset: (s * u64::from(member.bucket_size)) << 16,
                    ..bp.pos
                },
                ..bp
            };
            let want = format!("backpointer {}:", moved.pos);
            (vec![(BACKPOINTERS, moved)], want)
        }
        _ => panic!("unknown free-space case {case}"),
    };
    drop(dev);
    let mut w = Writer::open(FileDevice::open_rw(img).unwrap()).unwrap();
    for (id, k) in writes {
        w.insert(id, vec![k]).unwrap();
    }
    want
}
