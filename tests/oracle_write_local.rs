//! The write path without the reference tools: what this crate writes, this
//! crate reads back, on every architecture the host tiers run on. The
//! verdict on the bytes themselves is tests/write_oracle.rs, in the guest.
#![cfg(feature = "write")]

mod common;

use common::fixture;
use fs_bcachefs::write::Writer;
use fs_bcachefs::{Error, Filesystem};
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
        w.create_file(dir, b"large", &data, 0o644).unwrap();
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

/// Names that collide under crc32c in the write study's `collide` image
/// (tests/oracle_collisions.rs): the four the reference placed, then two
/// more.
const COLLIDING: [&str; 6] = [
    "CAAAAAAAAAAAAAAA",
    "CBBF@MBKAAAAAAAA",
    "COA@GJOCFAAAAAAA",
    "CLBGFFLIFAAAAAAA",
    "CABBF@MBKAAAAAAA",
    "CBAEGLNHKAAAAAAA",
];

/// `(offset, key type, name)` of every key of directory `dir`.
fn dir_keys(img: &std::path::Path, dir: u64) -> Vec<(u64, u8, String)> {
    use fs_bcachefs::btree::{self, btree_id};
    let dev = FileDevice::open(img).unwrap();
    let sb = fs_bcachefs::superblock::Superblock::read(&dev).unwrap();
    btree::walk(&dev, &sb, btree_id::DIRENTS)
        .unwrap()
        .into_iter()
        .filter(|k| k.pos.inode == dir)
        .map(|k| {
            let name = fs_bcachefs::inode::Dirent::from_key(&k)
                .map(|d| String::from_utf8_lossy(&d.name).into_owned())
                .unwrap_or_default();
            (k.pos.offset, k.key_type, name)
        })
        .collect()
}

/// Colliding names placed here go where the reference puts them (#78): a
/// fifth after the four, a removal inside the run leaves a whiteout, a new
/// colliding name takes it, and a removal at the run's end leaves nothing.
#[test]
fn colliding_names_are_placed_as_the_reference_places_them() {
    let img = scratch("write-study/collide.img", "collide");
    let dir = {
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        fs.lookup("/d").unwrap()
    };
    let h = dir_keys(&img, dir)
        .iter()
        .find(|k| k.2 == COLLIDING[0])
        .unwrap()
        .0;
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.create_file(dir, COLLIDING[4].as_bytes(), b"fifth\n", 0o644)
        .unwrap();
    w.unlink(dir, COLLIDING[2].as_bytes()).unwrap();
    w.create_file(dir, COLLIDING[5].as_bytes(), b"sixth\n", 0o644)
        .unwrap();
    w.unlink(dir, COLLIDING[4].as_bytes()).unwrap();
    drop(w);
    let run: Vec<(u64, u8, String)> = dir_keys(&img, dir)
        .into_iter()
        .filter(|k| k.0 >= h && k.0 < h + 8)
        .collect();
    let dirent = 10;
    assert_eq!(
        run,
        [
            (h, dirent, COLLIDING[0].to_string()),
            (h + 1, dirent, COLLIDING[1].to_string()),
            (h + 2, dirent, COLLIDING[5].to_string()),
            (h + 3, dirent, COLLIDING[3].to_string()),
        ]
    );
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    for n in [COLLIDING[0], COLLIDING[1], COLLIDING[3]] {
        assert_eq!(
            fs.read(fs.lookup(&format!("/d/{n}")).unwrap()).unwrap(),
            format!("{n}\n").as_bytes()
        );
    }
    assert_eq!(
        fs.read(fs.lookup(&format!("/d/{}", COLLIDING[5])).unwrap())
            .unwrap(),
        b"sixth\n"
    );
    for gone in [COLLIDING[2], COLLIDING[4]] {
        assert!(fs.lookup(&format!("/d/{gone}")).is_err(), "{gone}");
    }
}

/// Files written here on 4096-byte blocks (#87): each extent is cut and
/// sized in whole blocks, and a short last block stays inline, as
/// `data_layout` says; this crate reads every byte back.
#[test]
fn files_on_4096_byte_blocks_are_written_in_whole_blocks() {
    use fs_bcachefs::btree::{self, btree_id};
    let img = scratch("write-study/base-bs4k.img", "bs4k");
    let dir = {
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        fs.lookup("/d").unwrap()
    };
    let sizes = [1500usize, 2000, 4096, 5000, 9000];
    let file = |n: usize| -> Vec<u8> { (0..n).map(|i| (i * 31 + n) as u8).collect() };
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    for n in sizes {
        w.create_file(dir, format!("w{n}").as_bytes(), &file(n), 0o644)
            .unwrap_or_else(|e| panic!("w{n}: {e}"));
    }
    drop(w);
    let dev = FileDevice::open(&img).unwrap();
    let sb = fs_bcachefs::superblock::Superblock::read(&dev).unwrap();
    let keys = btree::walk(&dev, &sb, btree_id::EXTENTS).unwrap();
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    for n in sizes {
        let ino = fs.lookup(&format!("/d/w{n}")).unwrap();
        let (extents, inline) = fs_bcachefs::write::data_layout(n, 4096);
        let mine: Vec<_> = keys.iter().filter(|k| k.pos.inode == ino).collect();
        for k in mine.iter().filter(|k| k.key_type == 6) {
            assert_eq!(k.size % 8, 0, "w{n}: an extent of {} sectors", k.size);
            assert_eq!(
                k.pos.offset % 8,
                0,
                "w{n}: an extent ending at {}",
                k.pos.offset
            );
        }
        let ext: u64 = mine
            .iter()
            .filter(|k| k.key_type == 6)
            .map(|k| u64::from(k.size))
            .sum();
        assert_eq!(ext, (extents.div_ceil(4096) * 8) as u64, "w{n}");
        assert_eq!(
            mine.iter().any(|k| k.key_type == 17),
            inline > 0,
            "w{n}: inline"
        );
        assert_eq!(fs.read(ino).unwrap(), file(n), "w{n}: bytes");
    }
}

/// Hundreds of operations, one transaction each, on an image whose nodes
/// are 32 KiB: nodes fill and are rewritten or split, roots grow a level,
/// and everything still reads back.
#[test]
fn full_nodes_are_rewritten_and_split() {
    let img = scratch("write-study/base.img", "local-many");
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
        w.unlink(d, format!("f{i:05}").as_bytes())
            .unwrap_or_else(|e| panic!("unlink {i}: {e}"));
    }
    let big: Vec<u8> = (0..50_000u32).map(|i| (i % 251) as u8).collect();
    for i in 0..10 {
        w.create_file(d, format!("big{i}").as_bytes(), &big, 0o644)
            .unwrap();
    }
    drop(w);
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    assert_eq!(fs.readdir(d).unwrap().len(), 400 - 134 + 10 + 1);
    assert_eq!(
        fs.read(fs.lookup("/d/f00398").unwrap()).unwrap(),
        b"file 398\n"
    );
    assert!(
        fs.lookup("/d/f00399").is_err(),
        "399 is a multiple of 3: unlinked"
    );
    assert!(fs.read(fs.lookup("/d/big9").unwrap()).unwrap() == big);
}

