#![no_main]
//! A whole btree node: a header that says where its bsets end, and
//! packed keys whose format the node itself declares.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = fs_bcachefs_fuzz::btree_node(data);
});
