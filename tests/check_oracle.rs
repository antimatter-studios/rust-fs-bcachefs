//! The checker against the reference checker, on damaged copies of a
//! fixture, INSIDE the harness guest only (`chore test:vm`); anywhere else
//! it fails naming that. Nothing skips. Wherever the reference finds a
//! problem, this crate's checker must find one too.

#[cfg(feature = "write")]
#[path = "common/backpointer.rs"]
mod backpointer;
mod common;
#[cfg(feature = "write")]
#[path = "common/freespace.rs"]
mod freespace;
#[path = "common/journal_length.rs"]
mod journal_length;
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
        for case in ["word-one", "length", "snapshot"] {
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
