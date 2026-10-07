//! `Filesystem::read_range` against `Filesystem::read`: every file of every
//! fixture, read in windows of several sizes and at odd offsets, is the
//! whole file; the whole-file read is itself compared with the reference's
//! SHA-256 in `tests/oracle_fs.rs`, so this ties the windows to the oracle.

mod common;

use common::{fixture, manifest, SETS};
use fs_bcachefs::Filesystem;
use fs_core::FileDevice;

#[test]
fn windows_of_a_file_add_up_to_the_file() {
    let mut files = 0usize;
    for set in SETS {
        let fs =
            Filesystem::open(FileDevice::open(fixture(&format!("{set}.img"))).unwrap()).unwrap();
        for e in manifest(set).into_iter().filter(|e| e.kind == "file") {
            let size = e.size.unwrap_or(0);
            if size == 0 {
                continue;
            }
            let ino = fs.lookup(&e.path).unwrap();
            let whole = fs.read(ino).unwrap();
            assert_eq!(whole.len() as u64, size, "{set}:{}", e.path);
            let windows: &[usize] = if size <= 256 * 1024 {
                &[512, 4096, 3331]
            } else {
                &[65536, 70001]
            };
            for &w in windows {
                let mut got = Vec::with_capacity(whole.len());
                let mut off = 0u64;
                while off < size {
                    let piece = fs.read_range(ino, off, w).unwrap();
                    assert!(
                        !piece.is_empty(),
                        "{set}:{} window {w} at {off} is empty",
                        e.path
                    );
                    assert!(piece.len() <= w);
                    got.extend_from_slice(&piece);
                    off += piece.len() as u64;
                }
                assert!(
                    got == whole,
                    "{set}:{} read in {w}-byte windows differs",
                    e.path
                );
            }
            // Edges: past the end, across it, and a slice from the middle.
            assert!(
                fs.read_range(ino, size, 10).unwrap().is_empty(),
                "{set}:{}",
                e.path
            );
            assert_eq!(
                fs.read_range(ino, size - 1, 10).unwrap(),
                whole[whole.len() - 1..]
            );
            if size >= 12 {
                assert_eq!(
                    fs.read_range(ino, 7, 5).unwrap(),
                    whole[7..12],
                    "{set}:{}",
                    e.path
                );
            }
            files += 1;
        }
    }
    assert!(files >= 100, "only {files} files were read in windows");
}
