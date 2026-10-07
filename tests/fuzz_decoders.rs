//! The stable-toolchain half of the fuzzing setup: replay the corpus,
//! then mutate it, and refuse if a decoder panics, hangs, or if the
//! suite quietly stopped doing any work.
//!
//! # Why there are two halves
//!
//! `fuzz/` holds `cargo-fuzz` targets. Those are the explorer: they run
//! for as long as they are given, on nightly, and find inputs nobody
//! thought of. They cannot be a required check, because how long they
//! ran decides what they found, and a fresh discovery would fail
//! whichever unrelated pull request happened to be open.
//!
//! This suite is the gate. Deterministic, on the stable toolchain, in
//! every pull request, reading the same `fuzz/corpus/` the explorer
//! does. Anything the explorer finds is committed there and replayed
//! here from then on. What each target drives is `fuzz/shared/helpers.rs`,
//! included textually by both halves so a reproducer from one reproduces
//! in the other.
//!
//! # Why the corpus is real structures and not random bytes
//!
//! Every seed under `fuzz/corpus/` was cut out of a fixture the reference
//! tools made in the test VM (`scripts/make-fuzz-corpus.sh`, which leaves
//! committed reproducers alone), or derived from one. Random bytes are
//! refused by the magic check on the first line of every decoder here; a
//! real structure with one field changed reaches all of it.
//!
//! # Why the checksum is re-stamped
//!
//! See `fuzz/shared/helpers.rs`: a mutated structure is refused for its
//! checksum before anything behind the checksum runs, so each case is fed
//! twice, once torn and once with the checksum recomputed. A test below
//! proves the re-stamp is right, because a wrong one would make every
//! target exercise nothing and look exactly like a target finding no
//! bugs.
//!
//! # Why mutation preserves length
//!
//! The decoders here genuinely take variable lengths (a node is as long as
//! its parent says was written, a journal entry as long as its header
//! says), so length is a real input and the replay sweeps truncations
//! too. Mutation itself keeps the length, so a case is one structure with
//! something wrong inside it rather than a short buffer no device would
//! hand back.
//!
//! # What counts as a failure
//!
//! A panic, which the harness catches. A hang, which it does not: the
//! work runs on a second thread against a deadline, and on expiry this
//! suite names the target, the seed and the case. And doing nothing: the
//! case count is held to a floor, because a suite that stopped generating
//! work would pass faster than ever.
#![allow(dead_code)]

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

include!("../fuzz/shared/helpers.rs");

/// Distinct starting points for the mutation stream. Fixed, so a failure
/// reproduces from the message alone.
const SEEDS: u64 = 6;

/// Below this, the suite is not doing its job. The real number is several
/// times higher; this catches a target list or a corpus that collapsed.
const CASE_FLOOR: usize = 6_000;

/// Long enough that a loaded machine is never the reason, short enough
/// that a genuine hang is reported rather than left to the job timeout.
const DEADLINE: Duration = Duration::from_secs(180);

// ---------------------------------------------------------------- targets

struct Target {
    /// A directory under `fuzz/corpus/` and a `[[bin]]` in `fuzz/Cargo.toml`.
    corpus: &'static str,
    /// Mutated cases per (seed file, starting point) pair. Per target,
    /// because a case costs what its seed costs to copy and run: a node
    /// of 25 KiB with two thousand keys is not a 56-byte key.
    cases: usize,
    run: fn(&[u8]),
}

fn targets() -> Vec<Target> {
    vec![
        Target {
            corpus: "superblock",
            cases: 192,
            run: superblock,
        },
        Target {
            corpus: "btree_node",
            cases: 96,
            run: |b| {
                btree_node(b);
            },
        },
        Target {
            corpus: "jset",
            cases: 192,
            run: |b| {
                jset(b);
            },
        },
        Target {
            corpus: "key_values",
            cases: 384,
            run: key_values,
        },
        Target {
            corpus: "inode_v3",
            cases: 384,
            run: inode_v3,
        },
        Target {
            corpus: "extent_entries",
            cases: 384,
            run: extent_entries,
        },
        Target {
            corpus: "lz4_block",
            cases: 384,
            run: lz4_block,
        },
    ]
}

// ---------------------------------------------------------------- corpus

fn corpus_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fuzz/corpus")
}

