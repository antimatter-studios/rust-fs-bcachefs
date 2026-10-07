//! The C ABI: mount an image read-only, stat a path, list a directory,
//! read a file. Every entry point catches panics; a failure returns -1 (or
//! NULL) and leaves its reason in `fs_core_last_error_message()`.
//! `include/fs_bcachefs.h` is the contract.

use std::ffi::{c_char, c_int, c_void, CStr};
use std::sync::Arc;

use fs_core::ffi::{ffi_guard_or, set_last_error, FsCoreDevice};
use fs_core::{BlockDevice, FileDevice};

use crate::Filesystem;

/// An open, read-only filesystem.
pub struct fs_bcachefs_fs {
    /// The device is an fs_core trait object, which does not promise unwind
    /// safety; every entry point catches panics and leaves the handle
    /// usable, so the handle keeps promising it as it did with a concrete
    /// file device.
    fs: std::panic::AssertUnwindSafe<Filesystem<Arc<dyn BlockDevice>>>,
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
        match Filesystem::open(Arc::new(dev) as Arc<dyn BlockDevice>) {
            Ok(fs) => Box::into_raw(Box::new(fs_bcachefs_fs {
                fs: std::panic::AssertUnwindSafe(fs),
            })),
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
        match fs.lookup(path).and_then(|ino| fs.inode(ino)) {
            Ok(i) => {
                unsafe {
                    *out = fs_bcachefs_attr_t {
                        ino: i.ino,
                        mode: i.mode,
                        nlink: i.link_count(),
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
/// Only the extents covering the window are read, so reading a file in
/// pieces costs the pieces, not the file each time.
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
        let len = usize::try_from(length).unwrap_or(usize::MAX);
        match fs
            .lookup(path)
            .and_then(|ino| fs.read_range(ino, offset, len))
        {
            Ok(data) => {
                unsafe {
                    std::ptr::copy_nonoverlapping(data.as_ptr(), buf.cast::<u8>(), data.len())
                };
                data.len() as i64
            }
            Err(e) => {
                set_last_error(e.to_string());
                -1
            }
        }
    })
}

/// The C side's view of the last failure on this thread: the same message
/// as `fs_core_last_error_message()`. Never NULL.
#[no_mangle]
pub extern "C" fn fs_bcachefs_last_error() -> *const c_char {
    let p = fs_core::ffi::fs_core_last_error_message();
    if p.is_null() {
        c"".as_ptr()
    } else {
        p
    }
}

/// Mount through an fs_core device handle (a file, a slice, host callbacks:
/// `fs_core_device_from_callbacks`). The handle stays the caller's to
/// close; the filesystem holds its own reference to the device.
///
/// # Safety
/// `dev` must be NULL or a live `FsCoreDevice` handle.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_mount_with_fs_core_device(
    dev: *const FsCoreDevice,
) -> *mut fs_bcachefs_fs {
    ffi_guard_or(std::ptr::null_mut(), || {
        if dev.is_null() {
            set_last_error("dev is NULL");
            return std::ptr::null_mut();
        }
        let inner = Arc::clone(unsafe { &*dev }.inner());
        match Filesystem::open(inner) {
            Ok(fs) => Box::into_raw(Box::new(fs_bcachefs_fs {
                fs: std::panic::AssertUnwindSafe(fs),
            })),
            Err(e) => {
                set_last_error(e.to_string());
                std::ptr::null_mut()
            }
        }
    })
}

/// What `fs_bcachefs_get_volume_info` reports.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct fs_bcachefs_volume_info_t {
    pub block_size: u32,
    pub btree_node_size: u32,
    pub nr_devices: u32,
    pub version_major: u16,
    pub version_minor: u16,
    /// The external (user-visible) UUID.
    pub uuid: [u8; 16],
    /// NUL-terminated.
    pub label: [c_char; 33],
    /// 1 when the filesystem was shut down cleanly, 0 when it was read
    /// through a replay of its journal.
    pub clean: u8,
}

impl Default for fs_bcachefs_volume_info_t {
    fn default() -> Self {
        fs_bcachefs_volume_info_t {
            block_size: 0,
            btree_node_size: 0,
            nr_devices: 0,
            version_major: 0,
            version_minor: 0,
            uuid: [0; 16],
            label: [0; 33],
            clean: 0,
        }
    }
}

/// Fill `out` with the volume's identity and geometry. 0 or -1.
///
/// # Safety
/// `fs` a live handle or NULL; `out` writable or NULL.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_get_volume_info(
    fs: *mut fs_bcachefs_fs,
    out: *mut fs_bcachefs_volume_info_t,
) -> c_int {
    ffi_guard_or(-1, || {
        if fs.is_null() || out.is_null() {
            set_last_error("fs or out is NULL");
            return -1;
        }
        let sb = unsafe { &*fs }.fs.superblock();
        let mut v = fs_bcachefs_volume_info_t {
            block_size: u32::from(sb.block_size) * 512,
            btree_node_size: sb.btree_node_size() * 512,
            nr_devices: u32::from(sb.nr_devices),
            version_major: sb.version.major(),
            version_minor: sb.version.minor(),
            uuid: sb.user_uuid,
            clean: u8::from(sb.is_clean()),
            ..Default::default()
        };
        for (d, s) in v.label.iter_mut().zip(sb.label_str().bytes().take(32)) {
            *d = s as c_char;
        }
        unsafe { *out = v };
        0
    })
}

