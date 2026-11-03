//! The checker against the reference checker, on damaged copies of a
//! fixture, INSIDE the harness guest only (`chore test:vm`); anywhere else
//! it fails naming that. Nothing skips. Wherever the reference finds a
//! problem, this crate's checker must find one too.

#[cfg(feature = "write")]
#[path = "common/alloc_v4.rs"]
mod alloc_v4;
#[cfg(feature = "write")]
#[path = "common/backpointer.rs"]
mod backpointer;
mod common;
#[cfg(feature = "write")]
#[path = "common/freespace.rs"]
mod freespace;
#[path = "common/journal_length.rs"]
mod journal_length;
#[cfg(feature = "write")]
#[path = "common/key_field_bytes.rs"]
mod key_field_bytes;
#[cfg(feature = "write")]
#[path = "common/replicas_accounting.rs"]
mod replicas_accounting;
#[path = "common/superblock_copy.rs"]
mod superblock_copy;
#[path = "common/superblock_field.rs"]
mod superblock_field;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::{ScratchDir, ScratchImage};

use fs_bcachefs::bkey::{self, BkeyFormat};
use fs_bcachefs::btree::{btree_id, NodePtr};
use fs_bcachefs::check::check;
use fs_bcachefs::superblock::Superblock;
use fs_core::FileDevice;

fn fixture(name: &str) -> PathBuf {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(".vm-share/fixtures")
        .join(name);
    assert!(
        p.exists(),
        "{} is missing: build the fixtures with `chore fixtures`",
        p.display()
    );
    p
}

/// The chroot the reference tools live in (scripts/vm-setup.sh).
const REF_ROOT: &str = "/srv/ref-trixie";

/// The reference checker's exit status and printed verdict on a damaged key.
/// A crash, a hang or an internal fault stays private: it fails with no output.
#[cfg(feature = "write")]
fn reference_on_key_field(image: &Path) -> (i32, String) {
    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference checker is required: run `chore test:vm`"
    );
    let native = Command::new("sh")
        .args([
            "-c",
            "ulimit -c 0; exec timeout --kill-after=5 60 bcachefs-ref fsck -n \"$1\"",
            "reference",
        ])
        .arg(image)
        .output()
        .expect("chore test:vm provides the reference checker");
    let text = String::from_utf8_lossy(&native.stdout).into_owned()
        + &String::from_utf8_lossy(&native.stderr);
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
    let status = native.status.code();
    assert!(
        matches!(status, Some(0 | 1 | 9 | 12 | 13)) && !faulted,
        "reference fault or timeout: 1; no case details retained"
    );
    (status.unwrap(), text)
}

#[cfg(feature = "write")]
#[test]
fn both_checkers_agree_on_a_snapshot_reserved_pad_field() {
    let mut mismatches = Vec::new();
    for set in key_field_bytes::SNAPSHOT_SETS {
        for &(edits, refused) in key_field_bytes::SNAPSHOT_PAD {
            let (image, position) = key_field_bytes::plant_snapshot(set, edits);
            let (_, text) = reference_on_key_field(&image);
            assert_eq!(
                text.contains("reserved pad field nonzero"),
                refused,
                "{set} {edits:?}: {text}"
            );
            if refused {
                assert!(
                    text.lines().any(|line| {
                        line.contains("invalid bkey")
                            && line.contains("btree=snapshots ")
                            && line.contains("type snapshot ")
                            && line
                                .split_whitespace()
                                .any(|word| word == position.to_string())
                    }),
                    "the reference must name the planted snapshot: {text}"
                );
            }
            let report = check(&FileDevice::open(&image).unwrap()).unwrap();
            let found = report.problems.iter().any(|problem| {
                problem.kind == "snapshot_pad_nonzero"
                    && problem.detail.contains(&position.to_string())
            });
            if found != refused {
                mismatches.push(format!("{set} {edits:?}: {:?}", report.problems));
            }
        }
    }
    assert!(
        mismatches.is_empty(),
        "snapshot_pad_nonzero disagrees with the reference: {mismatches:#?}"
    );
}

#[cfg(feature = "write")]
#[test]
fn both_checkers_agree_on_an_unset_subvolume_state() {
    let mut mismatches = Vec::new();
    for set in key_field_bytes::SUBVOLUME_SETS {
        for &(edits, unset) in key_field_bytes::SUBVOLUME_STATE {
            let (image, position) = key_field_bytes::plant_subvolume(set, edits);
            let (_, text) = reference_on_key_field(&image);
            assert_eq!(
                text.contains("subvolume state unset"),
                unset,
                "{set} {edits:?}: {text}"
            );
            assert_eq!(
                text.contains("subvol_state_bad"),
                unset,
                "{set} {edits:?}: {text}"
            );
            let report = check(&FileDevice::open(&image).unwrap()).unwrap();
            let found = report.problems.iter().any(|problem| {
                problem.kind == "subvol_state_bad" && problem.detail.contains(&position.to_string())
            });
            if found != unset {
                mismatches.push(format!("{set} {edits:?}: {:?}", report.problems));
            }
        }
    }
    assert!(
        mismatches.is_empty(),
        "subvol_state_bad disagrees with the reference: {mismatches:#?}"
    );
}

#[cfg(feature = "write")]
#[test]
fn both_checkers_agree_on_a_dirent_type_byte() {
    let mut mismatches = Vec::new();
    for (set, original) in key_field_bytes::TYPE_BYTE_DIRENTS {
        for byte in key_field_bytes::type_bytes(set, original) {
            let image = key_field_bytes::plant_type_byte(set, original, byte);
            let (status, text) = reference_on_key_field(&image);
            let reference_clean = status == 0;
            assert_eq!(
                reference_clean,
                key_field_bytes::type_byte_is_clean(original, byte),
                "{set} {original:#x} {byte:#x}: {text}"
            );
            let report = check(&FileDevice::open(&image).unwrap()).unwrap();
            if report.clean() != reference_clean {
                mismatches.push(format!(
                    "{set} {original:#x} {byte:#x}: {:?}",
                    report.problems
                ));
            }
        }
    }
    assert!(
        mismatches.is_empty(),
        "the checkers disagree on dirent type bytes: {mismatches:#?}"
    );
}

#[cfg(feature = "write")]
fn agree_on_words(
    sets: &[&str],
    table: &[key_field_bytes::WordCase],
    plant: fn(&str, &[key_field_bytes::Edit]) -> (ScratchImage, fs_bcachefs::bkey::Bpos),
) -> Vec<String> {
    let mut mismatches = Vec::new();
    for set in sets {
        for &(edits, finding) in table {
            let (image, position) = plant(set, edits);
            let (status, text) = reference_on_key_field(&image);
            let report = check(&FileDevice::open(&image).unwrap()).unwrap();
            match finding {
                None => {
                    assert_eq!(status, 0, "{set} {edits:?}: {text}");
                    if !report.clean() {
                        mismatches.push(format!("{set} {edits:?}: {:?}", report.problems));
                    }
                }
                Some((says, kind)) => {
                    assert!(text.contains(says), "{set} {edits:?}: {text}");
                    let found = report.problems.iter().any(|problem| {
                        problem.kind == kind && problem.detail.contains(&position.to_string())
                    });
                    if !found {
                        mismatches.push(format!("{set} {edits:?}: {:?}", report.problems));
                    }
                }
            }
        }
    }
    mismatches
}

