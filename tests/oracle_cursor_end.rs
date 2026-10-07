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
