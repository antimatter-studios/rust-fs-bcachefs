//! What the checker misses in inode metadata that the reference checker
//! finds (#221): each test damages a copy of a fixture through the
//! writer's raw `insert` and requires the checker to name the damage.
//!
//! - a directory whose size is not 0 (the reference's "directory with
//!   nonzero i_size");
//! - an inode whose journal sequence is past any the journal reaches
//!   ("inode journal seq in future");
//! - an inode whose sector count is not what its extents add up to
//!   ("incorrect i_sectors").
//!
//! The changes include the ones the differential run measured (CI run
//! 38579070302), and the same changes are judged by the reference checker
//! itself in the guest (`tests/check_oracle.rs`).
#![cfg(feature = "write")]

mod common;

use common::fixture;
use fs_bcachefs::bkey::{key_type, Bkey};
use fs_bcachefs::btree::{self, btree_id};
use fs_bcachefs::check;
use fs_bcachefs::inode::InodeV3Raw;
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::write::Writer;
use fs_bcachefs::Filesystem;
use fs_core::FileDevice;

/// The superblock's clean field, whose body holds the journal sequence of
/// the clean shutdown at byte 8 (docs/clean-room.md).
const FIELD_CLEAN: u32 = 6;
/// Key types whose size an inode's sector count adds up (S3: the lister's
/// `bi_sectors` beside its extents on every listed fixture).
const COUNTED: [u8; 4] = [
    key_type::EXTENT,
    key_type::RESERVATION,
    key_type::REFLINK_P,
    key_type::INLINE_DATA,
];

/// The measured cases' inodes, by path: their numbers differ from one
/// build of the fixtures to the next. A directory of the kernel module's
/// written image (the one with options), the file of lz4 whose journal
/// sequence was damaged, and the files of lz4 and aged whose sector counts
/// were.
const KERNEL_WRITTEN_DIR: &str = "/optdir";
const LZ4_SEQ_FILE: &str = "/hello.txt";
const LZ4_SECTORS_FILE: &str = "/many/file-0271.txt";
const AGED_SECTORS_FILE: &str = "/many/f02467";

/// A damaged copy of a fixture, removed when dropped: the guest's temporary
/// directory is small, and copies left behind filled it before the next
/// suite (`oracle_write_pieces`) could make its own.
struct Scratch(std::path::PathBuf);

