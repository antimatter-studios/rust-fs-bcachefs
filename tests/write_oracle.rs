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

/// Files at and either side of the inline bounds (#79), created here and
/// one existing file rewritten to a mixed size: the extents btree holds the
/// key kinds `data_layout` chose, the reference checker passes the image,
/// and the reference implementation reads every byte back.
#[test]
fn files_at_the_inline_bounds_are_read_by_the_reference() {
    let img = scratch("write-study/base.img", "inline-bounds");
    let sizes = [256usize, 257, 512, 513, 768, 769, 1025, 2100];
    let file = |n: usize| -> Vec<u8> { (0..n).map(|i| (i * 13 + n) as u8).collect() };
    let (dir, existing) = {
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        (fs.lookup("/d").unwrap(), fs.lookup("/d/existing").unwrap())
    };
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    for n in sizes {
        w.create_file(dir, format!("s{n}").as_bytes(), &file(n), 0o644)
            .unwrap_or_else(|e| panic!("create s{n}: {e}"));
    }
    w.write_file(existing, &file(600)).unwrap();
    drop(w);
    let dev = FileDevice::open(&img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let extents = btree::walk(&dev, &sb, btree_id::EXTENTS).unwrap();
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    let all: Vec<(String, usize)> = sizes
        .iter()
        .map(|&n| (format!("s{n}"), n))
        .chain([("existing".to_string(), 600)])
        .collect();
    for (name, n) in &all {
        let ino = fs.lookup(&format!("/d/{name}")).unwrap();
        let kinds: Vec<u8> = extents
            .iter()
            .filter(|k| k.pos.inode == ino)
            .map(|k| k.key_type)
            .collect();
        let (e, i) = fs_bcachefs::write::data_layout(*n, 512);
        assert_eq!(
            (kinds.contains(&6), kinds.contains(&17)),
            (e > 0, i > 0),
            "{name}: key types {kinds:?}"
        );
    }
    assert_fsck_clean(&img);
    with_reference_mount(&img, |m| {
        for (name, n) in &all {
            assert_eq!(
                std::fs::read(m.join("d").join(name)).unwrap(),
                file(*n),
                "{name}: bytes, as the reference reads them"
            );
        }
    });
}

/// Colliding names placed by this crate in the crc32c `collide` image
/// (#78): a fifth after the reference's four, the third removed (leaving a
/// whiteout), a sixth in its place, the fifth removed. The reference
/// checker passes the image and the reference implementation reads every
/// name that is left, and none that is not.
#[test]
fn colliding_names_placed_here_are_read_by_the_reference() {
    let img = scratch("write-study/collide.img", "collide");
    let names = [
        "CAAAAAAAAAAAAAAA",
        "CBBF@MBKAAAAAAAA",
        "COA@GJOCFAAAAAAA",
        "CLBGFFLIFAAAAAAA",
        "CABBF@MBKAAAAAAA",
        "CBAEGLNHKAAAAAAA",
    ];
    let dir = {
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        fs.lookup("/d").unwrap()
    };
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.create_file(dir, names[4].as_bytes(), b"fifth\n", 0o644)
        .unwrap();
    w.unlink(dir, names[2].as_bytes()).unwrap();
    w.create_file(dir, names[5].as_bytes(), b"sixth\n", 0o644)
        .unwrap();
    w.unlink(dir, names[4].as_bytes()).unwrap();
    drop(w);
    assert_fsck_clean(&img);
    with_reference_mount(&img, |m| {
        let d = m.join("d");
        for n in [names[0], names[1], names[3]] {
            assert_eq!(
                std::fs::read(d.join(n)).unwrap(),
                format!("{n}\n").as_bytes(),
                "{n}"
            );
        }
        assert_eq!(std::fs::read(d.join(names[5])).unwrap(), b"sixth\n");
        for gone in [names[2], names[4]] {
            assert!(!d.join(gone).exists(), "{gone} is still listed");
        }
    });
}

/// Files this crate writes on 4096-byte blocks (#87), around one and two
/// blocks: the reference checker passes the image and the reference
/// implementation reads every byte back.
#[test]
fn files_on_4096_byte_blocks_are_read_by_the_reference() {
    let img = scratch("write-study/base-bs4k.img", "bs4k");
    let sizes = [1500usize, 2000, 4096, 5000, 9000];
    let file = |n: usize| -> Vec<u8> { (0..n).map(|i| (i * 31 + n) as u8).collect() };
    let dir = {
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        fs.lookup("/d").unwrap()
    };
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    for n in sizes {
        w.create_file(dir, format!("w{n}").as_bytes(), &file(n), 0o644)
            .unwrap_or_else(|e| panic!("w{n}: {e}"));
    }
    drop(w);
    assert_fsck_clean(&img);
    with_reference_mount(&img, |m| {
        for n in sizes {
            assert_eq!(
                std::fs::read(m.join("d").join(format!("w{n}"))).unwrap(),
                file(n),
                "w{n}: bytes, as the reference reads them"
            );
        }
    });
}

/// The same creation in the aged image, whose btrees are two levels deep
/// and whose leaves hold many bsets. How full its leaves are differs from
/// one fixture build to the next; a full one is rewritten or split, so the
/// create lands whatever their state, and the reference reads it back.
#[test]
fn a_file_created_in_the_aged_image_is_read_by_the_reference() {
    let img = scratch("aged.img", "create-aged");
    let dir = {
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        fs.lookup("/many").unwrap()
    };
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.create_file(
        dir,
        b"written-by-this-crate",
        b"appended to an aged tree\n",
        0o644,
    )
    .unwrap();
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
        for (i, &n) in sizes.iter().enumerate() {
            w.create_file(
                dir,
                format!("large-{i}").as_bytes(),
                &pattern(n, i as u8),
                0o644,
            )
            .unwrap_or_else(|e| panic!("{test}: {n} bytes: {e}"));
        }
        drop(w);
        assert_fsck_clean(&img);
        with_reference_mount(&img, |m| {
            for (i, &n) in sizes.iter().enumerate() {
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

/// Unlinking and rewriting large files frees their buckets the way the
/// reference does (need_discard, a new generation); the checker passes the
/// result and the mount reads what remains.
#[test]
fn freed_space_is_accepted_by_the_reference() {
    let img = scratch("write-study/base.img", "free");
    let dir = {
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        fs.lookup("/d").unwrap()
    };
    let big = pattern(300_000, 7);
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.create_file(dir, b"gone", &big, 0o644).unwrap();
    w.create_file(dir, b"kept", &big, 0o644).unwrap();
    let shrunk = w.create_file(dir, b"shrunk", &big, 0o644).unwrap();
    w.unlink(dir, b"gone").unwrap();
    w.write_file(shrunk, b"small now\n").unwrap();
    let grown = w.create_file(dir, b"grown", b"tiny", 0o644).unwrap();
    w.write_file(grown, &big[..100_000]).unwrap();
    drop(w);
    assert_fsck_clean(&img);
    with_reference_mount(&img, |m| {
        assert!(!m.join("d/gone").exists());
        assert!(std::fs::read(m.join("d/kept")).unwrap() == big);
        assert_eq!(std::fs::read(m.join("d/shrunk")).unwrap(), b"small now\n");
        assert!(std::fs::read(m.join("d/grown")).unwrap() == big[..100_000]);
    });
}

/// Journalled commits: the reference replays the entries this crate wrote
/// and its checker finds nothing to fix; after its own replay its mount
/// reads the result. An entry written without marking the superblock is
/// not replayed at all.
#[test]
fn journalled_commits_are_replayed_by_the_reference() {
    let img = scratch("write-study/base.img", "journal");
    let d = {
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        fs.lookup("/d").unwrap()
    };
    let big = pattern(300_000, 3);
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.journal_commits().unwrap();
    w.create_file(d, b"small", b"journalled\n", 0o644).unwrap();
    w.create_file(d, b"big", &big, 0o644).unwrap();
    w.mkdir(d, b"sub", 0o755).unwrap();
    w.unlink(d, b"existing").unwrap();
    drop(w);
    assert_fsck_clean(&img);
    let (ok, text) = reference(&["fsck", "-y", img.to_str().unwrap()]);
    assert!(ok, "the reference's replay failed:\n{text}");
    with_reference_mount(&img, |m| {
        assert_eq!(std::fs::read(m.join("d/small")).unwrap(), b"journalled\n");
        assert!(std::fs::read(m.join("d/big")).unwrap() == big);
        assert!(m.join("d/sub").is_dir());
        assert!(!m.join("d/existing").exists());
    });

    let img = scratch("write-study/base.img", "journal-crash");
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.journal_commits().unwrap();
    w.crash_before_superblock();
    w.create_file(d, b"lost", b"never committed\n", 0o644)
        .unwrap();
    drop(w);
    assert_fsck_clean(&img);
    with_reference_mount(&img, |m| {
        assert!(!m.join("d/lost").exists());
        assert!(m.join("d/existing").exists());
    });
}

/// Symlinks, hard links, attributes and extended attributes: the checker
/// passes them and the mount sees them.
#[test]
fn links_attributes_and_xattrs_are_read_by_the_reference() {
    let img = scratch("write-study/base.img", "links");
    let (d, existing) = {
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        (fs.lookup("/d").unwrap(), fs.lookup("/d/existing").unwrap())
    };
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.symlink(d, b"link", b"existing").unwrap();
    w.link(existing, d, b"second-name").unwrap();
    w.set_attributes(existing, Some(0o600), Some(1000), Some(1000))
        .unwrap();
    w.set_xattr(existing, b"user.colour", b"blue").unwrap();
    w.set_xattr(existing, b"user.shape", b"round").unwrap();
    w.set_xattr(existing, b"user.colour", b"green").unwrap();
    w.remove_xattr(existing, b"user.shape").unwrap();
    drop(w);
    assert_fsck_clean(&img);
    with_reference_mount(&img, |m| {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        assert_eq!(
            std::fs::read_link(m.join("d/link")).unwrap().to_str(),
            Some("existing")
        );
        let a = std::fs::metadata(m.join("d/existing")).unwrap();
        let b = std::fs::metadata(m.join("d/second-name")).unwrap();
        assert_eq!(a.ino(), b.ino());
        assert_eq!(
            (a.permissions().mode() & 0o7777, a.uid(), a.gid(), a.nlink()),
            (0o600, 1000, 1000, 2)
        );
        // getfattr is installed in the reference tools' chroot
        // (scripts/vm-setup.sh), where the mount is at MNT.
        let out = Command::new("chroot")
            .args([
                REF_ROOT,
                "getfattr",
                "-d",
                "--absolute-names",
                &format!("{MNT}/d/existing"),
            ])
            .output()
            .expect("chroot into the reference tools' root");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("user.colour=\"green\""), "{text}");
        assert!(!text.contains("user.shape"), "{text}");
    });
}

/// Xattrs with names of 8 bytes and more (#77): the reference does not put
/// these at the SipHash slot this writer computes. Set here, the reference
/// checker must pass the image and the reference mount must read them back
/// by name; if it cannot, the writer must refuse such names.
#[test]
fn long_xattr_names_set_here_are_read_by_the_reference() {
    let img = scratch("write-study/base.img", "long-xattrs");
    let existing = {
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        fs.lookup("/d/existing").unwrap()
    };
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.set_xattr(existing, b"user.greeting", b"hello").unwrap();
    w.set_xattr(existing, b"user.on-a-directory", b"dir")
        .unwrap();
    drop(w);
    assert_fsck_clean(&img);
    with_reference_mount(&img, |_| {
        for (name, value) in [("user.greeting", "hello"), ("user.on-a-directory", "dir")] {
            // By name, so the reference looks the xattr up rather than
            // listing every one.
            let out = Command::new("chroot")
                .args([
                    REF_ROOT,
                    "getfattr",
                    "-n",
                    name,
                    "--absolute-names",
                    &format!("{MNT}/d/existing"),
                ])
                .output()
                .expect("chroot into the reference tools' root");
            let text = String::from_utf8_lossy(&out.stdout).into_owned()
                + &String::from_utf8_lossy(&out.stderr);
            assert!(
                text.contains(&format!("{name}=\"{value}\"")),
                "{name}: {text}"
            );
        }
    });
}

/// Hundreds of operations on 32 KiB nodes: full nodes are rewritten into
/// fresh buckets or split, roots grow a level, old buckets are freed; the
/// checker passes the image and the mount reads every file.
#[test]
fn full_nodes_rewritten_and_split_pass_the_reference() {
    let img = scratch("write-study/base.img", "many");
    let d = {
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        fs.lookup("/d").unwrap()
    };
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    for i in 0..400 {
        w.create_file(
            d,
            format!("f{i:05}").as_bytes(),
            format!("file {i}\n").as_bytes(),
            0o644,
        )
        .unwrap_or_else(|e| panic!("create {i}: {e}"));
    }
    for i in (0..400).step_by(3) {
        w.unlink(d, format!("f{i:05}").as_bytes()).unwrap();
    }
    let big = pattern(50_000, 9);
    for i in 0..10 {
        w.create_file(d, format!("big{i}").as_bytes(), &big, 0o644)
            .unwrap();
    }
    drop(w);
    assert_fsck_clean(&img);
    with_reference_mount(&img, |m| {
        assert_eq!(
            std::fs::read_dir(m.join("d")).unwrap().count(),
            400 - 134 + 10 + 1
        );
        for i in (1..400).filter(|i| i % 3 != 0) {
            assert_eq!(
                std::fs::read(m.join(format!("d/f{i:05}"))).unwrap(),
                format!("file {i}\n").as_bytes()
            );
        }
        assert!(std::fs::read(m.join("d/big9")).unwrap() == big);
    });
}

/// Two journalled sessions, the second continuing the journal the first
/// left for replay: the reference replays both and finds nothing to fix.
#[test]
fn a_continued_journal_is_replayed_by_the_reference() {
    let img = scratch("write-study/base.img", "continue");
    let d = {
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        fs.lookup("/d").unwrap()
    };
    let mut w = Writer::open_journalled(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.create_file(d, b"first", b"one\n", 0o644).unwrap();
    drop(w);
    let mut w = Writer::open_journalled(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.create_file(d, b"second", &pattern(70_000, 5), 0o644)
        .unwrap();
    w.unlink(d, b"existing").unwrap();
    drop(w);
    assert_fsck_clean(&img);
    let (ok, text) = reference(&["fsck", "-y", img.to_str().unwrap()]);
    assert!(ok, "the reference's replay failed:\n{text}");
    with_reference_mount(&img, |m| {
        assert_eq!(std::fs::read(m.join("d/first")).unwrap(), b"one\n");
        assert!(std::fs::read(m.join("d/second")).unwrap() == pattern(70_000, 5));
        assert!(!m.join("d/existing").exists());
    });
}

/// A create while the inode allocation cursor (type 35 in the logged_ops
/// btree, 17) names a number in use (#107): the reference checker passes the
/// image, and the reference mount reads every file.
#[test]
fn a_create_over_a_taken_inode_number_is_read_by_the_reference() {
    let img = scratch("write-study/base.img", "ino-search");
    let d = Filesystem::open(FileDevice::open(&img).unwrap())
        .unwrap()
        .lookup("/d")
        .unwrap();
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    let a = w.create_file(d, b"a", b"first\n", 0o644).unwrap();
    w.create_file(d, b"b", b"second\n", 0o644).unwrap();
    drop(w);
    let mut cursor = {
        let dev = FileDevice::open(&img).unwrap();
        let sb = Superblock::read(&dev).unwrap();
        btree::walk(&dev, &sb, 17)
            .unwrap()
            .into_iter()
            .find(|k| k.key_type == 35)
            .expect("an inode allocation cursor")
    };
    cursor.value[8..16].copy_from_slice(&a.to_le_bytes());
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.insert(17, vec![cursor]).unwrap();
    w.create_file(d, b"c", b"third\n", 0o644)
        .unwrap_or_else(|e| panic!("a create over a taken number: {e}"));
    drop(w);
    assert_fsck_clean(&img);
    with_reference_mount(&img, |m| {
        for (name, data) in [("a", "first\n"), ("b", "second\n"), ("c", "third\n")] {
            assert_eq!(
                std::fs::read(m.join(format!("d/{name}"))).unwrap(),
                data.as_bytes(),
                "{name}"
            );
        }
    });
}

/// A rename over an existing name and a directory moved into another one
/// (#104): the reference checker passes the image and the reference mount
/// reads the result.
#[test]
fn rename_over_and_directory_moves_are_read_by_the_reference() {
    let img = scratch("write-study/base.img", "rename-move");
    let d = Filesystem::open(FileDevice::open(&img).unwrap())
        .unwrap()
        .lookup("/d")
        .unwrap();
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.create_file(d, b"other", b"other\n", 0o644).unwrap();
    w.rename(d, b"other", d, b"existing")
        .unwrap_or_else(|e| panic!("rename over existing: {e}"));
    let a = w.mkdir(d, b"a", 0o755).unwrap();
    let b = w.mkdir(d, b"b", 0o755).unwrap();
    let x = w.mkdir(a, b"x", 0o755).unwrap();
    w.create_file(x, b"f", b"in x\n", 0o644).unwrap();
    w.rename(a, b"x", b, b"x")
        .unwrap_or_else(|e| panic!("move a directory: {e}"));
    drop(w);
    assert_fsck_clean(&img);
    with_reference_mount(&img, |m| {
        assert_eq!(std::fs::read(m.join("d/existing")).unwrap(), b"other\n");
        assert!(!m.join("d/other").exists());
        assert_eq!(std::fs::read(m.join("d/b/x/f")).unwrap(), b"in x\n");
        assert!(!m.join("d/a/x").exists());
        use std::os::unix::fs::MetadataExt;
        assert_eq!(std::fs::metadata(m.join("d/a")).unwrap().nlink(), 2);
        assert_eq!(std::fs::metadata(m.join("d/b")).unwrap().nlink(), 3);
    });
}

/// Colliding xattr names on a crc32c image (#106), set, one removed and set
/// again: the reference checker passes the image and the reference mount
/// reads each one by name.
#[test]
fn colliding_xattrs_set_here_are_read_by_the_reference() {
    let img = scratch("write-study/collide.img", "xcollide");
    let f = Filesystem::open(FileDevice::open(&img).unwrap())
        .unwrap()
        .lookup("/d/plain")
        .unwrap();
    let names = [
        "user.CAAAAAAAAAAAAAAA",
        "user.CBBF@MBKAAAAAAAA",
        "user.COA@GJOCFAAAAAAA",
        "user.CLBGFFLIFAAAAAAA",
    ];
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    for n in names {
        w.set_xattr(f, n.as_bytes(), n.as_bytes())
            .unwrap_or_else(|e| panic!("set {n}: {e}"));
    }
    w.remove_xattr(f, names[1].as_bytes()).unwrap();
    w.set_xattr(f, names[1].as_bytes(), b"again").unwrap();
    drop(w);
    assert_fsck_clean(&img);
    with_reference_mount(&img, |_| {
        for (i, name) in names.iter().enumerate() {
            let value = if i == 1 { "again" } else { name };
            let out = Command::new("chroot")
                .args([
                    REF_ROOT,
                    "getfattr",
                    "-n",
                    name,
                    "--absolute-names",
                    &format!("{MNT}/d/plain"),
                ])
                .output()
                .expect("chroot into the reference tools' root");
            let text = String::from_utf8_lossy(&out.stdout).into_owned()
                + &String::from_utf8_lossy(&out.stderr);
            assert!(
                text.contains(&format!("{name}=\"{value}\"")),
                "{name}: {text}"
            );
        }
    });
}

/// Writes into part of a file, an append and truncations (#102): the
/// reference checker passes the image and the reference mount reads the
/// bytes the model says.
#[test]
fn ranged_writes_are_read_by_the_reference() {
    let img = scratch("write-study/base.img", "ranged");
    let d = Filesystem::open(FileDevice::open(&img).unwrap())
        .unwrap()
        .lookup("/d")
        .unwrap();
    let mut model = pattern(70_000, 3);
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    let f = w.create_file(d, b"ranged", &model, 0o644).unwrap();
    w.write_at(f, 1000, &[b'a'; 300]).unwrap();
    model[1000..1300].fill(b'a');
    w.write_at(f, 80_000, b"past the end").unwrap();
    model.resize(80_000, 0);
    model.extend_from_slice(b"past the end");
    w.append(f, b"appended").unwrap();
    model.extend_from_slice(b"appended");
    w.truncate(f, 75_000).unwrap();
    model.truncate(75_000);
    drop(w);
    assert_fsck_clean(&img);
    with_reference_mount(&img, |m| {
        assert!(std::fs::read(m.join("d/ranged")).unwrap() == model);
    });
}

/// Files written on every data checksum and on compressed filesystems
/// (#105): the reference checker passes each image and the reference mount
/// reads the file back.
#[test]
fn files_on_every_data_checksum_and_compression_are_read_by_the_reference() {
    let data = pattern(70_000, 11);
    for set in ["crc64", "xxhash", "nocsum", "lz4", "zstd", "gzip"] {
        let img = scratch(&format!("{set}.img"), &format!("data-{set}"));
        let root = Filesystem::open(FileDevice::open(&img).unwrap())
            .unwrap()
            .lookup("/")
            .unwrap();
        let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
        w.create_file(root, b"written-here", &data, 0o644)
            .unwrap_or_else(|e| panic!("{set}: {e}"));
        drop(w);
        assert_fsck_clean(&img);
        with_reference_mount(&img, |m| {
            assert!(
                std::fs::read(m.join("written-here")).unwrap() == data,
                "{set}: the reference read other bytes"
            );
        });
    }
}