#[cfg(feature = "write")]
#[test]
fn both_checkers_agree_on_a_snapshot_state_word() {
    let mismatches = agree_on_words(
        &key_field_bytes::SNAPSHOT_SETS,
        key_field_bytes::SNAPSHOT_STATE_WORDS,
        key_field_bytes::plant_snapshot,
    );
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

#[cfg(feature = "write")]
#[test]
fn both_checkers_agree_on_an_interior_snapshot_state_word() {
    let mismatches = agree_on_words(
        &key_field_bytes::INTERIOR_SNAPSHOT_SETS,
        key_field_bytes::INTERIOR_SNAPSHOT_WORDS,
        key_field_bytes::plant_interior_snapshot,
    );
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

#[cfg(feature = "write")]
#[test]
fn both_checkers_agree_on_a_subvolume_state_word() {
    let mismatches = agree_on_words(
        &key_field_bytes::SUBVOLUME_SETS,
        key_field_bytes::SUBVOLUME_STATE_WORDS,
        key_field_bytes::plant_subvolume,
    );
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

#[cfg(feature = "write")]
#[test]
fn both_checkers_agree_on_a_word_near_a_subvolume_state_other_than_live() {
    let mismatches = agree_on_words(
        &key_field_bytes::OTHER_STATE_SETS,
        key_field_bytes::SUBVOLUME_OTHER_STATE_WORDS,
        key_field_bytes::plant_subvolume,
    );
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

#[cfg(feature = "write")]
#[test]
fn both_checkers_agree_on_a_subvolume_pad_word() {
    let mismatches = agree_on_words(
        &key_field_bytes::SUBVOLUME_SETS,
        key_field_bytes::SUBVOLUME_PAD_WORDS,
        key_field_bytes::plant_subvolume,
    );
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// A lowered copy's verdict, without what differs between two runs on the
/// same image: where it is kept, the journal sequence and how long a pass took.
#[cfg(feature = "write")]
fn reference_verdict(image: &Path) -> (i32, Vec<String>) {
    let (status, text) = reference_on_key_field(image);
    let lines = text
        .lines()
        .filter(|line| {
            !line.contains(".img") && !line.contains("journal seq") && !line.contains("seconds)")
        })
        .map(str::to_string)
        .collect();
    (status, lines)
}

#[cfg(feature = "write")]
#[test]
fn both_checkers_agree_on_state_words_below_the_required_version() {
    use key_field_bytes::{lower_version, plant_snapshot, plant_subvolume, Edit, OLDER_MINORS};
    const STATE_KINDS: [&str; 6] = [
        "snapshot_state_bad",
        "snapshot_state_bitflip",
        "subvol_state_bad",
        "subvol_state_bitflip",
        "subvol_state_invalid",
        "subvol_pad_nonzero",
    ];
    type Plant = fn(&str, &[Edit]) -> (ScratchImage, fs_bcachefs::bkey::Bpos);
    let families = [
        (
            plant_snapshot as Plant,
            key_field_bytes::OLDER_SNAPSHOT_QUIET,
            key_field_bytes::OLDER_SNAPSHOT_JUDGED,
        ),
        (
            plant_subvolume as Plant,
            key_field_bytes::OLDER_SUBVOLUME_QUIET,
            key_field_bytes::OLDER_SUBVOLUME_JUDGED,
        ),
    ];
    let mut mismatches = Vec::new();
    for minor in OLDER_MINORS {
        let (control, _) = plant_subvolume("default", &[]);
        lower_version(&control, minor);
        let unchanged = reference_verdict(&control);
        for (plant, quiet, judged) in families {
            for &edits in quiet {
                let (image, _) = plant("default", edits);
                lower_version(&image, minor);
                assert_eq!(reference_verdict(&image), unchanged, "1.{minor} {edits:?}");
                let report = check(&FileDevice::open(&image).unwrap()).unwrap();
                if report
                    .problems
                    .iter()
                    .any(|p| STATE_KINDS.contains(&p.kind))
                {
                    mismatches.push(format!("1.{minor} {edits:?}: {:?}", report.problems));
                }
            }
            for &(edits, finding) in judged {
                let (says, kind) = finding.unwrap();
                let (image, position) = plant("default", edits);
                lower_version(&image, minor);
                let (_, text) = reference_on_key_field(&image);
                assert!(text.contains(says), "1.{minor} {edits:?}: {text}");
                let report = check(&FileDevice::open(&image).unwrap()).unwrap();
                let found = report.problems.iter().any(|problem| {
                    problem.kind == kind && problem.detail.contains(&position.to_string())
                });
                if !found {
                    mismatches.push(format!("1.{minor} {edits:?}: {:?}", report.problems));
                }
            }
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// The reference, asked to format an older version and to start it without
/// upgrading, writes the words itself: the older format's value is the same.
/// A zero word on what it made is no finding of its until 1.39 either.
#[cfg(feature = "write")]
#[test]
fn an_image_the_reference_started_at_an_older_version_holds_its_state_words() {
    use fs_bcachefs::btree;
    for minor in [33u16, 35, 36, 38] {
        let image = ScratchDir::new("older-format").into_image("older.img");
        std::fs::File::create(&image)
            .unwrap()
            .set_len(256 << 20)
            .unwrap();
        let version = format!("--version=1.{minor}");
        let reference = |args: &[&str]| {
            let out = Command::new("bcachefs-ref")
                .args(args)
                .arg(image.as_path())
                .output()
                .expect("chore test:vm provides the reference tools");
            assert!(
                out.status.success(),
                "{args:?} failed; no case details retained"
            );
        };
        reference(&["format", "-q", &version]);
        reference(&["fsck", "-y", "-o", "version_upgrade=none"]);
        let device = FileDevice::open(&image).unwrap();
        let superblock = Superblock::read(&device).unwrap();
        assert_eq!(superblock.version.minor(), minor);
        let keys = |id| btree::walk(&device, &superblock, id).unwrap();
        let subvolume = keys(8);
        assert_eq!(
            subvolume[0].value[40..44],
            [0x7e, 0x44, 0xd5, 0x4a],
            "1.{minor}"
        );
        let snapshot = keys(9);
        assert_eq!(
            snapshot[0].value[56..60],
            [0xa2, 0x47, 0x7c, 0x75],
            "1.{minor}"
        );
        assert!(check(&device).unwrap().clean(), "1.{minor}");
    }
}

#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_alloc_v4_values_that_do_not_fit_their_fields() {
    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference checker is required: run `chore test:vm`"
    );
    let mut mismatches = Vec::new();
    for set in ["default", "lz4"] {
        for &case in alloc_v4::CASES {
            let (image, position) = alloc_v4::plant(set, case);
            let native = Command::new("sh")
                .args([
                    "-c",
                    "ulimit -c 0; exec timeout --kill-after=5 60 bcachefs-ref fsck -n \"$1\"",
                    "reference",
                ])
                .arg(&*image)
                .output()
                .expect("chore test:vm provides the reference checker");
            assert!(
                matches!(native.status.code(), Some(0 | 9)),
                "reference fault or timeout: 1; no case details retained"
            );
            let text = String::from_utf8_lossy(&native.stdout).into_owned()
                + &String::from_utf8_lossy(&native.stderr);
            let size_refusal = text.contains("bad val size");
            assert_eq!(size_refusal, case.3.is_some(), "{set} {case:?}: {text}");
            if let Some(reason) = case.3 {
                assert!(text.contains(reason), "{set} {case:?}: {text}");
                assert!(
                    text.lines().any(|line| {
                        line.contains("invalid bkey")
                            && line.contains("btree=alloc ")
                            && line.contains("type alloc_v4 ")
                            && line
                                .split_whitespace()
                                .any(|word| word == position.to_string())
                    }),
                    "the reference must name the planted alloc_v4 key: {text}"
                );
                if case.2 >= 48 {
                    assert!(text.contains("alloc_v4_val_size_bad"), "{text}");
                }
            } else {
                assert!(
                    native.status.success(),
                    "reference control: {set} {case:?}: {text}"
                );
            }
            let report = check(&FileDevice::open(&image).unwrap()).unwrap();
            let found = report.problems.iter().any(|problem| {
                problem.kind == "alloc_v4_val_size_bad"
                    && problem.detail.contains(&position.to_string())
            });
            if found != size_refusal {
                mismatches.push(format!("{set} {case:?}: {:?}", report.problems));
            }
        }
    }
    assert!(
        mismatches.is_empty(),
        "alloc_v4_val_size_bad disagrees with the reference: {mismatches:#?}"
    );
}

#[test]
fn the_reference_reports_an_oversized_stale_journal_entry() {
    let dir = ScratchDir::new("reference-journal-length");
    assert_seen_by_reference(&dir);
    for (oversized, unknown_version) in [(false, false), (false, true), (true, false), (true, true)]
    {
        let img = dir.join(format!("{oversized}-{unknown_version}.img"));
        std::fs::copy(fixture("aged-unclean.img"), &img).unwrap();
        journal_length::plant(&img, oversized, unknown_version);
        let native = Command::new("timeout")
            .args(["--kill-after=5", "60", "bcachefs-ref", "fsck", "-n"])
            .arg(&img)
            .output()
            .expect("chore test:vm provides the reference checker");
        let text = String::from_utf8_lossy(&native.stdout).into_owned()
            + &String::from_utf8_lossy(&native.stderr);
        assert!(
            matches!(native.status.code(), Some(0 | 9))
                && ![
                    ".c:",
                    ".h:",
                    ".rs:",
                    "BUG",
                    "assertion",
                    "Assertion",
                    "backtrace",
                    "Backtrace",
                    "Call Trace"
                ]
                .iter()
                .any(|s| text.contains(s)),
            "reference fault or timeout: no case details retained"
        );
        assert_eq!(
            native.status.code(),
            Some(if oversized { 9 } else { 0 }),
            "{text}"
        );
        if oversized {
            assert!(
                text.contains("journal entry too big (34359738416 bytes), fixing"),
                "{text}"
            );
            assert!(text.contains("jset_past_bucket_end  1"), "{text}");
        } else {
            assert!(!text.contains("invalid journal entry"), "{text}");
        }
        let r = check(&FileDevice::open(&img).unwrap()).unwrap();
        assert_eq!(r.clean(), !oversized, "{:?}", r.problems);
    }
}

/// #261's selected/unselected-copy controls, judged independently before
/// comparison with this crate. Reference 1.39.7 returns exactly 0 or 1.
#[test]
fn the_reference_validates_only_the_selected_copys_optional_fields() {
    let dir = ScratchDir::new("check-sb-fields");
    assert_seen_by_reference(&dir);
    for (name, indexes, newer, expected) in [
        ("control", vec![], false, 0),
        ("primary", vec![0], false, 0),
        ("middle", vec![1], false, 0),
        ("last", vec![2], false, 1),
        ("newer", vec![0], true, 1),
        ("all", vec![0, 1, 2], false, 1),
    ] {
        let img = dir.join(format!("{name}.img"));
        std::fs::copy(fixture("aged-unclean.img"), &img).unwrap();
        let sb = Superblock::read(&FileDevice::open(&img).unwrap()).unwrap();
        assert_eq!(sb.layout.sb_offsets.len(), 3);
        for index in indexes {
            superblock_field::zero_field_length(&img, sb.layout.sb_offsets[index], newer, true);
        }
        let native = Command::new("timeout")
            .args(["--kill-after=5", "60", "bcachefs-ref", "fsck", "-n"])
            .arg(&img)
            .output()
            .expect("chore test:vm provides the reference checker");
        let text = String::from_utf8_lossy(&native.stdout).into_owned()
            + &String::from_utf8_lossy(&native.stderr);
        assert!(
            matches!(native.status.code(), Some(0 | 1))
                && ![
                    ".c:",
                    ".h:",
                    ".rs:",
                    "BUG",
                    "assertion",
                    "Assertion",
                    "backtrace",
                    "Backtrace",
                    "Call Trace"
                ]
                .iter()
                .any(|s| text.contains(s)),
            "reference fault or timeout: no case details retained"
        );
        assert_eq!(native.status.code(), Some(expected), "{name}: {text}");
        if expected == 1 {
            assert!(text.contains("invalid_sb_field_size"), "{name}: {text}");
            assert!(
                text.contains("optional field with size 0 (type 16)"),
                "{name}: {text}"
            );
        } else {
            assert!(!text.contains("error reading superblock"), "{name}: {text}");
        }
        let r = check(&FileDevice::open(&img).unwrap()).unwrap();
        assert_eq!(r.clean(), expected == 0, "{name}: {:?}\n{text}", r.problems);
    }
}

/// The reference tools' chroot sees `dir` at the same absolute path: the
/// test runner (scripts/test-scratch-run.sh, with FLTH_GUEST=1) bind-mounts
/// the run's scratch root there. The same directory, not merely one of the
/// same name, so an image path means the same thing on both sides.
fn assert_seen_by_reference(dir: &ScratchDir) {
    use std::os::unix::fs::MetadataExt;
    let ours = std::fs::metadata(dir.path()).unwrap();
    let inside = dir
        .path()
        .strip_prefix("/")
        .map(|rel| Path::new(REF_ROOT).join(rel));
    let same = matches!(
        inside.as_ref().map(std::fs::metadata),
        Ok(Ok(theirs)) if theirs.dev() == ours.dev() && theirs.ino() == ours.ino()
    );
    assert!(
        same,
        "{} is not visible inside {REF_ROOT}: run these tests through scripts/test-scratch-run.sh in the guest",
        dir.path().display()
    );
}

/// A damaged copy of the fixture in a private scratch directory, removed
/// when the returned image is dropped, which the reference tools' chroot
/// sees at the same path.
fn damaged(name: &str, damage: impl FnOnce(&mut Vec<u8>, u64)) -> ScratchImage {
    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference checker is not here: this runs inside the harness guest (`chore test:vm`)"
    );
    let src = fixture("default.img");
    let sb = Superblock::read(&FileDevice::open(&src).unwrap()).unwrap();
    let r = sb
        .btree_roots()
        .unwrap()
        .into_iter()
        .find(|r| r.btree_id == btree_id::DIRENTS)
        .unwrap();
    let k = bkey::decode(
        &r.key,
        &BkeyFormat {
            key_u64s: 5,
            nr_fields: 6,
            bits: [0; 6],
            field_offset: [0; 6],
        },
    )
    .unwrap();
    let node = NodePtr::from_key(&k).unwrap().ptrs[0].offset * 512;
    let mut b = std::fs::read(&src).unwrap();
    damage(&mut b, node);
    let dir = ScratchDir::new("check-oracle");
    assert_seen_by_reference(&dir);
    let file = format!("{name}.img");
    std::fs::write(dir.join(&file), &b).unwrap();
    dir.into_image(file)
}

/// Whether the reference checker found anything: a failing status, or an
/// error it reported and fixed nothing about (`-n`).
fn reference_finds_a_problem(img: &Path) -> (bool, String) {
    let out = Command::new("bcachefs-ref")
        .args(["fsck", "-n", img.to_str().unwrap()])
        .output()
        .unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    // Any line naming an error, but not the options line, whose
    // `errors=`/`fix_errors=` is no finding.
    let named = text.lines().any(|l| {
        !l.trim_start().starts_with("with options") && (l.contains("error") || l.contains("fixing"))
    });
    (!out.status.success() || named, text)
}

/// #256's exact word-one change and each changed field independently,
/// judged by the reference binary after unchanged controls pass. The
/// reference accepts the target snapshot changed alone (run 39094186901,
/// #348), so both checkers must call that case clean.
#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_a_backpointer_value_that_disagrees_with_its_extent() {
    use fs_bcachefs::btree;
    use fs_bcachefs::write::Writer;

    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference checker is not here: run `chore test:vm`"
    );
    for set in ["default", "block4k"] {
        for case in ["word-one", "length", "gen", "snapshot"] {
            let dir = ScratchDir::new("check-backpointer-oracle");
            assert_seen_by_reference(&dir);
            let img = dir.into_image(format!("{set}-{case}.img"));
            std::fs::copy(fixture(&format!("{set}.img")), &img).unwrap();
            let (bad, text) = reference_finds_a_problem(&img);
            assert!(!bad, "the unchanged control must be clean:\n{text}");
            assert!(check(&FileDevice::open(&img).unwrap()).unwrap().clean());
            let (extent, original) = backpointer::pair(&img);
            let mut changed = original.clone();
            backpointer::change(&mut changed, case);
            {
                let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
                w.insert(backpointer::TREE, vec![changed.clone()]).unwrap();
            }
            let dev = FileDevice::open(&img).unwrap();
            let sb = Superblock::read(&dev).unwrap();
            let stored = btree::walk(&dev, &sb, backpointer::TREE)
                .unwrap()
                .into_iter()
                .find(|k| k.pos == changed.pos)
                .expect("the changed backpointer is stored");
            assert_eq!(stored, changed, "only the selected value bytes change");
            assert!(
                btree::walk(&dev, &sb, btree_id::EXTENTS)
                    .unwrap()
                    .contains(&extent),
                "the extent is unchanged"
            );
            let rejected = case != "snapshot";
            let (bad, text) = reference_finds_a_problem(&img);
            assert_eq!(
                bad && text.contains("backpointer doesn't match extent it points to:"),
                rejected,
                "{set}/{case}: wrong reference finding:\n{text}"
            );
            assert_eq!(bad, rejected, "{set}/{case}: reference verdict:\n{text}");
            let ours = check(&dev).unwrap();
            assert_eq!(
                ours.problems
                    .iter()
                    .any(|p| p.kind == "backpointer_to_missing_ptr"
                        && p.detail.contains(&extent.pos.to_string())),
                rejected,
                "{set}/{case}: {:?}\n{text}",
                ours.problems
            );
            assert!(
                rejected || ours.clean(),
                "{set}/{case}: {:?}",
                ours.problems
            );
        }
    }
}

/// #471: a bucket_gens key whose type changed covers no bucket, so each bucket
/// it held is at generation 0 to the reference.
#[cfg(feature = "write")]
#[test]
fn both_checkers_report_buckets_a_changed_bucket_gens_key_no_longer_covers() {
    use fs_bcachefs::bkey::key_type;
    use fs_bcachefs::btree;
    use fs_bcachefs::write::Writer;

    const BUCKET_GENS: u8 = 14;
    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference checker is not here: run `chore test:vm`"
    );
    let dir = ScratchDir::new("check-oracle");
    assert_seen_by_reference(&dir);
    let img = dir.into_image("bucket-gens-type.img");
    std::fs::copy(fixture("kernel-rootsnap.img"), &img).unwrap();
    let dev = FileDevice::open(&img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let mut k = btree::walk(&dev, &sb, BUCKET_GENS)
        .unwrap()
        .into_iter()
        .next()
        .expect("the fixture has a bucket_gens key");
    k.key_type = key_type::DELETED;
    k.value.clear();
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.insert(BUCKET_GENS, vec![k]).unwrap();
    drop(w);
    let (bad, text) = reference_finds_a_problem(&img);
    assert!(
        bad && text.contains("incorrect gen in bucket_gens"),
        "the reference did not report it:\n{text}"
    );
    let ours = check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        ours.problems
            .iter()
            .any(|p| p.kind == "incorrect_gen_in_bucket_gens"),
        "not reported: {:?}\n{text}",
        ours.problems
    );
}

/// The reference recomputes alloc_v4's stripe-sector count from the bucket.
#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_a_wrong_alloc_stripe_sector_count() {
    use fs_bcachefs::btree;
    use fs_bcachefs::write::Writer;

    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference checker is not here: run `chore test:vm`"
    );
    let dir = ScratchDir::new("check-alloc-stripe-sectors");
    assert_seen_by_reference(&dir);
    let img = dir.into_image("stripe-sectors.img");
    std::fs::copy(fixture("default.img"), &img).unwrap();

    let root = fs_bcachefs::Filesystem::open(FileDevice::open(&img).unwrap())
        .unwrap()
        .lookup("/")
        .unwrap();
    let mut writer = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    writer
        .create_file(root, b"stripe-sectors.bin", &vec![0x5a; 64 << 10], 0o644)
        .unwrap();
    drop(writer);

    let (bad, text) = reference_finds_a_problem(&img);
    assert!(!bad, "the writer's unchanged image must be clean:\n{text}");
    assert!(check(&FileDevice::open(&img).unwrap()).unwrap().clean());

    let dev = FileDevice::open(&img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let alloc = btree::walk(&dev, &sb, btree_id::ALLOC)
        .unwrap()
        .into_iter()
        .find(|k| k.key_type == bkey::key_type::ALLOC_V4 && k.value.get(14) == Some(&4))
        .expect("the writer's file has a user alloc_v4 key");
    let original = u16::from_le_bytes(alloc.value[56..58].try_into().unwrap());
    let mut wrong = alloc.clone();
    wrong.value[56..58].copy_from_slice(&original.wrapping_add(1).to_le_bytes());
    let mut writer = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    writer.insert(btree_id::ALLOC, vec![wrong]).unwrap();
    drop(writer);

    let (bad, text) = reference_finds_a_problem(&img);
    assert!(bad && text.contains("wrong stripe_sectors"), "{text}");
    let ours = check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        ours.problems
            .iter()
            .any(|p| p.kind == "alloc_stripe_sectors_wrong"),
        "stripe_sectors was not checked against the reference finding: {:?}\n{text}",
        ours.problems
    );
}

/// A node's backpointer must describe the node, not just an allocated bucket.
#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_a_btree_backpointer_with_the_wrong_length() {
    use fs_bcachefs::btree;
    use fs_bcachefs::write::Writer;

    let dir = ScratchDir::new("check-node-backpointer");
    assert_seen_by_reference(&dir);
    let img = dir.into_image("node-backpointer.img");
    std::fs::copy(fixture("default.img"), &img).unwrap();
    let (bad, text) = reference_finds_a_problem(&img);
    assert!(!bad, "the unchanged fixture must be clean:\n{text}");
    assert!(check(&FileDevice::open(&img).unwrap()).unwrap().clean());

    let dev = FileDevice::open(&img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let root = sb
        .btree_roots()
        .unwrap()
        .into_iter()
        .find(|r| r.btree_id == btree_id::INODES)
        .unwrap();
    let key = bkey::decode(
        &root.key,
        &BkeyFormat {
            key_u64s: 5,
            nr_fields: 6,
            bits: [0; 6],
            field_offset: [0; 6],
        },
    )
    .unwrap();
    let ptr = NodePtr::from_key(&key).unwrap();
    assert_eq!(ptr.ptrs.len(), 1);
    let mut bp = btree::walk(&dev, &sb, backpointer::TREE)
        .unwrap()
        .into_iter()
        .find(|k| {
            k.key_type == 28
                && k.pos.inode == u64::from(ptr.ptrs[0].dev)
                && k.pos.offset == ptr.ptrs[0].offset << 16
                && k.pos.snapshot == 0
        })
        .expect("the inode root has an allocation backpointer");
    assert_eq!(bp.value.len(), 32);
    assert_eq!(&bp.value[..3], &[btree_id::INODES, root.level + 1, 3]);
    assert_eq!(
        u32::from_le_bytes(bp.value[8..12].try_into().unwrap()),
        sb.btree_node_size()
    );
    bp.value[8..12].copy_from_slice(&(sb.btree_node_size() - 1).to_le_bytes());
    let mut writer = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    writer.insert(backpointer::TREE, vec![bp.clone()]).unwrap();
    drop(writer);

    let dev = FileDevice::open(&img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    assert!(
        sb.btree_roots().unwrap().contains(&root),
        "the target node's root pointer is unchanged"
    );
    assert!(
        btree::walk(&dev, &sb, backpointer::TREE)
            .unwrap()
            .contains(&bp),
        "the exact damaged backpointer is stored"
    );
    let (bad, text) = reference_finds_a_problem(&img);
    assert!(
        bad && text.contains("backpointer doesn't match btree node it points to:"),
        "{text}"
    );
    let ours = check(&dev).unwrap();
    assert!(
        ours.problems
            .iter()
            .any(|p| p.kind == "backpointer_to_missing_ptr"),
        "a btree backpointer with the wrong length was not reported: {:?}\n{text}",
        ours.problems
    );
}

/// #433: a backpointer that names an extent whose pointer is elsewhere. Beside
/// the extent's own backpointer, so the extent's lookup still succeeds and
/// only a walk over the backpointers can see this one.
#[cfg(feature = "write")]
#[test]
fn both_checkers_report_a_backpointer_its_extent_does_not_point_at() {
    use fs_bcachefs::write::Writer;

    let dir = ScratchDir::new("check-backpointer-unclaimed");
    assert_seen_by_reference(&dir);
    let img = dir.into_image("backpointer-unclaimed.img");
    std::fs::copy(fixture("default.img"), &img).unwrap();
    let (_, bp) = backpointer::pair(&img);
    let mut stale = bp.clone();
    stale.pos.offset -= 1;
    let mut writer = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    writer
        .insert(backpointer::TREE, vec![stale.clone()])
        .unwrap();
    drop(writer);

    let (bad, text) = reference_finds_a_problem(&img);
    assert!(
        bad && text.contains("backpointer"),
        "the reference did not report it:\n{text}"
    );
    let ours = check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        ours.problems
            .iter()
            .any(|p| p.kind == "backpointer_to_missing_ptr"
                && p.detail.contains(&stale.pos.to_string())),
        "not reported: {:?}\n{text}",
        ours.problems
    );
}

