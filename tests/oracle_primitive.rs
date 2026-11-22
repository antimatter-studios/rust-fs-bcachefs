//! The primitive layer's evidence (src/primitive), against what the
//! reference tools recorded about the same images, and on copies this test
//! damages itself: a damaged structure must stay evidence, with the damage
//! named, where the strict reader refuses it.

mod common;

use common::{fixture, read_text, super_report, SETS};
use fs_bcachefs::bkey::{key_type, Bpos};
use fs_bcachefs::btree::{self, btree_id, NodePtr};
use fs_bcachefs::journal;
use fs_bcachefs::primitive::{self, CopyOrigin, NodeContext, Verdict};
use fs_bcachefs::superblock::{Superblock, SB_OFFSET};
use fs_core::FileDevice;
use std::collections::BTreeSet;
use std::ops::ControlFlow;

fn scratch(set: &str, test: &str) -> common::ScratchImage {
    let img = common::ScratchDir::new("primitive").into_image(format!("{test}.img"));
    common::copy_sparse(fixture(&format!("{set}.img")), &img);
    img
}

fn patch(img: &std::path::Path, at: u64, bytes: &[u8]) {
    use std::io::{Seek, SeekFrom, Write};
    let mut f = std::fs::OpenOptions::new().write(true).open(img).unwrap();
    f.seek(SeekFrom::Start(at)).unwrap();
    f.write_all(bytes).unwrap();
}

fn read_byte(img: &std::path::Path, at: u64) -> u8 {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(img).unwrap();
    f.seek(SeekFrom::Start(at)).unwrap();
    let mut b = [0u8; 1];
    f.read_exact(&mut b).unwrap();
    b[0]
}

/// A position as the reference lister prints it, `inode:offset:snapshot`.
fn parse_pos(s: &str) -> Bpos {
    let field = |f: &str| -> u64 {
        match f {
            "U32_MAX" => u64::from(u32::MAX),
            "U64_MAX" => u64::MAX,
            _ => f.parse().unwrap_or_else(|_| panic!("position field {f:?}")),
        }
    };
    let mut parts = s.split(':');
    Bpos {
        inode: field(parts.next().unwrap()),
        offset: field(parts.next().unwrap()),
        snapshot: field(parts.next().unwrap()) as u32,
    }
}

/// Every inode position the reference lister printed for a set.
fn reference_inode_positions(set: &str) -> BTreeSet<Bpos> {
    read_text(&format!("{set}.inodes.txt"))
        .lines()
        .filter(|l| l.starts_with("u64s "))
        .filter_map(|l| {
            let mut words = l.split_whitespace();
            words.find(|&w| w == "type")?;
            let ty = words.next()?;
            let pos = words.next()?;
            ty.starts_with("inode").then(|| parse_pos(pos))
        })
        .collect()
}

#[test]
fn every_set_surveys_its_copies_as_the_reader_and_the_reference_do() {
    for set in SETS {
        let dev = FileDevice::open(fixture(&format!("{set}.img"))).unwrap();
        let survey = primitive::survey_superblocks(&dev);
        let sb = Superblock::read(&dev).unwrap();
        assert_eq!(survey.copies.len(), sb.layout.sb_offsets.len(), "{set}");
        for copy in &survey.copies {
            assert_eq!(
                copy.verdict,
                Verdict::Valid,
                "{set}: copy at {}",
                copy.sector
            );
            assert_eq!(copy.recorded_sector, Some(copy.sector), "{set}");
        }
        assert_eq!(survey.copies[0].origin, CopyOrigin::Primary);
        assert!(
            survey.disagreements.is_empty(),
            "{set}: {:?}",
            survey.disagreements
        );
        let chosen = survey.superblock.as_ref().expect("a chosen copy");
        assert_eq!(
            chosen, &sb,
            "{set}: the survey chose another copy than the reader"
        );
        let reference_seq: u64 = super_report(set)["Sequence number"].parse().unwrap();
        assert_eq!(
            chosen.seq, reference_seq,
            "{set}: seq against the reference"
        );
        assert_eq!(survey.standalone_layout.verdict, Verdict::Valid, "{set}");
    }
}

