//! Which key types each btree holds, as the reference checker judges it
//! (#163, #305), INSIDE the harness guest only (`chore test:vm`); anywhere
//! else it fails naming that. Nothing skips.
//!
//! Every btree S1 names, in a copy of `default`, is planted with a key of
//! every type S1 names, and one past them, one key to an image. A btree
//! `default` records no root for is first given a root that holds no keys.
//! A resident type keeps its reference-written position and value. Other
//! keys sit after the btree's last key (bucket_gens uses an existing group),
//! and carry the value
//! of a key of its type the reference wrote into a fixture where one has,
//! so that a type the btree holds is not refused for its value instead. The
//! reference checker's verdict on the planted key is read from its printed
//! output, and nothing else of it, and this checker must report
//! `invalid_bkey` for exactly the keys the reference refuses. The pairs the
//! reference itself writes, across the fixtures, are printed beside any
//! disagreement.
//! Only a refusal the reference gives for the key's own type counts. A key
//! it refuses for something it names before the type -- its value size, a
//! size of zero, its snapshot -- is planted again with that changed (see
//! `replant`); a key it still refuses for anything but its type leaves that
//! type unresolved, and an unresolved type fails the test rather than
//! passing as accepted.
//!
//! A case on which the reference crashes or hangs is counted and never
//! described (#164's disclosure rule): the test fails naming only the
//! count.
#![cfg(feature = "write")]

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::process::Command;

use common::fixture;
use common::plant::{self, LOGGED_OPS};
use fs_bcachefs::bkey::{Bkey, Bpos};
use fs_bcachefs::btree;
use fs_bcachefs::check::{check, Report};
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

/// The btrees whose key types are judged: every btree S1 names (#305), not
/// only the three in which the reference checker's `invalid bkey` showed
/// up in #163's differential.
fn judged() -> Vec<u8> {
    (0..plant::BTREES.len() as u8).collect()
}
const INODES: u8 = 1;
const DIRENTS: u8 = 2;
const ALLOC: u8 = 4;
const REFLINK: u8 = 7;
const SNAPSHOTS: u8 = 9;
const DAMAGE: u8 = 28;
const FREESPACE: u8 = 11;
const KEY_LOGGED_OP_TRUNCATE: u8 = 32;
const XATTRS: u8 = 3;
/// `lru` is key type 26 in S1 11.5's list (`plant::KEY_TYPES`); 10 is
/// `dirent`, which the reference names as such when it is planted.
const KEY_LRU: u8 = 26;
const KEY_XATTR: u8 = 11;
const BUCKET_GENS: u8 = 14;
const KEY_STRIPE: u8 = 14;
const KEY_DAMAGE: u8 = 38;

/// Planted keys are judged one to an image. With eight to an image the
/// reference named only some of the keys it refuses (run 39020187551): of a
/// dirent planted into reconcile_work_phys among seven other keys it said
/// nothing, while the same dirent planted alone is refused for its type
/// (`check_oracle_reconcile_dirent.rs`, the same run).
///
/// Times a key refused for something other than its type is planted again
/// (`replant`): twice for its value size, as before #305, once for a size
/// of zero and once for its snapshot.
const REPLANTS: usize = 4;

/// The size a key refused for "size == 0" is planted again with: extents,
/// reflink and freespace name that before a key's type (docs/clean-room.md,
/// "The key types a btree holds"). Eight sectors; the planted position's
/// offset, 0x100 or more past the btree's last key, is always past it.
const PLANTED_SIZE: u32 = 8;

/// How long the reference checker may take over one planted image. It took
/// about 0.2 s an image across the three btrees judged before #305 (queue
/// run 39895568461), so a minute is far beyond a judgement and well short
/// of letting a few hangs among every btree's images run out the job.
const REFERENCE_TIMEOUT_SECONDS: &str = "60";

fn planted_types() -> Vec<u8> {
    (1..=plant::KEY_TYPES.len() as u8).collect()
}

#[test]
fn a_key_of_every_type_is_planted_into_every_btree() {
    let judged = judged();
    assert_eq!(
        judged.len(),
        plant::BTREES.len(),
        "every btree S1 names is judged"
    );
    for id in [
        0, INODES, DIRENTS, XATTRS, ALLOC, REFLINK, SNAPSHOTS, FREESPACE, DAMAGE,
    ] {
        assert!(
            judged.contains(&id),
            "btree {} is not judged",
            plant::BTREES[usize::from(id)]
        );
    }
    assert_eq!(plant::BTREES[usize::from(DAMAGE)], "damage");
    assert_eq!(plant::BTREES[usize::from(SNAPSHOTS)], "snapshots");
    assert_eq!(plant::BTREES[usize::from(REFLINK)], "reflink");
    assert_eq!(plant::BTREES[usize::from(ALLOC)], "alloc");
}

