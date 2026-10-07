//! The `bgcompress` fixture (`--background_compression=lz4`): every file
//! reads back to the manifest's SHA-256, or the extent that cannot be read
//! names the entry kind this reader does not decode and the lister's line
//! for it is the observation to decode it from (#52).

mod common;

use common::{fixture, manifest, read_text};
use fs_bcachefs::{Error, Filesystem};
use fs_core::FileDevice;
use sha2::{Digest, Sha256};

#[test]
fn every_file_reads_or_the_refusal_names_the_entry_kind_and_the_lister_shows_it() {
    let fs = Filesystem::open(FileDevice::open(fixture("bgcompress.img")).unwrap()).unwrap();
    let listing = read_text("bgcompress.extents.txt");
    let mut files = 0;
    for e in manifest("bgcompress")
        .into_iter()
        .filter(|e| e.kind == "file")
    {
        let ino = fs.lookup(&e.path).unwrap();
        match fs.read(ino) {
            Ok(data) => {
                assert_eq!(
                    Some(format!("{:x}", Sha256::digest(&data))),
                    e.sha256,
                    "{}",
                    e.path
                );
                files += 1;
            }
            Err(Error::Unsupported(m)) if m.contains("extent entry type") => {
                // The inode's first key and the entry lines indented under it.
                let key = format!(" {ino}:");
                let seen: Vec<&str> = listing
                    .lines()
                    .skip_while(|l| !(l.starts_with("u64s") && l.contains(&key)))
                    .enumerate()
                    .take_while(|(i, l)| *i == 0 || l.starts_with(' '))
                    .map(|(_, l)| l)
                    .collect();
                panic!(
                    "{}: {m}\nthe lister's view of inode {ino}'s extents (bgcompress.extents.txt):\n{}",
                    e.path,
                    seen.join("\n")
                );
            }
            Err(err) => panic!("{}: {err}", e.path),
        }
    }
    assert!(files > 100, "only {files} files");
}
