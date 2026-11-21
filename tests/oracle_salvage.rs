//! The salvage verbs (src/salvage), against the trees the reference
//! formatter was fed and the reference lister's view of the same images,
//! and on copies this test damages so that the strict reader refuses them.

mod common;

use common::{fixture, manifest, read_text, SETS};
use fs_bcachefs::btree::btree_id;
use fs_bcachefs::inode::ROOT_INO;
use fs_bcachefs::salvage::{Candidate, CandidateState, DamageSource, Salvage, Tree, WalkEnd};
use fs_bcachefs::Filesystem;
use fs_core::FileDevice;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::ops::ControlFlow;

fn scratch(set: &str, test: &str) -> common::ScratchImage {
    let img = common::ScratchDir::new("salvage").into_image(format!("{test}.img"));
    common::copy_sparse(fixture(&format!("{set}.img")), &img);
    img
}

fn patch(img: &std::path::Path, at: u64, bytes: &[u8]) {
    use std::io::{Seek, SeekFrom, Write};
    let mut f = std::fs::OpenOptions::new().write(true).open(img).unwrap();
    f.seek(SeekFrom::Start(at)).unwrap();
    f.write_all(bytes).unwrap();
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Every inode position the reference lister printed for a set.
fn reference_inodes(set: &str) -> BTreeSet<(u64, u32)> {
    read_text(&format!("{set}.inodes.txt"))
        .lines()
        .filter(|l| l.starts_with("u64s "))
        .filter_map(|l| {
            let mut words = l.split_whitespace();
            words.find(|&w| w == "type")?;
            let ty = words.next()?;
            let pos = words.next()?;
            if !ty.starts_with("inode") {
                return None;
            }
            let mut f = pos.split(':').skip(1);
            let ino = f.next()?.parse().ok()?;
            let snap = match f.next()? {
                "U32_MAX" => u32::MAX,
                s => s.parse().ok()?,
            };
            Some((ino, snap))
        })
        .collect()
}

/// Every path the live entries of a rebuilt tree spell, by inode, from the
/// root down.
fn live_paths(tree: &Tree) -> BTreeMap<String, u64> {
    let mut children: BTreeMap<u64, Vec<(String, u64)>> = BTreeMap::new();
    for n in &tree.nodes {
        for p in n.parents.iter().filter(|p| p.live) {
            children
                .entry(p.dir)
                .or_default()
                .push((String::from_utf8_lossy(&p.name).into_owned(), n.ino));
        }
    }
    let mut out = BTreeMap::new();
    let mut stack = vec![(String::new(), ROOT_INO)];
    while let Some((path, ino)) = stack.pop() {
        for (name, child) in children.get(&ino).cloned().unwrap_or_default() {
            let p = format!("{path}/{name}");
            if out.insert(p.clone(), child).is_none() {
                stack.push((p, child));
            }
        }
    }
    out
}

fn contents(s: &Salvage, dev: &FileDevice, c: &Candidate) -> (Vec<u8>, bool) {
    let walk = s.walk_extents(dev, c);
    let plan = s.plan(&walk);
    let mut out = Vec::new();
    let report = s.materialize(dev, &plan, &mut out).unwrap();
    assert_eq!(report.written, c.size);
    (out, report.is_complete() && walk.end == WalkEnd::Complete)
}

#[test]
fn the_live_candidates_are_the_references_inodes() {
    for set in SETS {
        let dev = FileDevice::open(fixture(&format!("{set}.img"))).unwrap();
        let s = Salvage::open(&dev).unwrap();
        let (candidates, report) = s.all_candidates(&dev);
        assert!(report.unreadable.is_empty(), "{set}");
        let live: BTreeSet<(u64, u32)> = candidates
            .iter()
            .filter(|c| c.state == CandidateState::Live)
            .map(|c| (c.ino, c.snapshot))
            .collect();
        assert_eq!(live, reference_inodes(set), "{set}");
        assert!(candidates.iter().all(|c| c.checksum_ok), "{set}");
    }
}

#[test]
fn every_file_materializes_to_the_bytes_the_formatter_was_fed() {
    for set in SETS {
        let dev = FileDevice::open(fixture(&format!("{set}.img"))).unwrap();
        let mut s = Salvage::open(&dev).unwrap();
        s.index(&dev);
        let (candidates, _) = s.all_candidates(&dev);
        let live: Vec<Candidate> = candidates
            .into_iter()
            .filter(|c| c.state == CandidateState::Live)
            .collect();
        let tree = s.rebuild_tree(&dev, &live);
        assert_eq!(tree.conflicts().count(), 0, "{set}");
        let paths = live_paths(&tree);
        let mut checked = 0;
        for e in manifest(set) {
            let ino = *paths
                .get(&e.path)
                .unwrap_or_else(|| panic!("{set}: {} is not in the rebuilt tree", e.path));
            if let Some(want) = e.ino {
                assert_eq!(ino, want, "{set}: {}", e.path);
            }
            let Some(sha) = &e.sha256 else { continue };
            let c = live.iter().find(|c| c.ino == ino).unwrap();
            let (bytes, complete) = contents(&s, &dev, c);
            assert!(complete, "{set}: {}", e.path);
            assert_eq!(&hex(&Sha256::digest(&bytes)), sha, "{set}: {}", e.path);
            checked += 1;
        }
        assert!(checked > 0, "{set}: no file checked");
    }
}

#[test]
fn a_clean_image_reports_no_damage_to_its_superblocks_tree_or_nodes() {
    for set in SETS {
        let dev = FileDevice::open(fixture(&format!("{set}.img"))).unwrap();
        let s = Salvage::open(&dev).unwrap();
        let mut damage = Vec::new();
        s.report_damage(&dev, |d| {
            damage.push(d);
            ControlFlow::Continue(())
        });
        // A journal ring holds torn and stale writes by design; nothing else
        // on a clean image may be damaged.
        let other: Vec<_> = damage
            .iter()
            .filter(|d| d.source != DamageSource::JournalEntry)
            .collect();
        assert!(other.is_empty(), "{set}: {other:?}");
    }
}

#[test]
fn files_whose_entries_are_gone_are_orphans_and_still_materialize() {
    let set = "default";
    let img = scratch(set, "dirents-gone");
    let dev = FileDevice::open(&img).unwrap();
    let s = Salvage::open(&dev).unwrap();
    let before: Vec<Candidate> = s
        .all_candidates(&dev)
        .0
        .into_iter()
        .filter(|c| c.state == CandidateState::Live)
        .collect();
    let paths = live_paths(&s.rebuild_tree(&dev, &before));
    let dirent_nodes: Vec<u64> = s
        .live_nodes()
        .nodes
        .iter()
        .filter(|(_, n)| n.btree_id == btree_id::DIRENTS)
        .map(|(&o, _)| o)
        .collect();
    let node_bytes = s.node_context().node_bytes;
    drop(dev);
    // Every dirents node, live or older, is zeroed: no name survives.
    let dev = FileDevice::open(&img).unwrap();
    let mut all_dirent_nodes = dirent_nodes.clone();
    fs_bcachefs::primitive::scan_nodes(&dev, s.node_context(), 0, u64::MAX, |n| {
        if n.btree_id == btree_id::DIRENTS {
            all_dirent_nodes.push(n.offset);
        }
        ControlFlow::Continue(())
    });
    drop(dev);
    for o in all_dirent_nodes {
        patch(&img, o, &vec![0u8; node_bytes]);
    }

    let dev = FileDevice::open(&img).unwrap();
    let strict = Filesystem::open(FileDevice::open(&img).unwrap())
        .and_then(|fs| fs.readdir(ROOT_INO).map(|d| d.len()));
    assert!(
        !matches!(strict, Ok(n) if n > 0),
        "the strict reader still lists the root: {strict:?}"
    );
    let s = Salvage::open(&dev).unwrap();
    let after: Vec<Candidate> = s
        .all_candidates(&dev)
        .0
        .into_iter()
        .filter(|c| c.state == CandidateState::Live)
        .collect();
    assert_eq!(
        after
            .iter()
            .map(|c| (c.ino, c.snapshot))
            .collect::<Vec<_>>(),
        before
            .iter()
            .map(|c| (c.ino, c.snapshot))
            .collect::<Vec<_>>()
    );
    let tree = s.rebuild_tree(&dev, &after);
    let orphans: BTreeSet<u64> = tree.orphans().map(|n| n.ino).collect();
    for e in manifest(set) {
        let ino = paths[&e.path];
        assert!(orphans.contains(&ino), "{} is not an orphan", e.path);
        let Some(sha) = &e.sha256 else { continue };
        let c = after.iter().find(|c| c.ino == ino).unwrap();
        let (bytes, complete) = contents(&s, &dev, c);
        assert!(complete, "{}", e.path);
        assert_eq!(&hex(&Sha256::digest(&bytes)), sha, "{}", e.path);
    }
    let mut damage = Vec::new();
    s.report_damage(&dev, |d| {
        damage.push(d);
        ControlFlow::Continue(())
    });
    for o in dirent_nodes {
        assert!(
            damage
                .iter()
                .any(|d| d.source == DamageSource::LiveTree && d.offset == o),
            "the zeroed live node at {o} is not reported: {damage:?}"
        );
    }
}

#[test]
fn a_damaged_extent_is_reported_missing_never_zeros_as_data() {
    let set = "default";
    let img = scratch(set, "extent-damage");
    let dev = FileDevice::open(&img).unwrap();
    let s = Salvage::open(&dev).unwrap();
    let live: Vec<Candidate> = s
        .all_candidates(&dev)
        .0
        .into_iter()
        .filter(|c| c.state == CandidateState::Live && c.is_file() && c.size > 4096)
        .collect();
    // A file whose data is checksummed, so damage to it is detectable.
    let (c, piece) = live
        .iter()
        .find_map(|c| {
            let walk = s.walk_extents(&dev, c);
            let p = walk.pieces.into_iter().find(|p| {
                matches!(&p.kind, fs_bcachefs::salvage::PieceKind::Data { csum_type, .. } if *csum_type != 0)
            })?;
            Some((c.clone(), p))
        })
        .expect("a checksummed extent");
    let fs_bcachefs::salvage::PieceKind::Data { device_offset, .. } = piece.kind else {
        unreachable!()
    };
    drop(dev);
    patch(&img, device_offset, b"damaged!");

    let dev = FileDevice::open(&img).unwrap();
    let walk = s.walk_extents(&dev, &c);
    let mut out = Vec::new();
    let report = s.materialize(&dev, &s.plan(&walk), &mut out).unwrap();
    let missing: Vec<_> = report.missing().collect();
    assert!(
        !missing.is_empty(),
        "the damaged extent is not reported missing"
    );
    assert!(missing
        .iter()
        .all(|r| r.output_offset < piece.file_offset + piece.length
            && r.output_offset + r.length > piece.file_offset));
}

#[test]
fn encrypted_and_multi_device_volumes_are_refused_by_name() {
    for (image, words) in [
        (
            "encrypted.img",
            "encrypted (its master key stored unsealed)",
        ),
        ("encrypted-passphrase.img", "sealed by a passphrase"),
        ("multi-0.img", "spans 2 devices"),
    ] {
        let dev = FileDevice::open(fixture(image)).unwrap();
        let err = Salvage::open(&dev).unwrap_err().to_string();
        assert!(err.contains(words), "{image}: {err}");
    }
}

#[test]
fn older_versions_hold_superseded_candidates() {
    let dev = FileDevice::open(fixture("aged.img")).unwrap();
    let s = Salvage::open(&dev).unwrap();
    let (candidates, _) = s.all_candidates(&dev);
    let superseded = candidates
        .iter()
        .filter(|c| c.state != CandidateState::Live)
        .count();
    assert!(superseded > 0, "the aged image keeps older inode versions");
}

/// Every file the reference kernel module listed after writing `kernel.img`
/// (snapshots, subvolumes and a reflinked clone among them) is a candidate
/// whose contents materialize to the size and bytes the mount read.
#[test]
fn every_file_the_kernel_listed_materializes_from_a_candidate() {
    let dev = FileDevice::open(fixture("kernel.img")).unwrap();
    let mut s = Salvage::open(&dev).unwrap();
    s.index(&dev);
    let (candidates, report) = s.all_candidates(&dev);
    assert!(report.unreadable.is_empty());
    let mut files = 0;
    for e in manifest("kernel").into_iter().filter(|e| e.kind == "file") {
        let ino = e.ino.expect("the kernel's manifest records inode numbers");
        let want = e.sha256.clone().unwrap();
        let found = candidates
            .iter()
            .filter(|c| c.ino == ino && c.state == CandidateState::Live)
            .any(|c| {
                let (bytes, complete) = contents(&s, &dev, c);
                complete
                    && Some(bytes.len() as u64) == e.size
                    && hex(&Sha256::digest(&bytes)) == want
            });
        assert!(
            found,
            "kernel {}: no candidate materializes to its bytes",
            e.path
        );
        files += 1;
    }
    assert!(files >= 10, "only {files} files compared");
}

/// An image cut short loses what was past the cut, and nothing before it:
/// the copy past the end is reported, and every candidate in what is left
/// is still found.
#[test]
fn a_truncated_image_keeps_everything_before_the_cut() {
    let set = "default";
    let img = scratch(set, "truncated");
    let dev = FileDevice::open(&img).unwrap();
    let s = Salvage::open(&dev).unwrap();
    let (before, _) = s.all_candidates(&dev);
    let size = std::fs::metadata(&img).unwrap().len();
    drop(dev);
    let cut = size / 2;
    std::fs::OpenOptions::new()
        .write(true)
        .open(&img)
        .unwrap()
        .set_len(cut)
        .unwrap();

    let dev = FileDevice::open(&img).unwrap();
    let s = Salvage::open(&dev).unwrap();
    let (after, report) = s.all_candidates(&dev);
    assert_eq!(report.end, cut);
    let kept: Vec<&Candidate> = before.iter().filter(|c| c.node_offset < cut).collect();
    assert!(!kept.is_empty(), "nothing before the cut to keep");
    assert_eq!(after.iter().collect::<Vec<_>>(), kept);
    let mut damage = Vec::new();
    s.report_damage(&dev, |d| {
        damage.push(d);
        ControlFlow::Continue(())
    });
    assert!(
        damage
            .iter()
            .any(|d| d.source == DamageSource::SuperblockCopy
                && matches!(d.verdict, fs_bcachefs::primitive::Verdict::Unreadable(_))),
        "the copy at the end of the device is not reported: {damage:?}"
    );
}
