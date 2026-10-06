#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    fs_bcachefs_fuzz::btree_node(data);
});
