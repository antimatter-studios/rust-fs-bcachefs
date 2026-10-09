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

/// A journalled session longer than its journal (#101). Nothing is
/// reclaimed, so every entry since the session began is one the replay
/// needs: an entry that does not fit must be refused, not written over the
/// oldest of them, and a session continuing the journal is held to the same.
/// The base image's journal is 16 buckets of 32 KiB, 1024 blocks, and every
/// entry takes at least one block, so 1100 creates cannot all fit.
#[test]
fn a_full_journal_is_refused_rather_than_overwritten() {
    let img = scratch("write-study/base.img", "local-journal-full");
    let d = {
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        fs.lookup("/d").unwrap()
    };
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.journal_commits().unwrap();
    let (mut made, refused) = create_until_refused(&mut w, d, "j", 1100);
    drop(w);
    match refused {
        Some(Error::Unsupported(m)) if m.contains("journal is full") => {}
        other => panic!("{} journalled creates, then: {other:?}", made.len()),
    }
    // A session continuing that journal may fill what is left of the last
    // bucket (64 blocks at most), and is then refused in turn.
    let mut w = Writer::open_journalled(FileDevice::open_rw(&img).unwrap()).unwrap();
    let (more, refused) = create_until_refused(&mut w, d, "k", 64);
    drop(w);
    match refused {
        Some(Error::Unsupported(m)) if m.contains("journal is full") => {}
        other => panic!(
            "{} creates in a continued session, then: {other:?}",
            more.len()
        ),
    }
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
    let data: Vec<u8> = (0..70_000u32).map(|i| (i * 7 % 253) as u8).collect();
    for set in ["crc64", "xxhash", "nocsum", "lz4", "zstd", "gzip"] {
        let img = scratch(&format!("{set}.img"), &format!("data-{set}"));
        let root = Filesystem::open(FileDevice::open(&img).unwrap())
            .unwrap()
            .lookup("/")
            .unwrap();
        let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
        let f = w
            .create_file(root, b"written-here", &data, 0o644)
            .unwrap_or_else(|e| panic!("{set}: {e}"));
        drop(w);
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        assert!(fs.read(f).unwrap() == data, "{set}: contents");
        assert_eq!(
            fs.read(fs.lookup("/hello.txt").unwrap()).unwrap(),
            b"hello world\n",
            "{set}: an old file"
        );
    }
}
