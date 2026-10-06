//! The C ABI at the level of the family's other drivers: volume info, stat
//! by path and by inode, a directory iterator, readlink, extended
//! attributes, mounting through an fs_core device, and the header itself
//! compiling as C. Checked against the aged fixture's manifest, which the
//! reference implementation's mount recorded.

mod common;

use std::ffi::{CStr, CString};

use fs_bcachefs::capi::*;

fn c(s: &str) -> CString {
    CString::new(s).unwrap()
}

fn mount(set: &str) -> *mut fs_bcachefs_fs {
    let img = c(common::fixture(&format!("{set}.img")).to_str().unwrap());
    let fs = unsafe { fs_bcachefs_mount(img.as_ptr()) };
    assert!(!fs.is_null(), "{set}: {}", last_error());
    fs
}

fn last_error() -> String {
    unsafe { CStr::from_ptr(fs_bcachefs_last_error()) }
        .to_string_lossy()
        .into_owned()
}

#[test]
fn volume_info_matches_the_superblock() {
    let fs = mount("aged");
    let mut v = fs_bcachefs_volume_info_t::default();
    assert_eq!(
        unsafe { fs_bcachefs_get_volume_info(fs, &mut v) },
        0,
        "{}",
        last_error()
    );
    let sb = fs_bcachefs::superblock::Superblock::read(
        &fs_core::FileDevice::open(common::fixture("aged.img")).unwrap(),
    )
    .unwrap();
    assert_eq!(v.block_size, u32::from(sb.block_size) * 512);
    assert_eq!(v.btree_node_size, sb.btree_node_size() * 512);
    assert_eq!(v.nr_devices, u32::from(sb.nr_devices));
    assert_eq!(v.uuid, sb.user_uuid);
    assert_eq!(
        (v.version_major, v.version_minor),
        (sb.version.major(), sb.version.minor())
    );
    assert_eq!(v.clean, 1);
    unsafe { fs_bcachefs_umount(fs) };
}

#[test]
fn stat_by_path_and_by_inode_agree_with_the_mount() {
    let fs = mount("aged");
    let mut checked = 0;
    for e in common::manifest("aged") {
        let p = c(&e.path);
        let mut a = fs_bcachefs_attr_t::default();
        assert_eq!(
            unsafe { fs_bcachefs_stat(fs, p.as_ptr(), &mut a) },
            0,
            "{}",
            e.path
        );
        assert_eq!(Some(a.ino), e.ino, "{}: inode", e.path);
        assert_eq!(
            Some(a.nlink),
            e.nlink,
            "{}: link count as the mount reports it",
            e.path
        );
        assert_eq!(a.mode & 0o7777, e.mode, "{}: permissions", e.path);
        let mut b = fs_bcachefs_attr_t::default();
        assert_eq!(unsafe { fs_bcachefs_stat_ino(fs, a.ino, &mut b) }, 0);
        assert_eq!(
            (a.ino, a.mode, a.size, a.nlink),
            (b.ino, b.mode, b.size, b.nlink)
        );
        checked += 1;
    }
    assert!(checked > 2000);
    unsafe { fs_bcachefs_umount(fs) };
}

#[test]
fn the_directory_iterator_lists_what_the_mount_listed() {
    let fs = mount("aged");
    let dir = c("/links");
    let it = unsafe { fs_bcachefs_dir_open(fs, dir.as_ptr()) };
    assert!(!it.is_null(), "{}", last_error());
    let mut names = Vec::new();
    loop {
        let d = unsafe { fs_bcachefs_dir_next(it) };
        if d.is_null() {
            break;
        }
        let d = unsafe { &*d };
        names.push(
            unsafe { CStr::from_ptr(d.name.as_ptr()) }
                .to_string_lossy()
                .into_owned(),
        );
        assert_eq!(d.name_len as usize, names.last().unwrap().len());
    }
    unsafe { fs_bcachefs_dir_close(it) };
    names.sort();
    assert_eq!(names, ["orig.txt", "second.txt", "sub"]);
    unsafe { fs_bcachefs_umount(fs) };
}

