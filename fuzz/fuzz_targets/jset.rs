#![no_main]
//! A journal entry: typed sub-entries each saying how long it is, and
//! unpacked keys inside them, read before any btree is trusted.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = fs_bcachefs_fuzz::jset(data);
});