/// Independently mutate each interpreted field, in both an interior root
/// and its leaf child. The length-only regression above stays unchanged.
#[cfg(feature = "write")]
#[test]
fn both_checkers_agree_on_btree_backpointer_field_mutations() {
    use fs_bcachefs::btree;
    use fs_bcachefs::write::Writer;

    for child in [false, true] {
        for byte in [0, 1, 2, 8, 16, 24] {
            let dir = ScratchDir::new("check-node-backpointer-fields");
            assert_seen_by_reference(&dir);
            let img = dir.into_image(format!("child-{child}-byte-{byte}.img"));
            std::fs::copy(fixture("default.img"), &img).unwrap();
            let (bad, text) = reference_finds_a_problem(&img);
            assert!(!bad, "the unchanged fixture must be clean:\n{text}");
            assert!(check(&FileDevice::open(&img).unwrap()).unwrap().clean());
            let dev = FileDevice::open(&img).unwrap();
            let sb = Superblock::read(&dev).unwrap();
            let root = sb
                .btree_roots()
                .unwrap()
                .into_iter()
                .find(|r| r.btree_id == btree_id::INODES)
                .unwrap();
            assert_eq!(
                root.level, 1,
                "the fixture supplies an interior root and leaf children"
            );
            let mut key = bkey::decode(
                &root.key,
                &BkeyFormat {
                    key_u64s: 5,
                    nr_fields: 6,
                    bits: [0; 6],
                    field_offset: [0; 6],
                },
            )
            .unwrap();
            if child {
                key = btree::read_node(&dev, &sb, &NodePtr::from_key(&key).unwrap())
                    .unwrap()
                    .keys
                    .into_iter()
                    .find(|k| k.key_type == bkey::key_type::BTREE_PTR_V2)
                    .unwrap();
            }
            let ptr = NodePtr::from_key(&key).unwrap();
            let mut bp = btree::walk(&dev, &sb, backpointer::TREE)
                .unwrap()
                .into_iter()
                .find(|k| {
                    k.key_type == 28
                        && k.pos.inode == u64::from(ptr.ptrs[0].dev)
                        && k.pos.offset == ptr.ptrs[0].offset << 16
                        && k.pos.snapshot == 0
                })
                .unwrap();
            bp.value[byte] ^= 1;
            let mut writer = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
            writer.insert(backpointer::TREE, vec![bp.clone()]).unwrap();
            drop(writer);
            let dev = FileDevice::open(&img).unwrap();
            let sb = Superblock::read(&dev).unwrap();
            assert!(sb.btree_roots().unwrap().contains(&root));
            assert!(btree::walk(&dev, &sb, backpointer::TREE)
                .unwrap()
                .contains(&bp));
            let out = Command::new("timeout")
                .args(["--kill-after=5", "60", "bcachefs-ref", "fsck", "-n"])
                .arg(&*img)
                .output()
                .unwrap();
            let text = String::from_utf8_lossy(&out.stdout).into_owned()
                + &String::from_utf8_lossy(&out.stderr);
            assert!(
                matches!(out.status.code(), Some(0 | 1 | 8 | 9))
                    && ![
                        ".c:",
                        ".h:",
                        "BUG",
                        "assertion",
                        "Assertion",
                        "backtrace",
                        "Backtrace",
                        "Call Trace"
                    ]
                    .iter()
                    .any(|s| text.contains(s)),
                "reference fault or timeout: no case details retained"
            );
            let ours = check(&dev).unwrap();
            let rejected = byte == 8;
            assert_eq!(
                !out.status.success(),
                rejected,
                "child={child} byte={byte}: {text}"
            );
            if rejected {
                assert!(
                    text.contains("backpointer doesn't match btree node it points to:"),
                    "{text}"
                );
            }
            assert_eq!(
                ours.problems
                    .iter()
                    .any(|p| p.kind == "backpointer_to_missing_ptr"
                        && p.detail.contains("btree inodes node")),
                rejected,
                "child={child} byte={byte}: {:?}\n{text}",
                ours.problems
            );
            assert!(rejected || ours.clean(), "{:?}", ours.problems);
        }
    }
}

/// Accounting counters are signed deltas summed at mount; the reference
/// refuses a sum below zero. The lowest accounting key is `nr_inodes`.
#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_a_negative_accounting_counter() {
    use fs_bcachefs::btree;
    use fs_bcachefs::write::Writer;

    const ACCOUNTING: u8 = 20;
    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference checker is not here: run `chore test:vm`"
    );
    let dir = ScratchDir::new("check-accounting-underflow");
    assert_seen_by_reference(&dir);
    let img = dir.into_image("accounting-underflow.img");
    std::fs::copy(fixture("default.img"), &img).unwrap();

    let (bad, text) = reference_finds_a_problem(&img);
    assert!(!bad, "the unchanged fixture must be clean:\n{text}");
    assert!(check(&FileDevice::open(&img).unwrap()).unwrap().clean());

    let dev = FileDevice::open(&img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let first = btree::walk(&dev, &sb, ACCOUNTING)
        .unwrap()
        .into_iter()
        .find(|k| k.value.len() >= 8)
        .expect("the fixture has an accounting key with a counter");
    let mut negative = first.clone();
    negative.value[..8].copy_from_slice(&(-1i64).to_le_bytes());
    let mut writer = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    writer.insert(ACCOUNTING, vec![negative]).unwrap();
    drop(writer);

    let (bad, text) = reference_finds_a_problem(&img);
    assert!(bad && text.contains("Accounting underflow"), "{text}");
    let ours = check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        ours.problems
            .iter()
            .any(|p| p.kind == "accounting_underflow"),
        "a negative accounting counter was not reported at {}: {:?}\n{text}",
        first.pos,
        ours.problems
    );
}

/// A btree's per-snapshot counters (kind 5: keys, bytes, data sectors) are
/// what the btree holds in that snapshot; the reference recounts them.
#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_a_per_snapshot_key_count_out_of_step() {
    use fs_bcachefs::btree;
    use fs_bcachefs::write::Writer;

    const ACCOUNTING: u8 = 20;
    let dirents_counter = 0x05ff_ffff_ff00_0000 | (u64::from(btree_id::DIRENTS) << 16);
    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference checker is not here: run `chore test:vm`"
    );
    let dir = ScratchDir::new("check-snapshot-accounting");
    assert_seen_by_reference(&dir);
    let img = dir.into_image("snapshot-accounting.img");
    std::fs::copy(fixture("default.img"), &img).unwrap();

    let (bad, text) = reference_finds_a_problem(&img);
    assert!(!bad, "the unchanged fixture must be clean:\n{text}");
    assert!(check(&FileDevice::open(&img).unwrap()).unwrap().clean());

    let dev = FileDevice::open(&img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let counter = btree::walk(&dev, &sb, ACCOUNTING)
        .unwrap()
        .into_iter()
        .find(|k| k.pos.inode == dirents_counter && k.value.len() >= 8)
        .expect("the fixture counts its dirents in the all-ones snapshot");
    let keys = i64::from_le_bytes(counter.value[..8].try_into().unwrap());
    let mut wrong = counter.clone();
    wrong.value[..8].copy_from_slice(&(keys + 1).to_le_bytes());
    let mut writer = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    writer.insert(ACCOUNTING, vec![wrong]).unwrap();
    drop(writer);

    let (bad, text) = reference_finds_a_problem(&img);
    assert!(
        bad && text.contains("accounting mismatch for snapshot") && text.contains("btree=dirents"),
        "{text}"
    );
    let ours = check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        ours.problems
            .iter()
            .any(|p| p.kind == "accounting_mismatch" && p.detail.contains("dirents")),
        "the dirents counter of {keys} + 1 keys was not reported: {:?}\n{text}",
        ours.problems
    );
}

/// #247's primary-only mutation, with equal-sequence backup controls and
/// a newer-primary control. Ask the actual checker before comparing ours.
#[test]
fn both_checkers_agree_when_superblock_bucket_sizes_differ() {
    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference checker is not here: run `chore test:vm`"
    );
    let sb = Superblock::read(&FileDevice::open(fixture("aged.img")).unwrap()).unwrap();
    let copies = &sb.layout.sb_offsets;
    assert_eq!(copies.len(), 3, "three reference-written copies");
    let dir = ScratchDir::new("check-superblock-selection");
    assert_seen_by_reference(&dir);
    let cases = [
        ("control", Vec::new(), false),
        ("primary-tie", vec![copies[0]], false),
        ("middle-tie", vec![copies[1]], false),
        ("last-tie", vec![copies[2]], false),
        ("primary-newer", vec![copies[0]], true),
        ("all-tie", copies.clone(), false),
    ];
    let mut observations = Vec::new();
    for (name, sectors, newer) in cases {
        let img = dir.join(format!("{name}.img"));
        std::fs::copy(fixture("aged.img"), &img).unwrap();
        for sector in sectors {
            superblock_copy::change_bucket_size(&img, sector, 8448, newer);
        }
        let native = Command::new("timeout")
            .args(["--kill-after=5", "60", "bcachefs-ref", "fsck", "-n"])
            .arg(&img)
            .output()
            .expect("the guest provides timeout and the reference checker");
        let text = String::from_utf8_lossy(&native.stdout).into_owned()
            + &String::from_utf8_lossy(&native.stderr);
        // A fault's output can quote GPL source. Keep no observation or
        // case detail if the reference faults or exceeds its deadline.
        assert!(
            matches!(native.status.code(), Some(0..=123))
                && ![
                    ".c:",
                    ".h:",
                    ".rs:",
                    "BUG",
                    "assertion",
                    "Assertion",
                    "backtrace",
                    "Backtrace",
                    "Call Trace",
                    "panicked at",
                    "Kernel panic",
                ]
                .iter()
                .any(|marker| text.contains(marker)),
            "one reference invocation failed or faulted; no case details retained"
        );
        // As in reference_finds_a_problem, -n can print a finding even
        // when the process exits successfully. Options are not findings.
        let named = text.lines().any(|line| {
            !line.trim_start().starts_with("with options")
                && (line.contains("error") || line.contains("fixing"))
        });
        let ours = check(&FileDevice::open(&img).unwrap()).unwrap();
        observations.push((name, native.status.success() && !named, text, ours));
    }
    // Collect every independent observation before asserting agreement, so
    // the red run records the controls as well as the original mismatch.
    for (name, clean, text, ours) in &observations {
        println!(
            "{name}: reference clean={clean}; ours clean={}; {} findings; first={:?}\n{text}",
            ours.clean(),
            ours.problems.len(),
            ours.problems.first()
        );
    }
    assert!(
        observations[0].1,
        "the unchanged reference control is clean"
    );
    assert!(observations[1].1, "the reference still accepts #247's case");
    for (name, clean, _, ours) in observations {
        assert_eq!(
            ours.clean(),
            clean,
            "{name}: {} findings; first={:?}",
            ours.problems.len(),
            ours.problems.first()
        );
    }
}

#[test]
fn where_the_reference_finds_damage_this_checker_does_too() {
    let cases = [
        damaged("node-header", |b, node| b[(node + 40) as usize] ^= 0xff),
        damaged("node-first-bset", |b, node| {
            // Find the first bset that holds keys and damage its first key.
            let at = node as usize;
            let mut off = 512;
            while u16::from_le_bytes([b[at + off + 16 + 22], b[at + off + 16 + 23]]) == 0 {
                off += 512;
            }
            b[at + off + 16 + 24 + 8] ^= 0xff;
        }),
    ];
    for img in &cases {
        let (theirs, text) = reference_finds_a_problem(img);
        assert!(
            theirs,
            "{}: the reference found nothing, so this case proves nothing:\n{text}",
            img.display()
        );
        let ours = check(&FileDevice::open(img).unwrap()).unwrap();
        assert!(
            !ours.clean(),
            "{}: the reference found damage this checker did not:\n{text}",
            img.display()
        );
    }
    // And on an undamaged copy, both are clean. A scratch copy: the
    // reference tools run in a chroot that sees the run's scratch root and
    // /share, not /repo.
    let clean = damaged("undamaged", |_, _| {});
    let (theirs, text) = reference_finds_a_problem(&clean);
    assert!(
        !theirs,
        "the reference finds a problem in the fixture:\n{text}"
    );
    assert!(check(&FileDevice::open(&clean).unwrap()).unwrap().clean());
}

/// The inode damage `tests/oracle_check_inode_metadata.rs` makes (#221),
/// judged by the reference checker itself: a directory of nonzero size,
/// an inode journal sequence in the future, and sector counts that are
/// not what the extents add up to. The reference finds each, and this
/// checker names each.
#[cfg(feature = "write")]
#[test]
fn where_the_reference_finds_inode_damage_this_checker_names_it() {
    use fs_bcachefs::bkey::key_type;
    use fs_bcachefs::btree;
    use fs_bcachefs::inode::InodeV3Raw;
    use fs_bcachefs::write::Writer;
    use fs_bcachefs::Filesystem;

    /// A copy of fixture `src` in a private scratch directory, with the
    /// inode at `path` changed by `change` through the writer. By path: inode numbers
    /// differ from one build of the fixtures to the next.
    fn rewritten(
        name: &str,
        src: &str,
        path: &str,
        change: impl FnOnce(&mut InodeV3Raw),
    ) -> ScratchImage {
        assert!(
            Path::new("/usr/local/bin/bcachefs-ref").exists(),
            "the reference checker is not here: this runs inside the harness guest (`chore test:vm`)"
        );
        let dir = ScratchDir::new("check-oracle");
        assert_seen_by_reference(&dir);
        let p = dir.into_image(format!("{name}.img"));
        std::fs::copy(fixture(src), &p).unwrap();
        let ino = Filesystem::open(FileDevice::open(&p).unwrap())
            .unwrap()
            .lookup(path)
            .unwrap();
        let dev = FileDevice::open(&p).unwrap();
        let sb = Superblock::read(&dev).unwrap();
        let mut k = btree::walk(&dev, &sb, btree_id::INODES)
            .unwrap()
            .into_iter()
            .find(|k| k.pos.inode == 0 && k.pos.offset == ino && k.key_type == key_type::INODE_V3)
            .unwrap_or_else(|| panic!("{src}: no inode {ino} at {path}"));
        let mut raw = InodeV3Raw::parse(&k.value).unwrap();
        change(&mut raw);
        k.value = raw.encode();
        let mut w = Writer::open(FileDevice::open_rw(&p).unwrap()).unwrap();
        w.insert(btree_id::INODES, vec![k]).unwrap();
        p
    }

    let cases = [
        (
            rewritten("inode-dir-size", "kernel-written.img", "/optdir", |i| {
                i.size = u64::MAX
            }),
            "directory_nonzero_i_size",
        ),
        (
            rewritten("inode-seq-future", "lz4.img", "/hello.txt", |i| {
                i.journal_seq = 16_711_691
            }),
            "inode_journal_seq_in_future",
        ),
        (
            rewritten("inode-sectors-max", "aged.img", "/many/f02467", |i| {
                i.sectors = u64::MAX
            }),
            "inode_i_sectors_wrong",
        ),
        (
            rewritten("inode-sectors-48", "lz4.img", "/many/file-0271.txt", |i| {
                i.sectors = (1 << 48) - 1
            }),
            "inode_i_sectors_wrong",
        ),
    ];
    for (img, kind) in &cases {
        let (theirs, text) = reference_finds_a_problem(img);
        assert!(
            theirs,
            "{}: the reference found nothing, so this case proves nothing:\n{text}",
            img.display()
        );
        let ours = check(&FileDevice::open(img).unwrap()).unwrap();
        assert!(
            ours.problems.iter().any(|p| p.kind == *kind),
            "{}: the reference found damage this checker did not name as {kind}: {:?}\n{text}",
            img.display(),
            ours.problems
        );
    }
}

