//! Lookups and listings go through a btree cursor: they read the nodes on
//! their path, not whole btrees. Measured with fs_core's CountingDevice on
//! the `large` fixture, whose inodes btree is 60-odd leaves wide.

mod common;

use std::sync::Arc;

use common::{fixture, read_text};
use fs_bcachefs::Filesystem;
use fs_core::{BlockRead, CountingDevice, FileDevice};

fn leaves(name: &str) -> usize {
    read_text(name)
        .lines()
        .filter(|l| l.starts_with("l 0 "))
        .count()
}

#[test]
fn a_lookup_and_a_read_touch_only_their_path() {
    let inode_leaves = leaves("large.inodes.formats.txt");
    assert!(
        inode_leaves >= 40,
        "large: only {inode_leaves} inode leaves; the bound below would prove nothing"
    );
    let dev: Arc<dyn BlockRead> = Arc::new(FileDevice::open(fixture("large.img")).unwrap());
    let counting = CountingDevice::new(dev);
    let fs = Filesystem::open(&counting).unwrap();
    let ino = fs.lookup("/wide/entry-12345").unwrap();
    assert_eq!(fs.read(ino).unwrap(), b"12345\n");
    let reads = counting.reads();
    // The superblock, then root and leaf of the dirents btree for each of
    // two components, of the inodes btree for each inode, and of the
    // extents btree: a few dozen at most, against every leaf for a whole
    // tree.
    assert!(
        reads < 40 && (reads as usize) < inode_leaves,
        "open, lookup and read took {reads} reads; the inodes btree alone has {inode_leaves} leaves"
    );
}

#[test]
fn a_large_directory_lists_whole_through_the_cursor() {
    let fs = Filesystem::open(FileDevice::open(fixture("large.img")).unwrap()).unwrap();
    let wide = fs.lookup("/wide").unwrap();
    let mut names: Vec<String> = fs
        .readdir(wide)
        .unwrap()
        .iter()
        .map(|d| String::from_utf8_lossy(&d.name).into_owned())
        .collect();
    names.sort();
    assert_eq!(names.len(), 30000);
    assert_eq!(names[0], "entry-00000");
    assert_eq!(names[29999], "entry-29999");
    for n in ["entry-00000", "entry-17000", "entry-29999"] {
        let i = fs.lookup(&format!("/wide/{n}")).unwrap();
        let want = format!(
            "{}\n",
            n.trim_start_matches("entry-").trim_start_matches('0')
        );
        let want = if want == "\n" {
            "0\n".to_string()
        } else {
            want
        };
        assert_eq!(fs.read(i).unwrap(), want.as_bytes(), "{n}");
    }
}
