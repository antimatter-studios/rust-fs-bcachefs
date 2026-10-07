#![no_main]
//! One unpacked key, its value fed to every decoder in turn.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = fs_bcachefs_fuzz::key_values(data);
});
