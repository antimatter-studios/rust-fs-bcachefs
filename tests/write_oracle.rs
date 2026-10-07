//! The write path, judged by the reference tools. Runs INSIDE the harness
//! guest only (`chore test:vm`, scripts/guest-suite.sh, with the `write`
//! feature), where the reference checker and the reference
//! implementation's mount exist; anywhere else every test fails naming
//! that. Nothing skips.
//!
//! Each test copies a fixture, writes to the copy with this crate, and then
//! requires (1) the reference checker to pass the image with nothing to fix
//! and (2) the reference implementation, mounting the image read-only, to
//! read back what was written.
#![cfg(feature = "write")]

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::fixture;
use fs_bcachefs::btree::{self, btree_id};
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::write::Writer;
use fs_bcachefs::Filesystem;
use fs_core::FileDevice;

/// The chroot the reference tools live in (scripts/vm-setup.sh), and the
/// mount point the read-back uses inside it.
const REF_ROOT: &str = "/srv/ref-trixie";
const MNT: &str = "/mnt/write-oracle";

fn require_guest() {
    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference tools are not here: these tests run inside the harness guest (`chore test:vm`)"
    );
}

/// A fresh copy of a fixture under /share, which the reference tools' chroot
/// also sees at /share.
fn scratch(fixture_name: &str, test: &str) -> PathBuf {
    require_guest();
    let dir = PathBuf::from("/share/write-oracle");
    std::fs::create_dir_all(&dir).unwrap();
    let img = dir.join(format!("{test}.img"));
    std::fs::copy(fixture(fixture_name), &img).unwrap();
    img
}

fn reference(args: &[&str]) -> (bool, String) {
    let out = Command::new("bcachefs-ref").args(args).output().unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    (out.status.success(), text)
}

/// The reference checker passes the image and fixes nothing.
fn assert_fsck_clean(img: &Path) {
    let (ok, text) = reference(&["fsck", "-n", img.to_str().unwrap()]);
    // Any line naming an error, but not the options line, whose
    // `fix_errors=no` is no finding.
    let complaint = text.lines().find(|l| {
        !l.trim_start().starts_with("with options")
            && (l.contains("fixing") || l.contains("error") || l.contains("Error"))
    });
    assert!(
        ok && complaint.is_none(),
        "the reference checker did not pass {}:\n{text}",
        img.display()
    );
}

