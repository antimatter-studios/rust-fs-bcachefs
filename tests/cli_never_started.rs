//! The read tools on an image the reference formatter wrote and nothing has
//! started (#534). The superblock in `tests/data` is one the formatter wrote at
//! `--version=1.38` on a 256 MiB image, and `tests/read_oracle_never_started.rs`
//! holds the same behaviour to the reference tools in the guest, where they
//! exist. docs/clean-room.md, "Images formatted and never started", has the
//! measurement.
//!
//! No tool, no fixture, no VM: the unit tier, built with `--features cli`.

mod common;
#[path = "common/never_started.rs"]
mod never_started;

use std::io::{Seek, SeekFrom, Write};
use std::process::{Command, Output};

use common::{ScratchDir, ScratchImage};
use fs_bcachefs::{Error, Filesystem};
use fs_core::FileDevice;
use never_started::{Edit, On, CLAIMS_A_START, DEVICE_BYTES};

const SUPERBLOCK: &[u8] = include_bytes!("data/never-started-superblock-1.38.bin");
const REFUSAL: &str = "has not been started";
const EXIT_USAGE: i32 = 2;
const EXIT_REFUSED: i32 = 4;

fn image(edit: Edit) -> ScratchImage {
    let image = ScratchDir::new("cli-never-started").into_image("image.img");
    let mut file = std::fs::File::create(&image).unwrap();
    file.set_len(DEVICE_BYTES).unwrap();
    for (at, bytes) in never_started::image(SUPERBLOCK, On::All, edit) {
        file.seek(SeekFrom::Start(at)).unwrap();
        file.write_all(&bytes).unwrap();
    }
    image
}

fn fs_bcachefs(image: &ScratchImage, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rust-fs-bcachefs"))
        .arg("fs")
        .arg(image.as_path())
        .args(args)
        .output()
        .unwrap()
}

fn text(output: &[u8]) -> String {
    String::from_utf8_lossy(output).trim().to_owned()
}

/// One property, as `get KEY --text` prints it.
fn property(image: &ScratchImage, key: &str) -> String {
    let out = fs_bcachefs(image, &["get", key, "--text"]);
    assert!(
        out.status.success(),
        "get {key}: {:?}: {}",
        out.status.code(),
        text(&out.stderr)
    );
    text(&out.stdout)
}

#[test]
fn get_reads_the_superblock_and_says_it_has_not_started() {
    let image = image(|_| {});
    for (key, want) in [
        ("fs", "bcachefs"),
        ("bcachefs.version", "1.38"),
        ("bcachefs.started", "false"),
        ("bcachefs.clean", "false"),
        ("bcachefs.encrypted", "false"),
        ("bcachefs.devices", "1"),
        ("bcachefs.btree_node_size", "131072"),
        ("block_size", "512"),
        ("total_bytes", "268435456"),
        ("dirty", "true"),
    ] {
        assert_eq!(property(&image, key), want, "{key}");
    }
    assert_eq!(property(&image, "bcachefs.uuid").len(), 36);
}

/// Nothing records what is free until a start has made the freespace btree.
#[test]
fn get_does_not_claim_a_free_space_nothing_records() {
    let image = image(|_| {});
    let out = fs_bcachefs(&image, &["get", "free_bytes"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains("null"), "{}", text(&out.stdout));
}

#[test]
fn the_whole_report_is_given() {
    let image = image(|_| {});
    let out = fs_bcachefs(&image, &["get"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let report = text(&out.stdout);
    for key in ["\"version\"", "\"started\"", "\"total_bytes\""] {
        assert!(report.contains(key), "{key} in {report}");
    }
}

/// `version` is not a key on any image: it is `bcachefs.version`. The image is
/// no reason to say anything else, and nothing about a journal.
#[test]
fn an_unknown_key_is_the_usage_error_it_is_on_a_started_image() {
    let image = image(|_| {});
    let out = fs_bcachefs(&image, &["get", "version"]);
    assert_eq!(out.status.code(), Some(EXIT_USAGE), "{}", text(&out.stderr));
    let error = text(&out.stderr);
    assert!(error.contains("bcachefs.<field>"), "{error}");
    assert!(!error.contains("journal"), "{error}");
}

#[test]
fn what_needs_a_directory_is_refused_by_name() {
    let image = image(|_| {});
    for args in [
        &["ls"][..],
        &["ls", "/lost+found"],
        &["ls", "-R"],
        &["tree"],
        &["stat", "/"],
        &["read", "/file"],
        &["export", "/file", "/nonexistent-dest"],
        &["mkdir", "/dir"],
        &["rm", "/file"],
    ] {
        let out = fs_bcachefs(&image, args);
        let error = text(&out.stderr);
        assert_eq!(out.status.code(), Some(EXIT_REFUSED), "{args:?}: {error}");
        assert!(error.contains(REFUSAL), "{args:?}: {error}");
        assert!(!error.contains("not implemented"), "{args:?}: {error}");
        assert!(!error.contains("journal"), "{args:?}: {error}");
        assert!(out.stdout.is_empty(), "{args:?}");
    }
}

#[test]
fn the_library_refuses_it_as_the_state_it_is_in() {
    let image = image(|_| {});
    let dev = FileDevice::open(&image).unwrap();
    let Err(error) = Filesystem::open(dev) else {
        panic!("a filesystem nothing has started was opened");
    };
    assert!(matches!(error, Error::Unsupported(_)), "{error}");
    assert!(error.to_string().contains(REFUSAL), "{error}");
    assert!(error.is_state_refusal(), "{error}");
}

/// A superblock that records a start is read as the started filesystem it says
/// it is, as the checker does: its journal is what is wrong with it.
#[test]
fn a_superblock_that_records_a_start_is_not_taken_for_one() {
    for (name, edit) in CLAIMS_A_START {
        let image = image(*edit);
        let out = fs_bcachefs(&image, &["ls"]);
        let error = text(&out.stderr);
        assert!(!out.status.success(), "{name}");
        assert!(!error.contains(REFUSAL), "{name}: {error}");
    }
}
