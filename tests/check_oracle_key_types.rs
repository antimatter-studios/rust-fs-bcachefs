//! Which key types each btree holds, as the reference checker judges it
//! (#163), INSIDE the harness guest only (`chore test:vm`); anywhere else it
//! fails naming that. Nothing skips.
//!
//! Every btree of a copy of `default` is planted with a key of every type
//! S1 names, and one past them, eight to an image. A btree `default`
//! records no root for is first given the empty deleted_inodes btree's.
//! Each planted key sits at its own position after the btree's last key,
//! and carries the value of a key of its type the reference wrote into a
//! fixture where one has, so that a type the btree holds is not refused for
//! its value instead. The reference checker's verdict on each planted key
//! is read from its printed output, and nothing else of it, and this
//! checker must report `invalid_bkey` for exactly the keys the reference
//! refuses. The pairs the reference itself writes, across the fixtures,
//! are printed beside any disagreement.
//!
//! A case on which the reference crashes or hangs is counted and never
//! described (#164's disclosure rule): the test fails naming only the
//! count.
#![cfg(feature = "write")]

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::Command;

use common::fixture;
use common::plant::{self, move_root, DELETED_INODES};
use fs_bcachefs::bkey::{Bkey, Bpos};
use fs_bcachefs::btree;
use fs_bcachefs::check::check;
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::write::Writer;
use fs_core::FileDevice;

/// The fixtures the reference tools made, read for the key types they
/// hold and a value of each.
const MADE_BY_THE_REFERENCE: &[&str] = &[
    "default",
    "aged",
    "aged-unclean",
    "lz4",
    "block4k",
    "casefold",
    "large",
    "kernel",
    "kernel-reuse",
    "kernel-rootsnap",
    "kernel-prealloc",
];

/// Planted keys to an image: few enough that a checker which reports a
/// run of one kind of error briefly still names each.
const PER_IMAGE: usize = 8;

/// The reference's output on a planted image the test shows in full when
/// it fails, so what its verdicts look like is in the log.
const SHOWN: &[&str] = &["logged_ops", "accounting", "reconcile_work_phys"];

/// What the reference wrote, across its fixtures.
#[derive(Default)]
struct Written {
    /// Per btree, the key types found in it.
    types: BTreeMap<u8, BTreeSet<u8>>,
    /// Per key type, the value of the first key of it found.
    values: BTreeMap<u8, Vec<u8>>,
    /// Per btree, the snapshot field of the first key found in it.
    snapshot: BTreeMap<u8, u32>,
}

fn written() -> Written {
    let mut w = Written::default();
    for name in MADE_BY_THE_REFERENCE {
        let dev = FileDevice::open(fixture(&format!("{name}.img"))).unwrap();
        let sb = Superblock::read(&dev).unwrap();
        let replay = btree::journal_for(&dev, &sb).unwrap_or_else(|e| panic!("{name}: {e}"));
        let roots: Vec<u8> = match &replay {
            Some(rp) => rp.roots.iter().map(|(id, _, _)| *id).collect(),
            None => sb
                .btree_roots()
                .unwrap()
                .iter()
                .map(|r| r.btree_id)
                .collect(),
        };
        for id in roots {
            let keys = btree::walk_replayed(&dev, &sb, id, replay.as_ref())
                .unwrap_or_else(|e| panic!("{name}: btree {id}: {e}"));
            for k in keys {
                w.types.entry(id).or_default().insert(k.key_type);
                w.values.entry(k.key_type).or_insert(k.value);
                w.snapshot.entry(id).or_insert(k.pos.snapshot);
            }
        }
    }
    w
}

/// Where a key of type `t` is planted: after the btree's last key, or from
/// the start of an empty one.
fn planted_at(last: Option<Bpos>, snapshot: u32, t: u8) -> Bpos {
    let step = 0x100 + u64::from(t);
    match last {
        Some(p) => match p.offset.checked_add(step) {
            Some(offset) => Bpos { offset, ..p },
            None => Bpos {
                inode: p.inode + 1,
                offset: step,
                snapshot: p.snapshot,
            },
        },
        None => Bpos {
            inode: 0,
            offset: step,
            snapshot,
        },
    }
}

