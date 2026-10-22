//! Which key types each btree this checker judges holds, as the reference
//! checker judges it (#163), INSIDE the harness guest only (`chore
//! test:vm`); anywhere else it fails naming that. Nothing skips.
//!
//! Each judged btree of a copy of `default` is planted with a key of every
//! type S1 names, and one past them, one key to an image. A btree
//! `default` records no root for is first given the root of a btree that
//! holds no keys. The planted key sits after the btree's last key, and
//! carries the value of a key of its type the reference wrote into a
//! fixture where one has, so that a type the btree holds is not refused for
//! its value instead. The reference checker's verdict on the planted key
//! is read from its printed output, and nothing else of it, and this
//! checker must report `invalid_bkey` for exactly the keys the reference
//! refuses. The pairs the reference itself writes, across the fixtures,
//! are printed beside any disagreement.
//! Only a refusal the reference gives for the key's own type counts. A key
//! it refuses for its value size is planted once more with a zeroed value
//! of the size it names; a key it still refuses for anything but its type
//! leaves that type unresolved, and an unresolved type fails the test
//! rather than passing as accepted.
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
use common::plant::{self, empty_root, move_root, ACCOUNTING, LOGGED_OPS, RECONCILE_WORK_PHYS};
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

/// The btrees whose key types this checker judges: the three in which the
/// reference checker's `invalid bkey` showed up in #163's differential.
/// Every other btree is #305.
const JUDGED: &[u8] = &[LOGGED_OPS, ACCOUNTING, RECONCILE_WORK_PHYS];
const FREESPACE: u8 = 11;
const KEY_LOGGED_OP_TRUNCATE: u8 = 32;
const XATTRS: u8 = 3;
const KEY_LRU: u8 = 10;
const KEY_XATTR: u8 = 11;

/// Planted keys are judged one to an image. With eight to an image the
/// reference named only some of the keys it refuses (run 39020187551): of a
/// dirent planted into reconcile_work_phys among seven other keys it said
/// nothing, while the same dirent planted alone is refused for its type
/// (`check_oracle_reconcile_dirent.rs`, the same run).
///
/// Times a key refused for its value size is planted again, at the size
/// the reference names.
const RESIZES: usize = 2;

