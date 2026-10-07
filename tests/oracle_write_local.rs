//! The write path without the reference tools: what this crate writes, this
//! crate reads back, on every architecture the host tiers run on. The
//! verdict on the bytes themselves is tests/write_oracle.rs, in the guest.
#![cfg(feature = "write")]

mod common;

use common::fixture;
use fs_bcachefs::write::Writer;
use fs_bcachefs::Filesystem;
use fs_core::FileDevice;

fn scratch(name: &str, test: &str) -> std::path::PathBuf {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tmp/write-local");
    std::fs::create_dir_all(&dir).unwrap();
    let img = dir.join(format!("{test}.img"));
    std::fs::copy(fixture(name), &img).unwrap();
    img
}

/// Whether creating a file in `dir` is refused, on a copy of `img`,
/// because a node it would go in is full; when it is, the copy must be byte
/// for byte what it was.
fn refused_as_full(img: &std::path::Path, dir: u64) -> bool {
    let probe = img.with_extension("probe.img");
    std::fs::copy(img, &probe).unwrap();
    let before = std::fs::read(&probe).unwrap();
    let mut w = Writer::open(FileDevice::open_rw(&probe).unwrap()).unwrap();
    let refused = match w.create_file(dir, b"probe", b"", 0o644) {
        Ok(_) => false,
        Err(fs_bcachefs::Error::Unsupported(m)) if m.contains("the node is full") => {
            drop(w);
            assert!(
                std::fs::read(&probe).unwrap() == before,
                "refused, but written"
            );
            true
        }
        Err(e) => panic!("{e}"),
    };
    std::fs::remove_file(&probe).unwrap();
    refused
}

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
        // The aged image is aged by a live mount, so how full its leaves are
        // differs from one fixture build to the next. Until nodes are split,
        // a full leaf must be refused before anything is written.
        if test == "local-aged" && refused_as_full(&img, dir) {
            continue;
        }
        let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
        let a = w.create_file(dir, b"one", b"first\n", 0o644).unwrap();
        let b = w.create_file(dir, b"two", b"", 0o600).unwrap();
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
    assert!(
        w.create_file(dir, b"big", &[0u8; 249], 0o644).is_err(),
        "over the inline limit"
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