/// A journal left for replay is continued by the next writer, and one
/// replay applies both sessions.
#[test]
fn a_journal_left_for_replay_is_continued() {
    let img = scratch("write-study/base.img", "local-continue");
    let d = {
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        fs.lookup("/d").unwrap()
    };
    let mut w = Writer::open_journalled(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.create_file(d, b"first", b"one\n", 0o644).unwrap();
    drop(w);
    assert!(
        Writer::open(FileDevice::open_rw(&img).unwrap()).is_err(),
        "in place on an unreplayed journal"
    );
    let mut w = Writer::open_journalled(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.create_file(d, b"second", b"two\n", 0o644).unwrap();
    w.unlink(d, b"existing").unwrap();
    drop(w);
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    assert_eq!(fs.read(fs.lookup("/d/first").unwrap()).unwrap(), b"one\n");
    assert_eq!(fs.read(fs.lookup("/d/second").unwrap()).unwrap(), b"two\n");
    assert!(fs.lookup("/d/existing").is_err());
}

/// Creates `<prefix>0000`, `<prefix>0001`, ... in `dir`, each holding its
/// own name, until one is refused or `cap` are made.
fn create_until_refused(
    w: &mut Writer<FileDevice>,
    dir: u64,
    prefix: &str,
    cap: usize,
) -> (Vec<String>, Option<Error>) {
    let mut made = Vec::new();
    while made.len() < cap {
        let name = format!("{prefix}{:04}", made.len());
        match w.create_file(dir, name.as_bytes(), name.as_bytes(), 0o644) {
            Ok(_) => made.push(name),
            Err(e) => return (made, Some(e)),
        }
    }
    (made, None)
}

/// A journalled session longer than its journal (#101, #113). The base
/// image's journal is 16 buckets of 32 KiB, 1024 blocks, and every entry
/// takes at least one block, so 1100 creates cannot fit: when the journal
/// is full it is reclaimed, everything it holds written into the nodes in
/// place and the superblock marked clean, and the session goes on. A
/// session continuing that journal does the same. Every file reads back.
#[test]
fn a_full_journal_is_reclaimed_and_the_session_goes_on() {
    let img = scratch("write-study/base.img", "local-journal-full");
    let d = {
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        fs.lookup("/d").unwrap()
    };
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.journal_commits().unwrap();
    let (mut made, refused) = create_until_refused(&mut w, d, "j", 1100);
    drop(w);
    assert!(
        refused.is_none(),
        "{} journalled creates, then: {refused:?}",
        made.len()
    );
    let mut w = Writer::open_journalled(FileDevice::open_rw(&img).unwrap()).unwrap();
    let (more, refused) = create_until_refused(&mut w, d, "k", 1100);
    drop(w);
    assert!(
        refused.is_none(),
        "{} creates in a continued session, then: {refused:?}",
        more.len()
    );
    made.extend(more);
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    assert_eq!(fs.readdir(d).unwrap().len(), made.len() + 1);
    for name in &made {
        let f = fs.lookup(&format!("/d/{name}")).unwrap();
        assert_eq!(fs.read(f).unwrap(), name.as_bytes());
    }
}

/// The inode allocation cursor's key: type 35 in the logged_ops btree (17).
fn inode_cursor(img: &std::path::Path) -> fs_bcachefs::bkey::Bkey {
    use fs_bcachefs::{btree, superblock::Superblock};
    let dev = FileDevice::open(img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    btree::walk(&dev, &sb, 17)
        .unwrap()
        .into_iter()
        .find(|k| k.key_type == 35)
        .expect("an inode allocation cursor")
}

/// A cursor whose next number is already an inode's (#107), as a cursor
/// lagging another one would leave it (S1 11.5: the cursors are per-CPU):
/// a create takes the next free number above it, and the cursor moves past
/// what it took.
#[test]
fn a_create_skips_inode_numbers_already_in_use() {
    let img = scratch("write-study/base.img", "ino-search");
    let d = Filesystem::open(FileDevice::open(&img).unwrap())
        .unwrap()
        .lookup("/d")
        .unwrap();
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    let a = w.create_file(d, b"a", b"first\n", 0o644).unwrap();
    let b = w.create_file(d, b"b", b"second\n", 0o644).unwrap();
    drop(w);
    // Back to `a`: its number and the one after it are both taken.
    let mut cursor = inode_cursor(&img);
    cursor.value[8..16].copy_from_slice(&a.to_le_bytes());
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.insert(17, vec![cursor]).unwrap();
    let c = w
        .create_file(d, b"c", b"third\n", 0o644)
        .unwrap_or_else(|e| panic!("a create over a taken number: {e}"));
    drop(w);
    assert_eq!(c, b + 1, "the next free number above the cursor");
    let next = u64::from_le_bytes(inode_cursor(&img).value[8..16].try_into().unwrap());
    assert_eq!(next, c + 1, "the cursor moves past what the create took");
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    for (name, ino, data) in [
        ("a", a, "first\n"),
        ("b", b, "second\n"),
        ("c", c, "third\n"),
    ] {
        assert_eq!(fs.lookup(&format!("/d/{name}")).unwrap(), ino, "{name}");
        assert_eq!(fs.read(ino).unwrap(), data.as_bytes(), "{name}");
    }
}

/// A rename over an existing name (#104): the name now holds the renamed
/// file, the old name is gone, and the file it replaced is no longer found.
#[test]
fn a_rename_over_an_existing_name_replaces_it() {
    let img = scratch("write-study/base.img", "rename-over");
    let d = Filesystem::open(FileDevice::open(&img).unwrap())
        .unwrap()
        .lookup("/d")
        .unwrap();
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    let other = w.create_file(d, b"other", b"other\n", 0o644).unwrap();
    w.rename(d, b"other", d, b"existing")
        .unwrap_or_else(|e| panic!("rename over existing: {e}"));
    drop(w);
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    assert_eq!(fs.lookup("/d/existing").unwrap(), other);
    assert_eq!(fs.read(other).unwrap(), b"other\n");
    assert!(fs.lookup("/d/other").is_err(), "the old name is gone");
    assert_eq!(fs.readdir(d).unwrap().len(), 1, "one entry in /d");
}

/// A directory moved into another directory (#104): it and what it holds
/// are found at the new path, and each parent's link count follows its
/// subdirectories.
#[test]
fn a_directory_moves_to_another_directory() {
    let img = scratch("write-study/base.img", "move-dir");
    let d = Filesystem::open(FileDevice::open(&img).unwrap())
        .unwrap()
        .lookup("/d")
        .unwrap();
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    let a = w.mkdir(d, b"a", 0o755).unwrap();
    let b = w.mkdir(d, b"b", 0o755).unwrap();
    let x = w.mkdir(a, b"x", 0o755).unwrap();
    let f = w.create_file(x, b"f", b"in x\n", 0o644).unwrap();
    w.rename(a, b"x", b, b"x")
        .unwrap_or_else(|e| panic!("move a directory: {e}"));
    drop(w);
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    assert_eq!(fs.lookup("/d/b/x").unwrap(), x);
    assert_eq!(fs.lookup("/d/b/x/f").unwrap(), f);
    assert_eq!(fs.read(f).unwrap(), b"in x\n");
    assert!(fs.lookup("/d/a/x").is_err(), "gone from its old parent");
    assert_eq!(
        fs.inode(a).unwrap().link_count(),
        2,
        "a lost a subdirectory"
    );
    assert_eq!(fs.inode(b).unwrap().link_count(), 3, "b gained one");
}

/// A directory cannot move into itself or below it, as rename(2) refuses,
/// and the refusal writes nothing.
#[test]
fn a_directory_cannot_move_below_itself() {
    let img = scratch("write-study/base.img", "move-below");
    let d = Filesystem::open(FileDevice::open(&img).unwrap())
        .unwrap()
        .lookup("/d")
        .unwrap();
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    let a = w.mkdir(d, b"a", 0o755).unwrap();
    let x = w.mkdir(a, b"x", 0o755).unwrap();
    drop(w);
    let before = std::fs::read(&img).unwrap();
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    for into in [a, x] {
        assert!(
            w.rename(d, b"a", into, b"a").is_err(),
            "moved /d/a into inode {into}"
        );
    }
    drop(w);
    assert!(
        std::fs::read(&img).unwrap() == before,
        "refused, but written"
    );
}

/// The names that collide under crc32c (tests/oracle_collisions.rs), as
/// xattr names on a crc32c image (#106): all four are set, the second is
/// removed and set again, and every one reads back by name.
#[test]
fn colliding_xattrs_on_a_crc32c_image_read_back() {
    let img = scratch("write-study/collide.img", "xcollide");
    let f = Filesystem::open(FileDevice::open(&img).unwrap())
        .unwrap()
        .lookup("/d/plain")
        .unwrap();
    let names: Vec<String> = COLLIDING[..4].iter().map(|n| format!("user.{n}")).collect();
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    for n in &names {
        w.set_xattr(f, n.as_bytes(), n.as_bytes())
            .unwrap_or_else(|e| panic!("set {n}: {e}"));
    }
    w.remove_xattr(f, names[1].as_bytes()).unwrap();
    w.set_xattr(f, names[1].as_bytes(), b"again").unwrap();
    drop(w);
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    let mut got: Vec<(Vec<u8>, Vec<u8>)> = fs
        .xattrs(f)
        .unwrap()
        .into_iter()
        .map(|x| (x.name, x.value))
        .collect();
    got.sort();
    let mut want: Vec<(Vec<u8>, Vec<u8>)> = names
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let v = if i == 1 {
                b"again".to_vec()
            } else {
                n.as_bytes().to_vec()
            };
            (n.as_bytes().to_vec(), v)
        })
        .collect();
    want.sort();
    assert_eq!(got, want);
}

/// Writes into part of a file (#102): at an offset inside it, across its
/// end, past its end with a gap, an append, and truncations shorter and
/// longer; after each, every byte reads back as the model says.
#[test]
fn ranged_writes_appends_and_truncations_read_back() {
    let img = scratch("write-study/base.img", "ranged");
    let d = Filesystem::open(FileDevice::open(&img).unwrap())
        .unwrap()
        .lookup("/d")
        .unwrap();
    let start: Vec<u8> = (0..70_000u32).map(|i| (i % 251) as u8).collect();
    let mut model = start.clone();
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    let f = w.create_file(d, b"ranged", &start, 0o644).unwrap();
    let step = |model: &mut Vec<u8>, offset: usize, data: &[u8]| {
        if offset + data.len() > model.len() {
            model.resize(offset + data.len(), 0);
        }
        model[offset..offset + data.len()].copy_from_slice(data);
    };
    for (offset, data) in [
        (1000usize, vec![b'a'; 300]),
        (69_900, vec![b'b'; 500]),
        (80_000, vec![b'c'; 10]),
    ] {
        w.write_at(f, offset as u64, &data)
            .unwrap_or_else(|e| panic!("write at {offset}: {e}"));
        step(&mut model, offset, &data);
    }
    w.append(f, b"appended").unwrap();
    let end = model.len();
    step(&mut model, end, b"appended");
    w.truncate(f, 5000).unwrap();
    model.truncate(5000);
    w.truncate(f, 6000).unwrap();
    model.resize(6000, 0);
    w.write_at(f, 10, b"tiny").unwrap();
    step(&mut model, 10, b"tiny");
    drop(w);
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        fs.read(f).unwrap() == model,
        "contents differ from the model"
    );
    assert_eq!(fs.inode(f).unwrap().size, 6000);
}

