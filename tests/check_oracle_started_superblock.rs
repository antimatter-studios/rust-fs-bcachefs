//! The checker against the reference checker on the superblock of a filesystem
//! the reference formatter started, INSIDE the harness guest only
//! (`chore test:vm`); anywhere else it fails naming that. Nothing skips (#533).
//!
//! The reference refuses such a superblock before it reads anything else, and so
//! must this checker. A crash, a hang or an internal fault of the reference
//! stays private: it fails with no output.

mod common;
#[path = "common/never_started.rs"]
mod never_started;
#[path = "common/started_superblock.rs"]
mod started_superblock;

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::Command;

use common::{ScratchDir, ScratchImage};
use fs_bcachefs::check::check;
use fs_core::FileDevice;
use never_started::{Case, DEVICE_BYTES};
use started_superblock::{edited_copies, CASES, REFERENCE_SAYS_MORE};

const REFERENCE: &str = "/usr/local/bin/bcachefs-ref";
/// How many runs the reference gets on an image it is expected to accept: it
/// reports such an image rarely, as on images nothing has started (#548). One
/// acceptance settles it, and an image it reports every time still fails.
const RUNS_TO_ACCEPT: usize = 3;

/// The reference checker's exit status. Nothing of a fault is kept.
fn reference(image: &Path) -> i32 {
    let out = common::reference_runs::output(&["fsck", "-n"], image);
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    let faulted = [
        "BUG",
        "ssertion",
        "acktrace",
        "Call Trace",
        "panicked at",
        "Kernel panic",
    ]
    .iter()
    .any(|fault| text.contains(fault));
    let status = out.status.code();
    assert!(
        matches!(status, Some(0 | 1 | 9 | 12 | 13)) && !faulted,
        "reference fault: 1; no case details retained"
    );
    status.unwrap()
}

/// Whether the reference reports the image, given what the table expects.
fn reference_reports(image: &Path, expected: bool) -> bool {
    let mut status = reference(image);
    if !expected {
        for _ in 1..RUNS_TO_ACCEPT {
            if status == 0 {
                break;
            }
            status = reference(image);
        }
    }
    status != 0
}

fn format(args: &[&str]) -> ScratchImage {
    assert!(
        Path::new(REFERENCE).exists(),
        "the reference formatter is required: run `chore test:vm`"
    );
    let image = ScratchDir::new("started-superblock").into_image("image.img");
    std::fs::File::create(&image)
        .unwrap()
        .set_len(DEVICE_BYTES)
        .unwrap();
    let out = Command::new("bcachefs-ref")
        .args(["format", "-q", "--version=1.39"])
        .args(args)
        .arg(image.as_path())
        .output()
        .expect("chore test:vm provides the reference formatter");
    assert!(out.status.success(), "format {args:?} failed");
    image
}

fn edited(formatted: &Path, case: &Case) -> ScratchImage {
    let image = ScratchDir::new("started-superblock-edit").into_image("image.img");
    common::copy_sparse(formatted, &image);
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&image)
        .unwrap();
    let ranges = edited_copies(
        &mut |at, len| {
            let mut bytes = vec![0; len];
            file.seek(SeekFrom::Start(at)).unwrap();
            file.read_exact(&mut bytes).unwrap();
            bytes
        },
        case.on,
        case.edit,
    );
    for (at, bytes) in ranges {
        file.seek(SeekFrom::Start(at)).unwrap();
        file.write_all(&bytes).unwrap();
    }
    drop(file);
    image
}

/// An `Err` is a filesystem the checker cannot check: not a clean verdict.
fn checker_reports(image: &Path) -> bool {
    check(&FileDevice::open(image).unwrap()).map_or(true, |report| !report.clean())
}

#[test]
fn both_checkers_agree_on_every_superblock_edit_of_a_started_image() {
    let formatted = format(&[]);
    let mut mismatches = Vec::new();
    for case in CASES {
        let image = edited(&formatted, case);
        let reference = reference_reports(&image, case.reported);
        let checker = checker_reports(&image);
        if reference != case.reported || checker != case.reported {
            mismatches.push(format!(
                "{}: table reported={}, reference reports={reference}, checker reports={checker}",
                case.name, case.reported
            ));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// These the table does not hold this checker to: the issues they name are
/// about what the reference finds past the superblock. The reference is held to
/// its verdict, so that a change in it is seen.
#[test]
fn the_reference_reports_what_a_pass_over_the_filesystem_finds() {
    let formatted = format(&[]);
    let mut mismatches = Vec::new();
    for case in REFERENCE_SAYS_MORE {
        let image = edited(&formatted, case);
        if reference_reports(&image, case.reported) != case.reported {
            mismatches.push(case.name);
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

#[test]
fn both_checkers_accept_what_every_formatter_option_leaves_started() {
    const ACCEPTED: &[&[&str]] = &[
        &["--block_size=4096"],
        &["--btree_node_size=64k"],
        &["--bucket_size=64k", "--btree_node_size=32k"],
        &["--errors=panic"],
        &["--metadata_checksum=none"],
        &["--metadata_checksum=xxhash"],
        &["--data_checksum=none"],
        &["--compression=zstd"],
        &["--compression=zstd:5"],
        &["--background_compression=lz4:3"],
        &["--str_hash=crc32c"],
        &["--erasure_code"],
        &["--ec_max_data_blocks=4"],
        &["--casefold"],
        &["--inodes_32bit"],
        &["--shard_inode_numbers_bits=3"],
        &["--gc_reserve_percent=20"],
        &["--gc_reserve_bytes=16M"],
        &["--root_reserve_percent=5"],
        &["--wide_macs"],
        &["--noacl"],
        &["--usrquota", "--grpquota", "--prjquota"],
        &["--write_degraded=no"],
        &["--missing_dev_timeout=60"],
        &["--scrub_journal_max_rewind_secs=5"],
        &["--nojournal_transaction_names"],
        &["--allocator_stuck_timeout=60"],
        &["--nocow"],
        &["-L", "label"],
        &["-U", "11111111-2222-3333-4444-555555555555"],
        &["--superblock_size=4M"],
        &["--label=a.b"],
        &["--label=a", "--foreground_target=a"],
        &["--durability=2"],
        &["--failure_domain=z"],
        &["--data_replicas=2"],
        &["--write_error_timeout=60"],
        &["--checksum_err_retry_nr=5"],
        &["--encoded_extent_max=1M"],
    ];
    let mut mismatches = Vec::new();
    for args in ACCEPTED {
        let image = format(args);
        let reference = reference_reports(&image, false);
        let report = check(&FileDevice::open(&image).unwrap());
        let clean = report.as_ref().is_ok_and(|r| r.clean());
        if reference || !clean {
            mismatches.push(format!(
                "{args:?}: reference reports={reference}, checker {:?}",
                report.map(|r| r.problems).map_err(|e| e.to_string())
            ));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}
