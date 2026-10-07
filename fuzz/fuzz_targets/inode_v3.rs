#![no_main]
//! An inode_v3 value: a varint stream whose field count the value
//! declares, and an encoder that must give the bytes back.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = fs_bcachefs_fuzz::inode_v3(data);
});