/// Every seed in one corpus directory, sorted so the order is the same
/// everywhere.
fn seeds(corpus: &str) -> Vec<(String, Vec<u8>)> {
    let dir = corpus_root().join(corpus);
    let mut out: Vec<(String, Vec<u8>)> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("reading the corpus directory {}: {e}", dir.display()))
        .map(|entry| {
            let path = entry.expect("corpus directory entry").path();
            let bytes = std::fs::read(&path)
                .unwrap_or_else(|e| panic!("reading the seed {}: {e}", path.display()));
            let name = path
                .file_name()
                .expect("seed file name")
                .to_string_lossy()
                .into_owned();
            (name, bytes)
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

// ---------------------------------------------------------------- mutation

/// xorshift64*. Small, deterministic, and not a dependency.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        // Any non-zero state will do; the constant keeps seed 0 from
        // being a fixed point.
        Rng(seed ^ 0x9e37_79b9_7f4a_7c15)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            0
        } else {
            (self.next() % bound as u64) as usize
        }
    }
}

/// One mutation of a real structure, preserving its length.
///
/// The operations are chosen for what they reach rather than for
/// variety: a flipped bit finds a boundary check that is off by one, an
/// extreme field value finds arithmetic that overflows, a swapped pair of
/// words finds a decoder that trusted two fields to be ordered, and a
/// small delta to a word finds a length that is believed one byte too
/// far. bcachefs is little-endian on disk, so the fields are written that
/// way.
fn mutate(seed: &[u8], rng: &mut Rng) -> Vec<u8> {
    let mut out = seed.to_vec();
    if out.is_empty() {
        return out;
    }
    match rng.below(5) {
        0 => {
            for _ in 0..=rng.below(8) {
                let at = rng.below(out.len());
                out[at] ^= 1u8 << rng.below(8);
            }
        }
        1 => {
            let at = rng.below(out.len());
            let len = 1 + rng.below(16.min(out.len() - at));
            let fill = if rng.next() & 1 == 0 { 0x00 } else { 0xff };
            out[at..at + len].fill(fill);
        }
        2 => {
            let width = [2usize, 4, 8][rng.below(3)];
            if out.len() >= width {
                let at = rng.below(out.len() - width + 1) & !(width - 1);
                if at + width <= out.len() {
                    let value: u64 = match rng.below(4) {
                        0 => 0,
                        1 => 1,
                        2 => u64::MAX,
                        _ => rng.next(),
                    };
                    out[at..at + width].copy_from_slice(&value.to_le_bytes()[..width]);
                }
            }
        }
        3 => {
            if out.len() >= 8 {
                let a = rng.below(out.len() / 4) * 4;
                let b = rng.below(out.len() / 4) * 4;
                if a + 4 <= out.len() && b + 4 <= out.len() {
                    for i in 0..4 {
                        out.swap(a + i, b + i);
                    }
                }
            }
        }
        _ => {
            if out.len() >= 4 {
                let at = rng.below(out.len() / 4) * 4;
                if at + 4 <= out.len() {
                    let word = u32::from_le_bytes(out[at..at + 4].try_into().expect("4 bytes"));
                    let delta = [1i64, -1, 2, -2, 255, -255][rng.below(6)];
                    let changed = (i64::from(word).wrapping_add(delta)) as u32;
                    out[at..at + 4].copy_from_slice(&changed.to_le_bytes());
                }
            }
        }
    }
    out
}