/// The dirent damage `tests/oracle_check_dirent_hash.rs` makes (#221),
/// judged by the reference checker itself. Differential run 38752704728
/// (job 116482951023, kernel seed 45) grounds the class: the reference
/// reported "hash table key at wrong offset" and counted
/// `hash_table_key_wrong_offset` where this crate called the image clean.
/// A dirent five slots past its hash with empty slots between is that
/// damage to both checkers; one slot past a whiteout left in its own slot
/// is reachable, and neither reports it.
#[cfg(feature = "write")]
#[test]
fn where_the_reference_judges_a_displaced_dirent_this_checker_agrees() {
    use fs_bcachefs::bkey::{key_type, Bkey, Bpos};
    use fs_bcachefs::btree;
    use fs_bcachefs::inode::{Dirent, InodeV3Raw, FIELD_NAMES};
    use fs_bcachefs::write::Writer;

    /// A copy of `default` in a private scratch directory, removed when
    /// dropped, with `hello.txt`'s dirent moved `by` slots on and a key of
    /// type `left` in its own slot.
    fn displaced(name: &str, by: u64, left: u8) -> ScratchImage {
        assert!(
            Path::new("/usr/local/bin/bcachefs-ref").exists(),
            "the reference checker is not here: this runs inside the harness guest (`chore test:vm`)"
        );
        let dir = ScratchDir::new("check-oracle");
        assert_seen_by_reference(&dir);
        let img = dir.into_image(format!("{name}.img"));
        std::fs::copy(fixture("default.img"), &img).unwrap();
        let d = {
            let dev = FileDevice::open(&img).unwrap();
            let sb = Superblock::read(&dev).unwrap();
            btree::walk(&dev, &sb, btree_id::DIRENTS)
                .unwrap()
                .into_iter()
                .find(|k| Dirent::from_key(k).is_ok_and(|d| d.name == b"hello.txt"))
                .expect("the dirent's key")
        };
        let at_slot = Bkey {
            key_type: left,
            value: Vec::new(),
            ..d.clone()
        };
        // The file's inode names the slot its dirent sits in; it follows
        // the move, so the only damage is where the dirent now hashes.
        let target = Dirent::from_key(&d).unwrap().inum;
        let mut ino = {
            let dev = FileDevice::open(&img).unwrap();
            let sb = Superblock::read(&dev).unwrap();
            btree::walk(&dev, &sb, btree_id::INODES)
                .unwrap()
                .into_iter()
                .find(|k| {
                    k.pos.inode == 0 && k.pos.offset == target && k.key_type == key_type::INODE_V3
                })
                .expect("hello.txt's inode")
        };
        let mut raw = InodeV3Raw::parse(&ino.value).unwrap();
        let at = 2 * 4 + FIELD_NAMES.iter().position(|&n| n == "dir_offset").unwrap();
        assert_eq!(
            raw.varints.get(at).copied(),
            Some(d.pos.offset),
            "hello.txt's inode does not name its dirent's slot"
        );
        raw.varints[at] = d.pos.offset + by;
        ino.value = raw.encode();
        let moved = Bkey {
            pos: Bpos {
                offset: d.pos.offset + by,
                ..d.pos
            },
            ..d
        };
        let dirents = vec![at_slot, moved];
        let inodes = vec![ino];
        // The writer's insert leaves each btree's key and byte counters as
        // they were, so they move here by what these keys add and remove.
        let counters = {
            let dev = FileDevice::open(&img).unwrap();
            let sb = Superblock::read(&dev).unwrap();
            let all = btree::walk(&dev, &sb, ACCOUNTING).unwrap();
            [(btree_id::DIRENTS, &dirents), (btree_id::INODES, &inodes)]
                .into_iter()
                .filter_map(|(id, keys)| {
                    let (n, bytes) = counted(&dev, &sb, id, keys);
                    if n == 0 && bytes == 0 {
                        return None;
                    }
                    let at = 0x05ff_ffff_ff00_0000 | (u64::from(id) << 16);
                    let mut k = all
                        .iter()
                        .find(|k| {
                            k.key_type == ACCOUNTING_KEY && k.pos.inode == at && k.pos.offset == 0
                        })
                        .cloned()
                        .expect("the btree's key and byte counter");
                    for (i, delta) in [n, bytes].into_iter().enumerate() {
                        let v = u64::from_le_bytes(k.value[i * 8..i * 8 + 8].try_into().unwrap());
                        let v = v.checked_add_signed(delta).expect("a counter below zero");
                        k.value[i * 8..i * 8 + 8].copy_from_slice(&v.to_le_bytes());
                    }
                    Some(k)
                })
                .collect::<Vec<_>>()
        };
        let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
        w.insert(btree_id::DIRENTS, dirents).unwrap();
        w.insert(btree_id::INODES, inodes).unwrap();
        w.insert(ACCOUNTING, counters).unwrap();
        drop(w);
        img
    }

    /// The accounting btree, and its key type (S8: kind 5, `snapshot
    /// id=4294967295 btree=NAME`, holds keys, bytes, 0).
    const ACCOUNTING: u8 = 20;
    const ACCOUNTING_KEY: u8 = 34;

    /// What inserting `keys` into btree `id` changes in its key and byte
    /// counters: each replaces the key at its position, and one of type
    /// `deleted` removes it. A key's bytes are its 40-byte header and value.
    fn counted(dev: &FileDevice, sb: &Superblock, id: u8, keys: &[Bkey]) -> (i64, i64) {
        let now = btree::walk(dev, sb, id).unwrap();
        let size = |k: &Bkey| match k.key_type {
            key_type::DELETED => (0, 0),
            _ => (1, 40 + k.value.len() as i64),
        };
        keys.iter().fold((0, 0), |(n, bytes), k| {
            let (on, ob) = now.iter().find(|o| o.pos == k.pos).map_or((0, 0), size);
            let (kn, kb) = size(k);
            (n + kn - on, bytes + kb - ob)
        })
    }

    fn wrong_offset(img: &Path) -> bool {
        check(&FileDevice::open(img).unwrap())
            .unwrap()
            .problems
            .iter()
            .any(|p| p.kind == "hash_table_key_wrong_offset")
    }

    let away = displaced("dirent-wrong-offset", 5, key_type::DELETED);
    let (theirs, text) = reference_finds_a_problem(&away);
    assert!(
        theirs && text.contains("hash table key at wrong offset"),
        "{}: the reference did not report the dirent at a wrong offset, so this \
         case proves nothing:\n{text}",
        away.display()
    );
    assert!(
        wrong_offset(&away),
        "{}: the reference found a dirent at a wrong offset this checker did not \
         name as hash_table_key_wrong_offset:\n{text}",
        away.display()
    );

    let past = displaced("dirent-past-whiteout", 1, key_type::HASH_WHITEOUT);
    let (theirs, text) = reference_finds_a_problem(&past);
    assert!(
        !theirs,
        "{}: the reference found a problem in a dirent reachable over a \
         whiteout:\n{text}",
        past.display()
    );
    assert!(
        !wrong_offset(&past),
        "{}: the reference calls a dirent reachable over a whiteout clean, and \
         this checker reported it at a wrong offset",
        past.display()
    );
}

/// The inode cases damage their images through the writer: without it
/// they would vanish, and a case that does not run fails.
#[cfg(not(feature = "write"))]
#[test]
fn the_inode_cases_need_the_write_feature() {
    panic!("the inode cases need --features write, as scripts/guest-suite.sh runs them");
}

/// The snapshot damage `tests/oracle_check_snapshots.rs` makes (#220),
/// judged by the reference checker itself: a snapshot tree naming a
/// subvolume that is not there, skiplist entries that are not ancestors,
/// a snapshot naming a tree that is not there or not its own, and a parent
/// cycle. The reference finds each, and this checker names each.
#[cfg(feature = "write")]
#[test]
fn where_the_reference_finds_snapshot_damage_this_checker_names_it() {
    use fs_bcachefs::btree;
    use fs_bcachefs::write::Writer;

    /// Btrees and key types (docs/clean-room.md, "Snapshots and
    /// subvolumes -- read"), and the kernel image's snapshots: `sv`'s
    /// (...292), its snapshot `snap`'s (...293) and their parent (...294).
    const SNAPSHOTS: u8 = 9;
    const SNAPSHOT_TREES: u8 = 15;
    const KEY_SNAPSHOT: u8 = 22;
    const KEY_SNAPSHOT_TREE: u8 = 31;
    const INTERIOR: u64 = u32::MAX as u64 - 1;
    const SV: u64 = u32::MAX as u64 - 3;
    const SNAP: u32 = u32::MAX - 2;

    /// A copy of fixture `src` in a private scratch directory, removed when
    /// dropped, with the key at `0:offset` of btree `id` changed by `change`
    /// through the writer.
    fn rewritten(
        name: &str,
        src: &str,
        (id, key_type, offset): (u8, u8, u64),
        change: impl FnOnce(&mut Vec<u8>),
    ) -> ScratchImage {
        assert!(
            Path::new("/usr/local/bin/bcachefs-ref").exists(),
            "the reference checker is not here: this runs inside the harness guest (`chore test:vm`)"
        );
        let dir = ScratchDir::new("check-oracle");
        assert_seen_by_reference(&dir);
        let p = dir.into_image(format!("{name}.img"));
        std::fs::copy(fixture(src), &p).unwrap();
        let dev = FileDevice::open(&p).unwrap();
        let sb = Superblock::read(&dev).unwrap();
        let mut k = btree::walk(&dev, &sb, id)
            .unwrap()
            .into_iter()
            .find(|k| k.pos.inode == 0 && k.pos.offset == offset && k.key_type == key_type)
            .unwrap_or_else(|| panic!("{src}: btree {id} has no key at 0:{offset}"));
        change(&mut k.value);
        let mut w = Writer::open(FileDevice::open_rw(&p).unwrap()).unwrap();
        w.insert(id, vec![k]).unwrap();
        p
    }
    let snapshot = |offset| (SNAPSHOTS, KEY_SNAPSHOT, offset);
    let put = |at: usize, x: u64, width: usize| {
        move |v: &mut Vec<u8>| v[at..at + width].copy_from_slice(&x.to_le_bytes()[..width])
    };

    let cases = [
        (
            rewritten(
                "snap-tree-missing-subvol",
                "default.img",
                (SNAPSHOT_TREES, KEY_SNAPSHOT_TREE, 1),
                put(0, u64::MAX, 8),
            ),
            "snapshot_tree_to_missing_subvol",
        ),
        (
            rewritten(
                "snap-skiplist-other-tree",
                "kernel.img",
                snapshot(INTERIOR),
                put(32, u64::MAX, 8),
            ),
            "snapshot_bad_skiplist",
        ),
        (
            rewritten(
                "snap-skiplist-sibling",
                "kernel.img",
                snapshot(SV),
                put(28, SNAP.into(), 4),
            ),
            "snapshot_bad_skiplist",
        ),
        (
            rewritten(
                "snap-tree-missing",
                "kernel-rootsnap.img",
                snapshot(INTERIOR),
                put(16, 1, 8),
            ),
            "snapshot_to_bad_snapshot_tree",
        ),
        (
            rewritten("snap-tree-wrong", "kernel.img", snapshot(SV), put(20, 1, 4)),
            "snapshot_to_bad_snapshot_tree",
        ),
        (
            rewritten(
                "snap-parent-cycle",
                "kernel.img",
                snapshot(INTERIOR),
                put(4, SV, 4),
            ),
            "snapshot_to_bad_snapshot_tree",
        ),
    ];
    for (img, kind) in &cases {
        let (theirs, text) = reference_finds_a_problem(img);
        assert!(
            theirs,
            "{}: the reference found nothing, so this case proves nothing:\n{text}",
            img.display()
        );
        let ours = check(&FileDevice::open(img).unwrap()).unwrap();
        assert!(
            ours.problems.iter().any(|p| p.kind == *kind),
            "{}: the reference found damage this checker did not name as {kind}: {:?}\n{text}",
            img.display(),
            ours.problems
        );
    }
}

/// #419: a subvolume_children entry deleted and none added. The reference
/// reports the subvolume as "not set in subvolume_children btree".
#[cfg(feature = "write")]
#[test]
fn both_checkers_report_a_subvolume_whose_children_entry_is_gone() {
    use fs_bcachefs::bkey::key_type;
    use fs_bcachefs::btree;
    use fs_bcachefs::write::Writer;

    const SUBVOLUME_CHILDREN: u8 = 19;
    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference checker is not here: this runs inside the harness guest (`chore test:vm`)"
    );
    let dir = ScratchDir::new("check-oracle");
    assert_seen_by_reference(&dir);
    let img = dir.into_image("children-entry-gone.img");
    std::fs::copy(fixture("kernel.img"), &img).unwrap();
    let dev = FileDevice::open(&img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let mut k = btree::walk(&dev, &sb, SUBVOLUME_CHILDREN)
        .unwrap()
        .into_iter()
        .find(|k| k.pos.inode == 1 && k.pos.offset == 3)
        .expect("kernel.img has a subvolume_children entry at 1:3");
    k.key_type = key_type::DELETED;
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.insert(SUBVOLUME_CHILDREN, vec![k]).unwrap();
    drop(w);
    let (theirs, text) = reference_finds_a_problem(&img);
    assert!(
        theirs && text.contains("subvolume not set in subvolume_children btree"),
        "the reference did not report the missing entry:\n{text}"
    );
    let ours = check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        ours.problems
            .iter()
            .any(|p| p.kind == "subvol_children_entry_missing"),
        "not reported: {:?}\n{text}",
        ours.problems
    );
}

/// #426: the last backpointer of each fixture with its 32-byte value cut by 8
/// bytes. The reference refuses it as "bad val size (24 < 32)".
#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_a_backpointer_value_too_short() {
    use fs_bcachefs::btree;
    use fs_bcachefs::write::Writer;

    const BACKPOINTERS: u8 = 13;
    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference checker is not here: this runs inside the harness guest (`chore test:vm`)"
    );
    for src in ["kernel.img", "default.img"] {
        let dir = ScratchDir::new("check-oracle");
        assert_seen_by_reference(&dir);
        let img = dir.into_image(format!("short-bp-{src}"));
        std::fs::copy(fixture(src), &img).unwrap();
        let dev = FileDevice::open(&img).unwrap();
        let sb = Superblock::read(&dev).unwrap();
        let mut k = btree::walk(&dev, &sb, BACKPOINTERS)
            .unwrap()
            .pop()
            .expect("the fixture has a backpointer");
        assert_eq!(k.value.len(), 32);
        k.value.truncate(24);
        let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
        w.insert(BACKPOINTERS, vec![k]).unwrap();
        drop(w);
        let (theirs, text) = reference_finds_a_problem(&img);
        assert!(
            theirs && text.contains("24 < 32"),
            "{src}: the reference did not refuse the value's size:\n{text}"
        );
        let ours = check(&FileDevice::open(&img).unwrap()).unwrap();
        assert!(
            ours.problems
                .iter()
                .any(|p| p.kind == "bkey_val_size_too_small"),
            "{src}: not reported: {:?}\n{text}",
            ours.problems
        );
    }
}