#[test]
fn a_zeroed_primary_and_a_torn_copy_stay_evidence() {
    let img = scratch("default", "superblock-damage");
    let dev = FileDevice::open(&img).unwrap();
    let before = primitive::survey_superblocks(&dev);
    drop(dev);
    let second = before.copies[1].sector;
    patch(&img, SB_OFFSET, &vec![0u8; 8192]);
    // A byte inside the second copy's fields: its header still parses.
    let at = second * 512 + 0x300;
    patch(&img, at, &[read_byte(&img, at) ^ 0xff]);

    let dev = FileDevice::open(&img).unwrap();
    let survey = primitive::survey_superblocks(&dev);
    assert_eq!(survey.copies[0].verdict, Verdict::BadMagic);
    assert_eq!(survey.copies[0].seq, None);
    let torn = survey.copies.iter().find(|c| c.sector == second).unwrap();
    assert!(
        matches!(torn.verdict, Verdict::BadChecksum { .. }),
        "{:?}",
        torn.verdict
    );
    assert_eq!(
        torn.seq, before.copies[1].seq,
        "a torn copy still says its seq"
    );
    assert_eq!(torn.origin, CopyOrigin::StandaloneLayout);
    assert_eq!(survey.damaged().count(), 2);
    let third = before.copies[2].sector;
    assert_eq!(survey.chosen, Some(third));
    assert_eq!(
        Superblock::read(&dev).unwrap().offset,
        third,
        "the reader agrees"
    );
}

#[test]
fn the_live_leaf_inodes_are_the_references() {
    for set in SETS {
        let dev = FileDevice::open(fixture(&format!("{set}.img"))).unwrap();
        let sb = Superblock::read(&dev).unwrap();
        let ctx = NodeContext::from_superblock(&sb);
        let live = primitive::live_nodes(&dev, &sb);
        assert!(live.breaks.is_empty(), "{set}: {:?}", live.breaks);
        let mut ours = BTreeSet::new();
        for (&offset, n) in &live.nodes {
            let node = primitive::inspect_node(&dev, &ctx, offset)
                .unwrap()
                .unwrap();
            assert!(node.is_valid(), "{set}: live node at {offset}: {node:?}");
            assert_eq!(
                (node.btree_id, node.level, node.seq),
                (n.btree_id, n.level, n.seq)
            );
            if n.btree_id == btree_id::INODES && n.level == 0 {
                ours.extend(
                    node.merged_keys(true)
                        .into_iter()
                        .filter(|k| matches!(k.key_type, key_type::INODE_V3 | key_type::INODE_V2))
                        .map(|k| k.pos),
                );
            }
        }
        assert_eq!(ours, reference_inode_positions(set), "{set}");
    }
}

#[test]
fn the_scan_finds_every_live_node_and_older_versions_beside_them() {
    for set in SETS {
        let dev = FileDevice::open(fixture(&format!("{set}.img"))).unwrap();
        let sb = Superblock::read(&dev).unwrap();
        let ctx = NodeContext::from_superblock(&sb);
        let live = primitive::live_nodes(&dev, &sb);
        let mut scanned = Vec::new();
        let report = primitive::scan_nodes(&dev, &ctx, 0, u64::MAX, |n| {
            scanned.push((n.offset, n.seq));
            ControlFlow::Continue(())
        });
        assert!(report.unreadable.is_empty(), "{set}");
        assert_eq!(report.resume_at, None, "{set}");
        assert_eq!(report.found as usize, scanned.len(), "{set}");
        for (&offset, n) in &live.nodes {
            assert!(
                scanned.contains(&(offset, n.seq)),
                "{set}: live node at {offset} not found by the scan"
            );
        }
        // What the scan finds beyond the live tree is older versions of
        // nodes, each of a btree the live tree has.
        let live_ids: BTreeSet<u8> = live.nodes.values().map(|n| n.btree_id).collect();
        // The aged image was written to by the reference implementation, which
        // rewrote nodes to new places: older versions are there to find.
        if *set == "aged" {
            assert!(
                scanned.len() > live.nodes.len(),
                "{set}: no older node versions found ({} scanned, {} live)",
                scanned.len(),
                live.nodes.len()
            );
        }
        for &(offset, _) in scanned.iter().filter(|(o, _)| !live.nodes.contains_key(o)) {
            let node = primitive::inspect_node(&dev, &ctx, offset)
                .unwrap()
                .unwrap();
            assert!(
                live_ids.contains(&node.btree_id),
                "{set}: a node at {offset} of btree {} the live tree does not have",
                node.btree_id
            );
        }
    }
}