/// The case in flight, readable even if the lock was poisoned by the
/// panic being described.
fn describe(current: &Arc<Mutex<String>>) -> String {
    match current.lock() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

// ---------------------------------------------------------------- tests

#[test]
fn every_target_has_a_corpus() {
    for target in targets() {
        assert!(
            !seeds(target.corpus).is_empty(),
            "the target {0} reads fuzz/corpus/{0}, which holds no seeds -- a target with an \
             empty corpus runs no cases and would pass in silence. Rebuild it with \
             scripts/make-fuzz-corpus.sh",
            target.corpus,
        );
    }
}

/// Before any mutation: every seed, byte for byte, and every truncation of
/// it. This is the half that keeps a fixed finding fixed, so it is a test
/// on its own rather than the first iteration of the mutation loop.
#[test]
fn the_corpus_replays_exactly_as_committed_and_truncations_are_refused_cleanly() {
    let mut replayed = 0usize;
    for target in targets() {
        for (name, bytes) in seeds(target.corpus) {
            eprintln!("replaying {}/{}", target.corpus, name);
            (target.run)(&bytes);
            let step = (bytes.len() / 64).max(1);
            for n in (0..bytes.len()).step_by(step) {
                (target.run)(&bytes[..n]);
            }
            replayed += 1;
        }
    }
    assert!(
        replayed >= 12,
        "only {replayed} corpus files were replayed; the corpus has shrunk"
    );
}

/// The corpus is an oracle, not only fuel: the committed superblocks are
/// ones the reference formatter wrote, so this crate must read them and
/// find a filesystem in them.
#[test]
fn every_committed_superblock_parses_and_describes_its_filesystem() {
    for (name, data) in seeds("superblock") {
        let sb = Superblock::parse(&data).unwrap_or_else(|e| panic!("seed {name}: {e}"));
        assert_eq!(
            sb.version.major(),
            1,
            "seed {name}: version {:?}",
            sb.version
        );
        assert_eq!(sb.nr_devices, 1, "seed {name}");
        sb.members().unwrap_or_else(|e| panic!("seed {name}: {e}"));
        assert!(
            !sb.btree_roots().unwrap().is_empty(),
            "seed {name}: no btree roots"
        );
    }
}

/// Every committed node and journal entry parses, re-stamped and not.
///
/// This is what proves the re-stamp computes the right checksum. If it
/// did not, every mutated structure would be refused at the checksum and
/// the targets would exercise nothing -- and it would look exactly like a
/// target that finds no bugs.
#[test]
fn a_restamped_structure_still_parses() {
    for (name, data) in seeds("superblock") {
        let mut b = data.clone();
        restamp_superblock(&mut b);
        assert_eq!(
            b, data,
            "seed {name}: re-stamping an untouched superblock changed its bytes"
        );
    }
    let mut keys = 0;
    for (name, data) in seeds("btree_node") {
        let magic = le64(&data, 16);
        let mut stamped = data.clone();
        restamp_node(&mut stamped, 512);
        assert_eq!(
            stamped, data,
            "seed {name}: re-stamping an untouched node changed its bytes"
        );
        let node =
            Node::parse(&data, magic, 512, None).unwrap_or_else(|e| panic!("seed {name}: {e}"));
        assert!(!node.keys.is_empty(), "seed {name}: no keys");
        keys += node.keys.len();
        assert!(
            btree_node(&data) > 0,
            "seed {name}: the target decoded no key"
        );
    }
    assert!(keys > 100, "the node seeds hold only {keys} keys");
    for (name, data) in seeds("jset") {
        let magic = le64(&data, 16);
        let mut stamped = data.clone();
        restamp_jset(&mut stamped);
        assert_eq!(
            stamped, data,
            "seed {name}: re-stamping an untouched journal entry changed its bytes"
        );
        let j = journal::parse_jset(&data, magic)
            .unwrap_or_else(|e| panic!("seed {name}: {e}"))
            .unwrap_or_else(|| panic!("seed {name}: not a journal entry"));
        assert!(!j.entries.is_empty(), "seed {name}: no sub-entries");
    }
}

/// A corrupted structure is refused for its checksum in the build this
/// crate ships: there is no switch that turns verification off.
#[test]
fn checksums_are_verified() {
    for (name, mut data) in seeds("superblock") {
        // A byte of the label: inside the checksummed range, harmless to
        // the parse otherwise.
        data[0x48] ^= 0x5a;
        assert!(
            matches!(
                Superblock::parse(&data),
                Err(fs_bcachefs::Error::BadChecksum { .. })
            ),
            "seed {name}: a corrupted superblock was not refused for its checksum"
        );
        restamp_superblock(&mut data);
        Superblock::parse(&data)
            .unwrap_or_else(|e| panic!("seed {name}: refused after re-stamping: {e}"));
    }
    for (name, mut data) in seeds("btree_node") {
        let magic = le64(&data, 16);
        // The node's flags word: inside the checksummed range, read but not
        // interpreted, so the checksum is the only thing that can refuse
        // it. (Not a key byte: the formatter's first bset can be empty,
        // and a byte past it is the bset header of the next record.)
        data[24] ^= 0x5a;
        assert!(
            matches!(
                Node::parse(&data, magic, 512, None),
                Err(fs_bcachefs::Error::BadChecksum { .. })
            ),
            "seed {name}: a corrupted node was not refused for its checksum"
        );
    }
    for (name, mut data) in seeds("jset") {
        let magic = le64(&data, 16);
        data[60] ^= 0x5a;
        assert!(
            journal::parse_jset(&data, magic).unwrap().is_none(),
            "seed {name}: a corrupted journal entry was not refused for its checksum"
        );
    }
}

/// The key_values seeds: one unpacked key of each kind from the aged
/// fixture, which must decode as that kind.
#[test]
fn every_key_seed_decodes_as_its_kind() {
    for (name, data) in seeds("key_values") {
        let k = bkey::decode(&data, &UNPACKED).unwrap_or_else(|e| panic!("seed {name}: {e}"));
        let ok = match k.key_type {
            6 => DataExtent::from_key(&k).is_ok(),
            17 => !k.value.is_empty(),
            10 => Dirent::from_key(&k).is_ok(),
            11 => Xattr::from_key(&k).is_ok(),
            29 => Inode::from_key(&k).is_ok(),
            t => panic!("seed {name}: key type {t}"),
        };
        assert!(ok, "seed {name} does not decode");
    }
}

/// The inode_v3 seeds are values the reference wrote: decoding and
/// re-encoding one gives back its bytes exactly (docs/clean-room.md),
/// which is what makes the round-trip check in the target meaningful.
#[test]
fn every_inode_seed_round_trips_byte_for_byte() {
    for (name, data) in seeds("inode_v3") {
        let raw = InodeV3Raw::parse(&data).unwrap_or_else(|e| panic!("seed {name}: {e}"));
        assert_eq!(raw.encode(), data, "seed {name}: encode(decode(v)) != v");
        assert!(raw.mode() != 0, "seed {name}: mode 0");
    }
    for (name, data) in seeds("extent_entries") {
        let entries = extent::parse_entries(&data).unwrap_or_else(|e| panic!("seed {name}: {e}"));
        assert!(!entries.is_empty(), "seed {name}: no entries");
    }
    for (name, data) in seeds("lz4_block") {
        // Every lz4 seed is a real compressed extent, so some output
        // length decodes it; the target tries several, and this checks
        // that at least one gets through to the end of the block.
        let decoded = [39usize, 512, 4096, 65536]
            .iter()
            .any(|&n| fs_bcachefs::compress::lz4_block(&data, n).is_ok());
        assert!(
            decoded,
            "seed {name}: no output length decodes this lz4 block"
        );
    }
}

#[test]
fn deterministic_mutations_of_real_structures_are_survived() {
    let cases = Arc::new(AtomicUsize::new(0));
    let current = Arc::new(Mutex::new(String::from("(not started)")));
    let (done_tx, done_rx) = mpsc::channel::<()>();

    // A panic arrives with no clue which of thousands of cases caused it,
    // because the case is a local in another thread by the time the
    // message is printed. The hook prints the one that was in flight.
    {
        let current = Arc::clone(&current);
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = writeln!(
                std::io::stderr(),
                "fuzz_decoders: the case in flight was {}",
                describe(&current)
            );
            previous(info);
        }));
    }

    let worker = {
        let cases = Arc::clone(&cases);
        let current = Arc::clone(&current);
        std::thread::spawn(move || {
            for target in targets() {
                for (name, seed) in seeds(target.corpus) {
                    for s in 0..SEEDS {
                        let mut rng = Rng::new(s);
                        for case in 0..target.cases {
                            let input = mutate(&seed, &mut rng);
                            if let Ok(mut g) = current.lock() {
                                *g = format!("{}/{name} seed {s} case {case}", target.corpus);
                            }
                            (target.run)(&input);
                            cases.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
            let _ = done_tx.send(());
        })
    };

    match done_rx.recv_timeout(DEADLINE) {
        Ok(()) => {}
        Err(mpsc::RecvTimeoutError::Timeout) => panic!(
            "a decoder hung: after {DEADLINE:?} the case in flight is {}; {} cases had finished",
            describe(&current),
            cases.load(Ordering::Relaxed)
        ),
        // The worker panicked and dropped its sender; the join reports it.
        Err(mpsc::RecvTimeoutError::Disconnected) => {}
    }
    if let Err(panic) = worker.join() {
        std::panic::resume_unwind(panic);
    }
    let n = cases.load(Ordering::Relaxed);
    assert!(
        n >= CASE_FLOOR,
        "only {n} mutated cases ran (floor {CASE_FLOOR}): the target list or the corpus has collapsed"
    );
    eprintln!("fuzz_decoders: {n} mutated cases survived");
}
