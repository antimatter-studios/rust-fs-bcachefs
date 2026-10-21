### Fixed

- **The checker judges the freespace btree against the alloc keys (#163).**
  A freespace run holding a bucket in use, a free bucket that no run holds
  (with no alloc key, or with one of data type free), and a run on a device
  the filesystem does not have were reported clean; the reference checker
  reports each of them. They are now reported as
  `bucket_incorrectly_set_in_freespace_btree`,
  `hole_in_alloc_btree_missing_in_freespace_btree`,
  `bucket_incorrectly_unset_in_freespace_btree` and
  `freespace_entry_for_nonexistent_bucket`. A filesystem with a free bucket
  at generation 16 or more is not judged: how a run's position records
  such a generation has not been observed.
- **The checker reports a backpointer in a bucket with no alloc key
  (#163)**, as `backpointer_for_nonexistent_alloc_key`.