/// Files written on every data checksum the formatter offers, and on
/// compressed filesystems (#105), read back here: crc64 and xxhash in a
/// crc64 entry, none with only a pointer, and on lz4 and zstd stored
/// uncompressed, marked incompressible.
#[test]
fn files_on_every_data_checksum_and_compression_read_back() {
    // Compressible: a short repeating pattern. Incompressible: xorshift.
    let pattern: Vec<u8> = (0..70_000u32).map(|i| (i * 7 % 253) as u8).collect();
    let mut x = 0x9e37_79b9_7f4a_7c15u64;
    let random: Vec<u8> = (0..70_000)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect();
    // A compression counter (kind 4 in the top byte, the type in the next):
    // extents, sectors before, sectors after (#105).
    let counter = |img: &std::path::Path, ty: u64| -> Vec<u64> {
        use fs_bcachefs::{btree, superblock::Superblock};
        let dev = FileDevice::open(img).unwrap();
        let sb = Superblock::read(&dev).unwrap();
        btree::walk(&dev, &sb, 20)
            .unwrap()
            .into_iter()
            .find(|k| k.pos.inode == (4 << 56 | ty << 48))
            .map(|k| {
                k.value
                    .chunks_exact(8)
                    .map(|c| u64::from_le_bytes(c.try_into().unwrap()))
                    .collect()
            })
            .unwrap_or_else(|| vec![0, 0, 0])
    };
    let grew = |a: &[u64], b: &[u64]| -> Vec<u64> { a.iter().zip(b).map(|(a, b)| a - b).collect() };
    for (set, codec) in [
        ("crc64", None),
        ("xxhash", None),
        ("nocsum", None),
        ("lz4", Some(3u64)),
        ("zstd", Some(4)),
        ("gzip", Some(2)),
    ] {
        let img = scratch(&format!("{set}.img"), &format!("data-{set}"));
        let root = Filesystem::open(FileDevice::open(&img).unwrap())
            .unwrap()
            .lookup("/")
            .unwrap();
        let (inc0, cod0) = (counter(&img, 5), codec.map(|t| counter(&img, t)));
        let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
        let p = w
            .create_file(root, b"compressible", &pattern, 0o644)
            .unwrap_or_else(|e| panic!("{set}: {e}"));
        let r = w
            .create_file(root, b"random", &random, 0o644)
            .unwrap_or_else(|e| panic!("{set}: {e}"));
        drop(w);
        let inc = grew(&counter(&img, 5), &inc0);
        match (codec, cod0) {
            (Some(t), Some(c0)) => {
                // The pattern compresses: its extents, all 137 sectors of
                // it, fewer stored. The random file is stored as it is.
                let c = grew(&counter(&img, t), &c0);
                assert!(
                    c[0] >= 1 && c[1] == 137 && c[2] < 137,
                    "{set}: the compressed counter grew by {c:?}"
                );
                assert_eq!(inc, vec![3, 137, 137], "{set}: the incompressible counter");
            }
            _ => assert_eq!(inc, vec![0, 0, 0], "{set}: the incompressible counter"),
        }
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        assert!(
            fs.read(p).unwrap() == pattern,
            "{set}: compressible contents"
        );
        assert!(fs.read(r).unwrap() == random, "{set}: random contents");
        assert_eq!(
            fs.read(fs.lookup("/hello.txt").unwrap()).unwrap(),
            b"hello world\n",
            "{set}: an old file"
        );
    }
}

