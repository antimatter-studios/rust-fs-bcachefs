#![no_main]
//! A compressed extent's bytes through the hand-written LZ4 block
//! decoder: literal and match lengths, and back-references into the
//! output, all attacker-controlled.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = fs_bcachefs_fuzz::lz4_block(data);
});