#[test]
fn a_key_type_past_the_documented_names_remains_in_the_matrix() {
    let types = planted_types();
    assert_eq!(
        types.first(),
        Some(&1),
        "deleted keys do not survive merging"
    );
    assert_eq!(
        types.last(),
        Some(&40),
        "the unnamed boundary type is judged too"
    );
    assert_eq!(types.len(), 40);
    for t in 1..40 {
        assert!(types.contains(&t), "documented type {t} is missing");
    }
}

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

/// A type already present in default keeps its complete reference-written
/// key. Moving a dirent can make it point to its own directory; moving a
/// snapshot can put it past U32_MAX. Neither is a verdict about its type.
/// A bucket-generation group also keeps a position the reference wrote.
fn planted_key(id: u8, t: u8, resident: &[Bkey], written: &Written) -> Bkey {
    if let Some(key) = resident.iter().find(|k| k.key_type == t) {
        return key.clone();
    }
    let pos = if id == BUCKET_GENS {
        resident
            .first()
            .expect("default has a bucket-generation group")
            .pos
    } else {
        planted_at(
            resident.last().map(|k| k.pos),
            written.snapshot.get(&id).copied().unwrap_or(0),
            t,
        )
    };
    // Black-box probes (docs/clean-room.md): the stripe printer reads
    // sectors from bytes 0..2 and its pointer count from byte 3. One sector,
    // one reference-written pointer and an empty block count pass its key
    // validation. No stripe data layout is inferred beyond this control.
    let value = match t {
        KEY_STRIPE => {
            let extent = written.values.get(&6).expect("a reference-written extent");
            let ptr = extent
                .chunks_exact(8)
                .find(|w| w[0] & 1 != 0)
                .expect("the reference extent has a pointer");
            let mut value = vec![0; 24];
            value[0] = 1;
            value[3] = 1;
            value[8..16].copy_from_slice(ptr);
            value
        }
        KEY_DAMAGE => {
            // A zero id is refused even with a nonzero count. The reference
            // accepts a 16-byte value whose first byte is one.
            let mut value = vec![0; 16];
            value[0] = 1;
            value
        }
        _ => written.values.get(&t).cloned().unwrap_or_default(),
    };
    Bkey {
        key_type: t,
        size: 0,
        version_hi: 0,
        version_lo: 0,
        pos,
        value,
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

/// The word after `key` in `line`, as `reconcile_work_phys` after
/// `btree=` or `cookie` after ` type `.
fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let rest = line.split(key).nth(1)?;
    rest.split(|c: char| c.is_whitespace() || c == ':')
        .next()
        .filter(|w| !w.is_empty())
}

/// The reference's verdict on the planted key at `p`, from its output:
/// whether it refused the key for its type, and what it said of the key.
/// Only an `invalid bkey` line naming `p`, in btree B with type T, followed
/// by its own indented reason "invalid key type for btree B (T)" is a type
/// refusal. Any other reason the reference gives (a value size, a snapshot)
/// is kept as what it said, and leaves the type unresolved.
fn verdict(lines: &[&str], p: &str, others: &[String]) -> (bool, Option<String>) {
    let mut rejected = false;
    let mut said = None;
    for (i, l) in lines.iter().enumerate() {
        if !names(l, p) {
            continue;
        }
        // A pass's progress line ("check_allocations...") is not ended
        // before a key it reads is refused, so the refusal can follow it
        // on the same line (queue run 39847363486).
        let l = match l.find("invalid bkey") {
            Some(at) if l[..at].ends_with("...") => &l[at..],
            _ => *l,
        };
        if !l.starts_with("invalid bkey") {
            if l.to_lowercase().contains("invalid") {
                said.get_or_insert(l.replace(p, "<pos>"));
            }
            continue;
        }
        let reasons: Vec<&str> = lines[i + 1..]
            .iter()
            .take_while(|r| {
                r.starts_with(char::is_whitespace) && !others.iter().any(|o| names(r, o))
            })
            .map(|r| r.trim())
            .collect();
        let text = std::iter::once(l.replace(p, "<pos>"))
            .chain(reasons.iter().map(|r| r.to_string()))
            .collect::<Vec<_>>()
            .join(" | ");
        let of_type = match (field(l, "btree="), field(l, " type ")) {
            (Some(b), Some(t)) => {
                let wrong = format!("invalid key type for btree {b} ({t})");
                reasons.iter().any(|r| r.starts_with(&wrong))
            }
            _ => false,
        };
        if of_type {
            rejected = true;
            said = Some(text);
        } else {
            said.get_or_insert(text);
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

/// The value size the reference asks of a key it refused for its value
/// size: `N` in its "bad val size (n < N)".
fn wanted_val_size(said: &str) -> Option<usize> {
    let rest = said.split("bad val size (").nth(1)?;
    let (_, rest) = rest.split_once(" < ")?;
    rest.split(')').next()?.trim().parse().ok()
}

/// What a key the reference refused for something other than its type is
/// planted again with, so that its type is judged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Replant {
    /// A zeroed value of this many bytes: "bad val size (n < N)".
    ValueSize(usize),
    /// This size: "size == 0" (extents, reflink, freespace).
    Size(u32),
    /// This snapshot: "snapshot == 0" (damage) asks for one,
    /// "nonzero snapshot" (reconcile_work_phys) for none.
    Snapshot(u32),
}

impl Replant {
    fn apply(self, key: &mut Bkey) {
        match self {
            Replant::ValueSize(n) => key.value = vec![0; n],
            Replant::Size(n) => key.size = n,
            Replant::Snapshot(s) => key.pos.snapshot = s,
        }
    }
}

/// How to plant `key` again after the reference said `said` of it without
/// refusing its type, or `None` when what it said asks for nothing `key`
/// does not already have: a reason asked for once and still given is a
/// verdict, not a retry. The value size is asked for first, as the
/// reference names it before the rest.
fn replant(said: &str, key: &Bkey) -> Option<Replant> {
    if let Some(n) = wanted_val_size(said).filter(|&n| n > key.value.len()) {
        return Some(Replant::ValueSize(n.next_multiple_of(8)));
    }
    if said.contains("size == 0") && key.size == 0 {
        return Some(Replant::Size(PLANTED_SIZE));
    }
    if said.contains("snapshot == 0") && key.pos.snapshot == 0 {
        return Some(Replant::Snapshot(u32::MAX));
    }
    if said.contains("nonzero snapshot") && key.pos.snapshot != 0 {
        return Some(Replant::Snapshot(0));
    }
    None
}

/// What became of one planted image.
enum Judged {
    /// The reference crashed or hung rather than judged.
    Crashed,
    /// The reference's printed output, and this checker's report.
    Seen(String, Report),
}

/// A copy of `src` at `img`, with `key` planted into btree `id` (given a
/// root first if `default` records none), judged by both checkers. The
/// image is removed whatever happens. An `Err` names what failed.
fn judge(src: &[u8], img: &Path, rooted: bool, id: u8, key: &Bkey) -> Result<Judged, String> {
    plant::write_sparse(src, img);
    let judged = plant_and_run(img, rooted, id, key);
    std::fs::remove_file(img).unwrap();
    judged
}

fn plant_and_run(img: &Path, rooted: bool, id: u8, key: &Bkey) -> Result<Judged, String> {
    if !rooted {
        plant::give_root(img, id);
    }
    let planted = catch_unwind(AssertUnwindSafe(|| {
        let mut w = Writer::open(FileDevice::open_rw(img).unwrap())?;
        w.insert(id, vec![key.clone()])
    }));
    match planted {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(format!("the writer refused: {e}")),
        Err(_) => return Err("the writer panicked".into()),
    }
    let report = match catch_unwind(AssertUnwindSafe(|| check(&FileDevice::open(img).unwrap()))) {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => return Err(format!("this checker refused: {e}")),
        Err(_) => return Err("this checker panicked".into()),
    };
    let out = Command::new("timeout")
        .args([REFERENCE_TIMEOUT_SECONDS, "bcachefs-ref", "fsck", "-n"])
        .arg(img)
        .output()
        .unwrap();
    if crashed(out.status) {
        return Ok(Judged::Crashed);
    }
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    Ok(Judged::Seen(text, report))
}

fn names_of(types: &BTreeSet<u8>) -> String {
    let v: Vec<String> = types.iter().map(|&t| plant::type_name(t)).collect();
    format!("[{}]", v.join(" "))
}

#[test]
fn each_judged_btree_holds_the_key_types_the_reference_lets_it_hold() {
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
    // The run's scratch root: the reference tools' chroot sees it.
    let scratch = common::ScratchDir::new("check-oracle-key-types");
    let dir = scratch.path().to_path_buf();

    let types = planted_types();
    let mut theirs: BTreeMap<u8, BTreeSet<u8>> = BTreeMap::new();
    let mut ours: BTreeMap<u8, BTreeSet<u8>> = BTreeMap::new();
    // Keys the reference refused for something other than their type: whether
    // it lets the btree hold that type is not known, so they are compared by
    // neither side and fail the test as unresolved.
    let mut unresolved: BTreeMap<u8, BTreeSet<u8>> = BTreeMap::new();
    // Keys the reference judged and did not refuse.
    let mut accepted: BTreeMap<u8, BTreeSet<u8>> = BTreeMap::new();
    let mut said: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let mut failures = Vec::new();
    // Per btree, the reference's output on the first image whose verdict
    // this checker does not share or which is left unresolved.
    let mut shown: BTreeMap<u8, Vec<String>> = BTreeMap::new();
    let mut crashes = 0;
    let mut ratelimited = false;
    for id in judged() {
        let name = plant::BTREES[usize::from(id)];
        let has_root = rooted.contains(&id);
        let resident = if has_root {
            btree::walk(&default, &sb, id).unwrap()
        } else {
            Vec::new()
        };
        for &t in &types {
            let mut key = planted_key(id, t, &resident, &written);
            let img = dir.join(format!("{name}-{t}.img"));
            for attempt in 0..=REPLANTS {
                // A replant may move the key to another snapshot.
                let p = key.pos.to_string();
                let (text, report) = match judge(&src, &img, has_root, id, &key) {
                    Ok(Judged::Seen(text, report)) => (text, report),
                    Ok(Judged::Crashed) => {
                        crashes += 1;
                        break;
                    }
                    Err(e) => {
                        failures.push(format!("{name} {}: {e}", plant::type_name(t)));
                        break;
                    }
                };
                ratelimited |= text.to_lowercase().contains("ratelimit");
                let lines: Vec<&str> = text.lines().collect();
                let (rejected, text) = verdict(&lines, &p, &[]);
                // A refusal for what the reference judges before the type
                // (a value size, a size of zero, a snapshot) is not a
                // verdict on the type yet: the key is planted again with
                // what it asked for.
                let again = text
                    .as_deref()
                    .filter(|_| !rejected)
                    .and_then(|s| replant(s, &key));
                if let Some(text) = &text {
                    let text = replace_word(text, &plant::type_name(t), "<type>");
                    let e = said
                        .entry(text)
                        .or_insert((0, format!("{name} {}", plant::type_name(t))));
                    e.0 += 1;
                }
                if let Some(r) = again.filter(|_| attempt < REPLANTS) {
                    r.apply(&mut key);
                    continue;
                }
                let open = !rejected && text.is_some();
                let reported = report.problems.iter().any(|q| {
                    q.kind == "invalid_bkey"
                        && q.detail.contains(&format!("btree {name}: key {p} "))
                });
                if rejected {
                    theirs.entry(id).or_default().insert(t);
                }
                if open {
                    unresolved.entry(id).or_default().insert(t);
                } else if reported {
                    ours.entry(id).or_default().insert(t);
                }
                if !rejected && !open {
                    accepted.entry(id).or_default().insert(t);
                }
                if open || rejected != reported {
                    shown.entry(id).or_insert_with(|| {
                        std::iter::once(format!(
                            "## the reference on {name}, type {}",
                            plant::type_name(t)
                        ))
                        .chain(lines.iter().take(40).map(|l| l.chars().take(240).collect()))
                        .collect()
                    });
                }
                break;
            }
        }
    }

    let mut disagree = Vec::new();
    for id in judged() {
        let name = plant::BTREES[usize::from(id)];
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
    if crashes > 0 || !failures.is_empty() || !disagree.is_empty() || !unresolved.is_empty() {
        let mut msg = vec![format!(
            "{} btrees disagree, {} hold key types left unresolved, {} planted images failed, the reference crashed or hung on {crashes}{}",
            disagree.len(),
            unresolved.len(),
            failures.len(),
            if ratelimited { " (its output mentions rate limiting)" } else { "" }
        )];
        msg.extend(disagree);
        msg.extend(unresolved.iter().map(|(id, t)| {
            format!(
                "{}: unresolved, refused for something other than their type: {}",
                plant::BTREES.get(usize::from(*id)).copied().unwrap_or("?"),
                names_of(t)
            )
        }));
        msg.extend(failures);
        // What the checker's table is written from: per btree, the types
        // the reference judged and did not refuse.
        msg.push("## the key types the reference let each btree hold".into());
        msg.extend(accepted.iter().map(|(id, t)| {
            format!(
                "{}: {}",
                plant::BTREES.get(usize::from(*id)).copied().unwrap_or("?"),
                names_of(t)
            )
        }));
        msg.push("## what the reference said of the keys it refused".into());
        msg.extend(said.iter().take(200).map(|(text, (n, first))| {
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
        msg.extend(shown.into_values().flatten());
        panic!("{}", msg.join("\n"));
    }
    logged_op_truncate_in_freespace_is_rejected_by_both_checkers();
    lru_in_freespace_is_rejected_by_both_checkers();
    lru_in_xattrs_is_rejected_by_both_checkers();
}

fn logged_op_truncate_in_freespace_is_rejected_by_both_checkers() {
    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference checker is not here: this runs inside the harness guest (`chore test:vm`)"
    );
    let source = fixture("aged.img");
    let dev = FileDevice::open(&source).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    // The differential planted at 0:2040:0. The aged.img every CI run
    // builds has held its one free extent there so far, but which buckets
    // stay free is the kernel's choice, so the first set key is taken.
    let mut key = btree::walk(&dev, &sb, FREESPACE)
        .unwrap()
        .into_iter()
        .find(|k| k.key_type == 25)
        .expect("the aged fixture has a freespace set key");
    key.key_type = KEY_LOGGED_OP_TRUNCATE;
    let p = key.pos.to_string();

    let dir = common::ScratchDir::new("check-freespace-logged-op-truncate");
    let img = dir.join("aged-1659445.img");
    let bytes = std::fs::read(source).unwrap();
    let (text, report) = match judge(&bytes, &img, true, FREESPACE, &key).unwrap() {
        Judged::Seen(text, report) => (text, report),
        Judged::Crashed => panic!("the reference checker crashed on the recorded mutation"),
    };
    let lines: Vec<&str> = text.lines().collect();
    let (rejected, reason) = verdict(&lines, &p, &[]);
    assert!(
        rejected,
        "the reference did not reject the key for its type: {reason:?}\n{text}"
    );
    assert!(
        report.problems.iter().any(|problem| {
            problem.kind == "invalid_bkey"
                && problem
                    .detail
                    .contains(&format!("btree freespace: key {p}"))
        }),
        "the checker did not report the planted key: {:?}",
        report.problems
    );
}

fn lru_in_freespace_is_rejected_by_both_checkers() {
    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference checker is not here: this runs inside the harness guest (`chore test:vm`)"
    );
    let source = fixture("kernel.img");
    let dev = FileDevice::open(&source).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    // The differential planted at 0:266:0 in the fixtures of run
    // 38741040375. The kernel picks which buckets stay free, so the
    // kernel.img every CI run builds holds its freespace keys elsewhere;
    // any of them carries the mutation.
    let mut key = btree::walk(&dev, &sb, FREESPACE)
        .unwrap()
        .into_iter()
        .find(|k| k.key_type == 25)
        .expect("kernel.img has a freespace set key");
    key.key_type = KEY_LRU;
    let p = key.pos.to_string();

    let dir = common::ScratchDir::new("check-freespace-lru-key");
    let img = dir.join("kernel-1659510.img");
    let bytes = std::fs::read(source).unwrap();
    let (text, report) = match judge(&bytes, &img, true, FREESPACE, &key).unwrap() {
        Judged::Seen(text, report) => (text, report),
        Judged::Crashed => panic!("the reference checker crashed on the recorded mutation"),
    };
    let lines: Vec<&str> = text.lines().collect();
    let (rejected, reason) = verdict(&lines, &p, &[]);
    assert!(
        rejected,
        "the reference did not reject the key for its type: {reason:?}\n{text}"
    );
    assert!(
        reason
            .as_deref()
            .is_some_and(|s| s.contains("invalid key type for btree freespace (lru)")),
        "the reference refused the key for another reason: {reason:?}"
    );
    assert!(
        report.problems.iter().any(|problem| {
            problem.kind == "invalid_bkey"
                && problem
                    .detail
                    .contains(&format!("btree freespace: key {p}"))
        }),
        "the checker did not report the planted key: {:?}",
        report.problems
    );
}

fn lru_in_xattrs_is_rejected_by_both_checkers() {
    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference checker is not here: this runs inside the harness guest (`chore test:vm`)"
    );
    let source = fixture("aged.img");
    let dev = FileDevice::open(&source).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    // The differential planted at 2147487679:1366636819568056465:U32_MAX in
    // the fixtures of run 38741040375. An xattr's offset hashes its name
    // with a seed each inode draws when it is made, so the aged.img every
    // CI run builds holds its xattrs elsewhere; any of them carries the
    // mutation.
    let mut key = btree::walk(&dev, &sb, XATTRS)
        .unwrap()
        .into_iter()
        .find(|k| k.key_type == KEY_XATTR)
        .expect("the aged fixture has an xattr key");
    key.key_type = KEY_LRU;
    let p = key.pos.to_string();

    let dir = common::ScratchDir::new("check-xattrs-lru-key");
    let img = dir.join("aged-1659510.img");
    let bytes = std::fs::read(source).unwrap();
    let (text, report) = match judge(&bytes, &img, true, XATTRS, &key).unwrap() {
        Judged::Seen(text, report) => (text, report),
        Judged::Crashed => panic!("the reference checker crashed on the recorded mutation"),
    };
    let lines: Vec<&str> = text.lines().collect();
    let (rejected, reason) = verdict(&lines, &p, &[]);
    assert!(
        rejected,
        "the reference did not reject the key for its type: {reason:?}\n{text}"
    );
    assert!(
        reason
            .as_deref()
            .is_some_and(|s| s.contains("invalid key type for btree xattrs (lru)")),
        "the reference refused the key for another reason: {reason:?}"
    );
    assert!(
        report.problems.iter().any(|problem| {
            problem.kind == "invalid_bkey"
                && problem.detail.contains(&format!("btree xattrs: key {p}"))
        }),
        "the checker did not report the planted key: {:?}",
        report.problems
    );
}

/// The differential's kernel seed 1659466 changes the key at 1:4:0 to
/// inode_v3 and commits it in logged_ops. The reference refuses that type
/// there; the short value is retained so the mutation stays the recorded
/// one. That key is one of kernel.img's inode allocation cursors, which sit
/// at 1:1:0 upwards and number three or four from one fixture build to the
/// next (queue run 39868718949 held three), so the last one is taken.
#[test]
fn inode_v3_in_logged_ops_is_rejected_by_both_checkers() {
    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference checker is not here: this runs inside the harness guest (`chore test:vm`)"
    );
    let source = fixture("kernel.img");
    let dev = FileDevice::open(&source).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let mut key = btree::walk(&dev, &sb, LOGGED_OPS)
        .unwrap()
        .into_iter()
        .rfind(|k| k.pos.inode == 1 && k.pos.snapshot == 0)
        .expect("kernel.img holds an inode allocation cursor in logged_ops");
    assert_ne!(key.key_type, fs_bcachefs::bkey::key_type::INODE_V3);
    assert_eq!(key.value.len(), 16, "the recorded mutation keeps its value");
    key.key_type = fs_bcachefs::bkey::key_type::INODE_V3;
    let p = key.pos.to_string();

    let dir = common::ScratchDir::new("check-logged-ops-inode-v3");
    let img = dir.join("kernel-1659466.img");
    let bytes = std::fs::read(source).unwrap();
    let (text, report) = match judge(&bytes, &img, true, LOGGED_OPS, &key).unwrap() {
        Judged::Seen(text, report) => (text, report),
        Judged::Crashed => panic!("the reference checker crashed on the recorded mutation"),
    };
    let lines: Vec<&str> = text.lines().collect();
    let (rejected, reason) = verdict(&lines, &p, &[]);
    assert!(
        rejected,
        "the reference did not reject inode_v3 for its type: {reason:?}\n{text}"
    );
    assert!(
        reason
            .as_deref()
            .is_some_and(|s| s.contains("invalid key type for btree logged_ops (inode_v3)")),
        "the reference refused the key for another reason: {reason:?}"
    );
    assert!(
        report
            .problems
            .iter()
            .any(|problem| problem.kind == "invalid_bkey"
                && problem
                    .detail
                    .contains(&format!("btree logged_ops: key {p}"))),
        "the checker did not report the planted key: {:?}",
        report.problems
    );
}

/// What the reference printed for keys planted into reconcile_work_phys in
/// run 38722144899 (`vm.log` in its tier-logs-suite-in-vm artifact), on an
/// image it judged without crashing.
const OBSERVED: &[&str] = &[
    "invalid bkey in btree_node btree=reconcile_work_phys level=0: u64s 5 type deleted 4096:4748401515316737990:U32_MAX len 0 ver 0",
    "  nonzero snapshot, deleting",
    "invalid bkey in btree_node btree=reconcile_work_phys level=0: u64s 5 type hash_whiteout 0:260:0 len 0 ver 0",
    "  invalid key type for btree reconcile_work_phys (hash_whiteout), deleting",
    "invalid bkey in btree_node btree=reconcile_work_phys level=0: u64s 5 type cookie 0:259:0 len 0 ver 0",
    "  bad val size (0 < 8), deleting",
    "accounting_read... done (0 seconds)",
];

#[test]
fn a_key_refused_for_its_value_size_is_not_refused_for_its_type() {
    let (rejected, said) = verdict(OBSERVED, "0:259:0", &["0:260:0".to_string()]);
    assert!(
        !rejected,
        "the reference refused the cookie for its value size, not its type: {said:?}"
    );
    assert!(
        said.as_deref().is_some_and(|s| s.contains("bad val size")),
        "the value-size refusal is kept: {said:?}"
    );
}

#[test]
fn a_key_refused_for_its_snapshot_is_not_refused_for_its_type() {
    let (rejected, said) = verdict(&OBSERVED[..2], "4096:4748401515316737990:U32_MAX", &[]);
    assert!(
        !rejected,
        "the reference refused the key for its snapshot, not its type: {said:?}"
    );
    assert!(
        said.as_deref()
            .is_some_and(|s| s.contains("nonzero snapshot")),
        "the snapshot refusal is kept: {said:?}"
    );
}

#[test]
fn a_key_refused_for_its_type_is_counted() {
    let (rejected, said) = verdict(OBSERVED, "0:260:0", &["0:259:0".to_string()]);
    assert!(
        rejected,
        "the reference refused hash_whiteout for its type: {said:?}"
    );
    assert!(
        said.as_deref()
            .is_some_and(|s| s.contains("invalid key type")),
        "{said:?}"
    );
}

/// What the reference printed in queue run 39847363486: the refusal of a
/// key planted in xattrs followed its check_allocations progress text on
/// the same line.
#[test]
fn a_refusal_after_a_pass_progress_text_is_counted() {
    let lines = [
        "check_allocations...invalid bkey in btree_node btree=xattrs level=0: u64s 8 type lru 2147487676:3635906401912537942:U32_MAX len 0 ver 0",
        "  invalid key type for btree xattrs (lru), deleting",
        " done (0 seconds)",
    ];
    let (rejected, said) = verdict(&lines, "2147487676:3635906401912537942:U32_MAX", &[]);
    assert!(rejected, "{said:?}");
    assert!(
        said.as_deref()
            .is_some_and(|s| s.contains("invalid key type for btree xattrs (lru)")),
        "{said:?}"
    );
}

#[test]
fn a_key_the_reference_does_not_name_is_not_refused() {
    assert_eq!(verdict(OBSERVED, "0:261:0", &[]), (false, None));
}

#[test]
fn a_key_refused_for_its_value_size_is_planted_again_at_the_size_named() {
    let (_, said) = verdict(OBSERVED, "0:259:0", &["0:260:0".to_string()]);
    assert_eq!(
        said.as_deref().and_then(wanted_val_size),
        Some(8),
        "{said:?}"
    );
    let (_, said) = verdict(OBSERVED, "0:260:0", &["0:259:0".to_string()]);
    assert_eq!(
        said.as_deref().and_then(wanted_val_size),
        None,
        "a type refusal names no value size: {said:?}"
    );
}

/// A key as the matrix plants it: no size, at `snapshot`, with `value`.
fn planted(snapshot: u32, value: Vec<u8>) -> Bkey {
    Bkey {
        key_type: 2,
        size: 0,
        version_hi: 0,
        version_lo: 0,
        pos: Bpos {
            inode: 0,
            offset: 0x102,
            snapshot,
        },
        value,
    }
}

/// The reasons docs/clean-room.md ("The key types a btree holds") records
/// the reference giving before a key's type, in the form `verdict` keeps
/// them: the `invalid bkey` line, then its reasons after ` | `.
const SAID_SIZE_ZERO: &str = "invalid bkey in btree_node btree=extents level=0: u64s 5 type error <pos> len 0 ver 0 | size == 0, deleting";
const SAID_SNAPSHOT_ZERO: &str = "invalid bkey in btree_node btree=damage level=0: u64s 5 type error <pos> len 0 ver 0 | snapshot == 0, deleting";
const SAID_NONZERO_SNAPSHOT: &str = "invalid bkey in btree_node btree=reconcile_work_phys level=0: u64s 5 type deleted <pos> len 0 ver 0 | nonzero snapshot, deleting";
const SAID_VAL_SIZE: &str = "invalid bkey in btree_node btree=reconcile_work_phys level=0: u64s 5 type cookie <pos> len 0 ver 0 | bad val size (0 < 8), deleting";

#[test]
fn a_key_already_written_by_the_reference_keeps_its_position_and_value() {
    for (id, t) in [(DIRENTS, 10), (SNAPSHOTS, 22)] {
        let mut key = planted(u32::MAX, vec![7; 16]);
        key.key_type = t;
        key.version_lo = 123;
        let got = planted_key(id, t, &[key.clone()], &Written::default());
        assert_eq!(got.pos, key.pos);
        assert_eq!(got.value, key.value);
        assert_eq!(got.version_lo, key.version_lo);
    }
}

#[test]
fn a_key_in_bucket_gens_uses_a_reference_written_group() {
    let mut group = planted(0, vec![0; 256]);
    group.key_type = 30;
    group.pos.offset = 0;
    for t in [1, 2, 3, 25, 40] {
        let got = planted_key(BUCKET_GENS, t, &[group.clone()], &Written::default());
        assert_eq!(got.pos, group.pos);
        assert_eq!(got.key_type, t);
        assert!(got.value.is_empty());
    }
}

#[test]
fn a_key_of_type_stripe_or_damage_has_the_reference_judged_control_value() {
    let mut written = Written::default();
    let ptr = vec![1, 2, 3, 4, 5, 6, 7, 8];
    written.values.insert(6, [vec![2; 8], ptr.clone()].concat());
    let stripe = planted_key(6, KEY_STRIPE, &[], &written);
    assert_eq!(stripe.value.len(), 24);
    assert_eq!(&stripe.value[..8], &[1, 0, 0, 1, 0, 0, 0, 0]);
    assert_eq!(&stripe.value[8..16], &ptr);
    assert_eq!(&stripe.value[16..], &[0; 8]);
    let damage = planted_key(DAMAGE, KEY_DAMAGE, &[], &written);
    assert_eq!(damage.value.len(), 16);
    assert_eq!(damage.value[0], 1);
    assert!(damage.value[1..].iter().all(|&b| b == 0));
}

#[test]
fn a_key_without_a_resident_type_keeps_the_matrix_position_and_sample() {
    let mut written = Written::default();
    written.values.insert(KEY_XATTR, vec![42; 16]);
    written.snapshot.insert(XATTRS, u32::MAX);
    let key = planted_key(XATTRS, KEY_XATTR, &[], &written);
    assert_eq!(key.pos, planted_at(None, u32::MAX, KEY_XATTR));
    assert_eq!(key.value, vec![42; 16]);
    assert_eq!(key.size, 0);
}

#[test]
fn a_key_refused_for_a_size_of_zero_is_planted_again_with_a_size() {
    let mut key = planted(0, vec![]);
    let r = replant(SAID_SIZE_ZERO, &key);
    assert_eq!(r, Some(Replant::Size(PLANTED_SIZE)));
    r.unwrap().apply(&mut key);
    assert_eq!(key.size, PLANTED_SIZE);
    assert!(
        key.pos.offset >= u64::from(key.size),
        "an extent's position is its end, so it must lie past its size"
    );
    assert_eq!(
        replant(SAID_SIZE_ZERO, &key),
        None,
        "a key refused for a zero size once it has a size is a verdict"
    );
}

#[test]
fn a_key_refused_for_snapshot_zero_is_planted_again_at_a_snapshot() {
    let mut key = planted(0, vec![]);
    let r = replant(SAID_SNAPSHOT_ZERO, &key);
    assert_eq!(r, Some(Replant::Snapshot(u32::MAX)));
    r.unwrap().apply(&mut key);
    assert_eq!(key.pos.snapshot, u32::MAX);
    assert_eq!(key.pos.to_string(), "0:258:U32_MAX", "the position moves");
    assert_eq!(replant(SAID_SNAPSHOT_ZERO, &key), None);
}

#[test]
fn a_key_refused_for_a_nonzero_snapshot_is_planted_again_at_snapshot_zero() {
    let mut key = planted(u32::MAX, vec![]);
    let r = replant(SAID_NONZERO_SNAPSHOT, &key);
    assert_eq!(r, Some(Replant::Snapshot(0)));
    r.unwrap().apply(&mut key);
    assert_eq!(key.pos.snapshot, 0);
    assert_eq!(replant(SAID_NONZERO_SNAPSHOT, &key), None);
}

#[test]
fn a_key_refused_for_its_value_size_is_replanted_before_anything_else() {
    let mut key = planted(0, vec![]);
    let both = format!("{SAID_VAL_SIZE} | size == 0, deleting");
    assert_eq!(replant(&both, &key), Some(Replant::ValueSize(8)));
    replant(&both, &key).unwrap().apply(&mut key);
    assert_eq!(key.value, vec![0; 8]);
    assert_eq!(
        replant(&both, &key),
        Some(Replant::Size(PLANTED_SIZE)),
        "the next reason is asked for once the value fits"
    );
    // A value size that is not a multiple of eight is rounded up to one.
    let odd = SAID_VAL_SIZE.replace("(0 < 8)", "(0 < 12)");
    assert_eq!(
        replant(&odd, &planted(0, vec![])),
        Some(Replant::ValueSize(16))
    );
}

#[test]
fn a_key_refused_for_its_type_or_not_named_is_not_planted_again() {
    let key = planted(0, vec![]);
    let (rejected, said) = verdict(OBSERVED, "0:260:0", &["0:259:0".to_string()]);
    assert!(rejected);
    assert_eq!(said.as_deref().and_then(|s| replant(s, &key)), None);
    assert_eq!(replant("", &key), None);
    assert_eq!(
        replant("invalid bkey in btree_node btree=inodes level=0: u64s 5 type inode <pos> len 0 ver 0 | something else, deleting", &key),
        None,
        "a reason the matrix does not know how to satisfy is left unresolved"
    );
}