/// Mount the image read-only with the reference implementation, hand the
/// mount's root (as this process sees it) to `f`, and unmount.
fn with_reference_mount<T>(img: &Path, f: impl FnOnce(&Path) -> T) -> T {
    let host_mnt = PathBuf::from(format!("{REF_ROOT}{MNT}"));
    std::fs::create_dir_all(&host_mnt).unwrap();
    let mut child = Command::new("bcachefs-ref")
        .args([
            "fusemount",
            "-f",
            "-o",
            "ro,noatime",
            img.to_str().unwrap(),
            MNT,
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mounted = (0..60).any(|_| {
        std::thread::sleep(std::time::Duration::from_millis(500));
        Command::new("mountpoint")
            .args(["-q", host_mnt.to_str().unwrap()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    });
    assert!(
        mounted,
        "the reference implementation did not mount {}",
        img.display()
    );
    let out = f(&host_mnt);
    let _ = child.kill();
    let _ = child.wait();
    let _ = Command::new("fusermount3")
        .args(["-uz", host_mnt.to_str().unwrap()])
        .status();
    out
}

/// Re-stating the newest key of a btree changes nothing a reader can see,
/// but exercises the whole write path: a bset appended to the leaf, the new
/// length carried to the root and the superblock rewritten.
#[test]
fn an_identity_edit_passes_the_reference_checker() {
    for (fixture_name, test) in [
        ("write-study/base.img", "identity-base"),
        ("aged.img", "identity-aged"),
    ] {
        let img = scratch(fixture_name, test);
        for id in [btree_id::DIRENTS, btree_id::INODES, btree_id::EXTENTS] {
            let k = {
                let dev = FileDevice::open(&img).unwrap();
                let sb = Superblock::read(&dev).unwrap();
                btree::walk(&dev, &sb, id).unwrap().pop().unwrap()
            };
            let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
            w.insert(id, vec![k])
                .unwrap_or_else(|e| panic!("{test}: btree {id}: {e}"));
        }
        assert_fsck_clean(&img);
        // This crate still reads it, and so does the reference.
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        let root = fs.readdir(4096).unwrap().len();
        let theirs = with_reference_mount(&img, |m| std::fs::read_dir(m).unwrap().count());
        assert_eq!(root, theirs, "{test}: entries in /");
    }
}

/// Files this crate creates in an existing directory: the reference
/// checker passes the image, and the reference implementation lists each
/// file and reads back its bytes, size and mode.
#[test]
fn small_files_created_here_are_read_by_the_reference() {
    let img = scratch("write-study/base.img", "create-small");
    let files: Vec<(&str, Vec<u8>, u32)> = vec![
        ("new.txt", b"hello\n".to_vec(), 0o644),
        ("empty", Vec::new(), 0o600),
        (
            "full-inline.bin",
            (0..248u32).map(|i| (i * 7) as u8).collect(),
            0o640,
        ),
        (
            "a-rather-longer-name-for-a-file-created-by-this-writer.txt",
            b"x".to_vec(),
            0o644,
        ),
    ];
    let dir = {
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        fs.lookup("/d").unwrap()
    };
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    for (name, data, mode) in &files {
        w.create_file(dir, name.as_bytes(), data, *mode)
            .unwrap_or_else(|e| panic!("create {name}: {e}"));
    }
    drop(w);
    assert_fsck_clean(&img);
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    with_reference_mount(&img, |m| {
        let mut listed: Vec<String> = std::fs::read_dir(m.join("d"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        listed.sort();
        let mut want: Vec<String> = files.iter().map(|f| f.0.to_string()).collect();
        want.push("existing".into());
        want.sort();
        assert_eq!(listed, want, "the reference's listing of /d");
        for (name, data, mode) in &files {
            let p = m.join("d").join(name);
            assert_eq!(
                &std::fs::read(&p).unwrap(),
                data,
                "{name}: bytes, as the reference reads them"
            );
            use std::os::unix::fs::PermissionsExt;
            let md = std::fs::metadata(&p).unwrap();
            assert_eq!(md.permissions().mode() & 0o7777, *mode, "{name}: mode");
            assert_eq!(md.len(), data.len() as u64, "{name}: size");
            let ours = fs.read(fs.lookup(&format!("/d/{name}")).unwrap()).unwrap();
            assert_eq!(&ours, data, "{name}: bytes, as this crate reads them");
        }
    });
}

/// The same creations in the aged image, whose btrees are two levels deep
/// and whose leaves hold many bsets.
#[test]
fn a_file_created_in_the_aged_image_is_read_by_the_reference() {
    let img = scratch("aged.img", "create-aged");
    let dir = {
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        fs.lookup("/many").unwrap()
    };
    // The aged image is aged by a live mount, so how full its leaves are
    // differs from one fixture build to the next. Until nodes are split, a
    // full leaf must be refused before anything is written, and the
    // untouched image still passes the reference checker.
    let before = std::fs::read(&img).unwrap();
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    match w.create_file(
        dir,
        b"written-by-this-crate",
        b"appended to an aged tree\n",
        0o644,
    ) {
        Ok(_) => {}
        Err(fs_bcachefs::Error::Unsupported(m)) if m.contains("the node is full") => {
            drop(w);
            assert!(
                std::fs::read(&img).unwrap() == before,
                "refused, but written"
            );
            assert_fsck_clean(&img);
            return;
        }
        Err(e) => panic!("{e}"),
    }
    drop(w);
    assert_fsck_clean(&img);
    with_reference_mount(&img, |m| {
        assert_eq!(
            std::fs::read(m.join("many/written-by-this-crate")).unwrap(),
            b"appended to an aged tree\n"
        );
        // Everything that was there still is.
        assert!(std::fs::read_dir(m.join("many")).unwrap().count() > 1000);
    });
}

/// mkdir, unlink, rmdir, rename and rewriting a file, judged by the
/// reference checker and read back through the reference mount.
#[test]
fn namespace_operations_are_read_by_the_reference() {
    let img = scratch("write-study/base.img", "namespace");
    let d = {
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        fs.lookup("/d").unwrap()
    };
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    let sub = w.mkdir(d, b"sub", 0o750).unwrap();
    w.create_file(sub, b"inner.txt", b"inside\n", 0o644)
        .unwrap();
    w.create_file(d, b"doomed", b"soon gone", 0o644).unwrap();
    w.unlink(d, b"doomed").unwrap();
    w.rename(d, b"existing", sub, b"moved").unwrap();
    let f = w
        .create_file(d, b"rewritten", b"first version, rather long\n", 0o644)
        .unwrap();
    w.write_file(f, b"second\n").unwrap();
    w.mkdir(d, b"empty-dir", 0o755).unwrap();
    w.rmdir(d, b"empty-dir").unwrap();
    drop(w);
    assert_fsck_clean(&img);
    with_reference_mount(&img, |m| {
        let mut names: Vec<String> = std::fs::read_dir(m.join("d"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["rewritten", "sub"]);
        assert_eq!(
            std::fs::read(m.join("d/sub/inner.txt")).unwrap(),
            b"inside\n"
        );
        assert_eq!(
            std::fs::read(m.join("d/sub/moved")).unwrap(),
            b"an existing file\n"
        );
        assert_eq!(std::fs::read(m.join("d/rewritten")).unwrap(), b"second\n");
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let md = std::fs::metadata(m.join("d/sub")).unwrap();
        assert!(md.is_dir());
        assert_eq!(md.permissions().mode() & 0o7777, 0o750);
        assert_eq!(md.nlink(), 2);
        assert_eq!(std::fs::metadata(m.join("d")).unwrap().nlink(), 3);
    });
}

fn pattern(n: usize, seed: u8) -> Vec<u8> {
    (0..n).map(|i| (i % 251) as u8 ^ seed).collect()
}

/// Files too large to store inline: whole free buckets, checksummed
/// extents, alloc, freespace, backpointer, lru and accounting keys -- on
/// the write-study base, which has no lru btree yet (its root is made), and
/// on the aged image, whose buckets are four times larger and partly reused.
#[test]
fn large_files_created_here_are_read_by_the_reference() {
    for (set, dir_path, test, sizes) in [
        (
            "write-study/base.img",
            "/d",
            "large-base",
            vec![300_000usize, 249, 32_768, 1],
        ),
        (
            "aged.img",
            "/many",
            "large-aged",
            vec![1_000_000usize, 131_072 + 1],
        ),
    ] {
        let img = scratch(set, test);
        let dir = {
            let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
            fs.lookup(dir_path).unwrap()
        };
        let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
        // How full the aged image's leaves are differs from one fixture
        // build to the next. Until nodes are split, a create that finds one
        // full is refused with no metadata written: what was created before
        // it must still pass the reference checker and read back.
        let mut created = 0;
        for (i, &n) in sizes.iter().enumerate() {
            match w.create_file(
                dir,
                format!("large-{i}").as_bytes(),
                &pattern(n, i as u8),
                0o644,
            ) {
                Ok(_) => created += 1,
                Err(fs_bcachefs::Error::Unsupported(m))
                    if test == "large-aged" && m.contains("the node is full") =>
                {
                    break
                }
                Err(e) => panic!("{test}: {n} bytes: {e}"),
            }
        }
        drop(w);
        assert_fsck_clean(&img);
        with_reference_mount(&img, |m| {
            for (i, &n) in sizes.iter().enumerate().take(created) {
                let got = std::fs::read(
                    m.join(dir_path.trim_start_matches('/'))
                        .join(format!("large-{i}")),
                )
                .unwrap();
                assert!(
                    got == pattern(n, i as u8),
                    "{test}: large-{i}: the reference read different bytes"
                );
            }
        });
    }
}