impl std::ops::Deref for Scratch {
    type Target = std::path::Path;
    fn deref(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn scratch(src: &str, test: &str) -> Scratch {
    let dir = std::env::temp_dir().join(format!(
        "rust-fs-bcachefs-check-inode-metadata-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let img = dir.join(format!("{test}.img"));
    std::fs::copy(fixture(src), &img).unwrap();
    Scratch(img)
}

/// The keys of btree `id`, through the journal where the image needs it
/// (aged-unclean), as the checker reads them.
fn keys(img: &std::path::Path, id: u8) -> Vec<Bkey> {
    let dev = FileDevice::open(img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let replay = btree::journal_for(&dev, &sb).unwrap();
    btree::walk_replayed(&dev, &sb, id, replay.as_ref()).unwrap()
}

fn lookup(img: &std::path::Path, path: &str) -> u64 {
    Filesystem::open(FileDevice::open(img).unwrap())
        .unwrap()
        .lookup(path)
        .unwrap()
}

/// The journal sequence `img` was cleanly shut down at.
fn clean_seq(img: &std::path::Path) -> u64 {
    let sb = Superblock::read(&FileDevice::open(img).unwrap()).unwrap();
    let body = &sb.field(FIELD_CLEAN).expect("a clean fixture").body;
    u64::from_le_bytes(body[8..16].try_into().unwrap())
}

/// `img` with inode `ino`'s key changed by `change`.
fn rewrite_inode(img: &std::path::Path, ino: u64, change: impl FnOnce(&mut InodeV3Raw)) {
    let mut k = keys(img, btree_id::INODES)
        .into_iter()
        .find(|k| k.pos.inode == 0 && k.pos.offset == ino && k.key_type == key_type::INODE_V3)
        .unwrap_or_else(|| panic!("no inode {ino}"));
    let mut raw = InodeV3Raw::parse(&k.value).unwrap();
    change(&mut raw);
    k.value = raw.encode();
    let mut w = Writer::open(FileDevice::open_rw(img).unwrap()).unwrap();
    w.insert(btree_id::INODES, vec![k]).unwrap();
}

fn problems(img: &std::path::Path) -> Vec<check::Problem> {
    check::check(&FileDevice::open(img).unwrap())
        .expect("the checker runs")
        .problems
}

/// The checker's problems on `img`, which must include `kind`.
#[track_caller]
fn assert_reported(img: &std::path::Path, kind: &str, what: &str) {
    let problems = problems(img);
    assert!(
        problems.iter().any(|p| p.kind == kind),
        "{what} was not reported as {kind}: {problems:?}"
    );
}

/// The images the differential run lays its cases over, and the kernel
/// module's others: each is clean, and between them they hold directories,
/// every key type an inode's sector count adds up, keys at several
/// snapshots, and a journal replayed (aged-unclean), so the checks below
/// are not vacuous on them.
#[test]
fn every_fixture_with_the_counted_extent_types_is_clean() {
    let mut seen = std::collections::BTreeSet::new();
    for name in [
        "default",
        "block4k",
        "lz4",
        "aged",
        "aged-unclean",
        "kernel",
        "kernel-rootsnap",
        "kernel-written",
        "kernel-reuse",
        "kernel-prealloc",
    ] {
        let img = fixture(&format!("{name}.img"));
        seen.extend(
            keys(&img, btree_id::EXTENTS)
                .iter()
                .map(|k| k.key_type)
                .filter(|t| COUNTED.contains(t)),
        );
        let r = check::check(&FileDevice::open(&img).unwrap())
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(r.clean(), "{name}: {:?}", r.problems);
    }
    assert_eq!(
        seen.into_iter().collect::<Vec<_>>(),
        {
            let mut all = COUNTED.to_vec();
            all.sort();
            all
        },
        "every counted key type is on some fixture"
    );
}

/// The measured case (`kernel-written.img` seed 19): a directory's size
/// made all ones. And default's `/dir` given a block's size.
#[test]
fn a_directory_with_a_nonzero_size_is_reported() {
    let img = scratch("kernel-written.img", "dir-size-max");
    let ino = lookup(&img, KERNEL_WRITTEN_DIR);
    rewrite_inode(&img, ino, |i| i.size = u64::MAX);
    assert_reported(
        &img,
        "directory_nonzero_i_size",
        "a directory of size 2^64 - 1",
    );

    let img = scratch("default.img", "dir-size-block");
    let ino = lookup(&img, "/dir");
    rewrite_inode(&img, ino, |i| i.size = 4096);
    assert_reported(&img, "directory_nonzero_i_size", "/dir of size 4096");
}

/// The measured case (`lz4.img` seed 44): an inode's journal sequence
/// 16711691 on a filesystem cleanly shut down at 43 (the reference's
/// "recovering from clean shutdown, journal seq 43"). And default's
/// `/hello.txt` at the largest sequence there is.
#[test]
fn an_inode_journal_sequence_in_the_future_is_reported() {
    let img = scratch("lz4.img", "seq-measured");
    let ino = lookup(&img, LZ4_SEQ_FILE);
    rewrite_inode(&img, ino, |i| i.journal_seq = 16_711_691);
    assert_reported(
        &img,
        "inode_journal_seq_in_future",
        "an inode at journal sequence 16711691",
    );

    let img = scratch("default.img", "seq-max");
    let ino = lookup(&img, "/hello.txt");
    rewrite_inode(&img, ino, |i| i.journal_seq = u64::MAX);
    assert_reported(
        &img,
        "inode_journal_seq_in_future",
        "an inode at journal sequence 2^64 - 1",
    );
}

/// The sequence the filesystem was shut down at, and the next one, are
/// sequences the journal reaches: an inode at either is not in the future.
#[test]
fn an_inode_at_a_sequence_the_journal_reaches_is_not_reported() {
    for (test, ahead) in [("seq-clean", 0), ("seq-next", 1)] {
        let img = scratch("default.img", test);
        let seq = clean_seq(&img) + ahead;
        let ino = lookup(&img, "/hello.txt");
        rewrite_inode(&img, ino, |i| i.journal_seq = seq);
        let problems = problems(&img);
        assert!(
            !problems
                .iter()
                .any(|p| p.kind == "inode_journal_seq_in_future"),
            "an inode at sequence {seq}: {problems:?}"
        );
    }
}

/// The measured cases (`aged.img` seed 19, `lz4.img` seed 64): a file's
/// sector count made all ones, and 2^48 - 1, where its extents add up to
/// one sector. And default's `/hello.txt` given one sector more than its
/// extents.
#[test]
fn an_inode_whose_sectors_disagree_with_its_extents_is_reported() {
    let img = scratch("aged.img", "sectors-max");
    let ino = lookup(&img, AGED_SECTORS_FILE);
    rewrite_inode(&img, ino, |i| i.sectors = u64::MAX);
    assert_reported(&img, "inode_i_sectors_wrong", "a file of 2^64 - 1 sectors");

    let img = scratch("lz4.img", "sectors-48");
    let ino = lookup(&img, LZ4_SECTORS_FILE);
    rewrite_inode(&img, ino, |i| i.sectors = (1 << 48) - 1);
    assert_reported(&img, "inode_i_sectors_wrong", "a file of 2^48 - 1 sectors");

    let img = scratch("default.img", "sectors-plus-one");
    let ino = lookup(&img, "/hello.txt");
    rewrite_inode(&img, ino, |i| i.sectors += 1);
    assert_reported(
        &img,
        "inode_i_sectors_wrong",
        "a file one sector over its extents",
    );
}
