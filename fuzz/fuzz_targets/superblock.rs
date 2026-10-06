#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(sb) = fs_bcachefs::superblock::Superblock::parse(data) {
        let _ = sb.members();
        let _ = sb.btree_roots();
        let _ = sb.field_names();
    }
});