fn attr_of(i: &crate::inode::Inode) -> fs_bcachefs_attr_t {
    fs_bcachefs_attr_t {
        ino: i.ino,
        mode: i.mode,
        nlink: i.link_count(),
        uid: i.uid,
        gid: i.gid,
        size: i.size,
        sectors: i.sectors,
    }
}

/// Stat an inode by number. 0 or -1.
///
/// # Safety
/// `fs` a live handle or NULL; `out` writable or NULL.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_stat_ino(
    fs: *mut fs_bcachefs_fs,
    ino: u64,
    out: *mut fs_bcachefs_attr_t,
) -> c_int {
    ffi_guard_or(-1, || {
        if fs.is_null() || out.is_null() {
            set_last_error("fs or out is NULL");
            return -1;
        }
        match unsafe { &*fs }.fs.inode(ino) {
            Ok(i) => {
                unsafe { *out = attr_of(&i) };
                0
            }
            Err(e) => {
                set_last_error(e.to_string());
                -1
            }
        }
    })
}

/// One entry of a directory iterator.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct fs_bcachefs_dirent_t {
    pub ino: u64,
    /// DT_* numbering.
    pub d_type: u8,
    pub name_len: u16,
    /// NUL-terminated.
    pub name: [c_char; 256],
}

/// A directory being listed; each `fs_bcachefs_dir_next` returns the next
/// entry, valid until the next call or `fs_bcachefs_dir_close`.
pub struct fs_bcachefs_dir_iter {
    entries: Vec<crate::inode::Dirent>,
    next: usize,
    current: fs_bcachefs_dirent_t,
}

/// Open the directory at `path` for listing. NULL on failure.
///
/// # Safety
/// `fs` a live handle or NULL; `path` NUL-terminated or NULL.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_dir_open(
    fs: *mut fs_bcachefs_fs,
    path: *const c_char,
) -> *mut fs_bcachefs_dir_iter {
    ffi_guard_or(std::ptr::null_mut(), || {
        if fs.is_null() {
            set_last_error("fs is NULL");
            return std::ptr::null_mut();
        }
        let Some(path) = (unsafe { cstr(path, "path") }) else {
            return std::ptr::null_mut();
        };
        let fs = &unsafe { &*fs }.fs;
        match fs.lookup(path).and_then(|ino| fs.readdir(ino)) {
            Ok(entries) => Box::into_raw(Box::new(fs_bcachefs_dir_iter {
                entries: entries.to_vec(),
                next: 0,
                current: fs_bcachefs_dirent_t {
                    ino: 0,
                    d_type: 0,
                    name_len: 0,
                    name: [0; 256],
                },
            })),
            Err(e) => {
                set_last_error(e.to_string());
                std::ptr::null_mut()
            }
        }
    })
}

/// The next entry, or NULL at the end (or on a NULL iterator).
///
/// # Safety
/// `it` NULL or a live iterator.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_dir_next(
    it: *mut fs_bcachefs_dir_iter,
) -> *const fs_bcachefs_dirent_t {
    ffi_guard_or(std::ptr::null(), || {
        if it.is_null() {
            return std::ptr::null();
        }
        let it = unsafe { &mut *it };
        let Some(d) = it.entries.get(it.next) else {
            return std::ptr::null();
        };
        it.next += 1;
        let n = d.name.len().min(255);
        it.current.ino = d.inum;
        it.current.d_type = d.d_type;
        it.current.name_len = n as u16;
        it.current.name = [0; 256];
        for (dst, &b) in it.current.name.iter_mut().zip(&d.name[..n]) {
            *dst = b as c_char;
        }
        &it.current
    })
}

