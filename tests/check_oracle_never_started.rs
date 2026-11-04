//! The checker against the reference checker on images the reference
//! formatter made and nothing has started, INSIDE the harness guest only
//! (`chore test:vm`); anywhere else it fails naming that. Nothing skips (#505).
//!
//! The reference starts such an image in memory and accepts it. So must this
//! checker, and it must report the damage the reference reports on a superblock
//! that says the same thing. A crash, a hang or an internal fault of the
//! reference stays private: it fails with no output.

mod common;
#[path = "common/never_started.rs"]
mod never_started;

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::Command;

use common::{ScratchDir, ScratchImage};
use fs_bcachefs::check::check;
use fs_core::FileDevice;
use never_started::{Edit, On, CASES, CLAIMS_A_START, COPIES, DEVICE_BYTES};

const REFERENCE: &str = "/usr/local/bin/bcachefs-ref";

/// The reference checker's exit status and printed text. Nothing of a fault
/// is kept.
fn reference(image: &Path) -> (i32, String) {
    assert!(
        Path::new(REFERENCE).exists(),
        "the reference checker is required: run `chore test:vm`"
    );
    let out = Command::new("sh")
        .args([
            "-c",
            "ulimit -c 0; exec timeout --kill-after=5 60 bcachefs-ref fsck -n \"$1\"",
            "reference",
        ])
        .arg(image)
        .output()
        .expect("chore test:vm provides the reference checker");
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
        "reference fault or timeout: 1; no case details retained"
    );
    (status.unwrap(), text)
}

/// How many runs the reference gets on an image it is expected to accept. It
/// reports such an image rarely, about once in 6,000 runs when it was seen at all,
/// with "error creating lost+found Device or resource busy" (#548,
/// docs/clean-room.md). One run that accepts settles it, and an image it reports
/// every time still fails. An image expected to be reported gets one run: none of
/// those ever accepted.
const RUNS_TO_ACCEPT: usize = 3;

/// [`reference`], for an image the reference is expected to accept.
fn reference_expecting_acceptance(image: &Path) -> (i32, String) {
    let mut last = reference(image);
    for _ in 1..RUNS_TO_ACCEPT {
        if last.0 == 0 {
            break;
        }
        last = reference(image);
    }
    last
}

fn format(size: u64, args: &[&str]) -> ScratchImage {
    assert!(
        Path::new(REFERENCE).exists(),
        "the reference formatter is required: run `chore test:vm`"
    );
    let image = ScratchDir::new("never-started").into_image("image.img");
    std::fs::File::create(&image)
        .unwrap()
        .set_len(size)
        .unwrap();
    let out = Command::new("bcachefs-ref")
        .args(["format", "-q"])
        .args(args)
        .arg(image.as_path())
        .output()
        .expect("chore test:vm provides the reference formatter");
    assert!(out.status.success(), "format {args:?} failed");
    image
}

fn checker_reports(image: &Path) -> Result<bool, String> {
    match check(&FileDevice::open(image).unwrap()) {
        Ok(report) => Ok(!report.clean()),
        Err(e) => Err(e.to_string()),
    }
}

fn superblock_bytes(image: &Path) -> Vec<u8> {
    let mut file = std::fs::File::open(image).unwrap();
    let mut head = vec![0; 0x2f0];
    file.seek(SeekFrom::Start(8 * 512)).unwrap();
    file.read_exact(&mut head).unwrap();
    let u64s = u32::from_le_bytes(head[0x7c..0x80].try_into().unwrap()) as usize;
    let mut bytes = vec![0; 0x2f0 + u64s * 8];
    file.seek(SeekFrom::Start(8 * 512)).unwrap();
    file.read_exact(&mut bytes).unwrap();
    bytes
}

fn written(ranges: &[(u64, Vec<u8>)]) -> ScratchImage {
    let image = ScratchDir::new("never-started-edit").into_image("image.img");
    let mut file = std::fs::File::create(&image).unwrap();
    file.set_len(DEVICE_BYTES).unwrap();
    for (at, bytes) in ranges {
        file.seek(SeekFrom::Start(*at)).unwrap();
        file.write_all(bytes).unwrap();
    }
    image
}

