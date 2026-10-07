//! The write study's before/after pairs (scripts/guest-write-study.sh,
//! issue #20), read by this crate: each image the reference implementation
//! changed by one operation shows exactly that change, and its btrees agree
//! key for key with the reference lister's dump of them.

mod common;

use common::{fixture, read_text};
use fs_bcachefs::btree::{self, btree_id};
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::Filesystem;
use fs_core::FileDevice;

const OPS: &[&str] = &[
    "base",
    "create-small",
    "create-empty",
    "create-large",
    "mkdir",
    "unlink",
    "rename",
    "truncate",
    "overwrite",
];

fn open(op: &str) -> Filesystem<FileDevice> {
    Filesystem::open(FileDevice::open(fixture(&format!("write-study/{op}.img"))).unwrap())
        .unwrap_or_else(|e| panic!("{op}: {e}"))
}

fn names(fs: &Filesystem<FileDevice>, dir: &str) -> Vec<String> {
    let mut v: Vec<String> = fs
        .readdir(fs.lookup(dir).unwrap())
        .unwrap()
        .iter()
        .map(|d| String::from_utf8_lossy(&d.name).into_owned())
        .collect();
    v.sort();
    v
}

fn content(fs: &Filesystem<FileDevice>, path: &str) -> Vec<u8> {
    fs.read(fs.lookup(path).unwrap()).unwrap()
}

#[test]
fn each_image_shows_its_one_operation() {
    let base = open("base");
    assert_eq!(names(&base, "/d"), ["existing"]);
    assert_eq!(content(&base, "/d/existing"), b"an existing file\n");

    let fs = open("create-small");
    assert_eq!(names(&fs, "/d"), ["existing", "new.txt"]);
    assert_eq!(content(&fs, "/d/new.txt"), b"hello\n");

    let fs = open("create-empty");
    assert_eq!(names(&fs, "/d"), ["empty", "existing"]);
    assert_eq!(content(&fs, "/d/empty"), b"");

    let fs = open("create-large");
    assert_eq!(content(&fs, "/d/big.bin"), vec![b'x'; 300_000]);

    let fs = open("mkdir");
    assert!(fs.inode(fs.lookup("/d/sub").unwrap()).unwrap().is_dir());
    assert_eq!(names(&fs, "/d/sub"), Vec::<String>::new());

    assert_eq!(names(&open("unlink"), "/d"), Vec::<String>::new());

    let fs = open("rename");
    assert_eq!(names(&fs, "/d"), ["renamed"]);
    assert_eq!(content(&fs, "/d/renamed"), b"an existing file\n");

    assert_eq!(content(&open("truncate"), "/d/existing"), b"");
    assert_eq!(content(&open("overwrite"), "/d/existing"), b"changed\n");
}

#[test]
fn every_image_agrees_with_the_reference_lister() {
    for op in OPS {
        let dev = FileDevice::open(fixture(&format!("write-study/{op}.img"))).unwrap();
        let sb = Superblock::read(&dev).unwrap();
        for (id, name) in [
            (btree_id::INODES, "inodes"),
            (btree_id::DIRENTS, "dirents"),
            (btree_id::EXTENTS, "extents"),
        ] {
            let ours: Vec<String> = btree::walk(&dev, &sb, id)
                .unwrap_or_else(|e| panic!("{op} {name}: {e}"))
                .iter()
                .map(|k| format!("{} len {}", k.pos, k.size))
                .collect();
            let theirs: Vec<String> = read_text(&format!("write-study/{op}.{name}.txt"))
                .lines()
                .filter(|l| l.starts_with("u64s "))
                .map(|l| {
                    let w: Vec<&str> = l.split_whitespace().collect();
                    format!("{} len {}", w[4], w[6])
                })
                .collect();
            assert_eq!(ours, theirs, "{op} {name}");
        }
    }
}