/// The cases of #462, #463, #464 and #470: each key changed the way the
/// nightly differential runs changed it, which the reference refuses on read
/// and this checker has to name.
#[cfg(feature = "write")]
#[test]
fn where_the_reference_refuses_a_key_field_this_checker_names_it() {
    use fs_bcachefs::bkey::{key_type, Bkey};
    use fs_bcachefs::btree;
    use fs_bcachefs::write::Writer;

    type Change = fn(&mut Bkey);
    type Case = (&'static str, u8, u8, Option<u64>, Change, &'static str);
    const SUBVOLUMES: u8 = 8;
    const SNAPSHOTS: u8 = 9;
    const NEED_DISCARD: u8 = 12;
    const BUCKET_GENS: u8 = 14;
    const SUBVOLUME_CHILDREN: u8 = 19;
    const ACCOUNTING: u8 = 20;
    const KEY_SUBVOLUME: u8 = 21;
    const KEY_SNAPSHOT: u8 = 22;
    const KEY_SET: u8 = 25;
    const KEY_BUCKET_GENS: u8 = 30;
    const KEY_ACCOUNTING: u8 = 34;

    let snapshot: Change = |k| k.pos.snapshot = 1;
    let size: Change = |k| k.size = 1;
    let far: Change = |k| k.pos.offset = u64::MAX;
    let cases: &[Case] = &[
        (
            "gens-snap",
            BUCKET_GENS,
            KEY_BUCKET_GENS,
            None,
            snapshot,
            "bkey_snapshot_nonzero",
        ),
        (
            "children-snap",
            SUBVOLUME_CHILDREN,
            KEY_SET,
            None,
            snapshot,
            "bkey_snapshot_nonzero",
        ),
        (
            "indirect-snap",
            btree::btree_id::REFLINK,
            key_type::REFLINK_V,
            None,
            snapshot,
            "bkey_snapshot_nonzero",
        ),
        (
            "discard-size",
            NEED_DISCARD,
            KEY_SET,
            None,
            size,
            "bkey_size_nonzero",
        ),
        (
            "alloc-size",
            btree::btree_id::ALLOC,
            key_type::ALLOC_V4,
            None,
            size,
            "bkey_size_nonzero",
        ),
        (
            "subvol-far",
            SUBVOLUMES,
            KEY_SUBVOLUME,
            None,
            far,
            "subvol_pos_bad",
        ),
        (
            "snap-far",
            SNAPSHOTS,
            KEY_SNAPSHOT,
            None,
            far,
            "snapshot_pos_bad",
        ),
        (
            "snap-min",
            SNAPSHOTS,
            KEY_SNAPSHOT,
            None,
            |k| k.pos.offset = 0,
            "snapshot_pos_bad",
        ),
        (
            "indirect-inode",
            btree::btree_id::REFLINK,
            key_type::REFLINK_V,
            None,
            |k| k.pos.inode = u64::MAX,
            "reflink_v_pos_bad",
        ),
        (
            "acct-btree-far",
            ACCOUNTING,
            KEY_ACCOUNTING,
            Some(6),
            far,
            "accounting_key_junk_at_end",
        ),
        (
            "acct-snapshot-far",
            ACCOUNTING,
            KEY_ACCOUNTING,
            Some(5),
            far,
            "accounting_key_junk_at_end",
        ),
        (
            "front-pad",
            btree::btree_id::EXTENTS,
            key_type::REFLINK_P,
            None,
            |k| k.value[8..12].copy_from_slice(&u32::MAX.to_le_bytes()),
            "reflink_p_front_pad_bad",
        ),
    ];
    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference checker is not here: this runs inside the harness guest (`chore test:vm`)"
    );
    for (name, id, kind_of_key, acct, change, kind) in cases {
        let dir = ScratchDir::new("check-oracle");
        assert_seen_by_reference(&dir);
        let img = dir.into_image(format!("{name}.img"));
        // The first fixture that holds such a key.
        let found = ["default.img", "kernel.img", "aged.img"]
            .iter()
            .find_map(|src| {
                let dev = FileDevice::open(fixture(src)).unwrap();
                let sb = Superblock::read(&dev).unwrap();
                btree::walk(&dev, &sb, *id)
                    .unwrap()
                    .into_iter()
                    .find(|k| {
                        k.key_type == *kind_of_key && acct.is_none_or(|a| k.pos.inode >> 56 == a)
                    })
                    .map(|k| (*src, k))
            });
        let (src, mut k) = found.unwrap_or_else(|| panic!("{name}: no fixture holds such a key"));
        std::fs::copy(fixture(src), &img).unwrap();
        change(&mut k);
        let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
        w.insert(*id, vec![k]).unwrap();
        drop(w);
        let (theirs, text) = reference_finds_a_problem(&img);
        assert!(
            theirs,
            "{name}: the reference found nothing, so this case proves nothing:\n{text}"
        );
        let ours = check(&FileDevice::open(&img).unwrap()).unwrap();
        assert!(
            ours.problems.iter().any(|p| p.kind == *kind),
            "{name}: the reference found damage this checker did not name as {kind}: {:?}\n{text}",
            ours.problems
        );
    }
}

/// The retained default seed 51 (run 38749903964): the reference names
/// the nonzero snapshot field even though the snapshot value is unchanged.
#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_a_snapshot_key_with_a_nonzero_snapshot_field() {
    use fs_bcachefs::btree;
    use fs_bcachefs::write::Writer;

    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference checker is not here: run `chore test:vm`"
    );
    let dir = ScratchDir::new("check-oracle");
    assert_seen_by_reference(&dir);
    let img = dir.into_image("snapshot-field-nonzero.img");
    std::fs::copy(fixture("default.img"), &img).unwrap();
    let (control_bad, text) = reference_finds_a_problem(&img);
    assert!(!control_bad, "the unchanged control must be clean:\n{text}");
    assert!(check(&FileDevice::open(&img).unwrap()).unwrap().clean());
    let dev = FileDevice::open(&img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let mut k = btree::walk(&dev, &sb, 9)
        .unwrap()
        .into_iter()
        .find(|k| k.key_type == 22 && k.pos.inode == 0 && k.pos.offset == u64::from(u32::MAX))
        .expect("the root snapshot key exists");
    assert_eq!(k.pos.snapshot, 0);
    k.pos.snapshot = u32::MAX - 1;
    {
        let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
        w.insert(9, vec![k]).unwrap();
    }
    let (damaged, text) = reference_finds_a_problem(&img);
    assert!(
        damaged,
        "the reference must reject the retained mutation:\n{text}"
    );
    assert!(
        text.contains("nonzero snapshot"),
        "wrong native finding:\n{text}"
    );
    let ours = check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        ours.problems
            .iter()
            .any(|p| p.kind == "bkey_snapshot_nonzero"),
        "the snapshot field must be named: {:?}\n{text}",
        ours.problems
    );
}

/// Nightly-seed run 39018152705, seed 1659458 (#253): a snapshot's
/// inode position is zero, independently judged again on fresh fixtures.
#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_a_snapshot_key_with_a_nonzero_inode() {
    use fs_bcachefs::btree;
    use fs_bcachefs::write::Writer;

    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference checker is not here: run `chore test:vm`"
    );
    for inode in [1, u64::MAX] {
        let dir = ScratchDir::new("check-oracle");
        assert_seen_by_reference(&dir);
        let img = dir.into_image(format!("snapshot-inode-{inode}.img"));
        std::fs::copy(fixture("default.img"), &img).unwrap();
        let (control_bad, text) = reference_finds_a_problem(&img);
        assert!(!control_bad, "the unchanged control must be clean:\n{text}");
        assert!(check(&FileDevice::open(&img).unwrap()).unwrap().clean());
        let dev = FileDevice::open(&img).unwrap();
        let sb = Superblock::read(&dev).unwrap();
        let mut k = btree::walk(&dev, &sb, 9)
            .unwrap()
            .into_iter()
            .find(|k| k.key_type == 22 && k.pos.inode == 0 && k.pos.offset == u64::from(u32::MAX))
            .expect("the root snapshot key exists");
        let value = k.value.clone();
        k.pos.inode = inode;
        let pos = k.pos;
        {
            let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
            w.insert(9, vec![k]).unwrap();
        }
        let dev = FileDevice::open(&img).unwrap();
        let sb = Superblock::read(&dev).unwrap();
        let stored = btree::walk(&dev, &sb, 9)
            .unwrap()
            .into_iter()
            .find(|k| k.key_type == 22 && k.pos == pos)
            .expect("the mutated snapshot key is stored");
        assert_eq!(stored.value, value, "the snapshot value must be unchanged");
        let (damaged, text) = reference_finds_a_problem(&img);
        assert!(damaged, "the reference must reject inode {inode}:\n{text}");
        assert!(
            text.contains("snapshot_pos_bad"),
            "wrong native finding:\n{text}"
        );
        let ours = check(&dev).unwrap();
        assert!(
            ours.problems
                .iter()
                .any(|p| p.kind == "snapshot_pos_bad" && p.detail.contains(&pos.to_string())),
            "the snapshot inode must be named: {:?}\n{text}",
            ours.problems
        );
    }
}

/// Nightly-seed run 39018152705, seed 1659461 (#254): snapshot tree 1
/// at inode 1 is refused, independently judged again on a fresh fixture.
#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_a_snapshot_tree_key_with_a_nonzero_inode() {
    use fs_bcachefs::btree;
    use fs_bcachefs::write::Writer;

    const SNAPSHOT_TREES: u8 = 15;
    const KEY_SNAPSHOT_TREE: u8 = 31;
    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference checker is not here: run `chore test:vm`"
    );
    for inode in [1, u64::MAX] {
        let dir = ScratchDir::new("check-oracle");
        assert_seen_by_reference(&dir);
        let img = dir.into_image(format!("snapshot-tree-inode-{inode}.img"));
        std::fs::copy(fixture("default.img"), &img).unwrap();
        let (control_bad, text) = reference_finds_a_problem(&img);
        assert!(!control_bad, "the unchanged control must be clean:\n{text}");
        assert!(check(&FileDevice::open(&img).unwrap()).unwrap().clean());
        let dev = FileDevice::open(&img).unwrap();
        let sb = Superblock::read(&dev).unwrap();
        let mut k = btree::walk(&dev, &sb, SNAPSHOT_TREES)
            .unwrap()
            .into_iter()
            .find(|k| {
                k.key_type == KEY_SNAPSHOT_TREE
                    && k.pos.inode == 0
                    && k.pos.offset == 1
                    && k.pos.snapshot == 0
            })
            .expect("snapshot tree 1 exists");
        k.pos.inode = inode;
        let expected = k.clone();
        let pos = k.pos;
        {
            let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
            w.insert(SNAPSHOT_TREES, vec![k]).unwrap();
        }
        let dev = FileDevice::open(&img).unwrap();
        let sb = Superblock::read(&dev).unwrap();
        let stored = btree::walk(&dev, &sb, SNAPSHOT_TREES)
            .unwrap()
            .into_iter()
            .find(|k| k.key_type == KEY_SNAPSHOT_TREE && k.pos == pos)
            .expect("the mutated snapshot-tree key is stored");
        assert_eq!(stored, expected, "only the inode position must change");
        let (damaged, text) = reference_finds_a_problem(&img);
        assert!(damaged, "the reference must reject inode {inode}:\n{text}");
        assert!(
            text.contains("snapshot_tree_pos_bad"),
            "wrong native finding:\n{text}"
        );
        let ours = check(&dev).unwrap();
        assert!(
            ours.problems
                .iter()
                .any(|p| p.kind == "snapshot_tree_pos_bad" && p.detail.contains(&pos.to_string())),
            "the snapshot-tree inode must be named: {:?}\n{text}",
            ours.problems
        );
    }
}

/// Nightly run 38878947701, kernel seed 1659362 (#219): snapshot tree 1
/// at offset U64_MAX is refused. The checker refuses every offset past
/// U32_MAX, so the first of them is judged by the reference as well.
#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_a_snapshot_tree_key_past_offset_u32_max() {
    use fs_bcachefs::btree;
    use fs_bcachefs::write::Writer;

    const SNAPSHOT_TREES: u8 = 15;
    const KEY_SNAPSHOT_TREE: u8 = 31;
    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference checker is not here: run `chore test:vm`"
    );
    for offset in [u64::from(u32::MAX) + 1, u64::MAX] {
        let dir = ScratchDir::new("check-oracle");
        assert_seen_by_reference(&dir);
        let img = dir.into_image(format!("snapshot-tree-offset-{offset}.img"));
        std::fs::copy(fixture("kernel.img"), &img).unwrap();
        let (control_bad, text) = reference_finds_a_problem(&img);
        assert!(!control_bad, "the unchanged control must be clean:\n{text}");
        assert!(check(&FileDevice::open(&img).unwrap()).unwrap().clean());
        let dev = FileDevice::open(&img).unwrap();
        let sb = Superblock::read(&dev).unwrap();
        let mut k = btree::walk(&dev, &sb, SNAPSHOT_TREES)
            .unwrap()
            .into_iter()
            .find(|k| {
                k.key_type == KEY_SNAPSHOT_TREE
                    && k.pos.inode == 0
                    && k.pos.offset == 1
                    && k.pos.snapshot == 0
            })
            .expect("snapshot tree 1 exists");
        k.pos.offset = offset;
        let expected = k.clone();
        let pos = k.pos;
        {
            let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
            w.insert(SNAPSHOT_TREES, vec![k]).unwrap();
        }
        let dev = FileDevice::open(&img).unwrap();
        let sb = Superblock::read(&dev).unwrap();
        let stored = btree::walk(&dev, &sb, SNAPSHOT_TREES)
            .unwrap()
            .into_iter()
            .find(|k| k.key_type == KEY_SNAPSHOT_TREE && k.pos == pos)
            .expect("the mutated snapshot-tree key is stored");
        assert_eq!(stored, expected, "only the offset must change");
        let (damaged, text) = reference_finds_a_problem(&img);
        assert!(
            damaged,
            "the reference must reject offset {offset}:\n{text}"
        );
        assert!(
            text.contains("snapshot_tree_pos_bad"),
            "wrong native finding:\n{text}"
        );
        let ours = check(&dev).unwrap();
        assert!(
            ours.problems
                .iter()
                .any(|p| p.kind == "snapshot_tree_pos_bad" && p.detail.contains(&pos.to_string())),
            "the snapshot-tree offset must be named: {:?}\n{text}",
            ours.problems
        );
    }
}

/// Nightly run 39018152705, kernel-rootsnap seed 1659464 (#267): the
/// snapshot-tree key moves to offset 2 while its root still names the
/// missing snapshot U32_MAX. The reference rejects that tree; this checker
/// currently accepts it.
#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_a_snapshot_tree_with_a_missing_root_snapshot() {
    use fs_bcachefs::btree;
    use fs_bcachefs::write::Writer;

    const SNAPSHOT_TREES: u8 = 15;
    const KEY_SNAPSHOT_TREE: u8 = 31;
    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference checker is not here: run `chore test:vm`"
    );
    let dir = ScratchDir::new("snapshot-tree-root-snapshot");
    assert_seen_by_reference(&dir);
    let img = dir.into_image("missing-root-snapshot.img");
    std::fs::copy(fixture("kernel-rootsnap.img"), &img).unwrap();

    let dev = FileDevice::open(&img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let mut key = btree::walk(&dev, &sb, SNAPSHOT_TREES)
        .unwrap()
        .into_iter()
        .find(|k| {
            k.key_type == KEY_SNAPSHOT_TREE
                && k.pos.inode == 0
                && k.pos.offset == 1
                && k.pos.snapshot == 0
        })
        .expect("snapshot tree 1 exists");
    assert_eq!(
        u32::from_le_bytes(key.value[4..8].try_into().unwrap()),
        u32::MAX,
        "the fixture tree's root snapshot is the missing snapshot"
    );
    key.pos.offset = 2;
    let pos = key.pos;
    {
        let mut writer = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
        writer.insert(SNAPSHOT_TREES, vec![key]).unwrap();
    }

    let dev = FileDevice::open(&img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let stored = btree::walk(&dev, &sb, SNAPSHOT_TREES)
        .unwrap()
        .into_iter()
        .find(|k| k.key_type == KEY_SNAPSHOT_TREE && k.pos == pos)
        .expect("the moved snapshot-tree key is stored");
    assert_eq!(
        u32::from_le_bytes(stored.value[4..8].try_into().unwrap()),
        u32::MAX,
        "only the tree position changes"
    );

    let (damaged, text) = reference_finds_a_problem(&img);
    assert!(
        damaged && text.contains("snapshot tree points to missing/incorrect snapshot"),
        "the reference must name the missing root snapshot:\n{text}"
    );
    let ours = check(&dev).unwrap();
    assert!(
        ours.problems
            .iter()
            .any(|p| p.kind == "snapshot_tree_to_bad_root_snapshot"),
        "the missing root snapshot must be named: {:?}\n{text}",
        ours.problems
    );
}

/// Nightly-seed run 39018152705, default/block4k seed 1659490 (#255):
/// inode_v3 with a nonzero inode position is refused. Resolve the regular
/// file by path because freshly generated fixtures may renumber inodes.
#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_an_inode_key_with_a_nonzero_inode_position() {
    use fs_bcachefs::bkey::key_type;
    use fs_bcachefs::btree;
    use fs_bcachefs::write::Writer;
    use fs_bcachefs::Filesystem;

    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference checker is not here: run `chore test:vm`"
    );
    for inode in [1, u64::MAX] {
        let dir = ScratchDir::new("check-oracle");
        assert_seen_by_reference(&dir);
        let img = dir.into_image(format!("inode-position-{inode}.img"));
        std::fs::copy(fixture("default.img"), &img).unwrap();
        let (control_bad, text) = reference_finds_a_problem(&img);
        assert!(!control_bad, "the unchanged control must be clean:\n{text}");
        assert!(check(&FileDevice::open(&img).unwrap()).unwrap().clean());
        let ino = Filesystem::open(FileDevice::open(&img).unwrap())
            .unwrap()
            .lookup("/many/file-0200.txt")
            .unwrap();
        let dev = FileDevice::open(&img).unwrap();
        let sb = Superblock::read(&dev).unwrap();
        let mut k = btree::walk(&dev, &sb, btree_id::INODES)
            .unwrap()
            .into_iter()
            .find(|k| {
                k.key_type == key_type::INODE_V3
                    && k.pos.inode == 0
                    && k.pos.offset == ino
                    && k.pos.snapshot == u32::MAX
            })
            .expect("the regular file's inode_v3 key exists");
        let original = k.clone();
        k.pos.inode = inode;
        let pos = k.pos;
        {
            let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
            w.insert(btree_id::INODES, vec![k]).unwrap();
        }
        let dev = FileDevice::open(&img).unwrap();
        let sb = Superblock::read(&dev).unwrap();
        let mut stored = btree::walk(&dev, &sb, btree_id::INODES)
            .unwrap()
            .into_iter()
            .find(|k| k.key_type == key_type::INODE_V3 && k.pos == pos)
            .expect("the mutated inode key is stored");
        stored.pos.inode = 0;
        assert_eq!(stored, original, "only the inode position must change");
        let (damaged, text) = reference_finds_a_problem(&img);
        assert!(damaged, "the reference must reject inode {inode}:\n{text}");
        assert!(
            text.contains("inode_pos_inode_nonzero"),
            "wrong native finding:\n{text}"
        );
        let ours = check(&dev).unwrap();
        assert!(
            ours.problems.iter().any(|p| p.kind == "inode_pos_inode_nonzero"
                && p.detail.contains(&pos.to_string())),
            "the inode position must be named: {:?}\n{text}",
            ours.problems
        );
    }
}