/// Whether `line` names position `p`, not merely a longer one ending in it.
fn names(line: &str, p: &str) -> bool {
    line.match_indices(p).any(|(at, _)| {
        let before = line[..at].chars().next_back();
        let after = line[at + p.len()..].chars().next();
        !before.is_some_and(|c| c.is_ascii_alphanumeric())
            && !after.is_some_and(|c| c.is_ascii_alphanumeric())
    })
}

/// The reference's verdict on the planted key at `p`, from its output: the
/// lines around each one naming it, up to a line naming another planted
/// key, and whether they call it invalid.
fn verdict(lines: &[&str], p: &str, others: &[String]) -> (bool, Option<String>) {
    let mut rejected = false;
    let mut said = None;
    for (i, l) in lines.iter().enumerate() {
        if !names(l, p) {
            continue;
        }
        let mut window = vec![*l];
        for j in (i.saturating_sub(2)..i).rev() {
            if others.iter().any(|o| names(lines[j], o)) {
                break;
            }
            window.insert(0, lines[j]);
        }
        for l2 in lines.iter().skip(i + 1).take(2) {
            if others.iter().any(|o| names(l2, o)) {
                break;
            }
            window.push(l2);
        }
        let text = window.join(" | ");
        if text.to_lowercase().contains("invalid") {
            rejected = true;
            said.get_or_insert(text.replace(p, "<pos>"));
        }
    }
    (rejected, said)
}

/// `text` with each whole-word `word` (not a part of a longer name, as
/// `set` is of `offset`) replaced by `with`.
fn replace_word(text: &str, word: &str, with: &str) -> String {
    let part = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
    let mut out = String::new();
    let mut from = 0;
    for (at, _) in text.match_indices(word) {
        if at < from
            || part(text[..at].chars().next_back())
            || part(text[at + word.len()..].chars().next())
        {
            continue;
        }
        out.push_str(&text[from..at]);
        out.push_str(with);
        from = at + word.len();
    }
    out.push_str(&text[from..]);
    out
}

/// Whether the reference crashed or hung rather than judged.
fn crashed(status: std::process::ExitStatus) -> bool {
    match status.code() {
        None => true,
        Some(c) => c == 124 || c >= 128,
    }
}

fn names_of(types: &BTreeSet<u8>) -> String {
    let v: Vec<String> = types.iter().map(|&t| plant::type_name(t)).collect();
    format!("[{}]", v.join(" "))
}

