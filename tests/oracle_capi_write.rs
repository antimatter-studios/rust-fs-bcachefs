//! The C ABI's writes (#103), called as a C caller would: every operation
//! on a copy of the write study's base image, read back here; and the
//! refusals, each with its errno: the platform's own number for it, which
//! is what a C caller compares with. The reference checker's and mount's
//! verdict on the same calls is in tests/write_oracle.rs.
#![cfg(feature = "write")]

mod common;

use std::ffi::{c_void, CString};

use common::fixture;
use fs_bcachefs::capi::fs_bcachefs_last_errno;
use fs_bcachefs::capi_write::*;
use fs_bcachefs::Filesystem;
use fs_core::FileDevice;

fn c(s: &str) -> CString {
    CString::new(s).unwrap()
}

fn scratch(test: &str) -> common::ScratchImage {
    let img = common::ScratchDir::new("capi-write").into_image(format!("{test}.img"));
    std::fs::copy(fixture("write-study/base.img"), &img).unwrap();
    img
}

#[test]
fn every_write_entry_point_reads_back() {
    let img = scratch("all");
    let fs = unsafe { fs_bcachefs_mount_rw(c(img.to_str().unwrap()).as_ptr()) };
    assert!(!fs.is_null());
    let data = b"written through the C ABI\n";
    let call = |r: i32, what: &str| assert_eq!(r, 0, "{what}");
    unsafe {
        call(
            fs_bcachefs_create(
                fs,
                c("/d/new").as_ptr(),
                data.as_ptr() as *const c_void,
                data.len() as u64,
                0o640,
            ),
            "create",
        );
        call(fs_bcachefs_mkdir(fs, c("/d/sub").as_ptr(), 0o750), "mkdir");
        call(
            fs_bcachefs_rename(fs, c("/d/new").as_ptr(), c("/d/sub/moved").as_ptr()),
            "rename",
        );
        call(
            fs_bcachefs_symlink(fs, c("/d/link").as_ptr(), c("sub/moved").as_ptr()),
            "symlink",
        );
        call(
            fs_bcachefs_link(fs, c("/d/sub/moved").as_ptr(), c("/d/hard").as_ptr()),
            "link",
        );
        call(
            fs_bcachefs_chmod(fs, c("/d/sub/moved").as_ptr(), 0o600),
            "chmod",
        );
        call(
            fs_bcachefs_chown(fs, c("/d/sub/moved").as_ptr(), 1000, 1001),
            "chown",
        );
        call(
            fs_bcachefs_setxattr(
                fs,
                c("/d/sub/moved").as_ptr(),
                c("user.colour").as_ptr(),
                b"blue".as_ptr() as *const c_void,
                4,
            ),
            "setxattr",
        );
        call(
            fs_bcachefs_setxattr(
                fs,
                c("/d/sub/moved").as_ptr(),
                c("user.gone").as_ptr(),
                b"x".as_ptr() as *const c_void,
                1,
            ),
            "setxattr",
        );
        call(
            fs_bcachefs_removexattr(fs, c("/d/sub/moved").as_ptr(), c("user.gone").as_ptr()),
            "removexattr",
        );
        call(
            fs_bcachefs_write_file(
                fs,
                c("/d/existing").as_ptr(),
                b"replaced\n".as_ptr() as *const c_void,
                9,
            ),
            "write_file",
        );
        call(
            fs_bcachefs_mkdir(fs, c("/d/empty").as_ptr(), 0o755),
            "mkdir",
        );
        call(fs_bcachefs_rmdir(fs, c("/d/empty").as_ptr()), "rmdir");
        call(fs_bcachefs_unlink(fs, c("/d/hard").as_ptr()), "unlink");
        fs_bcachefs_rw_close(fs);
    }
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    let moved = fs.lookup("/d/sub/moved").unwrap();
    assert_eq!(fs.read(moved).unwrap(), data);
    let i = fs.inode(moved).unwrap();
    assert_eq!((i.mode & 0o7777, i.uid, i.gid), (0o600, 1000, 1001));
    assert_eq!(i.link_count(), 1, "the hard link was removed");
    let x: Vec<(Vec<u8>, Vec<u8>)> = fs
        .xattrs(moved)
        .unwrap()
        .into_iter()
        .map(|x| (x.name, x.value))
        .collect();
    assert_eq!(x, vec![(b"user.colour".to_vec(), b"blue".to_vec())]);
    assert_eq!(
        fs.read(fs.lookup("/d/link").unwrap()).unwrap(),
        b"sub/moved"
    );
    assert_eq!(
        fs.read(fs.lookup("/d/existing").unwrap()).unwrap(),
        b"replaced\n"
    );
    assert!(
        fs.lookup("/d/new").is_err()
            && fs.lookup("/d/empty").is_err()
            && fs.lookup("/d/hard").is_err()
    );
}