/// Run 39018152705, aged seed 1659486 (#258): a size-only change to
/// user.attr29 is refused by the reference as bkey_size_nonzero.
#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_an_xattr_key_with_a_nonzero_size() {
    use fs_bcachefs::bkey::key_type;
    use fs_bcachefs::btree;
    use fs_bcachefs::write::Writer;

    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference checker is not here: run `chore test:vm`"
    );
    for size in [1, u32::MAX] {
        let dir = ScratchDir::new("check-oracle");
        assert_seen_by_reference(&dir);
        let img = dir.into_image(format!("xattr-size-{size}.img"));
        std::fs::copy(fixture("aged.img"), &img).unwrap();
        let (control_bad, text) = reference_finds_a_problem(&img);
        assert!(!control_bad, "the unchanged control must be clean:\n{text}");
        assert!(check(&FileDevice::open(&img).unwrap()).unwrap().clean());
        let dev = FileDevice::open(&img).unwrap();
        let sb = Superblock::read(&dev).unwrap();
        let mut k = btree::walk(&dev, &sb, btree_id::XATTRS)
            .unwrap()
            .into_iter()
            .find(|k| {
                k.key_type == key_type::XATTR
                    && k.value.get(..2) == Some(&[0, 6])
                    && k.value.get(4..10) == Some(b"attr29")
            })
            .expect("the aged fixture has user.attr29");
        assert_eq!(k.size, 0);
        let original = k.clone();
        k.size = size;
        let pos = k.pos;
        {
            let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
            w.insert(btree_id::XATTRS, vec![k]).unwrap();
        }
        let dev = FileDevice::open(&img).unwrap();
        let sb = Superblock::read(&dev).unwrap();
        let mut stored = btree::walk(&dev, &sb, btree_id::XATTRS)
            .unwrap()
            .into_iter()
            .find(|k| k.key_type == key_type::XATTR && k.pos == pos)
            .expect("the mutated xattr key is stored");
        assert_eq!(stored.size, size);
        stored.size = 0;
        assert_eq!(stored, original, "only the xattr key size must change");
        let (damaged, text) = reference_finds_a_problem(&img);
        assert!(damaged, "the reference must reject size {size}:\n{text}");
        assert!(
            text.contains("bkey_size_nonzero") && text.contains("size != 0"),
            "wrong native finding:\n{text}"
        );
        let ours = check(&dev).unwrap();
        assert!(
            ours.problems
                .iter()
                .any(|p| p.kind == "bkey_size_nonzero" && p.detail.contains(&pos.to_string())),
            "the xattr key size must be named: {:?}\n{text}",
            ours.problems
        );
    }
}

/// Free-space and backpointer damage (#163, tests/common/freespace.rs):
/// on `default`, checked clean by both first, each case planted must be
/// found by the reference with the finding the differential baseline
/// records for it, and named by this checker with its kind.
#[cfg(feature = "write")]
#[test]
fn where_the_reference_finds_free_space_damage_this_checker_names_it() {
    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference checker is not here: run `chore test:vm`"
    );
    for &(case, kind, finding) in freespace::CASES {
        let dir = ScratchDir::new("check-freespace-oracle");
        assert_seen_by_reference(&dir);
        let img = dir.into_image(format!("{case}.img"));
        std::fs::copy(fixture("default.img"), &img).unwrap();
        let (bad, text) = reference_finds_a_problem(&img);
        assert!(!bad, "the unchanged control must be clean:\n{text}");
        assert!(check(&FileDevice::open(&img).unwrap()).unwrap().clean());
        let want = freespace::plant(&img, case);
        let (bad, text) = reference_finds_a_problem(&img);
        assert!(
            bad && text.to_lowercase().contains(finding),
            "{case}: the reference did not report {finding:?}:\n{text}"
        );
        let ours = check(&FileDevice::open(&img).unwrap()).unwrap();
        assert!(
            ours.problems
                .iter()
                .any(|p| p.kind == kind && p.detail.contains(&want)),
            "{case}: no {kind} naming {want:?}: {:?}\n{text}",
            ours.problems
        );
    }
}

/// A bucket_gens key has no extent size: kernel.img seed 1 in differential
/// run 38749903964 stored one with a nonzero size, which the reference
/// rejected (`bkey_size_nonzero`) and this checker called clean.
#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_a_bucket_gens_key_with_a_nonzero_size() {
    use fs_bcachefs::btree;
    use fs_bcachefs::write::Writer;

    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference checker is not here: run `chore test:vm`"
    );
    let dir = ScratchDir::new("check-oracle");
    assert_seen_by_reference(&dir);
    let img = dir.into_image("bucket-gens-size-nonzero.img");
    std::fs::copy(fixture("kernel.img"), &img).unwrap();
    let (control_bad, text) = reference_finds_a_problem(&img);
    assert!(!control_bad, "the unchanged control must be clean:\n{text}");
    assert!(check(&FileDevice::open(&img).unwrap()).unwrap().clean());
    let dev = FileDevice::open(&img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let mut k = btree::walk(&dev, &sb, 14)
        .unwrap()
        .into_iter()
        .find(|k| k.key_type == 30)
        .expect("a bucket_gens key exists");
    assert_eq!(k.size, 0);
    let (pos, value) = (k.pos, k.value.clone());
    k.size = 1;
    {
        let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
        w.insert(14, vec![k]).unwrap();
    }
    let dev = FileDevice::open(&img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let stored = btree::walk(&dev, &sb, 14)
        .unwrap()
        .into_iter()
        .find(|k| k.key_type == 30 && k.pos == pos)
        .expect("the mutated bucket_gens key is stored");
    assert_eq!(stored.size, 1, "the size mutation must be stored");
    assert_eq!(stored.value, value, "the generations must be unchanged");
    let (damaged, text) = reference_finds_a_problem(&img);
    assert!(
        damaged,
        "the reference must reject the retained mutation:\n{text}"
    );
    assert!(
        text.contains("size_nonzero") || text.contains("nonzero size"),
        "wrong native finding:\n{text}"
    );
    let ours = check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        ours.problems.iter().any(|p| p.kind == "bkey_size_nonzero"),
        "the size field must be named: {:?}\n{text}",
        ours.problems
    );
}

/// The snapshot cases damage their images through the writer: without it
/// they would vanish, and a case that does not run fails.
#[cfg(not(feature = "write"))]
#[test]
fn the_snapshot_cases_need_the_write_feature() {
    panic!("the snapshot cases need --features write, as scripts/guest-suite.sh runs them");
}

/// A `set` key in a reconcile btree with no work behind it (#163). Every
/// extent with a reconcile entry has a key at its own position in
/// reconcile_work and one at its backpointer's position in
/// reconcile_work_phys; the reference reports a key in either with no
/// such extent. The keys come from `reconcile.img`, written with reconcile
/// off so its extents are still waiting on background work (#420): on
/// kernel.img the module could finish that work before it unmounted.
#[cfg(feature = "write")]
fn both_checkers_reject_a_stray_reconcile_key(
    btree: u8,
    stray: impl Fn(&mut fs_bcachefs::bkey::Bkey),
    message: &str,
    kind: &str,
) {
    use fs_bcachefs::btree;
    use fs_bcachefs::write::Writer;

    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference checker is not here: run `chore test:vm`"
    );
    let dir = ScratchDir::new(&format!("check-reconcile-{kind}"));
    assert_seen_by_reference(&dir);
    let img = dir.into_image("reconcile.img");
    std::fs::copy(fixture("reconcile.img"), &img).unwrap();

    let (bad, text) = reference_finds_a_problem(&img);
    assert!(!bad, "the unchanged fixture must be clean:\n{text}");
    assert!(check(&FileDevice::open(&img).unwrap()).unwrap().clean());

    let dev = FileDevice::open(&img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let mut key = btree::walk(&dev, &sb, btree)
        .unwrap()
        .into_iter()
        .next()
        .expect("the fixture has extents waiting on background work");
    drop(dev);
    stray(&mut key);
    let mut writer = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    writer.insert(btree, vec![key.clone()]).unwrap();
    drop(writer);

    let (bad, text) = reference_finds_a_problem(&img);
    assert!(bad && text.contains(message), "{text}");
    let ours = check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        ours.problems.iter().any(|p| p.kind == kind),
        "the stray key at {:?} was not reported: {:?}\n{text}",
        key.pos,
        ours.problems
    );
}

/// A reconcile_work key at the next inode, where no extent is.
#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_a_reconcile_work_key_with_no_extent() {
    both_checkers_reject_a_stray_reconcile_key(
        18,
        |k| k.pos.inode += 1,
        "extent should be set in (none)",
        "reconcile_work_incorrectly_set",
    );
}

/// A reconcile_work_phys key one past a backpointer's position.
#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_a_reconcile_work_phys_key_with_no_backpointer() {
    both_checkers_reject_a_stray_reconcile_key(
        24,
        |k| k.pos.offset += 1,
        "backpointer should be set in (none)",
        "reconcile_work_phys_incorrectly_set",
    );
}

/// A snapshot node's subvol flag, bit 1 of its value's first word, is set
/// exactly when the node names a subvolume (#219). Flipping it on `sv`'s
/// leaf (subvolume 2) clears a flag that should be set; flipping it on
/// the interior node (no subvolume) sets one that should be clear. The
/// reference names both "has wrong subvol flag".
#[cfg(feature = "write")]
fn both_checkers_reject_a_snapshot_with_a_wrong_subvol_flag(offset: u64) {
    use fs_bcachefs::btree;
    use fs_bcachefs::write::Writer;

    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference checker is not here: run `chore test:vm`"
    );
    let dir = ScratchDir::new("check-snapshot-subvol-flag");
    assert_seen_by_reference(&dir);
    let img = dir.into_image(format!("subvol-flag-{offset}.img"));
    std::fs::copy(fixture("kernel.img"), &img).unwrap();

    let (bad, text) = reference_finds_a_problem(&img);
    assert!(!bad, "the unchanged fixture must be clean:\n{text}");
    assert!(check(&FileDevice::open(&img).unwrap()).unwrap().clean());

    let dev = FileDevice::open(&img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let mut k = btree::walk(&dev, &sb, 9)
        .unwrap()
        .into_iter()
        .find(|k| k.key_type == 22 && k.pos.inode == 0 && k.pos.offset == offset)
        .unwrap_or_else(|| panic!("kernel.img has no snapshot at 0:{offset}"));
    drop(dev);
    k.value[0] ^= 1 << 1;
    let mut writer = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    writer.insert(9, vec![k]).unwrap();
    drop(writer);

    let (bad, text) = reference_finds_a_problem(&img);
    assert!(
        bad && text.contains(&format!("snapshot node {offset} has wrong subvol flag")),
        "{text}"
    );
    let ours = check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        ours.problems
            .iter()
            .any(|p| p.kind == "snapshot_subvol_flag_wrong"),
        "snapshot {offset}'s flipped subvol flag was not reported: {:?}\n{text}",
        ours.problems
    );
}

/// `sv`'s leaf names subvolume 2, and its flag is cleared.
#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_a_subvolume_snapshot_without_its_subvol_flag() {
    both_checkers_reject_a_snapshot_with_a_wrong_subvol_flag(u64::from(u32::MAX - 3));
}

/// The interior node names no subvolume, and its flag is set.
#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_an_interior_snapshot_with_the_subvol_flag() {
    both_checkers_reject_a_snapshot_with_a_wrong_subvol_flag(u64::from(u32::MAX - 1));
}

/// A subvolume_children entry at `parent:child` names a subvolume `child`
/// whose `fs_parent` is `parent` (#219, #382). kernel.img holds `1:2` and
/// `1:3`: `sv` (2) and its snapshot `snap` (3), both with `fs_parent` 1,
/// `snap` with `creation_parent` 2. Each case moves one entry, a `deleted`
/// key at the old position and the entry at the new one, and the
/// reference reports "incorrect entry in subvolume_children btree P:C".
#[cfg(feature = "write")]
fn both_checkers_reject_a_moved_subvolume_children_entry(from: (u64, u64), to: (u64, u64)) {
    use fs_bcachefs::bkey::{key_type, Bkey};
    use fs_bcachefs::btree;
    use fs_bcachefs::write::Writer;

    const SUBVOLUME_CHILDREN: u8 = 19;
    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference checker is not here: run `chore test:vm`"
    );
    let dir = ScratchDir::new("check-subvol-children");
    assert_seen_by_reference(&dir);
    let img = dir.into_image(format!("subvol-children-{}-{}.img", to.0, to.1));
    std::fs::copy(fixture("kernel.img"), &img).unwrap();

    let (bad, text) = reference_finds_a_problem(&img);
    assert!(!bad, "the unchanged fixture must be clean:\n{text}");
    assert!(check(&FileDevice::open(&img).unwrap()).unwrap().clean());

    let dev = FileDevice::open(&img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let k = btree::walk(&dev, &sb, SUBVOLUME_CHILDREN)
        .unwrap()
        .into_iter()
        .find(|k| k.pos.inode == from.0 && k.pos.offset == from.1)
        .unwrap_or_else(|| panic!("kernel.img has no subvolume_children entry at {from:?}"));
    drop(dev);
    let gone = Bkey {
        key_type: key_type::DELETED,
        value: Vec::new(),
        ..k.clone()
    };
    let mut moved = k;
    moved.pos.inode = to.0;
    moved.pos.offset = to.1;
    let mut writer = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    writer
        .insert(SUBVOLUME_CHILDREN, vec![gone, moved])
        .unwrap();
    drop(writer);

    let (bad, text) = reference_finds_a_problem(&img);
    assert!(
        bad && text.contains(&format!(
            "incorrect entry in subvolume_children btree {}:{}",
            to.0, to.1
        )),
        "{text}"
    );
    let ours = check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        ours.problems
            .iter()
            .any(|p| p.kind == "subvol_children_entry_wrong"),
        "the subvolume_children entry moved to {to:?} was not reported: {:?}\n{text}",
        ours.problems
    );
}

/// `snap`'s entry moved under its `creation_parent`, 2: the entry's
/// parent is the `fs_parent`, 1, not the subvolume it was snapshotted from.
#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_a_subvolume_children_entry_under_the_creation_parent() {
    both_checkers_reject_a_moved_subvolume_children_entry((1, 3), (2, 3));
}

/// `snap`'s entry moved to inode U64_MAX, which no subvolume has, as
/// nightly run 39756419740 did.
#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_a_subvolume_children_entry_under_no_subvolume() {
    both_checkers_reject_a_moved_subvolume_children_entry((1, 3), (u64::MAX, 3));
}

/// `sv`'s entry moved to child 4, which no subvolume is.
#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_a_subvolume_children_entry_naming_no_subvolume() {
    both_checkers_reject_a_moved_subvolume_children_entry((1, 2), (1, 4));
}

