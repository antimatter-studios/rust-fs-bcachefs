//! Files as this crate reads them, compared with the tree the reference
//! formatter was given (every path, type, size, symlink target and the
//! SHA-256 of every file's contents) and with the reference lister's view
//! of every inode and directory entry.

mod common;

use common::{fixture, manifest, read_text, SETS};
use fs_bcachefs::Filesystem;
use fs_core::FileDevice;
use sha2::{Digest, Sha256};

fn open(set: &str) -> Filesystem<FileDevice> {
    Filesystem::open(FileDevice::open(fixture(&format!("{set}.img"))).unwrap())
        .unwrap_or_else(|e| panic!("{set}: {e}"))
}

#[test]
fn every_path_resolves_with_the_right_type() {
    for set in SETS {
        let fs = open(set);
        for e in manifest(set) {
            let ino = fs
                .lookup(&e.path)
                .unwrap_or_else(|err| panic!("{set} {}: {err}", e.path));
            let i = fs.inode(ino).unwrap();
            let ok = match e.kind.as_str() {
                "dir" => i.is_dir(),
                "file" => i.is_file(),
                "symlink" => i.is_symlink(),
                k => panic!("{k}"),
            };
            assert!(
                ok,
                "{set} {}: mode {:o} is not a {}",
                e.path, i.mode, e.kind
            );
            assert_eq!(i.mode & 0o7777, e.mode, "{set} {}: permissions", e.path);
            if let Some(size) = e.size {
                assert_eq!(i.size, size, "{set} {}: size", e.path);
            }
        }
    }
}

#[test]
fn every_directory_lists_exactly_its_entries() {
    for set in SETS {
        let fs = open(set);
        let m = manifest(set);
        for dir in m
            .iter()
            .map(|e| e.path.as_str())
            .chain(["/"])
            .filter(|p| *p == "/" || m.iter().any(|e| e.path == *p && e.kind == "dir"))
        {
            let prefix = if dir == "/" {
                "/".to_string()
            } else {
                format!("{dir}/")
            };
            let mut want: Vec<String> = m
                .iter()
                .filter_map(|e| {
                    e.path
                        .strip_prefix(&prefix)
                        .filter(|r| !r.contains('/'))
                        .map(str::to_string)
                })
                .collect();
            if dir == "/" {
                want.push("lost+found".into());
            }
            want.sort();
            let ino = fs.lookup(dir).unwrap();
            let mut got: Vec<String> = fs
                .readdir(ino)
                .unwrap()
                .iter()
                .map(|d| String::from_utf8_lossy(&d.name).into_owned())
                .collect();
            got.sort();
            assert_eq!(got, want, "{set} {dir}");
        }
    }
}

#[test]
fn every_file_and_symlink_reads_back_byte_for_byte() {
    for set in SETS {
        let fs = open(set);
        for e in manifest(set) {
            let ino = fs.lookup(&e.path).unwrap();
            match e.kind.as_str() {
                "file" => {
                    let data = fs
                        .read(ino)
                        .unwrap_or_else(|err| panic!("{set} {}: {err}", e.path));
                    let sum = format!("{:x}", Sha256::digest(&data));
                    assert_eq!(Some(sum), e.sha256, "{set} {}: sha256", e.path);
                }
                "symlink" => {
                    let t = fs.read(ino).unwrap();
                    assert_eq!(
                        Some(String::from_utf8(t).unwrap()),
                        e.target,
                        "{set} {}",
                        e.path
                    );
                }
                _ => {}
            }
        }
    }
}

#[test]
fn every_inode_matches_the_reference_lister() {
    for set in SETS {
        let fs = open(set);
        let text = read_text(&format!("{set}.inodes.txt"));
        let mut n = 0;
        let mut cur: Option<u64> = None;
        for line in text.lines() {
            if line.starts_with("u64s ") {
                let pos = line.split_whitespace().nth(4).unwrap();
                cur = Some(pos.split(':').nth(1).unwrap().parse().unwrap());
                n += 1;
                continue;
            }
            let (Some(ino), Some((k, v))) = (cur, line.trim().split_once('=')) else {
                continue;
            };
            let i = fs.inode(ino).unwrap();
            let v = v.split_whitespace().next().unwrap();
            let ours = match k {
                "mode" => format!("{:o}", i.mode),
                "bi_size" => i.size.to_string(),
                "bi_sectors" => i.sectors.to_string(),
                "bi_uid" => i.uid.to_string(),
                "bi_gid" => i.gid.to_string(),
                "bi_nlink" => i.nlink.to_string(),
                "bi_atime" => i.atime.to_string(),
                "bi_mtime" => i.mtime.to_string(),
                "bi_ctime" => i.ctime.to_string(),
                _ => continue,
            };
            assert_eq!(ours, v, "{set} inode {ino}: {k}");
        }
        assert!(n > 300, "{set}: the lister printed only {n} inodes");
    }
}
