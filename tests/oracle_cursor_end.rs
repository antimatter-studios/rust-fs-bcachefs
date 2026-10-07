//! The end of a btree, through the cursor: a seek past every key stands
//! before nothing rather than failing, and iteration from anywhere ends
//! with `None`. On every fixture the root key of every btree is at the
//! all-ones position, which the cursor used to rely on and now only
//! records (docs/clean-room.md).

mod common;

use common::{fixture, SETS};
use fs_bcachefs::bkey::Bpos;
use fs_bcachefs::btree::{btree_id, Cursor};
use fs_bcachefs::superblock::Superblock;
use fs_core::FileDevice;

#[test]
fn a_seek_past_the_last_key_stands_before_nothing() {
    for set in SETS {
        let dev = FileDevice::open(fixture(&format!("{set}.img"))).unwrap();
        let sb = Superblock::read(&dev).unwrap();
        for id in [
            btree_id::EXTENTS,
            btree_id::INODES,
            btree_id::DIRENTS,
            btree_id::XATTRS,
        ] {
            let mut c = Cursor::new(&dev, &sb, id, None).unwrap();
            // Every key, to the end.
            let mut n = 0usize;
            let mut last: Option<Bpos> = None;
            while let Some(k) = c.next_key().unwrap() {
                assert!(
                    last.is_none_or(|l| l < k.pos),
                    "{set} btree {id}: out of order"
                );
                last = Some(k.pos);
                n += 1;
            }
            assert!(
                c.next_key().unwrap().is_none(),
                "{set} btree {id}: None is not sticky"
            );
            // Past the last key, and past everything.
            if let Some(l) = last {
                c.seek(fs_bcachefs::btree::successor(l)).unwrap();
                assert!(
                    c.next_key().unwrap().is_none(),
                    "{set} btree {id}: a key past the last"
                );
            }
            c.seek(Bpos::MAX).unwrap();
            assert!(
                c.next_key().unwrap().is_none(),
                "{set} btree {id}: a key at SPOS_MAX"
            );
            c.seek(Bpos {
                inode: u64::MAX,
                offset: 0,
                snapshot: 0,
            })
            .unwrap();
            assert!(c.next_key().unwrap().is_none(), "{set} btree {id}");
            if id != btree_id::XATTRS {
                assert!(n > 0, "{set} btree {id}: no keys");
            }
        }
    }
}

/// A btree with no root recorded holds nothing. The reference formatter
/// records no xattrs root on a new filesystem, and the reference lister
/// lists that btree as empty rather than failing; so does this reader,
/// through a walk, a cursor and the filesystem's xattr lookup.
#[test]
fn a_btree_with_no_root_is_empty_as_the_lister_lists_it() {
    let dev = FileDevice::open(fixture("write-study/base.img")).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    assert!(
        !sb.btree_roots()
            .unwrap()
            .iter()
            .any(|r| r.btree_id == btree_id::XATTRS),
        "the fixture records an xattrs root, so this test would prove nothing"
    );
    let listed = common::read_text("write-study/base.xattrs.txt");
    assert!(
        !listed.lines().any(|l| l.starts_with("u64s")),
        "the lister lists keys: {listed}"
    );
    assert!(fs_bcachefs::btree::walk(&dev, &sb, btree_id::XATTRS)
        .unwrap()
        .is_empty());
    let mut c = Cursor::new(&dev, &sb, btree_id::XATTRS, None).unwrap();
    assert!(c.next_key().unwrap().is_none());
    c.seek(Bpos::default()).unwrap();
    assert!(c.next_key().unwrap().is_none());
    let fs = fs_bcachefs::Filesystem::open(dev).unwrap();
    let d = fs.lookup("/d").unwrap();
    assert!(fs.xattrs(d).unwrap().is_empty());
}
