### Fixed

- **The checker reports a key with a size or a snapshot its btree does not
  allow (#219).** An inode, dirent, subvolume, lru, backpointer,
  bucket_gens, snapshot_tree, inode allocation cursor or accounting key
  with a nonzero size, and a subvolume, snapshot, lru, need_discard or
  freespace key at a nonzero snapshot, were reported clean; the reference
  checker refuses each of them. They are now reported as
  `bkey_size_nonzero` and `bkey_snapshot_nonzero`.
