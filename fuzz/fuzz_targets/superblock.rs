#![no_main]
//! The superblock, read before anything is known: the checksum type,
//! the block size and the field table all come out of it.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = fs_bcachefs_fuzz::superblock(data);
});