/// Buckets this writer frees are used again in the same session (#132): a
/// 4 MiB file written and removed 25 times, 100 MiB through the 64 MiB base
/// image, which only works if each removal's buckets come back.
#[test]
fn a_session_reuses_the_buckets_it_frees() {
    let img = scratch("write-study/base.img", "reuse-freed");
    let d = Filesystem::open(FileDevice::open(&img).unwrap())
        .unwrap()
        .lookup("/d")
        .unwrap();
    let big: Vec<u8> = (0..4 * 1024 * 1024u32).map(|i| (i % 251) as u8).collect();
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    for i in 0..25 {
        w.create_file(d, b"big", &big, 0o644)
            .unwrap_or_else(|e| panic!("write {i}: {e}"));
        if i < 24 {
            w.unlink(d, b"big").unwrap();
        }
    }
    drop(w);
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    assert!(fs.read(fs.lookup("/d/big").unwrap()).unwrap() == big);
}

/// Hundreds of creates and removals and some large files, enough to fill,
/// rewrite and split nodes and grow a root, in `dir` of a copy of `set`;
/// every file read back (#109).
fn many_operations(set: &str, dir: &str, test: &str, n: usize) {
    let img = scratch(set, test);
    let d = Filesystem::open(FileDevice::open(&img).unwrap())
        .unwrap()
        .lookup(dir)
        .unwrap();
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    for i in 0..n {
        w.create_file(
            d,
            format!("g{i:05}").as_bytes(),
            format!("file {i}\n").as_bytes(),
            0o644,
        )
        .unwrap_or_else(|e| panic!("{set}: create {i}: {e}"));
    }
    for i in (0..n).step_by(3) {
        w.unlink(d, format!("g{i:05}").as_bytes())
            .unwrap_or_else(|e| panic!("{set}: unlink {i}: {e}"));
    }
    let big: Vec<u8> = (0..50_000u32).map(|i| (i % 249) as u8).collect();
    for i in 0..5 {
        w.create_file(d, format!("gbig{i}").as_bytes(), &big, 0o644)
            .unwrap_or_else(|e| panic!("{set}: big {i}: {e}"));
    }
    drop(w);
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    let dir = dir.trim_end_matches('/');
    for i in (0..n).filter(|i| i % 3 != 0) {
        let f = fs.lookup(&format!("{dir}/g{i:05}")).unwrap();
        assert_eq!(
            fs.read(f).unwrap(),
            format!("file {i}\n").as_bytes(),
            "{set}"
        );
    }
    assert!(
        fs.read(fs.lookup(&format!("{dir}/gbig4")).unwrap())
            .unwrap()
            == big
    );
}

/// 4096-byte blocks with 32 KiB nodes, the `block4k` geometry (#109). Each
/// appended bset takes a whole 4096-byte block, so a node fills after about
/// eight appends and is rewritten into a fresh bucket; the buckets freed
/// wait for discard and are not reused in one session (#132), so this small
/// image takes 80 creates, not 400: at 400 it ran out at create 259, and
/// at 150 the reference checker found no room left for its own journal.
#[test]
fn nodes_split_on_4096_byte_blocks_with_32k_nodes() {
    many_operations("block4k.img", "/", "many-block4k", 80);
}

/// 32 KiB nodes in 128 KiB buckets (#109): a new node takes a bucket of
/// its own and leaves the rest unused.
#[test]
fn nodes_split_where_nodes_are_smaller_than_buckets() {
    many_operations(
        "write-study/base-small-nodes.img",
        "/d",
        "many-small-nodes",
        400,
    );
}

/// A read/write device that records the offset of every read (#108).
struct CountingRw {
    inner: FileDevice,
    reads: std::sync::Mutex<Vec<u64>>,
}

impl CountingRw {
    fn new(inner: FileDevice) -> Self {
        CountingRw {
            inner,
            reads: Default::default(),
        }
    }
    fn offsets(&self) -> Vec<u64> {
        self.reads.lock().unwrap().clone()
    }
}

impl fs_core::BlockRead for CountingRw {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> fs_core::Result<()> {
        self.reads.lock().unwrap().push(offset);
        fs_core::BlockRead::read_at(&self.inner, offset, buf)
    }
    fn size_bytes(&self) -> u64 {
        fs_core::BlockRead::size_bytes(&self.inner)
    }
}

impl fs_core::BlockDevice for CountingRw {
    fn write_at(&self, offset: u64, buf: &[u8]) -> fs_core::Result<()> {
        fs_core::BlockDevice::write_at(&self.inner, offset, buf)
    }
    fn flush(&self) -> fs_core::Result<()> {
        fs_core::BlockDevice::flush(&self.inner)
    }
    fn is_writable(&self) -> bool {
        true
    }
}

