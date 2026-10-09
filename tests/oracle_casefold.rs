//! Casefolded directories (#54), on the `casefold` set: formatted with the
//! filesystem-wide casefold option and populated by the reference
//! formatter from a tree of mixed-case and non-ASCII names
//! (scripts/guest-build-fixtures.sh, "CASEFOLDED DIRECTORIES"). S1 (2.7)
//! says each entry of such a directory stores the name as given and its
//! folded form; a reader that takes the whole value for one name lists
//! neither.
//!
//! The listing test first checks that the set holds casefolded entries at
//! all (the lister prints each as `Name (casefold name)`), so a formatter
//! that stops making them fails here rather than passing on an image that
//! never exercises the layout. The reference mount finds `HELLO.txt` as
//! `Hello.TXT` in such a directory (casefold.txt), and so must this reader.

mod common;

use common::{fixture, manifest, read_text, Entry};
use fs_bcachefs::check::check;
use fs_bcachefs::{Error, Filesystem};
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
    let dirents = read_text("casefold.dirents.txt");
    let folded = dirents
        .lines()
        .filter(|l| l.contains(" (casefold "))
        .count();
    assert!(
        folded > 40,
        "the casefold set holds {folded} casefolded entries (casefold.dirents.txt; casefold.txt \
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

#[test]
fn a_name_in_a_casefolded_directory_is_found_in_any_case() {
    let fs = Filesystem::open(FileDevice::open(fixture("casefold.img")).unwrap()).unwrap();
    for (asked, stored) in [
        ("/HELLO.txt", "/Hello.TXT"),
        ("/hello.txt", "/Hello.TXT"),
        ("/mixed/INNER.MD", "/MiXeD/Inner.md"),
        ("/STRASSE.TXT", "/Stra\u{df}e.txt"),
        ("/many/file-07.txt", "/Many/File-07.Txt"),
    ] {
        assert_eq!(
            fs.lookup(asked).ok(),
            Some(fs.lookup(stored).unwrap()),
            "{asked} as {stored}"
        );
    }
    let missing = fs.lookup("/Hello.TXT.not");
    assert!(matches!(missing, Err(Error::NotFound(_))), "{missing:?}");
}

/// Every casefolded entry's folded name, as the reference stored it, is
/// what `inode::casefold` makes of its name (#111): ASCII and non-ASCII
/// alike, full folding and canonical decomposition included.
#[test]
fn every_folded_name_is_the_names_casefold() {
    use fs_bcachefs::btree::{self, btree_id};
    use fs_bcachefs::inode::{casefold, dirent_folded_name, Dirent};
    use fs_bcachefs::superblock::Superblock;
    let dev = FileDevice::open(fixture("casefold.img")).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let mut wrong = Vec::new();
    let (mut n, mut non_ascii) = (0, 0);
    for k in btree::walk(&dev, &sb, btree_id::DIRENTS).unwrap() {
        if k.key_type != fs_bcachefs::bkey::key_type::DIRENT {
            continue;
        }
        let Some(stored) = dirent_folded_name(&k).unwrap() else {
            continue;
        };
        let name = Dirent::from_key(&k).unwrap().name;
        n += 1;
        non_ascii += usize::from(!name.is_ascii());
        if casefold(&name).as_deref() != Some(&stored[..]) {
            wrong.push(format!(
                "{:?}: stored {:?}, folded here {:?}",
                String::from_utf8_lossy(&name),
                String::from_utf8_lossy(&stored),
                casefold(&name).map(|f| String::from_utf8_lossy(&f).into_owned())
            ));
        }
    }
    assert!(
        n > 40 && non_ascii >= 2,
        "{n} folded names, {non_ascii} not ASCII"
    );
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Every entry of the set is found by its name in upper and in lower case,
/// non-ASCII names included (#111), as the reference mount finds them.
#[test]
fn every_name_is_found_in_upper_and_lower_case() {
    let fs = Filesystem::open(FileDevice::open(fixture("casefold.img")).unwrap()).unwrap();
    let mut wrong = Vec::new();
    for e in casefold_manifest() {
        let want = fs.lookup(&e.path).unwrap();
        for asked in [e.path.to_uppercase(), e.path.to_lowercase()] {
            if fs.lookup(&asked).ok() != Some(want) {
                wrong.push(format!("{asked} as {}", e.path));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The reference checker passes the set (casefold.txt: `fsck: clean`), and
/// so does this crate's.
#[test]
fn the_checker_passes_the_casefold_set() {
    let r = check(&FileDevice::open(fixture("casefold.img")).unwrap()).unwrap();
    assert!(r.clean(), "{:?}", r.problems);
    assert!(read_text("casefold.txt").contains("fsck: clean"));
}

/// The writer places names by their own hash in the plain dirent layout,
/// so it must not touch a casefolded directory.
#[cfg(feature = "write")]
#[test]
fn the_writer_refuses_to_change_a_casefolded_directory() {
    use fs_bcachefs::write::Writer;
    let dir =
        std::env::temp_dir().join(format!("rust-fs-bcachefs-casefold-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let img = dir.join("casefold.img");
    std::fs::copy(fixture("casefold.img"), &img).unwrap();
    let before = std::fs::read(&img).unwrap();
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    let root = fs_bcachefs::inode::ROOT_INO;
    match w.create_file(root, b"new.txt", b"new\n", 0o644) {
        Err(Error::Unsupported(m)) => assert!(m.contains("casefold"), "{m}"),
        other => panic!("create in a casefolded directory: {other:?}"),
    }
    match w.mkdir(root, b"NewDir", 0o755) {
        Err(Error::Unsupported(m)) => assert!(m.contains("casefold"), "{m}"),
        other => panic!("mkdir in a casefolded directory: {other:?}"),
    }
    drop(w);
    let after = std::fs::read(&img).unwrap();
    assert!(after == before, "the image changed");
}