#[test]
fn readlink_returns_the_target() {
    let fs = mount("aged");
    for e in common::manifest("aged")
        .iter()
        .filter(|e| e.kind == "symlink")
    {
        let p = c(&e.path);
        let mut buf = vec![0u8; 512];
        let n = unsafe {
            fs_bcachefs_readlink(fs, p.as_ptr(), buf.as_mut_ptr().cast(), buf.len() as u64)
        };
        assert!(n >= 0, "{}: {}", e.path, last_error());
        assert_eq!(
            Some(String::from_utf8_lossy(&buf[..n as usize]).into_owned()),
            e.target
        );
    }
    unsafe { fs_bcachefs_umount(fs) };
}

#[test]
fn xattrs_list_and_get_like_the_mount() {
    let fs = mount("aged");
    let mut seen = 0;
    for e in common::manifest("aged") {
        let Some(want) = &e.xattrs else { continue };
        let p = c(&e.path);
        let need = unsafe { fs_bcachefs_listxattr(fs, p.as_ptr(), std::ptr::null_mut(), 0) };
        assert!(need > 0, "{}: {}", e.path, last_error());
        let mut buf = vec![0u8; need as usize];
        let n =
            unsafe { fs_bcachefs_listxattr(fs, p.as_ptr(), buf.as_mut_ptr().cast(), need as u64) };
        assert_eq!(n, need);
        let names: Vec<&[u8]> = buf.split(|&b| b == 0).filter(|s| !s.is_empty()).collect();
        for pair in want.split(';') {
            let (name, hex) = pair.split_once('=').unwrap();
            assert!(
                names.contains(&name.as_bytes()),
                "{}: {name} not listed",
                e.path
            );
            let cn = c(name);
            let need = unsafe {
                fs_bcachefs_getxattr(fs, p.as_ptr(), cn.as_ptr(), std::ptr::null_mut(), 0)
            };
            assert_eq!(need as usize, hex.len() / 2, "{}: {name} length", e.path);
            let mut v = vec![0u8; need as usize];
            let got = unsafe {
                fs_bcachefs_getxattr(
                    fs,
                    p.as_ptr(),
                    cn.as_ptr(),
                    v.as_mut_ptr().cast(),
                    need as u64,
                )
            };
            assert_eq!(got, need);
            let ours: String = v.iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(ours, hex, "{}: {name}", e.path);
        }
        seen += 1;
    }
    assert!(seen >= 5, "only {seen} paths with xattrs");
    unsafe { fs_bcachefs_umount(fs) };
}

#[test]
fn a_filesystem_mounts_through_an_fs_core_device() {
    let path = c(common::fixture("aged.img").to_str().unwrap());
    let dev = unsafe { fs_core::ffi::fs_core_file_open(path.as_ptr(), false) };
    assert!(!dev.is_null());
    let fs = unsafe { fs_bcachefs_mount_with_fs_core_device(dev) };
    assert!(!fs.is_null(), "{}", last_error());
    let mut a = fs_bcachefs_attr_t::default();
    let p = c("/links/orig.txt");
    assert_eq!(unsafe { fs_bcachefs_stat(fs, p.as_ptr(), &mut a) }, 0);
    assert_eq!(a.nlink, 3);
    unsafe { fs_bcachefs_umount(fs) };
    unsafe { fs_core::ffi::fs_core_device_close(dev) };
}

/// The header is valid C, with fs_core.h beside it, by the platform's own
/// compiler.
#[test]
fn the_header_compiles_as_c() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = std::process::Command::new("cc")
        .args(["-fsyntax-only", "-Wall", "-Werror", "-x", "c"])
        .arg("-I")
        .arg(root.join("include"))
        .arg("-I")
        .arg(root.join("../rust-fs-core/include"))
        .arg(root.join("include/fs_bcachefs.h"))
        .output()
        .expect("a C compiler (`cc`) is needed for this test: install one (Xcode's command line tools, build-essential)");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