/// Removing the `large` image's directory of 30000 names is refused as not
/// empty, and finding that out reads the leaf holding its first entry, not
/// every leaf of the directory (#193): one live dirent is enough. The reads
/// are bounded as a create's are: one node per level of the dirents btree,
/// and one leaf more, for the lookup of the name in the root and for the
/// directory's first entry.
#[test]
fn removing_a_full_directory_reads_its_first_entry_not_all_of_them() {
    use fs_bcachefs::{btree, superblock::Superblock};
    use std::collections::BTreeSet;
    let img = scratch("large.img", "rmdir-full");
    let dev = CountingRw::new(FileDevice::open(&img).unwrap());
    let sb = Superblock::read(&dev).unwrap();
    let root = sb
        .btree_roots()
        .unwrap()
        .into_iter()
        .find(|r| r.btree_id == btree::btree_id::DIRENTS)
        .unwrap();
    let depth = usize::from(root.level) + 1;
    let before = dev.offsets().len();
    btree::walk(&dev, &sb, btree::btree_id::DIRENTS).unwrap();
    let dirents: BTreeSet<u64> = dev.offsets()[before..].iter().copied().collect();
    assert!(
        dirents.len() > 3 * (depth + 1),
        "only {} dirents nodes: the bound proves nothing",
        dirents.len()
    );
    let mut w = Writer::open(CountingRw::new(FileDevice::open_rw(&img).unwrap())).unwrap();
    let opened = w.device().offsets().len();
    let err = w
        .rmdir(fs_bcachefs::inode::ROOT_INO, b"wide")
        .expect_err("/wide holds 30000 names");
    assert!(err.to_string().contains("not empty"), "{err}");
    let read = w.device().offsets()[opened..]
        .iter()
        .filter(|o| dirents.contains(o))
        .collect::<BTreeSet<_>>()
        .len();
    assert!(
        read <= 2 * (depth + 1),
        "refusing to remove a full directory read {read} of the {} dirents nodes, in a btree \
         of {depth} levels",
        dirents.len()
    );
}

/// A create in the `large` image's directory of 30000 names (#108) reads
/// the dirents and inodes nodes on its keys' paths, not every node of those
/// btrees. A lookup reads one node per level of its btree, and one leaf
/// more when what it reads runs past a leaf's end (#191): in the inodes
/// btree the directory's inode and the allocation cursor's next number, in
/// the dirents btree the name's run of hash slots. The name is the one of
/// 64 whose slot comes first, so nearly all of the directory lies after
/// it: a probe that read on past its run would read most of the dirents
/// nodes, whatever hash seed the fixture's directory was given.
#[test]
fn a_create_reads_paths_not_whole_btrees() {
    use fs_bcachefs::{btree, superblock::Superblock};
    use std::collections::BTreeSet;
    let img = scratch("large.img", "cursor-create");
    // Where each btree's nodes are (every offset a walk of it reads), and
    // how many levels it has.
    let dev = CountingRw::new(FileDevice::open(&img).unwrap());
    let sb = Superblock::read(&dev).unwrap();
    let roots = sb.btree_roots().unwrap();
    let tree = |id: u8| {
        let before = dev.offsets().len();
        btree::walk(&dev, &sb, id).unwrap();
        let nodes: BTreeSet<u64> = dev.offsets()[before..].iter().copied().collect();
        let root = roots.iter().find(|r| r.btree_id == id).unwrap();
        (nodes, usize::from(root.level) + 1)
    };
    let (dirents, dirents_depth) = tree(btree::btree_id::DIRENTS);
    let (inodes, inodes_depth) = tree(btree::btree_id::INODES);
    assert!(
        dirents.len() > 3 * (dirents_depth + 1),
        "only {} dirents nodes: the bound proves nothing",
        dirents.len()
    );
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    let wide = fs.lookup("/wide").unwrap();
    let dir = fs.inode(wide).unwrap();
    let name = (0..64)
        .map(|i| format!("new-entry-{i:02}"))
        .min_by_key(|n| {
            fs_bcachefs::inode::name_hash(dir.hash_type(), dir.hash_seed, n.as_bytes()).unwrap()
        })
        .unwrap();
    let mut w = Writer::open(CountingRw::new(FileDevice::open_rw(&img).unwrap())).unwrap();
    let opened = w.device().offsets().len();
    let f = w
        .create_file(wide, name.as_bytes(), b"new\n", 0o644)
        .unwrap();
    let offsets = w.device().offsets()[opened..].to_vec();
    drop(w);
    let read = |nodes: &BTreeSet<u64>| {
        offsets
            .iter()
            .filter(|o| nodes.contains(o))
            .collect::<BTreeSet<_>>()
            .len()
    };
    assert!(
        read(&dirents) <= dirents_depth + 1,
        "one create read {} of the {} dirents nodes, in a btree of {} levels",
        read(&dirents),
        dirents.len(),
        dirents_depth
    );
    assert!(
        read(&inodes) <= 2 * (inodes_depth + 1),
        "one create read {} of the {} inodes nodes, in a btree of {} levels",
        read(&inodes),
        inodes.len(),
        inodes_depth
    );
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    assert_eq!(fs.lookup(&format!("/wide/{name}")).unwrap(), f);
    assert_eq!(fs.read(f).unwrap(), b"new\n");
}

