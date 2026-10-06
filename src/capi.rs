//! The C ABI: mount an image read-only, stat a path, list a directory,
//! read a file. Every entry point catches panics; a failure returns -1 (or
//! NULL) and leaves its reason in `fs_core_last_error_message()`.
//! `include/fs_bcachefs.h` is the contract.

use std::ffi::{c_char, c_int, c_void, CStr};

use fs_core::ffi::{ffi_guard_or, set_last_error};
use fs_core::FileDevice;

use crate::Filesystem;

/// An open, read-only filesystem.
pub struct fs_bcachefs_fs {
    fs: Filesystem<FileDevice>,
}

/// What `fs_bcachefs_stat` reports.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct fs_bcachefs_attr_t {
    pub ino: u64,
    pub mode: u32,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub sectors: u64,
}

/// Called once per directory entry; return non-zero to stop.
pub type fs_bcachefs_dirent_cb = Option<
    unsafe extern "C" fn(
        ctx: *mut c_void,
        name: *const u8,
        name_len: usize,
        ino: u64,
        d_type: u8,
    ) -> c_int,
>;

unsafe fn cstr<'a>(p: *const c_char, what: &str) -> Option<&'a str> {
    if p.is_null() {
        set_last_error(format!("{what} is NULL"));
        return None;
    }
    match unsafe { CStr::from_ptr(p) }.to_str() {
        Ok(s) => Some(s),
        Err(_) => {
            set_last_error(format!("{what} is not UTF-8"));
            None
        }
    }
}

/// Open the image or device at `path` read-only. NULL on failure.
///
/// # Safety
/// `path` must be NULL or a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_mount(path: *const c_char) -> *mut fs_bcachefs_fs {
    ffi_guard_or(std::ptr::null_mut(), || {
        let Some(path) = (unsafe { cstr(path, "path") }) else {
            return std::ptr::null_mut();
        };
        let dev = match FileDevice::open(path) {
            Ok(d) => d,
            Err(e) => {
                set_last_error(e.to_string());
                return std::ptr::null_mut();
            }
        };
        match Filesystem::open(dev) {
            Ok(fs) => Box::into_raw(Box::new(fs_bcachefs_fs { fs })),
            Err(e) => {
                set_last_error(e.to_string());
                std::ptr::null_mut()
            }
        }
    })
}

/// Close a handle from `fs_bcachefs_mount`. NULL is a no-op.
///
/// # Safety
/// `fs` must be NULL or a live handle, not used again afterwards.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_umount(fs: *mut fs_bcachefs_fs) {
    if !fs.is_null() {
        drop(unsafe { Box::from_raw(fs) });
    }
}

/// Stat an absolute path (a symlink is not followed). 0 or -1.
///
/// # Safety
/// `fs` a live handle or NULL; `path` NUL-terminated or NULL; `out`
/// writable or NULL.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_stat(
    fs: *mut fs_bcachefs_fs,
    path: *const c_char,
    out: *mut fs_bcachefs_attr_t,
) -> c_int {
    ffi_guard_or(-1, || {
        if fs.is_null() || out.is_null() {
            set_last_error("fs or out is NULL");
            return -1;
        }
        let Some(path) = (unsafe { cstr(path, "path") }) else {
            return -1;
        };
        let fs = &unsafe { &*fs }.fs;
        match fs.lookup(path).and_then(|ino| fs.inode(ino).cloned()) {
            Ok(i) => {
                unsafe {
                    *out = fs_bcachefs_attr_t {
                        ino: i.ino,
                        mode: i.mode,
                        nlink: i.nlink,
                        uid: i.uid,
                        gid: i.gid,
                        size: i.size,
                        sectors: i.sectors,
                    }
                };
                0
            }
            Err(e) => {
                set_last_error(e.to_string());
                -1
            }
        }
    })
}

/// Call `cb` for each entry of the directory at `path`. 0 or -1.
///
/// # Safety
/// `fs` a live handle or NULL; `path` NUL-terminated or NULL; `cb` safe
/// to call with `ctx`.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_readdir(
    fs: *mut fs_bcachefs_fs,
    path: *const c_char,
    cb: fs_bcachefs_dirent_cb,
    ctx: *mut c_void,
) -> c_int {
    ffi_guard_or(-1, || {
        let (false, Some(cb)) = (fs.is_null(), cb) else {
            set_last_error("fs or cb is NULL");
            return -1;
        };
        let Some(path) = (unsafe { cstr(path, "path") }) else {
            return -1;
        };
        let fs = &unsafe { &*fs }.fs;
        match fs.lookup(path).and_then(|ino| fs.readdir(ino)) {
            Ok(entries) => {
                for d in entries {
                    if unsafe { cb(ctx, d.name.as_ptr(), d.name.len(), d.inum, d.d_type) } != 0 {
                        break;
                    }
                }
                0
            }
            Err(e) => {
                set_last_error(e.to_string());
                -1
            }
        }
    })
}

/// Copy up to `length` bytes of the file at `path`, from `offset`, into
/// `buf`. Returns the number of bytes copied (0 at or past the end), or -1.
///
/// # Safety
/// `fs` a live handle or NULL; `path` NUL-terminated or NULL; `buf`
/// writable for `length` bytes or NULL.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_read_file(
    fs: *mut fs_bcachefs_fs,
    path: *const c_char,
    buf: *mut c_void,
    offset: u64,
    length: u64,
) -> i64 {
    ffi_guard_or(-1, || {
        if fs.is_null() || buf.is_null() {
            set_last_error("fs or buf is NULL");
            return -1;
        }
        let Some(path) = (unsafe { cstr(path, "path") }) else {
            return -1;
        };
        let fs = &unsafe { &*fs }.fs;
        match fs.lookup(path).and_then(|ino| fs.read(ino)) {
            Ok(data) => {
                let start = (offset as usize).min(data.len());
                let n = (data.len() - start).min(length as usize);
                unsafe {
                    std::ptr::copy_nonoverlapping(data[start..].as_ptr(), buf.cast::<u8>(), n)
                };
                n as i64
            }
            Err(e) => {
                set_last_error(e.to_string());
                -1
            }
        }
    })
}
