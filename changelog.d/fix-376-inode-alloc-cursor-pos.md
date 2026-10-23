### Fixed

- **The checker reports an inode allocation cursor at a snapshot or off inode 1.** A `logged_ops` `inode_alloc_cursor` key with a snapshot is reported as `bkey_snapshot_nonzero`, and one at an inode other than 1 as `inode_alloc_cursor_inode_bad`, as the reference checker refused both in the nightly differential run.