fn planted_types() -> Vec<u8> {
    (1..=plant::KEY_TYPES.len() as u8).collect()
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
        move_root(img, empty_root(img, id), id);
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
        .args(["300", "bcachefs-ref", "fsck", "-n"])
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
    let mut said: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let mut failures = Vec::new();
    // Per btree, the reference's output on the first image whose verdict
    // this checker does not share or which is left unresolved.
    let mut shown: BTreeMap<u8, Vec<String>> = BTreeMap::new();
    let mut crashes = 0;
    let mut ratelimited = false;
    for &id in JUDGED {
        let name = plant::BTREES[usize::from(id)];
        let has_root = rooted.contains(&id);
        let last = if has_root {
            btree::walk(&default, &sb, id)
                .unwrap()
                .last()
                .map(|k| k.pos)
        } else {
            None
        };
        let snapshot = written.snapshot.get(&id).copied().unwrap_or(0);
        for &t in &types {
            let mut key = Bkey {
                key_type: t,
                size: 0,
                version_hi: 0,
                version_lo: 0,
                pos: planted_at(last, snapshot, t),
                value: written.values.get(&t).cloned().unwrap_or_default(),
            };
            let p = key.pos.to_string();
            let img = dir.join(format!("{name}-{t}.img"));
            for attempt in 0..=RESIZES {
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
                // A value-size refusal is not a verdict on the type yet:
                // the key is planted again with a zeroed value of the size
                // the reference names.
                let wanted = text
                    .as_deref()
                    .filter(|_| !rejected)
                    .and_then(wanted_val_size)
                    .filter(|&n| n > key.value.len());
                if let Some(text) = &text {
                    let text = replace_word(text, &plant::type_name(t), "<type>");
                    let e = said
                        .entry(text)
                        .or_insert((0, format!("{name} {}", plant::type_name(t))));
                    e.0 += 1;
                }
                if let Some(n) = wanted.filter(|_| attempt < RESIZES) {
                    key.value = vec![0; n.next_multiple_of(8)];
                    continue;
                }
                let open = !rejected && text.is_some();
                let reported = report
                    .problems
                    .iter()
                    .any(|q| q.kind == "invalid_bkey" && q.detail.contains(&format!("key {p} ")));
                if rejected {
                    theirs.entry(id).or_default().insert(t);
                }
                if open {
                    unresolved.entry(id).or_default().insert(t);
                } else if reported {
                    ours.entry(id).or_default().insert(t);
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
    for &id in JUDGED {
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
        msg.extend(shown.into_values().flatten());
        panic!("{}", msg.join("\n"));
    }
    logged_op_truncate_in_freespace_is_rejected_by_both_checkers();
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
    let pos = Bpos {
        inode: 0,
        offset: 2040,
        snapshot: 0,
    };
    let mut key = btree::walk(&dev, &sb, FREESPACE)
        .unwrap()
        .into_iter()
        .find(|k| k.pos == pos)
        .expect("the pinned aged fixture has the differential's freespace key");
    assert_eq!(key.key_type, 25, "the planted key must begin as a set");
    key.key_type = KEY_LOGGED_OP_TRUNCATE;

    let dir = common::ScratchDir::new("check-freespace-logged-op-truncate");
    let img = dir.join("aged-1659445.img");
    let bytes = std::fs::read(source).unwrap();
    let (text, report) = match judge(&bytes, &img, true, FREESPACE, &key).unwrap() {
        Judged::Seen(text, report) => (text, report),
        Judged::Crashed => panic!("the reference checker crashed on the recorded mutation"),
    };
    let lines: Vec<&str> = text.lines().collect();
    let (rejected, reason) = verdict(&lines, "0:2040:0", &[]);
    assert!(
        rejected,
        "the reference did not reject the key for its type: {reason:?}\n{text}"
    );
    assert!(
        report.problems.iter().any(|problem| {
            problem.kind == "invalid_bkey"
                && problem.detail.contains("btree freespace: key 0:2040:0")
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
    let pos = Bpos {
        inode: 2_147_487_679,
        offset: 1_366_636_819_568_056_465,
        snapshot: u32::MAX,
    };
    let mut key = btree::walk(&dev, &sb, XATTRS)
        .unwrap()
        .into_iter()
        .find(|k| k.pos == pos)
        .expect("the pinned aged fixture has the differential's xattr key");
    assert_eq!(
        key.key_type, KEY_XATTR,
        "the planted key must begin as an xattr"
    );
    key.key_type = KEY_LRU;

    let dir = common::ScratchDir::new("check-xattrs-lru-key");
    let img = dir.join("aged-1659510.img");
    let bytes = std::fs::read(source).unwrap();
    let (text, report) = match judge(&bytes, &img, true, XATTRS, &key).unwrap() {
        Judged::Seen(text, report) => (text, report),
        Judged::Crashed => panic!("the reference checker crashed on the recorded mutation"),
    };
    let lines: Vec<&str> = text.lines().collect();
    let p = "2147487679:1366636819568056465:U32_MAX";
    let (rejected, reason) = verdict(&lines, p, &[]);
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
                && problem
                    .detail
                    .contains("btree xattrs: key 2147487679:1366636819568056465:U32_MAX")
        }),
        "the checker did not report the planted key: {:?}",
        report.problems
    );
}

/// The differential's kernel seed 1659466 changes the key at 1:4:0 to
/// inode_v3 and commits it in logged_ops. The reference refuses that type
/// there; the short value is retained so the exact mutation stays pinned.
#[test]
fn inode_v3_in_logged_ops_is_rejected_by_both_checkers() {
    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference checker is not here: this runs inside the harness guest (`chore test:vm`)"
    );
    let source = fixture("kernel.img");
    let dev = FileDevice::open(&source).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let pos = Bpos {
        inode: 1,
        offset: 4,
        snapshot: 0,
    };
    let mut key = btree::walk(&dev, &sb, LOGGED_OPS)
        .unwrap()
        .into_iter()
        .find(|k| k.pos == pos)
        .expect("kernel.img has the differential's logged_ops key");
    assert_ne!(key.key_type, fs_bcachefs::bkey::key_type::INODE_V3);
    assert_eq!(key.value.len(), 16, "the recorded mutation keeps its value");
    key.key_type = fs_bcachefs::bkey::key_type::INODE_V3;

    let dir = common::ScratchDir::new("check-logged-ops-inode-v3");
    let img = dir.join("kernel-1659466.img");
    let bytes = std::fs::read(source).unwrap();
    let (text, report) = match judge(&bytes, &img, true, LOGGED_OPS, &key).unwrap() {
        Judged::Seen(text, report) => (text, report),
        Judged::Crashed => panic!("the reference checker crashed on the recorded mutation"),
    };
    let lines: Vec<&str> = text.lines().collect();
    let (rejected, reason) = verdict(&lines, "1:4:0", &[]);
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
                && problem.detail.contains("btree logged_ops: key 1:4:0")),
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
