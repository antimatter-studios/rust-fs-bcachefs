//! Whole-image fuzzing, the gate half (#152): deterministic cases over the
//! fixtures, on the stable toolchain, in every pull request. What a case is
//! and what it runs is `fuzz/shared/whole_image.rs`, included textually by
//! this gate and by the explorer (`fuzz/fuzz_targets/whole_image.rs`), so a
//! reproducer from one reproduces in the other.
//!
//! A case is one fixture with one change laid over it -- a btree node, a
//! journal entry or the superblock changed and (mostly) re-stamped, or one
//! field of one real key committed through the writer -- and then the whole
//! workload: open, a bounded walk, the checker, and on an image the checker
//! passes, writes and both again.
//!
//! What counts as a failure, as in `tests/fuzz_decoders.rs`: a panic, which
//! the hook below names the case of; a hang, caught by a deadline on a
//! second thread; and doing nothing, caught by a floor on the cases run. And
//! one more: an allocation over 1 GiB is refused, its backtrace printed,
//! which turns "the runner was killed for memory" into a named line of
//! `src/`.
//!
//! Every finding is committed under `fuzz/corpus/whole_image/` as the bytes
//! the explorer was given (a fixture number and a seed, see
//! `whole_image_case`), and replayed here from then on.
#![cfg(feature = "write")]
#![allow(dead_code, unused_imports)]

mod common;

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

include!("../fuzz/shared/helpers.rs");
include!("../fuzz/shared/whole_image.rs");

/// Cases per fixture, each its own seed.
const CASES: u64 = 24;

/// Below this the gate is not doing its job: the fixture list or the
/// targets a clean pass finds have collapsed.
const CASE_FLOOR: usize = 150;

/// Long enough that a loaded runner is never the reason, short enough that
/// a genuine hang is reported rather than left to the job's timeout.
const DEADLINE: Duration = Duration::from_secs(600);

// ---------------------------------------------------------------- the cap

/// The largest single allocation a case may make.
const CAP: usize = 1 << 30;

struct Capped;