/// Free an iterator. NULL is a no-op.
///
/// # Safety
/// `it` NULL or a live iterator, not used again afterwards.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_dir_close(it: *mut fs_bcachefs_dir_iter) {
    if !it.is_null() {
        drop(unsafe { Box::from_raw(it) });
    }
}

/// Copy up to `len` bytes of `src` into `buf`; `len` 0 asks for the size.
/// Returns the full size, or -1 when `buf` is too small.
unsafe fn copy_out(src: &[u8], buf: *mut c_void, len: u64) -> i64 {
    if len == 0 || buf.is_null() {
        return src.len() as i64;
    }
    if (len as usize) < src.len() {
        set_last_error(format!(
            "the buffer holds {len} bytes; {} are needed",
            src.len()
        ));
        return -1;
    }
    unsafe { std::ptr::copy_nonoverlapping(src.as_ptr(), buf.cast::<u8>(), src.len()) };
    src.len() as i64
}

/// A symlink's target into `buf` (not NUL-terminated). Returns its length,
/// or -1 (not a symlink, or `buf` too small).
///
/// # Safety
/// `fs` a live handle or NULL; `path` NUL-terminated or NULL; `buf`
/// writable for `len` bytes or NULL.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_readlink(
    fs: *mut fs_bcachefs_fs,
    path: *const c_char,
    buf: *mut c_void,
    len: u64,
) -> i64 {
    ffi_guard_or(-1, || {
        if fs.is_null() {
            set_last_error("fs is NULL");
            return -1;
        }
        let Some(path) = (unsafe { cstr(path, "path") }) else {
            return -1;
        };
        let fs = &unsafe { &*fs }.fs;
        let target = fs.lookup(path).and_then(|ino| {
            if !fs.inode(ino)?.is_symlink() {
                return Err(crate::Error::Corrupt(format!("{path} is not a symlink")));
            }
            fs.read(ino)
        });
        match target {
            Ok(t) => unsafe { copy_out(&t, buf, len) },
            Err(e) => {
                set_last_error(e.to_string());
                -1
            }
        }
    })
}

/// The names of a path's extended attributes, each NUL-terminated, into
/// `buf`, as listxattr(2) lays them out. `len` 0 asks for the size.
/// Returns the size, or -1.
///
/// # Safety
/// `fs` a live handle or NULL; `path` NUL-terminated or NULL; `buf`
/// writable for `len` bytes or NULL.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_listxattr(
    fs: *mut fs_bcachefs_fs,
    path: *const c_char,
    buf: *mut c_void,
    len: u64,
) -> i64 {
    ffi_guard_or(-1, || {
        if fs.is_null() {
            set_last_error("fs is NULL");
            return -1;
        }
        let Some(path) = (unsafe { cstr(path, "path") }) else {
            return -1;
        };
        let fs = &unsafe { &*fs }.fs;
        match fs.lookup(path).and_then(|ino| fs.xattrs(ino)) {
            Ok(list) => {
                let mut names = Vec::new();
                for x in list {
                    names.extend_from_slice(&x.name);
                    names.push(0);
                }
                unsafe { copy_out(&names, buf, len) }
            }
            Err(e) => {
                set_last_error(e.to_string());
                -1
            }
        }
    })
}

/// The value of the extended attribute `name` (with its namespace prefix,
/// `user.x`) of a path, into `buf`. `len` 0 asks for the size. Returns the
/// size, or -1 (no such attribute, or `buf` too small).
///
/// # Safety
/// `fs` a live handle or NULL; `path` and `name` NUL-terminated or NULL;
/// `buf` writable for `len` bytes or NULL.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_getxattr(
    fs: *mut fs_bcachefs_fs,
    path: *const c_char,
    name: *const c_char,
    buf: *mut c_void,
    len: u64,
) -> i64 {
    ffi_guard_or(-1, || {
        if fs.is_null() || name.is_null() {
            set_last_error("fs or name is NULL");
            return -1;
        }
        let Some(path) = (unsafe { cstr(path, "path") }) else {
            return -1;
        };
        let want = unsafe { CStr::from_ptr(name) }.to_bytes();
        let fs = &unsafe { &*fs }.fs;
        match fs.lookup(path).and_then(|ino| fs.xattrs(ino)) {
            Ok(list) => match list.iter().find(|x| x.name == want) {
                Some(x) => unsafe { copy_out(&x.value, buf, len) },
                None => {
                    set_last_error(format!(
                        "{path} has no attribute {}",
                        String::from_utf8_lossy(want)
                    ));
                    -1
                }
            },
            Err(e) => {
                set_last_error(e.to_string());
                -1
            }
        }
    })
}
