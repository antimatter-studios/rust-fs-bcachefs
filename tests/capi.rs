//! The C ABI over a real fixture (oracle tier), and its refusals.

mod common;

use std::ffi::{c_int, c_void, CString};

use fs_bcachefs::capi::*;
use sha2::{Digest, Sha256};

#[test]
fn null_and_missing_inputs_are_refused_not_crashed_on() {
    unsafe {
        assert!(fs_bcachefs_mount(std::ptr::null()).is_null());
        let nope = CString::new("/nonexistent/image").unwrap();
        assert!(fs_bcachefs_mount(nope.as_ptr()).is_null());
        let mut a = fs_bcachefs_attr_t::default();
        assert_eq!(
            fs_bcachefs_stat(std::ptr::null_mut(), nope.as_ptr(), &mut a),
            -1
        );
        assert_eq!(
            fs_bcachefs_read_file(
                std::ptr::null_mut(),
                nope.as_ptr(),
                std::ptr::null_mut(),
                0,
                0
            ),
            -1
        );
        assert_eq!(
            fs_bcachefs_readdir(
                std::ptr::null_mut(),
                nope.as_ptr(),
                None,
                std::ptr::null_mut()
            ),
            -1
        );
        fs_bcachefs_umount(std::ptr::null_mut());
    }
}

unsafe extern "C" fn collect(
    ctx: *mut c_void,
    name: *const u8,
    len: usize,
    _ino: u64,
    _t: u8,
) -> c_int {
    let v = unsafe { &mut *(ctx as *mut Vec<String>) };
    v.push(String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(name, len) }).into_owned());
    0
}

#[test]
fn the_abi_reads_a_fixture_like_the_library_does() {
    let img = CString::new(common::fixture("zstd.img").to_str().unwrap()).unwrap();
    let want = common::manifest("zstd");
    unsafe {
        let fs = fs_bcachefs_mount(img.as_ptr());
        assert!(!fs.is_null());
        let p = CString::new("/big/random.bin").unwrap();
        let mut a = fs_bcachefs_attr_t::default();
        assert_eq!(fs_bcachefs_stat(fs, p.as_ptr(), &mut a), 0);
        let mut buf = vec![0u8; a.size as usize + 10];
        let n = fs_bcachefs_read_file(fs, p.as_ptr(), buf.as_mut_ptr().cast(), 0, buf.len() as u64);
        assert_eq!(n as u64, a.size);
        let sum = format!("{:x}", Sha256::digest(&buf[..n as usize]));
        let e = want.iter().find(|e| e.path == "/big/random.bin").unwrap();
        assert_eq!(Some(sum), e.sha256);
        let mut names: Vec<String> = Vec::new();
        let d = CString::new("/dir").unwrap();
        assert_eq!(
            fs_bcachefs_readdir(
                fs,
                d.as_ptr(),
                Some(collect),
                (&mut names as *mut Vec<String>).cast()
            ),
            0
        );
        names.sort();
        assert_eq!(names, vec!["link-to-b", "notes.md", "sub"]);
        fs_bcachefs_umount(fs);
    }
}