#[test]
fn both_checkers_accept_every_version_the_formatter_leaves_to_its_first_start() {
    for minor in (0..=38).chain([40]) {
        let image = format(DEVICE_BYTES, &[&format!("--version=1.{minor}")]);
        let (status, text) = reference_expecting_acceptance(&image);
        assert_eq!(status, 0, "1.{minor}");
        assert!(text.contains("initializing new filesystem"), "1.{minor}");
        let report = check(&FileDevice::open(&image).unwrap()).unwrap();
        assert!(report.clean(), "1.{minor}: {:?}", report.problems);
        assert_eq!(report.inodes, 0, "1.{minor}");
    }
}

#[test]
fn the_formatter_starts_the_current_version_itself() {
    let image = format(DEVICE_BYTES, &["--version=1.39"]);
    let (status, text) = reference_expecting_acceptance(&image);
    assert_eq!(status, 0);
    assert!(!text.contains("initializing new filesystem"));
    let report = check(&FileDevice::open(&image).unwrap()).unwrap();
    assert!(report.clean(), "{:?}", report.problems);
    assert!(report.inodes > 0, "it was started, so it holds its root");
}

#[test]
fn both_checkers_agree_on_what_every_formatter_option_leaves() {
    const ACCEPTED: &[&[&str]] = &[
        &["--block_size=4096"],
        &["--btree_node_size=64k"],
        &["--bucket_size=64k", "--btree_node_size=32k"],
        &["--errors=panic"],
        &["--metadata_checksum=xxhash"],
        &["--metadata_checksum=none"],
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
    const REPORTED: &[&[&str]] = &[
        &["--state=ro"],
        &["--durability=0"],
        &["--metadata_replicas=2"],
        &["--durability=2", "--metadata_replicas=2"],
        &["--replicas=2"],
    ];
    let mut mismatches = Vec::new();
    for version in ["--version=1.38", "--version=1.0"] {
        for (args, expected) in ACCEPTED
            .iter()
            .map(|a| (a, 0))
            .chain(REPORTED.iter().map(|a| (a, 1)))
        {
            let all: Vec<&str> = std::iter::once(version)
                .chain(args.iter().copied())
                .collect();
            let image = format(DEVICE_BYTES, &all);
            let (status, _) = if expected == 0 {
                reference_expecting_acceptance(&image)
            } else {
                reference(&image)
            };
            let reports = checker_reports(&image);
            if status != expected || reports != Ok(expected != 0) {
                mismatches.push(format!("{all:?}: reference {status}, checker {reports:?}"));
            }
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

#[test]
fn both_checkers_accept_smaller_devices() {
    for size in [16u64 << 20, 64 << 20, 512 << 20] {
        let image = format(size, &["--version=1.38"]);
        assert_eq!(reference_expecting_acceptance(&image).0, 0, "{size}");
        assert_eq!(checker_reports(&image), Ok(false), "{size}");
    }
}

#[test]
fn both_checkers_agree_on_every_superblock_edit() {
    let formatted = format(DEVICE_BYTES, &["--version=1.38"]);
    let sb = superblock_bytes(&formatted);
    // The table's images are this image, byte for byte, unedited.
    let unedited = written(&never_started::image(&sb, On::All, |_| {}));
    assert_eq!(
        std::fs::read(&formatted).unwrap(),
        std::fs::read(&unedited).unwrap()
    );
    let mut mismatches = Vec::new();
    for case in CASES {
        let image = written(&never_started::image(&sb, case.on, case.edit));
        let (status, _) = if case.reported {
            reference(&image)
        } else {
            reference_expecting_acceptance(&image)
        };
        // An `Err` is a filesystem the checker cannot check: not a clean verdict.
        let reports = checker_reports(&image).unwrap_or(true);
        if (status != 0) != case.reported || reports != case.reported {
            mismatches.push(format!(
                "{}: table reported={}, reference {status}, checker reports={reports}",
                case.name, case.reported
            ));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// The first flag word's bit 0 is all the reference goes by: these it starts
/// anew and calls clean. A field a start leaves says this is no new
/// filesystem, so this checker reports them.
#[test]
fn this_checker_reports_a_formatted_image_recording_a_start() {
    let formatted = format(DEVICE_BYTES, &["--version=1.38"]);
    let sb = superblock_bytes(&formatted);
    for (name, edit) in CLAIMS_A_START {
        let image = written(&never_started::image(&sb, On::All, *edit));
        assert_eq!(reference_expecting_acceptance(&image).0, 0, "{name}");
        assert_eq!(checker_reports(&image), Ok(true), "{name}");
    }
}

#[test]
fn what_the_device_holds_outside_the_superblocks_is_not_read() {
    let formatted = format(DEVICE_BYTES, &["--version=1.38"]);
    let scribbled = ScratchDir::new("never-started-body").into_image("image.img");
    common::copy_sparse(&formatted, &scribbled);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(&scribbled)
        .unwrap();
    for at in [0x20_0000u64, 0x24_0000, 0x42_0000, 0x800_0000] {
        file.seek(SeekFrom::Start(at)).unwrap();
        let noise: Vec<u8> = (0..0x2_0000u32).map(|i| ((i * 251) >> 3) as u8).collect();
        file.write_all(&noise).unwrap();
    }
    drop(file);
    assert_eq!(reference_expecting_acceptance(&scribbled).0, 0);
    assert_eq!(checker_reports(&scribbled), Ok(false));
}

#[test]
fn a_superblock_copy_that_does_not_read_is_reported() {
    let formatted = format(DEVICE_BYTES, &["--version=1.38"]);
    for sector in COPIES {
        let image = ScratchDir::new("never-started-copy").into_image("image.img");
        common::copy_sparse(&formatted, &image);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(&image)
            .unwrap();
        file.seek(SeekFrom::Start(sector * 512)).unwrap();
        file.write_all(&[0; 4096]).unwrap();
        drop(file);
        // The reference reads another copy and goes on. A copy that does not
        // read is damage all the same, and this checker reports it on every
        // image, started or not (#261).
        assert_eq!(reference_expecting_acceptance(&image).0, 0, "{sector}");
        assert_eq!(checker_reports(&image), Ok(true), "{sector}");
    }
}

/// An edit to every copy of a started image, its checksum type set to none so
/// nothing needs restamping.
fn edit_started(image: &Path, edit: Edit) {
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(image)
        .unwrap();
    for sector in COPIES {
        let mut head = vec![0; 0x2f0];
        file.seek(SeekFrom::Start(sector * 512)).unwrap();
        file.read_exact(&mut head).unwrap();
        let u64s = u32::from_le_bytes(head[0x7c..0x80].try_into().unwrap()) as usize;
        let mut bytes = vec![0; 0x2f0 + u64s * 8];
        file.seek(SeekFrom::Start(sector * 512)).unwrap();
        file.read_exact(&mut bytes).unwrap();
        let flags = u64::from_le_bytes(bytes[0x90..0x98].try_into().unwrap()) & !0x3c;
        bytes[0x90..0x98].copy_from_slice(&flags.to_le_bytes());
        bytes[..16].fill(0);
        edit(&mut bytes);
        file.seek(SeekFrom::Start(sector * 512)).unwrap();
        file.write_all(&bytes).unwrap();
    }
}

fn clear_flag(b: &mut [u8], mask: u64) {
    let flags = u64::from_le_bytes(b[0x90..0x98].try_into().unwrap()) & !mask;
    b[0x90..0x98].copy_from_slice(&flags.to_le_bytes());
}

/// The fields a start leaves gone, and the words it set zero: a started image
/// whose superblock reads as a new one, with the started image's nodes and
/// journal still on the device.
fn looks_new(b: &mut Vec<u8>) {
    clear_flag(b, 0b11);
    b[0x70..0x78].fill(0);
    b[0xe0..0xe8].fill(0);
    b[0xb8..0xba].fill(0);
    b[0xbc..0xbe].fill(0);
    let u64s = u32::from_le_bytes(b[0x7c..0x80].try_into().unwrap()) as usize;
    let mut out = b[..0x2f0].to_vec();
    let mut at = 0x2f0;
    while at < 0x2f0 + u64s * 8 {
        let size = u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize * 8;
        let kind = u32::from_le_bytes(b[at + 4..at + 8].try_into().unwrap());
        if ![3, 6, 9, 10, 14, 16, 17].contains(&kind) {
            out.extend_from_slice(&b[at..at + size]);
        }
        at += size;
    }
    let left = ((out.len() - 0x2f0) / 8) as u32;
    out[0x7c..0x80].copy_from_slice(&left.to_le_bytes());
    *b = out;
}

/// `journal_v2`'s first range, in a started image of this size, zeroed.
fn zero_the_journal(image: &Path) {
    let sb = fs_bcachefs::superblock::Superblock::read(&FileDevice::open(image).unwrap()).unwrap();
    let body = &sb.field(9).expect("a started image has a journal").body;
    let (start, nr) = (
        u64::from_le_bytes(body[..8].try_into().unwrap()),
        u64::from_le_bytes(body[8..16].try_into().unwrap()),
    );
    let bucket = u64::from(sb.members().unwrap()[0].bucket_size) * 512;
    let mut file = std::fs::OpenOptions::new().write(true).open(image).unwrap();
    file.seek(SeekFrom::Start(start * bucket)).unwrap();
    file.write_all(&vec![0; (nr * bucket) as usize]).unwrap();
}

#[test]
fn both_checkers_on_a_started_image_that_claims_to_be_new() {
    struct Started {
        name: &'static str,
        edit: Edit,
        zero_journal: bool,
        reference_status: i32,
        checker_reports: bool,
    }
    let cases = [
        Started {
            name: "only its checksum removed",
            edit: |_| {},
            zero_journal: false,
            reference_status: 0,
            checker_reports: false,
        },
        Started {
            name: "marked unstarted, with everything a start leaves",
            edit: |b| clear_flag(b, 0b01),
            zero_journal: false,
            reference_status: 0,
            checker_reports: false,
        },
        Started {
            name: "marked unstarted and unclean, its journal intact",
            edit: |b| clear_flag(b, 0b11),
            zero_journal: false,
            reference_status: 0,
            checker_reports: false,
        },
        // The damage a claim must not hide: the reference starts it anew and
        // finds nothing, and its journal field says a journal was lost.
        Started {
            name: "marked unstarted and unclean, its journal lost",
            edit: |b| clear_flag(b, 0b11),
            zero_journal: true,
            reference_status: 0,
            checker_reports: true,
        },
        Started {
            name: "marked started and unclean, its journal lost",
            edit: |b| clear_flag(b, 0b10),
            zero_journal: true,
            reference_status: 1,
            checker_reports: true,
        },
        // Nothing left says it ever started, so both take it for new.
        Started {
            name: "every trace in the superblock removed",
            edit: looks_new,
            zero_journal: false,
            reference_status: 0,
            checker_reports: false,
        },
        Started {
            name: "every trace removed but marked started",
            edit: |b| {
                looks_new(b);
                let flags = u64::from_le_bytes(b[0x90..0x98].try_into().unwrap()) | 1;
                b[0x90..0x98].copy_from_slice(&flags.to_le_bytes());
            },
            zero_journal: false,
            reference_status: 1,
            checker_reports: true,
        },
    ];
    let mut mismatches = Vec::new();
    for case in cases {
        let image = format(DEVICE_BYTES, &["--version=1.39"]);
        if case.zero_journal {
            zero_the_journal(&image);
        }
        edit_started(&image, case.edit);
        let (status, _) = if case.reference_status == 0 {
            reference_expecting_acceptance(&image)
        } else {
            reference(&image)
        };
        let reports = checker_reports(&image);
        if status != case.reference_status || reports != Ok(case.checker_reports) {
            mismatches.push(format!(
                "{}: reference {status}, checker {reports:?}",
                case.name
            ));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}