#[test]
fn refusals_come_back_as_negative_errnos() {
    let img = scratch("errno");
    let fs = unsafe { fs_bcachefs_mount_rw(c(img.to_str().unwrap()).as_ptr()) };
    assert!(!fs.is_null());
    unsafe {
        assert_eq!(
            fs_bcachefs_unlink(fs, c("/d/nothing-here").as_ptr()),
            -libc::ENOENT,
            "ENOENT"
        );
        assert_eq!(
            fs_bcachefs_mkdir(fs, c("/no/such/dir").as_ptr(), 0o755),
            -libc::ENOENT,
            "ENOENT"
        );
        assert_eq!(
            fs_bcachefs_rmdir(fs, c("/d").as_ptr()),
            -libc::ENOTEMPTY,
            "ENOTEMPTY"
        );
        assert_eq!(
            fs_bcachefs_unlink(fs, std::ptr::null()),
            -libc::EINVAL,
            "EINVAL"
        );
        assert_eq!(
            fs_bcachefs_mkdir(std::ptr::null_mut(), c("/x").as_ptr(), 0o755),
            -libc::EINVAL,
            "EINVAL"
        );
        fs_bcachefs_rw_close(fs);
        assert!(fs_bcachefs_mount_rw(c("/no/such/image").as_ptr()).is_null());
        fs_bcachefs_rw_close(std::ptr::null_mut());
    }
}

/// A handle opened through an fs_core device writes as one opened by path
/// does: the same calls read back the same, and a refusal is the same
/// negative errno, which `fs_bcachefs_last_errno` repeats.
#[test]
fn a_device_handle_writes_as_a_path_handle_does() {
    let img = scratch("device");
    let path = c(img.to_str().unwrap());
    let dev = unsafe { fs_core::ffi::fs_core_file_open(path.as_ptr(), true) };
    assert!(!dev.is_null());
    let fs = unsafe { fs_bcachefs_mount_rw_with_fs_core_device(dev) };
    assert!(!fs.is_null());
    let data = b"written through an fs_core device\n";
    unsafe {
        assert_eq!(
            fs_bcachefs_create(
                fs,
                c("/d/dev-file").as_ptr(),
                data.as_ptr() as *const c_void,
                data.len() as u64,
                0o644,
            ),
            0,
            "create"
        );
        assert_eq!(
            fs_bcachefs_mkdir(fs, c("/d/dev-dir").as_ptr(), 0o755),
            0,
            "mkdir"
        );
        assert_eq!(
            fs_bcachefs_write_file(
                fs,
                c("/d/existing").as_ptr(),
                b"rewritten\n".as_ptr() as *const c_void,
                10,
            ),
            0,
            "write_file"
        );
        assert_eq!(
            fs_bcachefs_rmdir(fs, c("/d").as_ptr()),
            -libc::ENOTEMPTY,
            "rmdir of a directory that is not empty"
        );
        assert_eq!(fs_bcachefs_last_errno(), libc::ENOTEMPTY);
        assert_eq!(
            fs_bcachefs_unlink(fs, c("/d/nothing-here").as_ptr()),
            -libc::ENOENT
        );
        assert_eq!(fs_bcachefs_last_errno(), libc::ENOENT);
        fs_bcachefs_rw_close(fs);
        fs_core::ffi::fs_core_device_close(dev);
    }
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    assert_eq!(fs.read(fs.lookup("/d/dev-file").unwrap()).unwrap(), data);
    assert!(fs.inode(fs.lookup("/d/dev-dir").unwrap()).unwrap().is_dir());
    assert_eq!(
        fs.read(fs.lookup("/d/existing").unwrap()).unwrap(),
        b"rewritten\n"
    );
}

/// A device that cannot be written is refused at open, with EROFS, and a
/// NULL device with EINVAL.
#[test]
fn a_read_only_device_is_refused_at_open() {
    let img = scratch("device-ro");
    let path = c(img.to_str().unwrap());
    let dev = unsafe { fs_core::ffi::fs_core_file_open(path.as_ptr(), false) };
    assert!(!dev.is_null());
    unsafe {
        assert!(fs_bcachefs_mount_rw_with_fs_core_device(dev).is_null());
        assert_eq!(fs_bcachefs_last_errno(), libc::EROFS);
        fs_core::ffi::fs_core_device_close(dev);
        assert!(fs_bcachefs_mount_rw_with_fs_core_device(std::ptr::null()).is_null());
        assert_eq!(fs_bcachefs_last_errno(), libc::EINVAL);
    }
}
