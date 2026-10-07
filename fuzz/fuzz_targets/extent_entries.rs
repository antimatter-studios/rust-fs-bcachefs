#![no_main]
//! An extent value: self-describing entries whose type is the position
//! of the lowest set bit, and sizes that depend on it.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = fs_bcachefs_fuzz::extent_entries(data);
});