/// One accounting key of `default.img`, the first of `kind` (the top byte
/// of its position: 0 nr_inodes, 2 replicas, 3 dev_data_type), changed by
/// `change` and committed, a `deleted` key at the old position when the
/// position moved (#219). With `refused`, the reference must delete it on
/// read with that reason and this checker report `ours`; without, both
/// must call the image clean.
#[cfg(feature = "write")]
fn both_checkers_judge_a_changed_accounting_key(
    name: &str,
    kind: u64,
    change: impl FnOnce(&mut fs_bcachefs::bkey::Bkey),
    refused: Option<(&str, &str)>,
) {
    use fs_bcachefs::bkey::{key_type, Bkey};
    use fs_bcachefs::btree;
    use fs_bcachefs::write::Writer;

    const ACCOUNTING: u8 = 20;
    const KEY_ACCOUNTING: u8 = 34;
    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference checker is not here: run `chore test:vm`"
    );
    let dir = ScratchDir::new("check-accounting-key");
    assert_seen_by_reference(&dir);
    let img = dir.into_image(format!("accounting-{name}.img"));
    std::fs::copy(fixture("default.img"), &img).unwrap();

    let (bad, text) = reference_finds_a_problem(&img);
    assert!(!bad, "the unchanged fixture must be clean:\n{text}");
    assert!(check(&FileDevice::open(&img).unwrap()).unwrap().clean());

    let dev = FileDevice::open(&img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let k = btree::walk(&dev, &sb, ACCOUNTING)
        .unwrap()
        .into_iter()
        .find(|k| k.key_type == KEY_ACCOUNTING && k.pos.inode >> 56 == kind)
        .unwrap_or_else(|| panic!("default.img has no accounting key of kind {kind}"));
    drop(dev);
    let mut changed = k.clone();
    change(&mut changed);
    let mut keys = vec![changed.clone()];
    if changed.pos != k.pos {
        keys.push(Bkey {
            key_type: key_type::DELETED,
            value: Vec::new(),
            ..k
        });
        keys.sort_by_key(|k| k.pos);
    }
    let mut writer = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    writer.insert(ACCOUNTING, keys).unwrap();
    drop(writer);

    let (bad, text) = reference_finds_a_problem(&img);
    let ours = check(&FileDevice::open(&img).unwrap()).unwrap();
    match refused {
        Some((reason, kind)) => {
            assert!(
                bad && text.contains("invalid bkey in btree_node btree=accounting")
                    && text.contains(reason),
                "{text}"
            );
            assert!(
                ours.problems.iter().any(|p| p.kind == kind),
                "the accounting key changed to {} ({name}) was not reported as {kind}: {:?}\n{text}",
                changed.pos,
                ours.problems
            );
        }
        None => {
            assert!(!bad, "{text}");
            assert!(ours.clean(), "{name}: {:?}", ours.problems);
        }
    }
}

/// `nr_inodes` is the kind byte alone: a snapshot after it is junk.
#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_an_nr_inodes_key_with_a_snapshot() {
    both_checkers_judge_a_changed_accounting_key(
        "nr-inodes-snapshot",
        0,
        |k| k.pos.snapshot = 1,
        Some((
            "junk at end of accounting key",
            "accounting_key_junk_at_end",
        )),
    );
}

/// So is an offset.
#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_an_nr_inodes_key_with_an_offset() {
    both_checkers_judge_a_changed_accounting_key(
        "nr-inodes-offset",
        0,
        |k| k.pos.offset = 1,
        Some((
            "junk at end of accounting key",
            "accounting_key_junk_at_end",
        )),
    );
}

/// A `replicas btree: 1/1 [0]` entry is `[kind, data type, nr_devs,
/// nr_required, dev]`: a byte after its one device is junk.
#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_a_replicas_key_with_a_byte_past_its_devices() {
    both_checkers_judge_a_changed_accounting_key(
        "replicas-junk",
        2,
        |k| k.pos.inode |= 1 << 16,
        Some((
            "junk at end of accounting key",
            "accounting_key_junk_at_end",
        )),
    );
}

/// `dev_data_type` is `[kind, dev, data type]`: a byte after it is junk.
#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_a_dev_data_type_key_with_a_byte_past_its_data_type() {
    both_checkers_judge_a_changed_accounting_key(
        "dev-data-type-junk",
        3,
        |k| k.pos.inode |= 1 << 32,
        Some((
            "junk at end of accounting key",
            "accounting_key_junk_at_end",
        )),
    );
}

#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_a_replicas_key_with_no_devices() {
    both_checkers_judge_a_changed_accounting_key(
        "replicas-nr-devs-0",
        2,
        |k| k.pos.inode &= !(0xff << 40),
        Some((
            "accounting key replicas entry with nr_devs=0",
            "accounting_replicas_bad",
        )),
    );
}

/// `replicas btree: 2/1 [0]`: two copies required of one device.
#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_a_replicas_key_requiring_more_than_its_devices() {
    both_checkers_judge_a_changed_accounting_key(
        "replicas-nr-required",
        2,
        |k| k.pos.inode = (k.pos.inode & !(0xff << 32)) | (2 << 32),
        Some((
            "accounting key replicas entry with bad nr_required",
            "accounting_replicas_bad",
        )),
    );
}

/// `replicas btree: 1/2 [0 0]`: the same device twice.
#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_a_replicas_key_with_a_repeated_device() {
    both_checkers_judge_a_changed_accounting_key(
        "replicas-unsorted",
        2,
        |k| k.pos.inode = (k.pos.inode & !(0xff << 40)) | (2 << 40),
        Some((
            "accounting key replicas entry with unsorted devs",
            "accounting_replicas_bad",
        )),
    );
}

#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_an_nr_inodes_key_with_no_counter() {
    both_checkers_judge_a_changed_accounting_key(
        "nr-inodes-empty",
        0,
        |k| k.value.clear(),
        Some(("bad val size (0 < 8)", "accounting_key_counters_bad")),
    );
}

#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_an_nr_inodes_key_with_four_counters() {
    both_checkers_judge_a_changed_accounting_key(
        "nr-inodes-4",
        0,
        |k| k.value.resize(32, 0),
        Some((
            "accounting key with 4 counters, should be 1",
            "accounting_key_counters_bad",
        )),
    );
}

#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_a_dev_data_type_key_with_five_counters() {
    both_checkers_judge_a_changed_accounting_key(
        "dev-data-type-5",
        3,
        |k| k.value.resize(40, 0),
        Some((
            "accounting key with 5 counters, should be 3",
            "accounting_key_counters_bad",
        )),
    );
}

/// The reference accepts `nr_inodes` with three counters and
/// `dev_data_type` with one: fewer than four is not judged a fault.
#[cfg(feature = "write")]
#[test]
fn both_checkers_accept_accounting_keys_with_up_to_three_counters() {
    both_checkers_judge_a_changed_accounting_key("nr-inodes-3", 0, |k| k.value.resize(24, 0), None);
    both_checkers_judge_a_changed_accounting_key(
        "dev-data-type-1",
        3,
        |k| k.value.truncate(8),
        None,
    );
}

/// The last backpointer of `set` whose data type (value byte 2) is
/// `data_type` (3 btree, 4 user), changed by `change` and committed, a
/// `deleted` key at the old position when the position moved (#219). With
/// `refused`, the reference must delete it on read as an invalid
/// backpointers key with that reason and this checker report `ours` for
/// it; without, both must call the image clean.
#[cfg(feature = "write")]
fn both_checkers_judge_a_changed_backpointer_key(
    set: &str,
    data_type: u8,
    change: impl FnOnce(&mut fs_bcachefs::bkey::Bkey),
    refused: Option<(&str, &str)>,
) {
    use fs_bcachefs::bkey::{key_type, Bkey};
    use fs_bcachefs::btree;
    use fs_bcachefs::write::Writer;

    const BACKPOINTERS: u8 = 13;
    const KEY_BACKPOINTER: u8 = 28;
    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference checker is not here: run `chore test:vm`"
    );
    let dir = ScratchDir::new("check-backpointer-key");
    assert_seen_by_reference(&dir);
    let img = dir.into_image(format!("backpointer-{set}-{data_type}.img"));
    std::fs::copy(fixture(&format!("{set}.img")), &img).unwrap();

    let (bad, text) = reference_finds_a_problem(&img);
    assert!(!bad, "the unchanged fixture must be clean:\n{text}");
    assert!(check(&FileDevice::open(&img).unwrap()).unwrap().clean());

    let dev = FileDevice::open(&img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let k = btree::walk(&dev, &sb, BACKPOINTERS)
        .unwrap()
        .into_iter()
        .rev()
        .find(|k| k.key_type == KEY_BACKPOINTER && k.value.get(2) == Some(&data_type))
        .unwrap_or_else(|| panic!("{set}.img has no backpointer of data type {data_type}"));
    drop(dev);
    let mut changed = k.clone();
    change(&mut changed);
    let mut keys = vec![changed.clone()];
    if changed.pos != k.pos {
        keys.push(Bkey {
            key_type: key_type::DELETED,
            value: Vec::new(),
            ..k
        });
        keys.sort_by_key(|k| k.pos);
    }
    let mut writer = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    writer.insert(BACKPOINTERS, keys).unwrap();
    drop(writer);

    let (bad, text) = reference_finds_a_problem(&img);
    let ours = check(&FileDevice::open(&img).unwrap()).unwrap();
    match refused {
        Some((reason, kind)) => {
            assert!(
                bad && text.contains("invalid bkey in btree_node btree=backpointers")
                    && text.contains(reason),
                "{text}"
            );
            assert!(
                ours.problems
                    .iter()
                    .any(|p| p.kind == kind && p.detail.contains(&changed.pos.to_string())),
                "the backpointer changed to {} ({set}) was not reported as {kind}: {:?}\n{text}",
                changed.pos,
                ours.problems
            );
        }
        None => {
            assert!(!bad, "{text}");
            assert!(ours.clean(), "{set}: {:?}", ours.problems);
        }
    }
}

/// Backpointers are not snapshotted: the reference deletes one at
/// snapshot 1 as "nonzero snapshot", for user data and for a btree node.
#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_a_backpointer_at_a_snapshot() {
    for (set, data_type) in [("default", 4), ("kernel", 3)] {
        both_checkers_judge_a_changed_backpointer_key(
            set,
            data_type,
            |k| k.pos.snapshot = 1,
            Some(("nonzero snapshot", "bkey_snapshot_nonzero")),
        );
    }
}

/// A backpointer's level (value byte 1) of 5 is "backpointer level bad:
/// 5 >= 4"; the reference accepted 4 in the same probe.
#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_a_backpointer_level_past_four() {
    for (set, data_type) in [("default", 4), ("kernel", 3)] {
        both_checkers_judge_a_changed_backpointer_key(
            set,
            data_type,
            |k| k.value[1] = 5,
            Some(("backpointer level bad: 5 >= 4", "backpointer_level_bad")),
        );
    }
}

/// A btree node's backpointer at level 4 is accepted by both.
#[cfg(feature = "write")]
#[test]
fn both_checkers_accept_a_btree_node_backpointer_at_level_four() {
    both_checkers_judge_a_changed_backpointer_key("kernel", 3, |k| k.value[1] = 4, None);
}