std::thread_local! {
    static REPORTING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn refused(size: usize) {
    // The report allocates; one inside it is not reported again.
    REPORTING.with(|r| {
        if !r.get() {
            r.set(true);
            let bt = std::backtrace::Backtrace::force_capture();
            let _ = writeln!(
                std::io::stderr(),
                "oracle_fuzz_whole_image: an allocation of {size} bytes was refused (the cap is \
                 {CAP}); it was asked for at:\n{bt}"
            );
            r.set(false);
        }
    });
}

// SAFETY: every call is passed to the system allocator unchanged, except
// one over the cap, which returns null as an allocator that is out of
// memory does.
unsafe impl std::alloc::GlobalAlloc for Capped {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        if layout.size() > CAP {
            refused(layout.size());
            return std::ptr::null_mut();
        }
        unsafe { std::alloc::System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: std::alloc::Layout) -> *mut u8 {
        if layout.size() > CAP {
            refused(layout.size());
            return std::ptr::null_mut();
        }
        unsafe { std::alloc::System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, new_size: usize) -> *mut u8 {
        if new_size > CAP {
            refused(new_size);
            return std::ptr::null_mut();
        }
        unsafe { std::alloc::System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        unsafe { std::alloc::System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Capped = Capped;

// ---------------------------------------------------------------- fixtures

/// Each fixture of `WI_FIXTURES`, read once, and what a clean pass over
/// it found to mutate.
fn bases() -> Vec<(&'static str, Arc<Vec<u8>>, Targets)> {
    WI_FIXTURES
        .iter()
        .map(|&name| {
            let bytes = std::fs::read(common::fixture(name))
                .unwrap_or_else(|e| panic!("reading the fixture {name}: {e}"));
            let base = Arc::new(bytes);
            let t = targets(&base);
            (name, base, t)
        })
        .collect()
}

/// The committed reproducers, sorted so the order is the same everywhere.
fn corpus() -> Vec<(String, Vec<u8>)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fuzz/corpus/whole_image");
    let mut out: Vec<(String, Vec<u8>)> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .map(|entry| {
            let path = entry.expect("corpus entry").path();
            let bytes =
                std::fs::read(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
            (
                path.file_name()
                    .expect("file name")
                    .to_string_lossy()
                    .into_owned(),
                bytes,
            )
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// The case in flight, readable even if the lock was poisoned by the panic
/// being described.
fn describe(current: &Arc<Mutex<String>>) -> String {
    match current.lock() {
        Ok(g) => g.clone(),
        Err(p) => p.into_inner().clone(),
    }
}

// ---------------------------------------------------------------- tests

/// What the cases mutate is found, and found right: every fixture's clean
/// pass reads btree nodes, and a node, a journal entry and the superblock
/// it found are each unchanged by a re-stamp. A wrong classification or a
/// wrong re-stamp would make every case a torn write the checksum refuses,
/// which would look exactly like a gate that finds nothing.
#[test]
fn the_structures_a_clean_pass_reads_are_found_and_re_stamp_unchanged() {
    let mut jsets = 0;
    for (name, base, t) in bases() {
        assert!(
            !t.nodes.is_empty(),
            "{name}: a clean pass read no btree node"
        );
        assert!(!t.btrees.is_empty(), "{name}: no btree roots");
        let (at, len) = t
            .superblock
            .unwrap_or_else(|| panic!("{name}: no superblock read"));
        let sb = base[at as usize..at as usize + len].to_vec();
        let mut b = sb.clone();
        restamp_superblock(&mut b);
        assert!(b == sb, "{name}: re-stamping the superblock changed it");
        for &(at, len) in t.nodes.iter().take(4) {
            let node = base[at as usize..at as usize + len].to_vec();
            let mut b = node.clone();
            restamp_node(&mut b, t.block);
            assert!(b == node, "{name}: re-stamping the node at {at} changed it");
        }
        for &(at, len) in t.jsets.iter().take(4) {
            let j = base[at as usize..at as usize + len].to_vec();
            let mut b = j.clone();
            restamp_jset(&mut b);
            assert!(
                b == j,
                "{name}: re-stamping the journal entry at {at} changed it"
            );
        }
        jsets += t.jsets.len();
    }
    assert!(jsets > 0, "no fixture's clean pass read a journal entry");
}

/// The committed reproducers, each as the explorer was given it.
#[test]
fn the_corpus_replays() {
    let bases = bases();
    let corpus = corpus();
    assert!(
        !corpus.is_empty(),
        "fuzz/corpus/whole_image holds no case: the explorer would start from nothing"
    );
    for (name, data) in corpus {
        let Some((i, seed)) = wi_decode(&data) else {
            panic!("fuzz/corpus/whole_image/{name} is shorter than a case");
        };
        let (fixture, base, t) = &bases[i % bases.len()];
        eprintln!("replaying whole_image/{name}: {fixture} seed {seed}");
        let dev = Overlay::new(Arc::clone(base), false);
        apply_case(&dev, t, seed);
        workload(&dev, true);
    }
}

#[test]
fn deterministic_cases_over_every_fixture_are_survived() {
    let cases = Arc::new(AtomicUsize::new(0));
    let current = Arc::new(Mutex::new(String::from("(not started)")));
    let (done_tx, done_rx) = mpsc::channel::<()>();
    {
        let current = Arc::clone(&current);
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = writeln!(
                std::io::stderr(),
                "oracle_fuzz_whole_image: the case in flight was {}",
                describe(&current)
            );
            previous(info);
        }));
    }
    let worker = {
        let cases = Arc::clone(&cases);
        let current = Arc::clone(&current);
        std::thread::spawn(move || {
            for (i, (name, base, t)) in bases().into_iter().enumerate() {
                for seed in 0..CASES {
                    let dev = Overlay::new(Arc::clone(&base), false);
                    if let Ok(mut g) = current.lock() {
                        *g = format!("{name} seed {seed} (fixture {i}), being laid");
                    }
                    let what = apply_case(&dev, &t, seed);
                    if let Ok(mut g) = current.lock() {
                        *g = format!("{name} seed {seed} (fixture {i}): {what}");
                    }
                    workload(&dev, true);
                    cases.fetch_add(1, Ordering::Relaxed);
                }
            }
            let _ = done_tx.send(());
        })
    };
    match done_rx.recv_timeout(DEADLINE) {
        Ok(()) => {}
        Err(mpsc::RecvTimeoutError::Timeout) => panic!(
            "a case hung: after {DEADLINE:?} the case in flight is {}; {} cases had finished",
            describe(&current),
            cases.load(Ordering::Relaxed)
        ),
        Err(mpsc::RecvTimeoutError::Disconnected) => {}
    }
    if let Err(panic) = worker.join() {
        std::panic::resume_unwind(panic);
    }
    let n = cases.load(Ordering::Relaxed);
    assert!(
        n >= CASE_FLOOR,
        "only {n} whole-image cases ran (floor {CASE_FLOOR}): the fixture list has collapsed"
    );
}