#[test]
fn each_btree_holds_the_key_types_the_reference_lets_it_hold() {
    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference checker is not here: this runs inside the harness guest (`chore test:vm`)"
    );
    let written = written();
    let src = std::fs::read(fixture("default.img")).unwrap();
    let default = FileDevice::open(fixture("default.img")).unwrap();
    let sb = Superblock::read(&default).unwrap();
    let rooted: BTreeSet<u8> = sb
        .btree_roots()
        .unwrap()
        .iter()
        .map(|r| r.btree_id)
        .collect();
    // Under /share: the reference tools run in a chroot that sees it.
    let dir = PathBuf::from("/share/check-oracle/key-types");
    std::fs::create_dir_all(&dir).unwrap();

    let types: Vec<u8> = (1..=plant::KEY_TYPES.len() as u8).collect();
    let mut theirs: BTreeMap<u8, BTreeSet<u8>> = BTreeMap::new();
    let mut ours: BTreeMap<u8, BTreeSet<u8>> = BTreeMap::new();
    let mut said: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let mut failures = Vec::new();
    let mut shown = Vec::new();
    let mut crashes = 0;
    let mut ratelimited = false;
    for (id, name) in plant::BTREES.iter().enumerate() {
        let id = id as u8;
        let last = if rooted.contains(&id) {
            btree::walk(&default, &sb, id)
                .unwrap()
                .last()
                .map(|k| k.pos)
        } else {
            None
        };
        let snapshot = written.snapshot.get(&id).copied().unwrap_or(0);
        for group in types.chunks(PER_IMAGE) {
            let img = dir.join(format!("{name}-{}.img", group[0]));
            plant::write_sparse(&src, &img);
            if !rooted.contains(&id) {
                move_root(&img, DELETED_INODES, id);
            }
            let keys: Vec<Bkey> = group
                .iter()
                .map(|&t| Bkey {
                    key_type: t,
                    size: 0,
                    version_hi: 0,
                    version_lo: 0,
                    pos: planted_at(last, snapshot, t),
                    value: written.values.get(&t).cloned().unwrap_or_default(),
                })
                .collect();
            let planted = catch_unwind(AssertUnwindSafe(|| {
                let mut w = Writer::open(FileDevice::open_rw(&img).unwrap())?;
                w.insert(id, keys.clone())
            }));
            match planted {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    failures.push(format!("{name} {group:?}: the writer refused: {e}"));
                    std::fs::remove_file(&img).unwrap();
                    continue;
                }
                Err(_) => {
                    failures.push(format!("{name} {group:?}: the writer panicked"));
                    std::fs::remove_file(&img).unwrap();
                    continue;
                }
            }
            let report =
                match catch_unwind(AssertUnwindSafe(|| check(&FileDevice::open(&img).unwrap()))) {
                    Ok(Ok(r)) => r,
                    Ok(Err(e)) => {
                        failures.push(format!("{name} {group:?}: this checker refused: {e}"));
                        std::fs::remove_file(&img).unwrap();
                        continue;
                    }
                    Err(_) => {
                        failures.push(format!("{name} {group:?}: this checker panicked"));
                        std::fs::remove_file(&img).unwrap();
                        continue;
                    }
                };
            let out = Command::new("timeout")
                .args(["300", "bcachefs-ref", "fsck", "-n"])
                .arg(&img)
                .output()
                .unwrap();
            std::fs::remove_file(&img).unwrap();
            if crashed(out.status) {
                crashes += 1;
                continue;
            }
            let text = String::from_utf8_lossy(&out.stdout).into_owned()
                + &String::from_utf8_lossy(&out.stderr);
            ratelimited |= text.to_lowercase().contains("ratelimit");
            let lines: Vec<&str> = text.lines().collect();
            if group[0] == 1 && SHOWN.contains(name) {
                shown.push(format!("## the reference on {name}, types {group:?}"));
                shown.extend(lines.iter().take(40).map(|l| l.chars().take(240).collect()));
            }
            let at: Vec<String> = keys.iter().map(|k| k.pos.to_string()).collect();
            for (k, p) in keys.iter().zip(&at) {
                let others: Vec<String> = at.iter().filter(|o| *o != p).cloned().collect();
                let (rejected, text) = verdict(&lines, p, &others);
                if rejected {
                    theirs.entry(id).or_default().insert(k.key_type);
                }
                if let Some(text) = text {
                    let text = replace_word(&text, &plant::type_name(k.key_type), "<type>");
                    let e = said
                        .entry(text)
                        .or_insert((0, format!("{name} {}", plant::type_name(k.key_type))));
                    e.0 += 1;
                }
                if report
                    .problems
                    .iter()
                    .any(|q| q.kind == "invalid_bkey" && q.detail.contains(&format!("key {p} ")))
                {
                    ours.entry(id).or_default().insert(k.key_type);
                }
            }
        }
    }

    let mut disagree = Vec::new();
    for (id, name) in plant::BTREES.iter().enumerate() {
        let id = id as u8;
        let t = theirs.get(&id).cloned().unwrap_or_default();
        let o = ours.get(&id).cloned().unwrap_or_default();
        if t != o {
            disagree.push(format!(
                "{name}: the reference refuses {}; this checker reports {}",
                names_of(&t),
                names_of(&o)
            ));
        }
    }
    if crashes > 0 || !failures.is_empty() || !disagree.is_empty() {
        let mut msg = vec![format!(
            "{} btrees disagree, {} planted images failed, the reference crashed or hung on {crashes}{}",
            disagree.len(),
            failures.len(),
            if ratelimited { " (its output mentions rate limiting)" } else { "" }
        )];
        msg.extend(disagree);
        msg.extend(failures);
        msg.push("## what the reference said of the keys it refused".into());
        msg.extend(said.iter().take(60).map(|(text, (n, first))| {
            format!(
                "{n}x (first {first}): {}",
                text.chars().take(300).collect::<String>()
            )
        }));
        msg.push("## the key types the reference wrote, by btree".into());
        msg.extend(written.types.iter().map(|(id, t)| {
            format!(
                "{}: {}",
                plant::BTREES.get(usize::from(*id)).copied().unwrap_or("?"),
                names_of(t)
            )
        }));
        msg.extend(shown);
        panic!("{}", msg.join("\n"));
    }
}
