//! A write or truncate at a large offset must not take down the process
//! (#156). The writer rewrites a file whole, so it held the whole new size
//! in memory before anything checked that it could: `truncate` to 1 TiB, or
//! a one-byte `write_at` at 1 TiB, asked for a terabyte of zeros, and the
//! allocation failure aborted the process, which neither the caller nor the
//! C ABI's guard can catch. Each case runs in a child process, on a copy of
//! `default` (64 MiB), and must come back refused, the file unchanged.
#![cfg(feature = "write")]

mod common;

use common::fixture;
use fs_bcachefs::inode::ROOT_INO;
use fs_bcachefs::write::Writer;
use fs_bcachefs::Filesystem;
use fs_core::FileDevice;

const TIB: u64 = 1 << 40;

fn scratch(test: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "rust-fs-bcachefs-large-offset-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let img = dir.join(format!("{test}.img"));
    std::fs::copy(fixture("default.img"), &img).unwrap();
    img
}

const CHILD: &str = "RUST_FS_BCACHEFS_LARGE_OFFSET_CHILD";

/// Run test `name` again in a child process with CHILD set, and return what
/// it printed; the child does the write, so an abort ends only the child.
fn in_child(name: &str) -> (std::process::ExitStatus, String) {
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([name, "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, name)
        .output()
        .unwrap();
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status, text)
}

fn is_child(name: &str) -> bool {
    std::env::var(CHILD).is_ok_and(|v| v == name)
}

/// In the child: a copy of `default` with a small file, `op` applied to it,
/// and a line saying whether it was refused and the file left as it was.
fn child(test: &str, op: impl Fn(&mut Writer<FileDevice>, u64) -> fs_bcachefs::Result<()>) {
    let img = scratch(test);
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    let ino = w
        .create_file(ROOT_INO, b"small", b"small\n", 0o644)
        .unwrap();
    let got = op(&mut w, ino);
    drop(w);
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    match got {
        Err(e) if fs.read(ino).unwrap() == b"small\n" => println!("CHILD-REFUSED {e}"),
        Err(e) => println!("CHILD-REFUSED-BUT-CHANGED {e}"),
        Ok(()) => println!("CHILD-ACCEPTED"),
    }
}

fn assert_refused(name: &str, what: &str) {
    let (status, text) = in_child(name);
    assert!(
        status.success() && text.contains("CHILD-REFUSED "),
        "{what} on a 64 MiB filesystem was not refused with the file left as it was \
         (status {status:?}; an abort is SIGABRT, a kill SIGKILL):\n{text}"
    );
}

#[test]
fn a_truncate_to_1_tib_is_refused_without_aborting() {
    let name = "a_truncate_to_1_tib_is_refused_without_aborting";
    if is_child(name) {
        child("truncate", |w, ino| w.truncate(ino, TIB));
        return;
    }
    assert_refused(name, "a truncate to 1 TiB");
}

#[test]
fn a_write_at_1_tib_is_refused_without_aborting() {
    let name = "a_write_at_1_tib_is_refused_without_aborting";
    if is_child(name) {
        child("write-at", |w, ino| w.write_at(ino, TIB, b"x"));
        return;
    }
    assert_refused(name, "a one-byte write at 1 TiB");
}