#[test]
fn a_stopped_scan_resumes_where_it_stopped() {
    let dev = FileDevice::open(fixture("aged.img")).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let ctx = NodeContext::from_superblock(&sb);
    let mut whole = Vec::new();
    primitive::scan_nodes(&dev, &ctx, 0, u64::MAX, |n| {
        whole.push(n.offset);
        ControlFlow::Continue(())
    });
    assert!(whole.len() > 3, "{whole:?}");
    let mut parts = Vec::new();
    let mut from = 0;
    loop {
        let report = primitive::scan_nodes(&dev, &ctx, from, u64::MAX, |n| {
            parts.push(n.offset);
            ControlFlow::Break(())
        });
        match report.resume_at {
            Some(at) => from = at,
            None => break,
        }
    }
    assert_eq!(parts, whole);
}

#[test]
fn a_damaged_bset_keeps_its_keys_where_the_reader_refuses_the_node() {
    let img = scratch("default", "node-damage");
    let dev = FileDevice::open(&img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let ctx = NodeContext::from_superblock(&sb);
    let live = primitive::live_nodes(&dev, &sb);
    let (&offset, _) = live
        .nodes
        .iter()
        .find(|(_, n)| n.btree_id == btree_id::INODES && n.level == 0)
        .expect("an inodes leaf");
    let before = primitive::inspect_node(&dev, &ctx, offset)
        .unwrap()
        .unwrap();
    drop(dev);
    // Byte 72 of the header: covered by the first bset's checksum, read by
    // nothing.
    patch(&img, offset + 72, &[read_byte(&img, offset + 72) ^ 0x01]);

    let dev = FileDevice::open(&img).unwrap();
    let after = primitive::inspect_node(&dev, &ctx, offset)
        .unwrap()
        .unwrap();
    assert!(
        matches!(after.bsets[0].verdict, Verdict::BadChecksum { .. }),
        "{:?}",
        after.bsets[0]
    );
    assert!(!after.is_valid());
    assert_eq!(after.keys, before.keys, "every key is still evidence");
    assert_eq!(after.merged_keys(false), before.merged_keys(true));
    assert!(after.merged_keys(true).is_empty() || after.bsets.len() > 1);

    let ptr = NodePtr {
        seq: before.seq,
        sectors_written: (before.written_bytes / 512) as u16,
        min_key: before.min_key,
        ptrs: vec![fs_bcachefs::extent::Ptr {
            dev: 0,
            offset: offset / 512,
            gen: 0,
            flags: 0,
        }],
    };
    assert!(
        matches!(
            btree::read_node(&dev, &sb, &ptr),
            Err(fs_bcachefs::Error::BadChecksum { .. })
        ),
        "the strict reader refuses the node"
    );
}

/// `(seq, bytes, last_seq, flush, sector)` of every entry the reference
/// listed, as tests/oracle_journal.rs reads them.
fn reference_journal(set: &str) -> Vec<(u64, usize, u64, bool, u64)> {
    let text = read_text(&format!("{set}.journal-headers.txt"));
    let mut out = Vec::new();
    let mut cur: Option<(u64, usize, u64, bool)> = None;
    for line in text.lines() {
        let t = line.trim();
        if let Some(n) = t.strip_prefix("journal entry") {
            cur = Some((n.trim().parse().unwrap(), 0, 0, false));
        } else if let (Some(c), Some(v)) = (cur.as_mut(), t.strip_prefix("bytes")) {
            c.1 = v.trim().parse().unwrap();
        } else if let (Some(c), Some(v)) = (cur.as_mut(), t.strip_prefix("last seq")) {
            c.2 = v.trim().parse().unwrap();
        } else if let (Some(c), Some(v)) = (cur.as_mut(), t.strip_prefix("flush")) {
            c.3 = v.trim() == "1";
        } else if let (Some(c), Some(v)) = (cur, t.strip_prefix("written at")) {
            let sector = v.rsplit("(sector ").next().unwrap().trim_end_matches(')');
            out.push((c.0, c.1, c.2, c.3, sector.parse().unwrap()));
            cur = None;
        }
    }
    assert!(out.len() > 100, "the reference listed only {}", out.len());
    out
}

#[test]
fn the_journal_survey_lists_every_entry_the_reference_does() {
    let set = "aged-unclean";
    let dev = FileDevice::open(fixture(&format!("{set}.img"))).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let survey = primitive::survey_journal(&dev, &sb);
    assert_eq!(survey.error, None);
    assert!(survey.unreadable.is_empty());
    for (seq, bytes, last_seq, flush, sector) in reference_journal(set) {
        let e = survey
            .entries
            .iter()
            .find(|e| e.seq == seq && e.verdict.is_valid())
            .unwrap_or_else(|| panic!("entry {seq}: listed by the reference, not surveyed"));
        assert_eq!(
            (e.bytes, e.last_seq, e.flush, e.sector),
            (bytes, last_seq, flush, sector),
            "entry {seq}"
        );
    }
    let replay = journal::replay(&dev, &sb).unwrap();
    let newest = survey.newest_flush().expect("a flush entry");
    assert_eq!(
        newest.seq, replay.seq,
        "the replay starts where the survey says"
    );
    let roots: Vec<_> = newest
        .roots
        .iter()
        .map(|r| (r.btree_id, r.level, r.key.clone()))
        .collect();
    assert_eq!(roots, replay.roots);
}

#[test]
fn a_torn_journal_entry_is_listed_and_the_bucket_read_past_it() {
    let set = "aged-unclean";
    let img = scratch(set, "journal-damage");
    let dev = FileDevice::open(&img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let before = primitive::survey_journal(&dev, &sb);
    let strict_before = journal::read_entries(&dev, &sb).unwrap().len();
    drop(dev);
    // An entry with valid neighbours on both sides in its own bucket.
    let victim = before
        .entries
        .windows(3)
        .find(|w| {
            w.iter().all(|e| e.verdict.is_valid())
                && w[0].sector < w[1].sector
                && w[1].sector < w[2].sector
                && w[1].bytes > 512
        })
        .map(|w| w[1].clone())
        .expect("an entry between two others");
    let at = victim.sector * 512 + 200;
    patch(&img, at, &[read_byte(&img, at) ^ 0xff]);

    let dev = FileDevice::open(&img).unwrap();
    let after = primitive::survey_journal(&dev, &sb);
    assert_eq!(after.entries.len(), before.entries.len());
    let torn = after
        .entries
        .iter()
        .find(|e| e.sector == victim.sector)
        .unwrap();
    assert!(
        matches!(torn.verdict, Verdict::BadChecksum { .. }),
        "{:?}",
        torn.verdict
    );
    assert_eq!(torn.seq, victim.seq);
    let valid_after = after
        .entries
        .iter()
        .filter(|e| e.verdict.is_valid())
        .count();
    let valid_before = before
        .entries
        .iter()
        .filter(|e| e.verdict.is_valid())
        .count();
    assert_eq!(valid_after, valid_before - 1);
    let strict_after = journal::read_entries(&dev, &sb).unwrap().len();
    assert!(
        strict_after < strict_before,
        "the strict reader ends the bucket at the torn entry ({strict_after} of {strict_before})"
    );
}