/// Setting an xattr reads the xattrs btree on the path to the name's slot
/// and the run of taken slots from there, not every xattr of the inode
/// after it (#193). /hello.txt is given enough xattrs, put in place in one
/// insert, that its table spans many leaves; the new name is the one of 64
/// whose slot comes first, so nearly all of the table lies after it.
#[test]
fn an_xattr_set_reads_its_run_not_the_rest_of_the_table() {
    use fs_bcachefs::bkey::{key_type, Bkey, Bpos};
    use fs_bcachefs::btree::{self, btree_id};
    use fs_bcachefs::superblock::Superblock;
    use std::collections::BTreeSet;
    let img = scratch("default.img", "xattr-probe-run");
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    let ino = fs.lookup("/hello.txt").unwrap();
    let inode = fs.inode(ino).unwrap();
    let snapshot = {
        let dev = FileDevice::open(&img).unwrap();
        let sb = Superblock::read(&dev).unwrap();
        btree::walk(&dev, &sb, btree_id::INODES)
            .unwrap()
            .into_iter()
            .find(|k| k.pos.inode == 0 && k.pos.offset == ino && k.key_type == key_type::INODE_V3)
            .unwrap()
            .pos
            .snapshot
    };
    drop(fs);
    let slot = |name: &[u8]| {
        fs_bcachefs::xattr::slot(inode.hash_type(), inode.hash_seed, 0, name).unwrap()
    };
    // A user xattr with a 1500-byte value, at its own slot.
    let xattr = |i: usize| {
        // The slot hash leaves the last byte of this 14-byte message unhashed
        // (`xattr::name_slot`), so the name ends in a constant byte and every
        // digit is hashed: `filler-00000` to `filler-00009` would share one
        // slot, and an insert keeps one key per position (#202).
        let name = format!("filler-{i:05}.");
        let value = vec![b'v'; 1500];
        let mut v = vec![0u8, name.len() as u8];
        v.extend_from_slice(&(value.len() as u16).to_le_bytes());
        v.extend_from_slice(name.as_bytes());
        v.extend_from_slice(&value);
        v.resize(v.len().div_ceil(8) * 8, 0);
        Bkey {
            key_type: key_type::XATTR,
            size: 0,
            version_hi: 0,
            version_lo: 0,
            pos: Bpos {
                inode: ino,
                offset: slot(name.as_bytes()),
                snapshot,
            },
            value: v,
        }
    };
    // The first xattr makes the btree's root through the writer; the rest
    // go in by the batch until the table spans enough nodes for the bound
    // to mean something.
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.set_xattr(ino, b"user.first", b"1").unwrap();
    let nodes = |img: &std::path::Path| {
        let dev = CountingRw::new(FileDevice::open(img).unwrap());
        let sb = Superblock::read(&dev).unwrap();
        btree::walk(&dev, &sb, btree_id::XATTRS).unwrap();
        let root = sb
            .btree_roots()
            .unwrap()
            .into_iter()
            .find(|r| r.btree_id == btree_id::XATTRS)
            .unwrap();
        (
            dev.offsets().into_iter().collect::<BTreeSet<u64>>(),
            usize::from(root.level) + 1,
        )
    };
    let mut made = 0;
    let (mut xattr_nodes, mut depth) = (BTreeSet::new(), 1);
    for _ in 0..40 {
        w.insert(btree_id::XATTRS, (made..made + 250).map(xattr).collect())
            .unwrap();
        made += 250;
        (xattr_nodes, depth) = nodes(&img);
        if xattr_nodes.len() > 4 * (depth + 1) {
            break;
        }
    }
    drop(w);
    assert!(
        xattr_nodes.len() > 4 * (depth + 1),
        "only {} xattrs nodes after {made} xattrs: the bound proves nothing",
        xattr_nodes.len()
    );
    // Every inserted xattr reads back before the set, each at a slot of its
    // own (#202).
    let before = Filesystem::open(FileDevice::open(&img).unwrap())
        .unwrap()
        .xattrs(ino)
        .unwrap()
        .len();
    assert_eq!(before, made + 1, "the xattrs that read back before the set");

    let name = (0..64)
        .map(|i| format!("new-xattr-{i:02}"))
        .min_by_key(|n| slot(n.as_bytes()))
        .unwrap();
    let mut w = Writer::open(CountingRw::new(FileDevice::open_rw(&img).unwrap())).unwrap();
    let opened = w.device().offsets().len();
    w.set_xattr(ino, format!("user.{name}").as_bytes(), b"new")
        .unwrap();
    let read = w.device().offsets()[opened..]
        .iter()
        .filter(|o| xattr_nodes.contains(o))
        .collect::<BTreeSet<_>>()
        .len();
    drop(w);
    assert!(
        read <= depth + 1,
        "one xattr set read {read} of the {} xattrs nodes, in a btree of {depth} levels",
        xattr_nodes.len()
    );
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    let full = format!("user.{name}");
    let set = fs.xattrs(ino).unwrap();
    assert!(
        set.iter()
            .any(|x| x.name == full.as_bytes() && x.value == b"new"),
        "{full} does not read back"
    );
    assert_eq!(
        set.len(),
        before + 1,
        "the set adds its xattr and loses none of the {before} before it"
    );
}

