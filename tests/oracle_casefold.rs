//! Casefolded directories (#54), on the `casefold` set: formatted with the
//! filesystem-wide casefold option and populated by the reference
//! formatter from a tree of mixed-case and non-ASCII names
//! (scripts/guest-build-fixtures.sh, "CASEFOLDED DIRECTORIES"). S1 (2.7)
//! says each entry of such a directory stores the name as given and its
//! folded form; a reader that takes the whole value for one name lists
//! neither.
//!
//! The test first checks that the set holds a casefolded directory at all,
//! so a formatter that stops making one fails here rather than passing on
//! an image that never exercises the layout.

mod common;

use common::{fixture, manifest, read_text, Entry};
use fs_bcachefs::Filesystem;
use fs_core::FileDevice;
use sha2::{Digest, Sha256};

/// The manifest's paths as the tree had them: the build's `json.dump`
/// writes a non-ASCII character as `\uXXXX`, which the shared reader
/// keeps as written.
fn casefold_manifest() -> Vec<Entry> {
    let mut m = manifest("casefold");
    for e in &mut m {
        let mut out = String::new();
        let mut rest = e.path.as_str();
        while let Some(i) = rest.find("\\u") {
            out.push_str(&rest[..i]);
            let code = u32::from_str_radix(&rest[i + 2..i + 6], 16).unwrap();
            out.push(char::from_u32(code).unwrap());
            rest = &rest[i + 6..];
        }
        out.push_str(rest);
        e.path = out;
    }
    m
}

#[test]
fn every_name_in_a_casefolded_directory_lists_as_given_and_reads_back() {
    let inodes = read_text("casefold.inodes.txt");
    let casefolded = inodes
        .lines()
        .map(str::trim)
        .any(|l| l.starts_with("bi_casefold=") && l != "bi_casefold=0");
    assert!(
        casefolded,
        "the casefold set holds no inode with bi_casefold set (casefold.inodes.txt; casefold.txt \
         has the formatter's answer)"
    );
    let fs = Filesystem::open(FileDevice::open(fixture("casefold.img")).unwrap()).unwrap();
    let m = casefold_manifest();
    let dirs = m
        .iter()
        .filter(|e| e.kind == "dir")
        .map(|e| e.path.as_str());
    for dir in dirs.chain(["/"]) {
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
        assert_eq!(got, want, "{dir}");
    }
    for e in &m {
        let ino = fs
            .lookup(&e.path)
            .unwrap_or_else(|err| panic!("{}: {err}", e.path));
        match e.kind.as_str() {
            "file" => {
                let data = fs.read(ino).unwrap();
                let sum = format!("{:x}", Sha256::digest(&data));
                assert_eq!(Some(sum), e.sha256, "{}", e.path);
            }
            "symlink" => {
                let t = String::from_utf8(fs.read(ino).unwrap()).unwrap();
                assert_eq!(Some(t), e.target, "{}", e.path);
            }
            _ => {}
        }
    }
}