/// The inode_v3 key of `path` in `set.img`, its third value word changed
/// by `change` and committed in place (#219). With `refused`, the
/// reference must delete it on read as an invalid inodes key with that
/// reason and this checker report `ours` for it; without, both must call
/// the image clean.
#[cfg(feature = "write")]
fn both_checkers_judge_a_changed_inode_key(
    set: &str,
    path: &str,
    change: impl FnOnce(u64) -> u64,
    refused: Option<(&str, &'static str)>,
) {
    both_checkers_judge_an_inode_key(set, path, |k| {
        let word = u64::from_le_bytes(k.value[16..24].try_into().unwrap());
        k.value[16..24].copy_from_slice(&change(word).to_le_bytes());
        refused.map(|(reason, kind)| (reason.to_string(), kind))
    });
}

/// The accounting btree, which `both_checkers_judge_an_inode_key` writes.
#[cfg(feature = "write")]
const ACCOUNTING_BTREE: u8 = 20;

/// The accounting key counting the keys and bytes of the inodes btree in
/// `img` (S8: kind 5, `snapshot id=4294967295 btree=inodes`, holds keys,
/// bytes, 0), its bytes moved by `grew`.
#[cfg(feature = "write")]
fn inodes_byte_counter_moved(img: &Path, grew: i64) -> bkey::Bkey {
    use fs_bcachefs::btree;

    const ACCOUNTING_KEY: u8 = 34;
    let at = 0x05ff_ffff_ff00_0000 | (u64::from(btree_id::INODES) << 16);
    let dev = FileDevice::open(img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let mut k = btree::walk(&dev, &sb, ACCOUNTING_BTREE)
        .unwrap()
        .into_iter()
        .find(|k| k.key_type == ACCOUNTING_KEY && k.pos.inode == at && k.pos.offset == 0)
        .expect("the inodes btree's key and byte counter");
    let bytes = u64::from_le_bytes(k.value[8..16].try_into().unwrap());
    let bytes = bytes
        .checked_add_signed(grew)
        .expect("a counter below zero");
    k.value[8..16].copy_from_slice(&bytes.to_le_bytes());
    k
}

/// The inode_v3 key of `path` in `set.img` changed by `change` and committed
/// in place. `change` says what must follow from the key it was given: a
/// reason the reference must delete it for and the kind this checker must
/// report for it, or nothing, when both must call the image clean. A change
/// to the value's length moves the btree's byte counter with it, so the
/// length is not itself a problem.
#[cfg(feature = "write")]
fn both_checkers_judge_an_inode_key(
    set: &str,
    path: &str,
    change: impl FnOnce(&mut bkey::Bkey) -> Option<(String, &'static str)>,
) {
    use fs_bcachefs::bkey::key_type;
    use fs_bcachefs::btree;
    use fs_bcachefs::write::Writer;
    use fs_bcachefs::Filesystem;

    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference checker is not here: run `chore test:vm`"
    );
    let dir = ScratchDir::new("check-inode-key");
    assert_seen_by_reference(&dir);
    let img = dir.into_image(format!("inode-{set}.img"));
    std::fs::copy(fixture(&format!("{set}.img")), &img).unwrap();

    let (bad, text) = reference_finds_a_problem(&img);
    assert!(!bad, "the unchanged fixture must be clean:\n{text}");
    assert!(check(&FileDevice::open(&img).unwrap()).unwrap().clean());

    let ino = Filesystem::open(FileDevice::open(&img).unwrap())
        .unwrap()
        .lookup(path)
        .unwrap();
    let dev = FileDevice::open(&img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let mut k = btree::walk(&dev, &sb, btree_id::INODES)
        .unwrap()
        .into_iter()
        .find(|k| k.key_type == key_type::INODE_V3 && k.pos.inode == 0 && k.pos.offset == ino)
        .unwrap_or_else(|| panic!("{set}.img has no inode_v3 key for {path}"));
    drop(dev);
    let stored = k.value.len() as i64;
    let refused = change(&mut k);
    let grew = k.value.len() as i64 - stored;
    let counter = (grew != 0).then(|| inodes_byte_counter_moved(&img, grew));
    let pos = k.pos;
    let mut writer = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    writer.insert(btree_id::INODES, vec![k]).unwrap();
    if let Some(counter) = counter {
        writer.insert(ACCOUNTING_BTREE, vec![counter]).unwrap();
    }
    drop(writer);

    let (bad, text) = reference_finds_a_problem(&img);
    let ours = check(&FileDevice::open(&img).unwrap()).unwrap();
    match refused {
        Some((reason, kind)) => {
            assert!(
                bad && text.contains("invalid bkey in btree_node btree=inodes")
                    && text.contains(&reason),
                "{reason}:\n{text}"
            );
            assert!(
                ours.problems
                    .iter()
                    .any(|p| p.kind == kind && p.detail.contains(&pos.to_string())),
                "the inode {pos} of {path} ({set}) was not reported as {kind}: {:?}\n{text}",
                ours.problems
            );
        }
        None => {
            assert!(!bad, "{text}");
            assert!(ours.clean(), "{set} {path}: {:?}", ours.problems);
        }
    }
}

/// The string hash type is bits 20..24 of the word: 3 (siphash) in every
/// fixture, 4 and above "invalid str hash type (N >= 4)".
#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_an_inode_with_an_unknown_str_hash_type() {
    for (set, path, hash) in [
        ("default", "/many/file-0200.txt", 4),
        ("default", "/", 15),
        ("kernel", "/", 8),
    ] {
        both_checkers_judge_a_changed_inode_key(
            set,
            path,
            |w| (w & !(0xf << 20)) | (hash << 20),
            Some((
                &format!("invalid str hash type ({hash} >= 4), deleting"),
                "inode_str_hash_invalid",
            )),
        );
    }
}

/// The reference accepts a regular file whose hash type is 0: only 4 and
/// above are refused on the key itself.
#[cfg(feature = "write")]
#[test]
fn both_checkers_accept_a_file_inode_with_hash_type_zero() {
    both_checkers_judge_a_changed_inode_key(
        "default",
        "/many/file-0200.txt",
        |w| w & !(0xf << 20),
        None,
    );
}

/// Bits 32..36 of the word, 3 in every fixture, are read as fields_start,
/// twice their value: below 6, or past the value's length in u64s, which
/// differs with the fixture (#495), is "invalid fields_start (got N, min 6
/// max M)".
#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_an_inode_with_fields_start_out_of_range() {
    both_checkers_reject_fields_start_out_of_range(times_as_made);
}

/// fields_start equal to the value's length is accepted, on this file and on
/// kernel's root, whatever that length is in the fixture (#495).
#[cfg(feature = "write")]
#[test]
fn both_checkers_accept_an_inode_with_fields_start_at_its_value_length() {
    both_checkers_accept_fields_start_at_the_value_length(times_as_made);
}

/// The inode's times as the fixture holds them.
#[cfg(feature = "write")]
fn times_as_made(_varints: &mut [u64]) {}

/// The first varint of each of the four times, which take the eight varints
/// that come first, in the varint's longest form, 9 bytes.
#[cfg(feature = "write")]
fn longest_times(varints: &mut [u64]) {
    for time in varints[..8].iter_mut().step_by(2) {
        *time = 1 << 60;
    }
}

/// All four times zero: one byte for each of their eight varints.
#[cfg(feature = "write")]
fn shortest_times(varints: &mut [u64]) {
    varints[..8].fill(0);
}

/// The inode_v3 value in `k` encoded again, with its times changed by `times`.
#[cfg(feature = "write")]
fn retime(k: &mut bkey::Bkey, times: fn(&mut [u64])) {
    use fs_bcachefs::inode::InodeV3Raw;

    let mut raw = InodeV3Raw::parse(&k.value).unwrap();
    times(&mut raw.varints);
    k.value = raw.encode();
}

/// Bits 32..36 of the third value word of the inode_v3 value in `k`, which
/// are read as fields_start at twice their value, set to `nibble`.
#[cfg(feature = "write")]
fn set_fields_start(k: &mut bkey::Bkey, nibble: u64) {
    let word = u64::from_le_bytes(k.value[16..24].try_into().unwrap());
    let word = (word & !(0xf << 32)) | (nibble << 32);
    k.value[16..24].copy_from_slice(&word.to_le_bytes());
}

/// Each case's bits for fields_start, given the value's length in u64s once
/// its times have changed: `max / 2 + 1` is the smallest value past that
/// length, and the reference's "max" is that length, read from the key.
#[cfg(feature = "write")]
fn both_checkers_reject_fields_start_out_of_range(times: fn(&mut [u64])) {
    type Nibble = fn(u64) -> u64;
    let cases: [(&str, &str, Nibble); 5] = [
        ("default", "/many/file-0200.txt", |_| 2),
        ("default", "/many/file-0200.txt", |max| max / 2 + 1),
        ("default", "/", |max| max / 2 + 1),
        ("kernel", "/", |_| 0),
        ("kernel", "/", |_| 15),
    ];
    for (set, path, nibble) in cases {
        both_checkers_judge_an_inode_key(set, path, |k| {
            retime(k, times);
            let max = k.value.len() as u64 / 8;
            let nibble = nibble(max);
            set_fields_start(k, nibble);
            Some((
                format!(
                    "invalid fields_start (got {}, min 6 max {max}), deleting",
                    2 * nibble
                ),
                "inode_fields_start_bad",
            ))
        });
    }
}

/// fields_start is twice the bits it is read from, so it only reaches the end
/// of a value whose length in u64s is even. A value of odd length takes the 8
/// zero bytes the reference accepts after it (docs/clean-room.md), so nothing
/// is left past fields_start for the reference to decode as a field.
#[cfg(feature = "write")]
fn both_checkers_accept_fields_start_at_the_value_length(times: fn(&mut [u64])) {
    for (set, path) in [("default", "/many/file-0200.txt"), ("kernel", "/")] {
        both_checkers_judge_an_inode_key(set, path, |k| {
            retime(k, times);
            if (k.value.len() / 8) % 2 == 1 {
                k.value.extend_from_slice(&[0; 8]);
            }
            set_fields_start(k, k.value.len() as u64 / 16);
            None
        });
    }
}

/// An inode's times are varints, so the same inode's value differs in length
/// with what they hold, and the reference's "max" follows it: the value of
/// this file was 14 u64s in one fresh fixture and 15 in another (#495). The
/// times alone trouble neither checker, whichever length they encode to.
#[cfg(feature = "write")]
#[test]
fn both_checkers_accept_an_inode_whose_times_encode_longer_or_shorter() {
    let mut lengths = Vec::new();
    for times in [longest_times, shortest_times] {
        both_checkers_judge_an_inode_key("default", "/many/file-0200.txt", |k| {
            retime(k, times);
            lengths.push(k.value.len());
            None
        });
    }
    assert!(
        lengths[0] > lengths[1],
        "the times changed nothing in the value's length: {lengths:?}"
    );
}

#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_fields_start_out_of_range_on_longer_times() {
    both_checkers_reject_fields_start_out_of_range(longest_times);
}

#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_fields_start_out_of_range_on_shorter_times() {
    both_checkers_reject_fields_start_out_of_range(shortest_times);
}

#[cfg(feature = "write")]
#[test]
fn both_checkers_accept_fields_start_at_the_length_of_longer_times() {
    both_checkers_accept_fields_start_at_the_value_length(longest_times);
}

#[cfg(feature = "write")]
#[test]
fn both_checkers_accept_fields_start_at_the_length_of_shorter_times() {
    both_checkers_accept_fields_start_at_the_value_length(shortest_times);
}

/// One option field of the inode at `path` in default.img set to `value`,
/// the has-options flag (bit 12) set or cleared, committed in place (#379).
/// The inode's stored field count grows to hold the field when it must.
/// With `should_be`, the reference must report "inode has_inode_opts flag
/// wrong, should be N" and this checker inode_has_inode_opts_flag_wrong;
/// without, neither may.
#[cfg(feature = "write")]
fn both_checkers_judge_the_options_flag(
    path: &str,
    field: &str,
    value: u64,
    flagged: bool,
    should_be: Option<u8>,
) {
    use fs_bcachefs::bkey::key_type;
    use fs_bcachefs::btree;
    use fs_bcachefs::inode::{InodeV3Raw, FIELD_NAMES};
    use fs_bcachefs::write::Writer;
    use fs_bcachefs::Filesystem;

    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference checker is not here: run `chore test:vm`"
    );
    let dir = ScratchDir::new("check-options-flag");
    assert_seen_by_reference(&dir);
    let img = dir.into_image(format!("options-{field}.img"));
    std::fs::copy(fixture("default.img"), &img).unwrap();

    let ino = Filesystem::open(FileDevice::open(&img).unwrap())
        .unwrap()
        .lookup(path)
        .unwrap();
    let dev = FileDevice::open(&img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let mut k = btree::walk(&dev, &sb, btree_id::INODES)
        .unwrap()
        .into_iter()
        .find(|k| k.key_type == key_type::INODE_V3 && k.pos.inode == 0 && k.pos.offset == ino)
        .unwrap_or_else(|| panic!("default.img has no inode_v3 key for {path}"));
    drop(dev);
    let mut raw = InodeV3Raw::parse(&k.value).unwrap();
    assert_eq!(raw.flags & (1 << 12), 0, "{path} carries no option");
    // Four times of two varints each come before the named fields.
    let at = FIELD_NAMES.iter().position(|n| *n == field).unwrap();
    let nr_fields = 4 + at + 1;
    if ((raw.flags >> 24) & 0xff) < nr_fields as u64 {
        raw.flags = (raw.flags & !(0xff << 24)) | ((nr_fields as u64) << 24);
        raw.varints.resize(nr_fields + 4, 0);
    }
    raw.varints[8 + at] = value;
    if flagged {
        raw.flags |= 1 << 12;
    }
    k.value = raw.encode();
    let pos = k.pos;
    let mut writer = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    writer.insert(btree_id::INODES, vec![k]).unwrap();
    drop(writer);

    let (_, text) = reference_finds_a_problem(&img);
    let ours = check(&FileDevice::open(&img).unwrap()).unwrap();
    let named = ours.problems.iter().any(|p| {
        p.kind == "inode_has_inode_opts_flag_wrong" && p.detail.contains(&pos.to_string())
    });
    match should_be {
        Some(n) => {
            assert!(
                text.contains(&format!("inode has_inode_opts flag wrong, should be {n}")),
                "{path} {field}={value} flagged={flagged}: the reference did not judge the flag:\n{text}"
            );
            assert!(
                named,
                "{path} {field}={value} flagged={flagged}: the inode {pos} was not reported as inode_has_inode_opts_flag_wrong: {:?}\n{text}",
                ours.problems
            );
        }
        None => {
            assert!(
                !text.contains("has_inode_opts flag wrong"),
                "{path} {field}={value} flagged={flagged}:\n{text}"
            );
            assert!(
                !named,
                "{path} {field}={value} flagged={flagged}: {:?}",
                ours.problems
            );
        }
    }
}

/// Differential run 39756419740 (block4k.img seed 1659867): the reference
/// reported "inode has_inode_opts flag wrong, should be 1" for a file of
/// 21 stored fields whose flag was clear, and this checker called it
/// clean. Measured one field at a time (docs/clean-room.md, #379): the
/// reference counts `project`, `nocow` and `inodes_32bit` as options too,
/// which this checker did not.
#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_an_unflagged_inode_with_a_project_nocow_or_inodes_32bit() {
    for field in ["project", "nocow", "inodes_32bit"] {
        both_checkers_judge_the_options_flag("/hello.txt", field, 1, false, Some(1));
    }
}

/// A flagged inode whose only option is `project` or `nocow` is right.
#[cfg(feature = "write")]
#[test]
fn both_checkers_accept_a_flagged_inode_whose_only_option_is_project_or_nocow() {
    for field in ["project", "nocow"] {
        both_checkers_judge_the_options_flag("/hello.txt", field, 1, true, None);
    }
}

/// `casefold` counts as an option on a directory and not on a file.
#[cfg(feature = "write")]
#[test]
fn both_checkers_count_casefold_as_an_option_only_on_a_directory() {
    both_checkers_judge_the_options_flag("/", "casefold", 1, false, Some(1));
    both_checkers_judge_the_options_flag("/", "casefold", 1, true, None);
    both_checkers_judge_the_options_flag("/hello.txt", "casefold", 1, false, None);
    both_checkers_judge_the_options_flag("/hello.txt", "casefold", 1, true, Some(0));
}

/// Change one snapshot's value on a copy of `image`, then require both
/// checkers to object: the reference by `said`, this crate by `kind`.
#[cfg(feature = "write")]
fn both_checkers_reject_a_changed_snapshot(
    tag: &str,
    image: &str,
    offset: u64,
    change: impl FnOnce(&mut [u8]),
    said: &str,
    kind: &str,
) {
    use fs_bcachefs::btree;
    use fs_bcachefs::write::Writer;

    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference checker is not here: run `chore test:vm`"
    );
    let dir = ScratchDir::new(tag);
    assert_seen_by_reference(&dir);
    let img = dir.into_image(format!("{tag}.img"));
    std::fs::copy(fixture(image), &img).unwrap();
    let (bad, text) = reference_finds_a_problem(&img);
    assert!(!bad, "the unchanged fixture must be clean:\n{text}");
    assert!(check(&FileDevice::open(&img).unwrap()).unwrap().clean());

    let dev = FileDevice::open(&img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let mut k = btree::walk(&dev, &sb, 9)
        .unwrap()
        .into_iter()
        .find(|k| k.key_type == 22 && k.pos.inode == 0 && k.pos.offset == offset)
        .unwrap_or_else(|| panic!("{image} has no snapshot at 0:{offset}"));
    drop(dev);
    change(&mut k.value);
    let mut writer = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    writer.insert(9, vec![k]).unwrap();
    drop(writer);

    let (bad, text) = reference_finds_a_problem(&img);
    assert!(bad && text.contains(said), "{text}");
    let ours = check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        ours.problems.iter().any(|p| p.kind == kind),
        "{kind} was not reported: {:?}\n{text}",
        ours.problems
    );
}

/// A snapshot's depth is how many parents it has (#466).
#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_a_snapshot_whose_depth_disagrees_with_its_parents() {
    both_checkers_reject_a_changed_snapshot(
        "check-snapshot-depth",
        "kernel-rootsnap.img",
        4294967294,
        |v| v[24..28].copy_from_slice(&u32::MAX.to_le_bytes()),
        "incorrect depth field",
        "snapshot_bad_depth",
    );
}

/// A snapshot's subvolume names that snapshot back (#467).
#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_a_snapshot_whose_subvolume_does_not_point_back() {
    both_checkers_reject_a_changed_snapshot(
        "check-snapshot-backref",
        "kernel-rootsnap.img",
        4294967294,
        |v| v[16..20].copy_from_slice(&1u32.to_le_bytes()),
        "doesn't point back",
        "snapshot_subvol_backref_wrong",
    );
}

/// A snapshot's child is a snapshot that exists (#468).
#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_a_snapshot_child_that_does_not_exist() {
    both_checkers_reject_a_changed_snapshot(
        "check-snapshot-child",
        "kernel.img",
        4294967292,
        |v| v[8..12].copy_from_slice(&3858276524u32.to_le_bytes()),
        "does not exist",
        "snapshot_bad_child",
    );
}

/// Each change to a replicas accounting key, on a copy of a fixture both
/// checkers call clean: the reference prints what the table measured, and
/// this checker names the kind, at the key's position.
#[cfg(feature = "write")]
fn both_checkers_judge_replicas_accounting_keys(cases: &[replicas_accounting::Case]) {
    let mut mismatches = Vec::new();
    for set in replicas_accounting::SETS {
        let unchanged = replicas_accounting::copy(set);
        let (status, text) = reference_on_key_field(&unchanged);
        assert_eq!(status, 0, "{set} must be clean to start from:\n{text}");
        assert!(check(&FileDevice::open(&unchanged).unwrap())
            .unwrap()
            .clean());
        for case in cases {
            let (image, position) = replicas_accounting::plant(set, case);
            let (status, text) = reference_on_key_field(&image);
            let ours = check(&FileDevice::open(&image).unwrap()).unwrap();
            let agreed = match case.verdict {
                Some((printed, kind)) => {
                    status != 0
                        && text.contains(printed)
                        && ours.problems.iter().any(|problem| {
                            problem.kind == kind && problem.detail.contains(&position.to_string())
                        })
                }
                None => status == 0 && ours.clean(),
            };
            if !agreed {
                mismatches.push(format!(
                    "{set} {case:?}: reference {status}: {text}\nours: {:?}",
                    ours.problems
                ));
            }
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// A device the filesystem lacks, or an entry requiring no copies (#422).
#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_a_replicas_key_naming_a_missing_device_or_requiring_no_copies() {
    both_checkers_judge_replicas_accounting_keys(replicas_accounting::DEVICES_AND_COPIES);
}

/// A data type the reference has no name for (#423).
#[cfg(feature = "write")]
#[test]
fn both_checkers_reject_a_replicas_key_with_an_unknown_data_type() {
    both_checkers_judge_replicas_accounting_keys(replicas_accounting::UNKNOWN_DATA_TYPES);
}

/// The same entries holding nothing are not judged.
#[cfg(feature = "write")]
#[test]
fn both_checkers_accept_a_replicas_key_holding_nothing() {
    both_checkers_judge_replicas_accounting_keys(replicas_accounting::HOLDING_NOTHING);
}