/// A node written into a reused bucket is read as itself, not as the node
/// that was there before (#108, #132): the writer's node cache keys a node
/// by its bucket's generation too, which reuse raises. Read through a stale
/// entry, a removal missed extents of the file it removed, and the
/// reference checker found them in missing inodes. Every extent left after
/// a session of writes and removals belongs to an inode that exists.
#[test]
fn a_session_that_reuses_buckets_leaves_no_extents_of_removed_files() {
    use fs_bcachefs::btree::{self, btree_id};
    use fs_bcachefs::superblock::Superblock;
    let img = scratch("write-study/base.img", "reuse-orphans");
    let d = Filesystem::open(FileDevice::open(&img).unwrap())
        .unwrap()
        .lookup("/d")
        .unwrap();
    let big: Vec<u8> = (0..4 * 1024 * 1024u32).map(|i| (i % 251) as u8).collect();
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    for i in 0..25 {
        w.create_file(d, b"big", &big, 0o644)
            .unwrap_or_else(|e| panic!("write {i}: {e}"));
        if i < 24 {
            w.unlink(d, b"big").unwrap();
        }
    }
    drop(w);
    let dev = FileDevice::open(&img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let inodes: std::collections::BTreeSet<u64> = btree::walk(&dev, &sb, btree_id::INODES)
        .unwrap()
        .iter()
        .map(|k| k.pos.offset)
        .collect();
    let orphans: std::collections::BTreeSet<u64> = btree::walk(&dev, &sb, btree_id::EXTENTS)
        .unwrap()
        .iter()
        .map(|k| k.pos.inode)
        .filter(|ino| !inodes.contains(ino))
        .collect();
    assert!(orphans.is_empty(), "extents of removed inodes {orphans:?}");
}

/// A filesystem whose root subvolume the reference kernel module
/// snapshotted (#138): the root and its snapshot share every key made before
/// the snapshot, at snapshot u32::MAX. A write into the root is either
/// refused, or seen in the root and nowhere in the snapshot, which must read
/// exactly as the kernel left it.
#[test]
fn a_write_into_a_snapshotted_root_leaves_the_snapshot_as_it_was() {
    let rec = common::read_text("kernel.txt");
    assert!(
        rec.lines().any(|l| l == "rootsnap: ok"),
        "the kernel oracle made no snapshotted root (chore fixtures):\n{rec}"
    );
    let img = scratch("kernel-rootsnap.img", "rootsnap");
    let snap_before = {
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        let mut names: Vec<Vec<u8>> = fs
            .readdir_at(fs.resolve("/snap").unwrap())
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        names.sort();
        let kept = fs.read_at(fs.resolve("/snap/kept.txt").unwrap()).unwrap();
        assert_eq!(kept, b"before the snapshot\n", "the snapshot's kept.txt");
        names
    };
    let root = Filesystem::open(FileDevice::open(&img).unwrap())
        .unwrap()
        .resolve("/")
        .unwrap()
        .ino;
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    match w.create_file(root, b"written.txt", b"written\n", 0o644) {
        Err(Error::Unsupported(_)) => return,
        Err(e) => panic!("create in a snapshotted root: {e}"),
        Ok(_) => {}
    }
    drop(w);
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    let written = fs
        .resolve("/written.txt")
        .unwrap_or_else(|e| panic!("the root does not see what was written: {e}"));
    assert_eq!(fs.read_at(written).unwrap(), b"written\n");
    let mut names: Vec<Vec<u8>> = fs
        .readdir_at(fs.resolve("/snap").unwrap())
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    names.sort();
    assert_eq!(
        names, snap_before,
        "a write into the root changed its snapshot"
    );
    assert_eq!(
        fs.read_at(fs.resolve("/snap/kept.txt").unwrap()).unwrap(),
        b"before the snapshot\n",
        "the snapshot's kept.txt after a write into the root"
    );
}

/// Reflinked files removed (#140), judged here: unlinking the clone of
/// big/random.bin leaves the original reading as it did and this checker
/// clean, the shared data one reference fewer; unlinking the original too
/// frees the shared data, and the checker is still clean.
#[test]
fn reflinked_files_are_freed_one_reference_at_a_time() {
    use fs_bcachefs::btree::{self, btree_id};
    use fs_bcachefs::superblock::Superblock;
    let img = scratch("kernel.img", "reflink-free");
    let indirect = |img: &std::path::Path| {
        let dev = FileDevice::open(img).unwrap();
        let sb = Superblock::read(&dev).unwrap();
        btree::walk(&dev, &sb, btree_id::REFLINK)
            .unwrap()
            .into_iter()
            .filter(|k| k.key_type == fs_bcachefs::bkey::key_type::REFLINK_V)
            .count()
    };
    let shared = indirect(&img);
    assert!(shared > 0, "the kernel image holds no shared data");
    let (big, original) = {
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        let big = fs.lookup("/big").unwrap();
        (big, fs.read(fs.lookup("/big/random.bin").unwrap()).unwrap())
    };
    let clean = |img: &std::path::Path, after: &str| {
        let r = fs_bcachefs::check::check(&FileDevice::open(img).unwrap()).unwrap();
        assert!(r.clean(), "after {after}: {:?}", r.problems);
    };

    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.unlink(big, b"random-clone.bin")
        .unwrap_or_else(|e| panic!("unlinking the clone: {e}"));
    drop(w);
    clean(&img, "unlinking the clone");
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    assert!(fs.lookup("/big/random-clone.bin").is_err());
    assert!(fs.read(fs.lookup("/big/random.bin").unwrap()).unwrap() == original);
    drop(fs);
    assert_eq!(indirect(&img), shared, "shared data freed while referenced");

    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.unlink(big, b"random.bin")
        .unwrap_or_else(|e| panic!("unlinking the original: {e}"));
    drop(w);
    clean(&img, "unlinking the original");
    assert_eq!(indirect(&img), 0, "shared data no file references is kept");
}

/// Data is written as the file's options say, not only the filesystem's
/// (#142): on a copy of the kernel image, whose filesystem compresses
/// nothing, a rewrite of opts/compression-lz4 is stored lz4-compressed, and
/// a file created in opts/dir, whose compression is zstd, inherits that
/// option as the module's own file there did and is stored zstd-compressed.
#[test]
fn data_is_written_as_the_files_options_say() {
    use fs_bcachefs::btree::{self, btree_id};
    use fs_bcachefs::extent::{compression, DataExtent};
    use fs_bcachefs::superblock::Superblock;
    let img = scratch("kernel.img", "file-options");
    let text: Vec<u8> = (0..2000)
        .flat_map(|i| format!("a line of text in a file with options, {i:05}\n").into_bytes())
        .collect();
    let (lz4, dir, theirs) = {
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        let theirs = fs.inode(fs.lookup("/opts/dir/inherited").unwrap()).unwrap();
        (
            fs.lookup("/opts/compression-lz4").unwrap(),
            fs.lookup("/opts/dir").unwrap(),
            theirs,
        )
    };
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.write_file(lz4, &text).unwrap();
    let ours = w.create_file(dir, b"ours", &text, 0o644).unwrap();
    drop(w);
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    let mine = fs.inode(ours).unwrap();
    assert_eq!(mine.options, theirs.options, "the inherited options");
    assert_eq!(
        mine.flags & (1 << 12),
        theirs.flags & (1 << 12),
        "the has-options flag"
    );
    assert!(fs.read(ours).unwrap() == text);
    assert!(fs.read(lz4).unwrap() == text);
    let dev = FileDevice::open(&img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let codecs = |ino: u64| -> Vec<u8> {
        btree::walk(&dev, &sb, btree_id::EXTENTS)
            .unwrap()
            .iter()
            .filter(|k| k.pos.inode == ino && k.key_type == fs_bcachefs::bkey::key_type::EXTENT)
            .map(|k| {
                DataExtent::from_key(k)
                    .unwrap()
                    .crc
                    .map_or(0, |c| c.compression_type)
            })
            .collect()
    };
    let (a, b) = (codecs(lz4), codecs(ours));
    assert!(
        !a.is_empty() && a.iter().all(|&c| c == compression::LZ4),
        "lz4 file: {a:?}"
    );
    assert!(
        !b.is_empty() && b.iter().all(|&c| c == compression::ZSTD),
        "new file: {b:?}"
    );
    let r = fs_bcachefs::check::check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(r.clean(), "{:?}", r.problems);
}

/// Every xattr a run of `Writer::insert` batches puts in place reads back,
/// through `Filesystem::xattrs` and through a walk of the btree alike, once
/// the table spreads over many leaves (#202). Batches of 250 xattrs with
/// 1,500-byte values, each at its own slot, fill and split the leaves; each
/// batch is checked as it lands, so the first to lose keys is the one named.
/// The keys the first version of these tests lost shared slots, and an
/// insert keeps one key per position.
#[test]
fn every_xattr_a_run_of_batches_inserts_reads_back() {
    use fs_bcachefs::bkey::{key_type, Bkey, Bpos};
    use fs_bcachefs::btree::{self, btree_id};
    use fs_bcachefs::superblock::Superblock;
    let img = scratch("default.img", "xattr-batches");
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    let ino = fs.lookup("/hello.txt").unwrap();
    let inode = fs.inode(ino).unwrap();
    let before = fs.xattrs(ino).unwrap().len();
    drop(fs);
    let snapshot = {
        let dev = FileDevice::open(&img).unwrap();
        let sb = Superblock::read(&dev).unwrap();
        btree::walk(&dev, &sb, btree_id::INODES)
            .unwrap()
            .into_iter()
            .find(|k| k.pos.inode == 0 && k.pos.offset == ino && k.key_type == key_type::INODE_V3)
            .unwrap()
            .pos
            .snapshot
    };
    let xattr = |i: usize| {
        // The slot hash leaves the last byte of this 14-byte message unhashed
        // (`xattr::name_slot`), so the name ends in a constant byte and every
        // digit is hashed: `filler-00000` to `filler-00009` would share one
        // slot, and an insert keeps one key per position (#202).
        let name = format!("filler-{i:05}.");
        let value = vec![b'v'; 1500];
        // Namespace 0, the user namespace in this format.
        let mut v = vec![0u8, name.len() as u8];
        v.extend_from_slice(&(value.len() as u16).to_le_bytes());
        v.extend_from_slice(name.as_bytes());
        v.extend_from_slice(&value);
        v.resize(v.len().div_ceil(8) * 8, 0);
        Bkey {
            key_type: key_type::XATTR,
            size: 0,
            version_hi: 0,
            version_lo: 0,
            pos: Bpos {
                inode: ino,
                offset: fs_bcachefs::xattr::slot(
                    inode.hash_type(),
                    inode.hash_seed,
                    0,
                    name.as_bytes(),
                )
                .unwrap(),
                snapshot,
            },
            value: v,
        }
    };
    // One writer for every batch, as a caller filling a table would hold
    // it; each batch is read back between them through a device of its own.
    // The first xattr makes the btree's root, if it has none, through the
    // writer's own path.
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.set_xattr(ino, b"user.first", b"1").unwrap();
    let before = before + 1;
    let mut made = 0;
    for batch in 1..=5 {
        w.insert(btree_id::XATTRS, (made..made + 250).map(xattr).collect())
            .unwrap();
        made += 250;
        let dev = FileDevice::open(&img).unwrap();
        let sb = Superblock::read(&dev).unwrap();
        let walked = btree::walk(&dev, &sb, btree_id::XATTRS)
            .unwrap()
            .into_iter()
            .filter(|k| k.pos.inode == ino && k.key_type == key_type::XATTR)
            .count();
        let read = Filesystem::open(FileDevice::open(&img).unwrap())
            .unwrap()
            .xattrs(ino)
            .unwrap()
            .len();
        let root = sb
            .btree_roots()
            .unwrap()
            .into_iter()
            .find(|r| r.btree_id == btree_id::XATTRS)
            .map(|r| r.level);
        assert_eq!(
            (read, walked),
            (before + made, before + made),
            "after batch {batch} ({made} xattrs inserted on {before}): Filesystem::xattrs reads \
             {read} and a walk of the btree finds {walked}; the root is at level {root:?}"
        );
    }
    drop(w);
    let r = fs_bcachefs::check::check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(r.clean(), "{:?}", r.problems);
}

/// Unlinking a reflinked file reads the reflink btree along its pointer's
/// run of indirect extents, not every node after the run's start (#193).
/// The kernel image's reflink btree is padded, in batches, with indirect
/// extents past every real one until it spans many nodes; unlinking the
/// clone of big/random.bin then reads the path to its run and the leaves the
/// run is in. The padding is never read as data: it lies after the run.
#[test]
fn unlinking_a_reflinked_file_reads_its_run_not_the_rest_of_the_reflink_btree() {
    use fs_bcachefs::bkey::{key_type, Bkey, Bpos};
    use fs_bcachefs::btree::{self, btree_id};
    use fs_bcachefs::superblock::Superblock;
    use std::collections::BTreeSet;
    let img = scratch("kernel.img", "reflink-run");
    let big = Filesystem::open(FileDevice::open(&img).unwrap())
        .unwrap()
        .lookup("/big")
        .unwrap();
    let last = {
        let dev = FileDevice::open(&img).unwrap();
        let sb = Superblock::read(&dev).unwrap();
        btree::walk(&dev, &sb, btree_id::REFLINK)
            .unwrap()
            .iter()
            .map(|k| k.pos.offset)
            .max()
            .expect("the kernel image holds shared data")
    };
    // An indirect extent of 8 sectors, one reference, with a kilobyte of
    // value so a few hundred fill a node.
    let pad = |i: u64| {
        let mut value = vec![0u8; 1024];
        value[..8].copy_from_slice(&1u64.to_le_bytes());
        Bkey {
            key_type: key_type::REFLINK_V,
            size: 8,
            version_hi: 0,
            version_lo: 0,
            pos: Bpos {
                inode: 0,
                offset: last + 1_000_000 + 8 * (i + 1),
                snapshot: 0,
            },
            value,
        }
    };
    let nodes = |img: &std::path::Path| {
        let dev = CountingRw::new(FileDevice::open(img).unwrap());
        let sb = Superblock::read(&dev).unwrap();
        btree::walk(&dev, &sb, btree_id::REFLINK).unwrap();
        let root = sb
            .btree_roots()
            .unwrap()
            .into_iter()
            .find(|r| r.btree_id == btree_id::REFLINK)
            .unwrap();
        (
            dev.offsets().into_iter().collect::<BTreeSet<u64>>(),
            usize::from(root.level) + 1,
        )
    };
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    let mut made = 0;
    let (mut reflink_nodes, mut depth) = (BTreeSet::new(), 1);
    for _ in 0..40 {
        w.insert(btree_id::REFLINK, (made..made + 200).map(pad).collect())
            .unwrap();
        made += 200;
        (reflink_nodes, depth) = nodes(&img);
        if reflink_nodes.len() > 4 * (depth + 2) {
            break;
        }
    }
    drop(w);
    assert!(
        reflink_nodes.len() > 4 * (depth + 2),
        "only {} reflink nodes after {made} padding extents: the bound proves nothing",
        reflink_nodes.len()
    );

    let mut w = Writer::open(CountingRw::new(FileDevice::open_rw(&img).unwrap())).unwrap();
    let opened = w.device().offsets().len();
    w.unlink(big, b"random-clone.bin")
        .unwrap_or_else(|e| panic!("unlinking the clone: {e}"));
    let read = w.device().offsets()[opened..]
        .iter()
        .filter(|o| reflink_nodes.contains(o))
        .collect::<BTreeSet<_>>()
        .len();
    drop(w);
    assert!(
        read <= depth + 2,
        "unlinking the clone read {read} of the {} reflink nodes, in a btree of {depth} levels",
        reflink_nodes.len()
    );
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    assert!(fs.lookup("/big/random-clone.bin").is_err());
    assert!(fs.lookup("/big/random.bin").is_ok());
}
