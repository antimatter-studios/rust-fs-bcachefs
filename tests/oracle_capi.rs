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

/// A failure names its errno, the platform's number, as POSIX gives each
/// case: ENOENT for a path that is not there, EINVAL for a readlink of
/// what is not a symlink and for a NULL argument, ERANGE for a buffer too
/// small, and ENOATTR (ENODATA on Linux) for an attribute a file does not
/// have. The paths are the mount's (the aged manifest).
#[test]
fn each_failure_names_its_errno() {
    #[cfg(target_vendor = "apple")]
    const ENOATTR: i32 = libc::ENOATTR;
    #[cfg(not(target_vendor = "apple"))]
    const ENOATTR: i32 = libc::ENODATA;

    let fs = mount("aged");
    let entries = common::manifest("aged");
    let file = entries
        .iter()
        .find(|e| e.kind == "file" && e.xattrs.is_some())
        .expect("the aged set has a file with xattrs");
    let link = entries
        .iter()
        .find(|e| e.kind == "symlink")
        .expect("the aged set has a symlink");
    let missing = c("/no/such/path");
    let mut a = fs_bcachefs_attr_t::default();
    let errno = |what: &str, failed: bool| {
        assert!(failed, "{what} succeeded");
        let e = fs_bcachefs_last_errno();
        (what.to_string(), e)
    };
    let mut buf = [0u8; 1];
    let got = vec![
        errno(
            "stat of a missing path",
            unsafe { fs_bcachefs_stat(fs, missing.as_ptr(), &mut a) } == -1,
        ),
        errno(
            "dir_open of a missing path",
            unsafe { fs_bcachefs_dir_open(fs, missing.as_ptr()) }.is_null(),
        ),
        errno(
            "read of a missing path",
            unsafe { fs_bcachefs_read_file(fs, missing.as_ptr(), buf.as_mut_ptr().cast(), 0, 1) }
                == -1,
        ),
        errno(
            "readlink of a file",
            unsafe { fs_bcachefs_readlink(fs, c(&file.path).as_ptr(), buf.as_mut_ptr().cast(), 1) }
                == -1,
        ),
        errno(
            "readlink into one byte",
            unsafe { fs_bcachefs_readlink(fs, c(&link.path).as_ptr(), buf.as_mut_ptr().cast(), 1) }
                == -1,
        ),
        errno(
            "getxattr of a name the file lacks",
            unsafe {
                fs_bcachefs_getxattr(
                    fs,
                    c(&file.path).as_ptr(),
                    c("user.no-such-attribute").as_ptr(),
                    std::ptr::null_mut(),
                    0,
                )
            } == -1,
        ),
        errno(
            "stat into NULL",
            unsafe { fs_bcachefs_stat(fs, missing.as_ptr(), std::ptr::null_mut()) } == -1,
        ),
    ];
    let want = vec![
        ("stat of a missing path".to_string(), libc::ENOENT),
        ("dir_open of a missing path".to_string(), libc::ENOENT),
        ("read of a missing path".to_string(), libc::ENOENT),
        ("readlink of a file".to_string(), libc::EINVAL),
        ("readlink into one byte".to_string(), libc::ERANGE),
        ("getxattr of a name the file lacks".to_string(), ENOATTR),
        ("stat into NULL".to_string(), libc::EINVAL),
    ];
    assert_eq!(got, want);
    unsafe { fs_bcachefs_umount(fs) };
}

/// Capacity and free space are what the reference tools report for every
/// image of the write study: show-super's member buckets from `First
/// bucket` on, times `Bucket size`, and the lister's freespace runs, each
/// `len` buckets long.
#[test]
fn statfs_reports_the_reference_tools_capacity_and_free_space() {
    let sets = [
        "base",
        "create-empty",
        "create-small",
        "create-large",
        "mkdir",
        "overwrite",
        "rename",
        "truncate",
        "unlink",
    ];
    for name in sets {
        let set = format!("write-study/{name}");
        let r = common::super_report(&set);
        let bucket = common::parse_size(&r["dev.Bucket size"]);
        let first: u64 = r["dev.First bucket"].parse().unwrap();
        let buckets: u64 = r["dev.Buckets"].parse().unwrap();
        let free_buckets: u64 = common::read_text(&format!("{set}.freespace.txt"))
            .lines()
            .filter(|l| l.contains(" type set 0:"))
            .map(|l| {
                let len = l.split(" len ").nth(1).expect("a run's len");
                len.split_whitespace()
                    .next()
                    .unwrap()
                    .parse::<u64>()
                    .unwrap()
            })
            .sum();
        assert!(free_buckets > 0, "{set}: no freespace runs in the listing");
        let fs = mount(&set);
        let mut st = fs_bcachefs_statfs_t::default();
        assert_eq!(
            unsafe { fs_bcachefs_statfs(fs, &mut st) },
            0,
            "{set}: {}",
            last_error()
        );
        assert_eq!(
            (st.bucket_size as u64, st.total_bytes, st.free_bytes),
            (bucket, (buckets - first) * bucket, free_buckets * bucket),
            "{set}"
        );
        assert_eq!(
            st.block_size as u64,
            common::parse_size(&r["block_size"]),
            "{set}"
        );
        unsafe { fs_bcachefs_umount(fs) };
    }
}

/// `text`, a time as the reference tools print it ("Tue Oct  6 23:35:42
/// 2026"), as seconds since the epoch, read as UTC by the platform's
/// date(1): BSD's takes `-j -f`, GNU's `-d`.
fn epoch_of(text: &str) -> i64 {
    let run = |args: &[&str]| {
        std::process::Command::new("date")
            .args(args)
            .env("TZ", "UTC")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| s.trim().parse::<i64>().ok())
    };
    run(&["-j", "-f", "%a %b %e %H:%M:%S %Y", text, "+%s"])
        .or_else(|| run(&["-d", text, "+%s"]))
        .unwrap_or_else(|| panic!("date(1) cannot read {text:?} as a time"))
}

/// Every inode's times, through `fs_bcachefs_stat_times`, fall on the day
/// the reference tools made the image: within a day of the superblock's
/// "Time of last write" as show-super prints it (a day either side, since
/// the guest's time zone is not known here). A time read without its base,
/// or in the wrong unit, lands decades away.
#[test]
fn times_fall_on_the_day_the_reference_made_the_image() {
    let set = "aged";
    let last_write = common::super_report(set)["Time of last write"].clone();
    let last = epoch_of(&last_write);
    let fs = mount(set);
    let mut checked = 0;
    for e in common::manifest(set) {
        let mut t = fs_bcachefs_times_t::default();
        let p = c(&e.path);
        assert_eq!(
            unsafe { fs_bcachefs_stat_times(fs, p.as_ptr(), &mut t) },
            0,
            "{}: {}",
            e.path,
            last_error()
        );
        for (what, sec, nsec) in [
            ("atime", t.atime_sec, t.atime_nsec),
            ("mtime", t.mtime_sec, t.mtime_nsec),
            ("ctime", t.ctime_sec, t.ctime_nsec),
        ] {
            assert!(nsec < 1_000_000_000, "{}: {what} nsec {nsec}", e.path);
            assert!(
                (last - 86_400..=last + 86_400).contains(&sec),
                "{}: {what} {sec} is not within a day of {last_write} ({last})",
                e.path
            );
        }
        checked += 1;
    }
    assert!(checked > 100, "only {checked} paths");
    unsafe { fs_bcachefs_umount(fs) };
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
