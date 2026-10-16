//! The checker against the reference checker, on damaged copies of a
//! fixture, INSIDE the harness guest only (`chore test:vm`); anywhere else
//! it fails naming that. Nothing skips. Wherever the reference finds a
//! problem, this crate's checker must find one too.

use std::path::{Path, PathBuf};
use std::process::Command;

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

fn damaged(name: &str, damage: impl FnOnce(&mut Vec<u8>, u64)) -> PathBuf {
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
    std::fs::create_dir_all("/share/check-oracle").unwrap();
    let p = PathBuf::from(format!("/share/check-oracle/{name}.img"));
    std::fs::write(&p, &b).unwrap();
    p
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
    // And on an undamaged copy, both are clean. A copy under /share: the
    // reference tools run in a chroot that sees /share and not /repo.
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

    /// A copy of fixture `src` under /share, with the inode at `path`
    /// changed by `change` through the writer. By path: inode numbers
    /// differ from one build of the fixtures to the next.
    fn rewritten(
        name: &str,
        src: &str,
        path: &str,
        change: impl FnOnce(&mut InodeV3Raw),
    ) -> PathBuf {
        assert!(
            Path::new("/usr/local/bin/bcachefs-ref").exists(),
            "the reference checker is not here: this runs inside the harness guest (`chore test:vm`)"
        );
        std::fs::create_dir_all("/share/check-oracle").unwrap();
        let p = PathBuf::from(format!("/share/check-oracle/{name}.img"));
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

    /// A copy under /share, removed when dropped, pass or panic.
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// A copy of `default` under /share with `hello.txt`'s dirent moved
    /// `by` slots on and a key of type `left` in its own slot.
    fn displaced(name: &str, by: u64, left: u8) -> Scratch {
        assert!(
            Path::new("/usr/local/bin/bcachefs-ref").exists(),
            "the reference checker is not here: this runs inside the harness guest (`chore test:vm`)"
        );
        std::fs::create_dir_all("/share/check-oracle").unwrap();
        let img = Scratch(PathBuf::from(format!("/share/check-oracle/{name}.img")));
        std::fs::copy(fixture("default.img"), &img.0).unwrap();
        let d = {
            let dev = FileDevice::open(&img.0).unwrap();
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
            let dev = FileDevice::open(&img.0).unwrap();
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
            let dev = FileDevice::open(&img.0).unwrap();
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
        let mut w = Writer::open(FileDevice::open_rw(&img.0).unwrap()).unwrap();
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
    let (theirs, text) = reference_finds_a_problem(&away.0);
    assert!(
        theirs && text.contains("hash table key at wrong offset"),
        "{}: the reference did not report the dirent at a wrong offset, so this \
         case proves nothing:\n{text}",
        away.0.display()
    );
    assert!(
        wrong_offset(&away.0),
        "{}: the reference found a dirent at a wrong offset this checker did not \
         name as hash_table_key_wrong_offset:\n{text}",
        away.0.display()
    );

    let past = displaced("dirent-past-whiteout", 1, key_type::HASH_WHITEOUT);
    let (theirs, text) = reference_finds_a_problem(&past.0);
    assert!(
        !theirs,
        "{}: the reference found a problem in a dirent reachable over a \
         whiteout:\n{text}",
        past.0.display()
    );
    assert!(
        !wrong_offset(&past.0),
        "{}: the reference calls a dirent reachable over a whiteout clean, and \
         this checker reported it at a wrong offset",
        past.0.display()
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

    /// A copy of fixture `src` under /share, with the key at `0:offset` of
    /// btree `id` changed by `change` through the writer.
    fn rewritten(
        name: &str,
        src: &str,
        (id, key_type, offset): (u8, u8, u64),
        change: impl FnOnce(&mut Vec<u8>),
    ) -> PathBuf {
        assert!(
            Path::new("/usr/local/bin/bcachefs-ref").exists(),
            "the reference checker is not here: this runs inside the harness guest (`chore test:vm`)"
        );
        std::fs::create_dir_all("/share/check-oracle").unwrap();
        let p = PathBuf::from(format!("/share/check-oracle/{name}.img"));
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
    std::fs::create_dir_all("/share/check-oracle").unwrap();
    let img = PathBuf::from("/share/check-oracle/snapshot-field-nonzero.img");
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
    std::fs::create_dir_all("/share/check-oracle").unwrap();
    let img = PathBuf::from("/share/check-oracle/bucket-gens-size-nonzero.img");
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
