#![no_main]
//! A whole filesystem with one change laid over it, then open, walk,
//! check and write (fuzz/shared/whole_image.rs). The input is a fixture
//! number and a seed; the fixtures are read from `BCACHEFS_FIXTURES`, or
//! the checkout's `.vm-share/fixtures`.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    fs_bcachefs_fuzz::whole_image_case(data);
});
