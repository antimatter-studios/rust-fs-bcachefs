//! `fs.bcachefs export` takes the shared tree options (#342): `--recursive`,
//! its `--max-depth` bound, `--preserve=links,times` (also given twice) and
//! `--verify`. The parser accepts them, so a TARGET that is not a bcachefs
//! image fails at the open with status 1, never as a usage error with status
//! 2, and nothing is written at DEST. The help shows the recursive form.
//!
//! What the exported trees hold, against the reference implementation's
//! own record, is tests/cli/test-export-tree.sh; what `--preserve=xattrs`
//! carries (#442), against the reference mount's listing, is
//! tests/cli/test-export-xattrs.sh.
//!
//! No tool, no fixture, no VM: the unit tier, built with `--features cli`.

mod common;

use common::ScratchDir;
use std::process::Command;

fn fs_bcachefs() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rust-fs-bcachefs"));
    cmd.arg("fs");
    cmd
}

#[test]
fn export_takes_the_shared_tree_options_before_the_image_is_read() {
    let dir = ScratchDir::new("export-tree-options");
    let img = dir.join("not-an-image.img");
    std::fs::write(&img, vec![0u8; 1 << 20]).unwrap();
    let dest = dir.join("dest");
    for flags in [
        &["--recursive"][..],
        &["--recursive", "--max-depth", "3"],
        &["--recursive", "--preserve=links,times"],
        &["--recursive", "--preserve", "links", "--preserve", "times"],
        &["--recursive", "--verify", "--replace"],
        &["--verify"],
    ] {
        let out = fs_bcachefs()
            .arg(&img)
            .arg("export")
            .args(flags)
            .arg("/tree")
            .arg(&dest)
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(1),
            "{flags:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            out.stdout.is_empty(),
            "{flags:?}: {}",
            String::from_utf8_lossy(&out.stdout)
        );
        assert!(!dest.exists(), "{flags:?} wrote {}", dest.display());
    }
}

#[test]
fn export_takes_preserve_xattrs_before_the_image_is_read() {
    let dir = ScratchDir::new("export-xattrs-options");
    let img = dir.join("not-an-image.img");
    std::fs::write(&img, vec![0u8; 1 << 20]).unwrap();
    let dest = dir.join("dest");
    for flags in [
        &["--recursive", "--preserve=xattrs"][..],
        &["--recursive", "--preserve=links,times,xattrs"],
        &["--recursive", "--preserve", "links", "--preserve", "xattrs"],
        &["--preserve=xattrs"],
    ] {
        let out = fs_bcachefs()
            .arg(&img)
            .arg("export")
            .args(flags)
            .arg("/xattr/one.txt")
            .arg(&dest)
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(1),
            "{flags:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!dest.exists(), "{flags:?} wrote {}", dest.display());
    }
}

#[test]
fn export_help_shows_the_recursive_form() {
    let out = fs_bcachefs()
        .args(["disk.img", "export", "--help"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let help = String::from_utf8_lossy(&out.stdout);
    for words in [
        "--recursive",
        "--max-depth",
        "--preserve",
        "--verify",
        "export --recursive /etc etc",
        "export --recursive --preserve=links,times /etc etc",
        "export --recursive --preserve=xattrs ",
    ] {
        assert!(help.contains(words), "export --help omits {words}:\n{help}");
    }
}
