//! The write path without the reference tools: what this crate writes, this
//! crate reads back, on every architecture the host tiers run on. The
//! verdict on the bytes themselves is tests/write_oracle.rs, in the guest.
#![cfg(feature = "write")]

mod common;

use common::fixture;
use fs_bcachefs::write::Writer;
use fs_bcachefs::Filesystem;
use fs_core::FileDevice;

fn scratch(name: &str, test: &str) -> std::path::PathBuf {
    // The system's temporary directory, not the checkout's tmp/: these
    // images are tens of megabytes each, and a checkout on slow storage
    // made copying them most of the run (measured: 384 s on an SD card).
    let dir = std::env::temp_dir().join(format!(
        "rust-fs-bcachefs-write-local-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let img = dir.join(format!("{test}.img"));
    std::fs::copy(fixture(name), &img).unwrap();
    img
}

/// The two files the create test makes, in order.
const CREATES: [(&[u8], &[u8], u32); 2] = [(b"one", b"first\n", 0o644), (b"two", b"", 0o600)];

/// Whether making `CREATES` in `dir`, on a copy of `img`, is refused
/// because a node one of them would go in is full; when one is, the copy
/// must be byte for byte what it was before that create. Every create the
/// test makes is probed: a leaf with room for one entry passes a probe of
/// one and fails the test's second (measured in CI on an aged image).
fn refused_as_full(img: &std::path::Path, dir: u64) -> bool {
    let probe = img.with_extension("probe.img");
    std::fs::copy(img, &probe).unwrap();
    let mut refused = false;
    for (name, data, mode) in CREATES {
        let before = std::fs::read(&probe).unwrap();
        let mut w = Writer::open(FileDevice::open_rw(&probe).unwrap()).unwrap();
        match w.create_file(dir, name, data, mode) {
            Ok(_) => {}
            Err(fs_bcachefs::Error::Unsupported(m)) if m.contains("the node is full") => {
                drop(w);
                assert!(
                    std::fs::read(&probe).unwrap() == before,
                    "refused, but written"
                );
                refused = true;
                break;
            }
            Err(e) => panic!("{e}"),
        }
    }
    std::fs::remove_file(&probe).unwrap();
    refused
}

#[test]
fn created_files_read_back_and_the_old_ones_are_untouched() {
    for (set, dir_path, test) in [
        ("write-study/base.img", "/d", "local-base"),
        ("aged.img", "/many", "local-aged"),
    ] {
        let img = scratch(set, test);
        let (dir, before) = {
            let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
            let dir = fs.lookup(dir_path).unwrap();
            (dir, fs.readdir(dir).unwrap().len())
        };
        // The aged image is aged by a live mount, so how full its leaves are
        // differs from one fixture build to the next. Until nodes are split,
        // a full leaf must be refused before anything is written.
        if test == "local-aged" && refused_as_full(&img, dir) {
            continue;
        }
        let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
        let [(n1, d1, m1), (n2, d2, m2)] = CREATES;
        let a = w.create_file(dir, n1, d1, m1).unwrap();
        let b = w.create_file(dir, n2, d2, m2).unwrap();
        assert_eq!(b, a + 1, "{test}: inode numbers follow the cursor");
        drop(w);
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        assert_eq!(fs.readdir(dir).unwrap().len(), before + 2, "{test}");
        assert_eq!(
            fs.read(fs.lookup(&format!("{dir_path}/one")).unwrap())
                .unwrap(),
            b"first\n"
        );
        let two = fs
            .inode(fs.lookup(&format!("{dir_path}/two")).unwrap())
            .unwrap();
        assert_eq!(
            (two.size, two.mode & 0o7777, two.link_count()),
            (0, 0o600, 1),
            "{test}"
        );
    }
}

#[test]
fn what_is_refused_is_refused_before_anything_is_written() {
    let img = scratch("write-study/base.img", "local-refused");
    let before = std::fs::read(&img).unwrap();
    let dir = {
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        fs.lookup("/d").unwrap()
    };
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    // Larger than the 64 MiB image's free space: refused while planning,
    // before any data is written.
    assert!(
        w.create_file(dir, b"big", &vec![0u8; 80 << 20], 0o644)
            .is_err(),
        "larger than the free space"
    );
    assert!(
        w.create_file(dir, b"existing", b"x", 0o644).is_err(),
        "a name already there"
    );
    assert!(
        w.create_file(dir, b"a/b", b"x", 0o644).is_err(),
        "a slash in the name"
    );
    assert!(
        w.create_file(4096 + 999_999, b"x", b"x", 0o644).is_err(),
        "no such directory"
    );
    drop(w);
    assert!(
        std::fs::read(&img).unwrap() == before,
        "a refused create wrote to the image"
    );
    let unclean = scratch("aged-unclean.img", "local-unclean");
    assert!(
        Writer::open(FileDevice::open_rw(&unclean).unwrap()).is_err(),
        "an unclean image"
    );
}

/// mkdir, unlink, rmdir, rename and rewriting a file's contents, read back
/// by this crate.
#[test]
fn namespace_operations_read_back() {
    let img = scratch("write-study/base.img", "local-namespace");
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
    let e = w.mkdir(d, b"empty-dir", 0o755).unwrap();
    w.rmdir(d, b"empty-dir").unwrap();
    assert!(w.rmdir(d, b"sub").is_err(), "a directory that is not empty");
    drop(w);
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    let mut names: Vec<String> = fs
        .readdir(d)
        .unwrap()
        .iter()
        .map(|x| String::from_utf8_lossy(&x.name).into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["rewritten", "sub"]);
    assert_eq!(
        fs.read(fs.lookup("/d/sub/inner.txt").unwrap()).unwrap(),
        b"inside\n"
    );
    assert_eq!(
        fs.read(fs.lookup("/d/sub/moved").unwrap()).unwrap(),
        b"an existing file\n"
    );
    assert_eq!(
        fs.read(fs.lookup("/d/rewritten").unwrap()).unwrap(),
        b"second\n"
    );
    let s = fs.inode(sub).unwrap();
    assert!(s.is_dir());
    assert_eq!((s.mode & 0o7777, s.link_count()), (0o750, 2));
    assert_eq!(
        fs.inode(d).unwrap().link_count(),
        3,
        "/d holds one subdirectory"
    );
    assert!(
        fs.inode(e).is_err(),
        "the removed directory's inode is gone"
    );
}

/// Large files read back by this crate, byte for byte.
#[test]
fn large_files_read_back() {
    for (set, dir_path, test) in [
        ("write-study/base.img", "/d", "local-large-base"),
        ("aged.img", "/many", "local-large-aged"),
    ] {
        let img = scratch(set, test);
        let dir = {
            let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
            fs.lookup(dir_path).unwrap()
        };
        let data: Vec<u8> = (0..700_001u32).map(|i| (i % 253) as u8).collect();
        let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
        // How full the aged image's leaves are differs from one fixture
        // build to the next. Until nodes are split, a create that finds one
        // full is refused with no metadata written: the name must not exist.
        match w.create_file(dir, b"large", &data, 0o644) {
            Ok(_) => {}
            Err(fs_bcachefs::Error::Unsupported(m))
                if test == "local-large-aged" && m.contains("the node is full") =>
            {
                drop(w);
                let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
                assert!(fs.lookup(&format!("{dir_path}/large")).is_err(), "{test}");
                continue;
            }
            Err(e) => panic!("{test}: {e}"),
        }
        drop(w);
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        let ino = fs.lookup(&format!("{dir_path}/large")).unwrap();
        assert!(fs.read(ino).unwrap() == data, "{test}");
        assert_eq!(
            fs.inode(ino).unwrap().sectors,
            700_001u64.div_ceil(512),
            "{test}: sectors"
        );
    }
}

/// Large files unlinked and rewritten: the space comes back and what
/// remains reads back.
#[test]
fn allocated_space_is_freed() {
    let img = scratch("write-study/base.img", "local-free");
    let dir = {
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        fs.lookup("/d").unwrap()
    };
    let big: Vec<u8> = (0..300_000u32).map(|i| (i % 249) as u8).collect();
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.create_file(dir, b"gone", &big, 0o644).unwrap();
    let kept = w.create_file(dir, b"kept", &big, 0o644).unwrap();
    let shrunk = w.create_file(dir, b"shrunk", &big, 0o644).unwrap();
    w.unlink(dir, b"gone").unwrap();
    w.write_file(shrunk, b"small now\n").unwrap();
    let grown = w.create_file(dir, b"grown", b"tiny", 0o644).unwrap();
    w.write_file(grown, &big[..100_000]).unwrap();
    drop(w);
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    assert!(fs.lookup("/d/gone").is_err());
    assert!(fs.read(kept).unwrap() == big);
    assert_eq!(fs.read(shrunk).unwrap(), b"small now\n");
    assert!(fs.read(grown).unwrap() == big[..100_000]);
}

/// Journalled commits: this crate reads the result through its own replay,
/// and an entry whose superblock never said "replay me" changes nothing.
#[test]
fn journalled_commits_read_back_and_an_unmarked_one_is_ignored() {
    let img = scratch("write-study/base.img", "local-journal");
    let d = {
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        fs.lookup("/d").unwrap()
    };
    let big: Vec<u8> = (0..300_000u32).map(|i| (i % 247) as u8).collect();
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.journal_commits().unwrap();
    w.create_file(d, b"small", b"journalled\n", 0o644).unwrap();
    w.create_file(d, b"big", &big, 0o644).unwrap();
    w.mkdir(d, b"sub", 0o755).unwrap();
    w.unlink(d, b"existing").unwrap();
    drop(w);
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        !fs.superblock().is_clean(),
        "journalled commits leave the image for replay"
    );
    assert_eq!(
        fs.read(fs.lookup("/d/small").unwrap()).unwrap(),
        b"journalled\n"
    );
    assert!(fs.read(fs.lookup("/d/big").unwrap()).unwrap() == big);
    assert!(fs.inode(fs.lookup("/d/sub").unwrap()).unwrap().is_dir());
    assert!(fs.lookup("/d/existing").is_err());

    let img = scratch("write-study/base.img", "local-journal-crash");
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.journal_commits().unwrap();
    w.crash_before_superblock();
    w.create_file(d, b"lost", b"never committed\n", 0o644)
        .unwrap();
    drop(w);
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    assert!(fs.superblock().is_clean());
    assert!(
        fs.lookup("/d/lost").is_err(),
        "an entry the superblock never marked was read"
    );
    assert_eq!(
        fs.read(fs.lookup("/d/existing").unwrap()).unwrap(),
        b"an existing file\n"
    );
}

/// Symlinks, hard links, attributes and extended attributes, read back.
#[test]
fn links_attributes_and_xattrs_read_back() {
    let img = scratch("write-study/base.img", "local-links");
    let d = {
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        fs.lookup("/d").unwrap()
    };
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    let existing = {
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        fs.lookup("/d/existing").unwrap()
    };
    let l = w.symlink(d, b"link", b"existing").unwrap();
    w.link(existing, d, b"second-name").unwrap();
    w.set_attributes(existing, Some(0o600), Some(1000), Some(1000))
        .unwrap();
    w.set_xattr(existing, b"user.colour", b"blue").unwrap();
    w.set_xattr(existing, b"user.shape", b"round").unwrap();
    w.set_xattr(existing, b"user.colour", b"green").unwrap();
    w.remove_xattr(existing, b"user.shape").unwrap();
    drop(w);
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    assert!(fs.inode(l).unwrap().is_symlink());
    assert_eq!(fs.read(fs.lookup("/d/link").unwrap()).unwrap(), b"existing");
    assert_eq!(fs.lookup("/d/second-name").unwrap(), existing);
    let i = fs.inode(existing).unwrap();
    assert_eq!(
        (i.mode & 0o7777, i.uid, i.gid, i.link_count()),
        (0o600, 1000, 1000, 2)
    );
    let x: Vec<(Vec<u8>, Vec<u8>)> = fs
        .xattrs(existing)
        .unwrap()
        .into_iter()
        .map(|x| (x.name, x.value))
        .collect();
    assert_eq!(x, vec![(b"user.colour".to_vec(), b"green".to_vec())]);
}

/// A superblock whose time precision is not nanoseconds: the times this
/// writer stamps would be in the wrong unit, so it refuses before writing.
#[test]
fn a_time_precision_other_than_nanoseconds_is_refused_before_writing() {
    use fs_bcachefs::superblock::{Superblock, SB_HEADER_BYTES, SB_OFFSET};
    use std::io::{Read, Seek, SeekFrom, Write};
    let img = scratch("write-study/base.img", "precision");
    let sb = Superblock::read(&FileDevice::open(&img).unwrap()).unwrap();
    assert_eq!(sb.time_precision, 1, "the fixture stamps nanoseconds");
    // Every copy: the reader takes the highest seq, and they all share one.
    for &sector in &sb.layout.sb_offsets {
        let mut f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&img)
            .unwrap();
        f.seek(SeekFrom::Start(sector * 512)).unwrap();
        let mut b = vec![0u8; SB_HEADER_BYTES + sb.u64s as usize * 8];
        f.read_exact(&mut b).unwrap();
        b[0x8c..0x90].copy_from_slice(&1000u32.to_le_bytes());
        let c = fs_bcachefs::csum::compute(sb.csum_type(), &b[16..]).unwrap();
        b[0..16].fill(0);
        b[0..8].copy_from_slice(&c.to_le_bytes());
        f.seek(SeekFrom::Start(sector * 512)).unwrap();
        f.write_all(&b).unwrap();
    }
    let _ = SB_OFFSET;
    let before = std::fs::read(&img).unwrap();
    match Writer::open(FileDevice::open_rw(&img).unwrap()) {
        Err(fs_bcachefs::Error::Unsupported(m)) if m.contains("precision") => {}
        Ok(_) => panic!("a writer opened a filesystem whose time unit it does not stamp"),
        Err(e) => panic!("{e}"),
    }
    assert!(
        std::fs::read(&img).unwrap() == before,
        "refused, but written"
    );
    // The reader is unaffected: times are reported in the superblock's unit.
    Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
}
